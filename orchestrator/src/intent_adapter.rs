//! Intent-engine adapter — converts proto [`IntentActionGraph`] payloads into
//! the orchestrator's internal decomposition plan (sub-tasks + dependency
//! edges) that [`crate::runtime::OrchestratorRuntime::submit`] wires into a
//! [`crate::task_graph::TaskGraph`].

use std::collections::{HashMap, VecDeque};

use cognos_ipc_grpc::proto::v1::{
    Intent as ProtoIntent, IntentActionEdge, IntentActionGraph, IntentActionNode, IntentResponse,
};
use thiserror::Error;

use crate::runtime::{classify_intent, decompose_into_tasks, Intent};

// ─── Errors ─────────────────────────────────────────────────────────────────

/// Failure modes when talking to the intent-engine or interpreting its graph.
#[derive(Debug, Error)]
pub enum DecomposeError {
    /// gRPC dial / RPC / timeout — the caller may fall back to the legacy path.
    #[error("intent-engine transport error: {0}")]
    Transport(String),
    /// The engine answered but the payload is unusable (bad status, cycle, …).
    #[error("intent-engine response invalid: {0}")]
    InvalidResponse(String),
}

impl DecomposeError {
    pub fn is_transport(&self) -> bool {
        matches!(self, Self::Transport(_))
    }
}

// ─── Internal plan ──────────────────────────────────────────────────────────

/// A sub-task the orchestrator knows how to schedule (mirrors the legacy
/// [`crate::runtime::SubTask`] shape, extended with an optional target).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedSubTask {
    pub description: String,
    pub capability: String,
    pub action: String,
    pub target: String,
}

/// One node in a decomposition plan, optionally linked back to a proto node id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedNode {
    pub sub_task: PlannedSubTask,
    /// Proto `node_id` when the plan came from the intent-engine.
    pub proto_node_id: Option<String>,
}

/// A full decomposition: nodes + dependency edges as node indices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecompositionPlan {
    pub nodes: Vec<PlannedNode>,
    /// `(from_index, to_index)` — `to` depends on `from` completing first.
    pub edges: Vec<(usize, usize)>,
}

// ─── Action → capability mapping ────────────────────────────────────────────

/// Map a canonical action name (from the intent-engine or the legacy classifier)
/// to the capability an agent must advertise to execute it.
pub fn action_to_capability(action: &str) -> String {
    let lower = action.to_lowercase();

    // Exact / prefix rules ordered from most specific to least.
    if lower.contains("delete") || lower == "file.delete" || lower.contains("delete_files")
        || lower == "delete_path"
    {
        return "file.write".into();
    }
    if lower.contains("move") || lower.contains("rename") || lower == "file.move" {
        return "file.write".into();
    }
    if lower.contains("search") || lower.contains("find") || lower == "file.search" {
        return "file.read".into();
    }
    if lower.starts_with("file.") || lower == "open_files" || lower == "create_dir"
        || lower == "create_file" || lower == "execute_open"
    {
        if lower == "create_dir" || lower == "create_file" || lower.contains("create") {
            return "file.write".into();
        }
        return "file.read".into();
    }
    if lower.contains("validate") || lower.contains("test") || lower == "coding.validate" {
        return "coding.validate".into();
    }
    if lower.contains("plan") || lower == "coding.plan" || lower == "create_plan" {
        return "coding.plan".into();
    }
    if lower.starts_with("coding.") || lower.contains("implement") || lower.contains("debug")
        || lower.contains("refactor") || lower == "execute_code"
    {
        return "coding.execute".into();
    }
    if lower.contains("install") || lower == "install_package" || lower.starts_with("pkg.") {
        return "pkg.execute".into();
    }
    if lower.contains("security_check") || lower.contains("review") {
        return "security.review".into();
    }
    if lower.contains("analyze") || lower.contains("audit") {
        return "security.analyze".into();
    }
    if lower.starts_with("security.") || lower == "gather_state" {
        return "security.read".into();
    }
    if lower.starts_with("memory.") || lower == "resolve_target" || lower == "memory_search" {
        return "memory.read".into();
    }
    if lower.starts_with("system.") {
        return "system.config".into();
    }
    if lower.starts_with("intent.") || lower == "disambiguate" {
        return "intent.disambiguate".into();
    }

    "general.execute".into()
}

// ─── Proto conversion ───────────────────────────────────────────────────────

/// Convert a proto [`IntentActionGraph`] into a [`DecompositionPlan`].
///
/// Validates that dependency edges reference known nodes and that the graph is
/// acyclic. A cycle is rejected with [`DecomposeError::InvalidResponse`].
pub fn proto_graph_to_plan(graph: &IntentActionGraph) -> Result<DecompositionPlan, DecomposeError> {
    if graph.nodes.is_empty() {
        return Err(DecomposeError::InvalidResponse(
            "action graph has no nodes".into(),
        ));
    }

    // Index proto node ids → plan index (in proto node order for stability).
    let mut id_to_idx: HashMap<String, usize> = HashMap::new();
    let nodes: Vec<PlannedNode> = graph
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| {
            id_to_idx.insert(n.node_id.clone(), i);
            PlannedNode {
                sub_task: proto_node_to_sub_task(n),
                proto_node_id: Some(n.node_id.clone()),
            }
        })
        .collect();

    let mut edges: Vec<(usize, usize)> = Vec::new();
    for IntentActionEdge { from_node, to_node } in &graph.deps {
        let from = id_to_idx.get(from_node).ok_or_else(|| {
            DecomposeError::InvalidResponse(format!("unknown edge source node: {from_node}"))
        })?;
        let to = id_to_idx.get(to_node).ok_or_else(|| {
            DecomposeError::InvalidResponse(format!("unknown edge target node: {to_node}"))
        })?;
        edges.push((*from, *to));
    }

    if has_cycle(nodes.len(), &edges) {
        return Err(DecomposeError::InvalidResponse(
            "action graph contains a cycle".into(),
        ));
    }

    Ok(DecompositionPlan { nodes, edges })
}

fn proto_node_to_sub_task(node: &IntentActionNode) -> PlannedSubTask {
    let capability = action_to_capability(&node.action);
    let description = if node.target.is_empty() {
        format!("Execute {} via intent-engine", node.action)
    } else {
        format!("{} → {}", node.action, node.target)
    };
    PlannedSubTask {
        description,
        capability,
        action: node.action.clone(),
        target: node.target.clone(),
    }
}

/// Kahn topological sort — returns `true` when a cycle is present.
fn has_cycle(node_count: usize, edges: &[(usize, usize)]) -> bool {
    let mut indegree = vec![0usize; node_count];
    for &(_, to) in edges {
        if to < node_count {
            indegree[to] += 1;
        }
    }
    let mut queue: VecDeque<usize> = indegree
        .iter()
        .enumerate()
        .filter(|(_, d)| **d == 0)
        .map(|(i, _)| i)
        .collect();
    let mut visited = 0usize;
    while let Some(id) = queue.pop_front() {
        visited += 1;
        for &(from, to) in edges {
            if from == id && to < node_count {
                indegree[to] -= 1;
                if indegree[to] == 0 {
                    queue.push_back(to);
                }
            }
        }
    }
    visited != node_count
}

// ─── IPC dispatch ───────────────────────────────────────────────────────────

/// Ask the intent-engine (via `DispatchIntent`) to parse `intent` and return a
/// [`DecompositionPlan`]. Transport failures are tagged
/// [`DecomposeError::Transport`] so the caller can fall back; semantic failures
/// (bad status, cycle, empty graph) are [`DecomposeError::InvalidResponse`].
pub async fn decompose_via_intent_engine(
    client: &cognos_ipc_grpc::client::CognosClient,
    intent: &Intent,
) -> Result<DecompositionPlan, DecomposeError> {
    let req = ProtoIntent {
        intent_id: intent.id.0.to_string(),
        utterance: intent.text.clone(),
        session_id: intent.user_id.clone(),
        ..Default::default()
    };

    let resp = client
        .dispatch_intent(req)
        .await
        .map_err(|e| DecomposeError::Transport(e.to_string()))?;

    response_to_plan(&resp)
}

fn response_to_plan(resp: &IntentResponse) -> Result<DecompositionPlan, DecomposeError> {
    if resp.status != "ok" {
        return Err(DecomposeError::InvalidResponse(format!(
            "status={} message={}",
            resp.status, resp.message
        )));
    }
    let graph = resp
        .action_graph
        .as_ref()
        .ok_or_else(|| DecomposeError::InvalidResponse("missing action_graph".into()))?;
    proto_graph_to_plan(graph)
}

// ─── Legacy fallback ────────────────────────────────────────────────────────

/// Build a decomposition plan using the local keyword classifier (the pre-IPC
/// path). Produces a linear chain of edges matching the old `submit` behaviour.
pub fn decompose_locally(intent: &Intent) -> DecompositionPlan {
    let action = classify_intent(&intent.text);
    let legacy = decompose_into_tasks(&action, intent);
    let nodes: Vec<PlannedNode> = legacy
        .into_iter()
        .map(|sub| PlannedNode {
            sub_task: PlannedSubTask {
                description: sub.description,
                capability: sub.capability,
                action: sub.action,
                target: String::new(),
            },
            proto_node_id: None,
        })
        .collect();
    let edges: Vec<(usize, usize)> = (1..nodes.len()).map(|i| (i - 1, i)).collect();
    DecompositionPlan { nodes, edges }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    use crate::runtime::IntentId;

    fn sample_intent(text: &str) -> Intent {
        Intent {
            id: IntentId(Uuid::new_v4()),
            user_id: "user.test".into(),
            text: text.into(),
            context: Default::default(),
            priority: Default::default(),
        }
    }

    fn node(id: &str, action: &str, target: &str) -> IntentActionNode {
        IntentActionNode {
            node_id: id.into(),
            intent_id: Uuid::new_v4().to_string(),
            action: action.into(),
            target: target.into(),
            confidence: 0.9,
            hal_pre_score: 0.1,
        }
    }

    #[test]
    fn action_to_capability_maps_file_open() {
        assert_eq!(action_to_capability("file.open"), "file.read");
        assert_eq!(action_to_capability("open_files"), "file.read");
    }

    #[test]
    fn proto_three_node_chain_converts_with_deps() {
        let graph = IntentActionGraph {
            nodes: vec![
                node("a", "create_dir", "~/p"),
                node("b", "create_file", "~/p/f"),
                node("c", "open_files", "~/p/f"),
            ],
            deps: vec![
                IntentActionEdge {
                    from_node: "a".into(),
                    to_node: "b".into(),
                },
                IntentActionEdge {
                    from_node: "b".into(),
                    to_node: "c".into(),
                },
            ],
            intent: None,
        };
        let plan = proto_graph_to_plan(&graph).expect("valid graph");
        assert_eq!(plan.nodes.len(), 3);
        assert_eq!(plan.edges, vec![(0, 1), (1, 2)]);
        assert_eq!(plan.nodes[0].sub_task.action, "create_dir");
        assert_eq!(plan.nodes[0].sub_task.capability, "file.write");
        assert_eq!(plan.nodes[2].sub_task.capability, "file.read");
    }

    #[test]
    fn proto_cycle_is_rejected() {
        let graph = IntentActionGraph {
            nodes: vec![node("a", "a", "t"), node("b", "b", "t")],
            deps: vec![
                IntentActionEdge {
                    from_node: "a".into(),
                    to_node: "b".into(),
                },
                IntentActionEdge {
                    from_node: "b".into(),
                    to_node: "a".into(),
                },
            ],
            intent: None,
        };
        let err = proto_graph_to_plan(&graph).expect_err("cycle");
        assert!(matches!(err, DecomposeError::InvalidResponse(_)));
        assert!(err.to_string().contains("cycle"));
    }

    #[test]
    fn local_fallback_coding_produces_three_linear_nodes() {
        let plan = decompose_locally(&sample_intent("please implement the feature"));
        assert_eq!(plan.nodes.len(), 3);
        assert_eq!(plan.edges, vec![(0, 1), (1, 2)]);
        assert_eq!(plan.nodes[0].sub_task.capability, "coding.plan");
        assert!(plan.nodes.iter().all(|n| n.proto_node_id.is_none()));
    }
}
