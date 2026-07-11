//! Action graph — converts a resolved intent into an ordered DAG of actions.
//!
//! The graph is the unit handed to HAL: every node is scored independently
//! by the risk model (R(A), docs/SPEC.md), and nothing executes until HAL
//! returns a decision for that node. The intent-engine builds and orders
//! the graph; it never executes anything itself.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use uuid::Uuid;

use crate::disambiguation::ResolvedIntent;
use crate::parser::IntentSchema;

/// One executable action proposed to HAL.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionNode {
    pub node_id: Uuid,
    /// The intent this action originates from (audit linkage).
    pub intent_id: Uuid,
    /// Verb, e.g. "open_files".
    pub action: String,
    /// Target path or resource.
    pub target: String,
    /// Confidence inherited from the candidate action.
    pub confidence: f32,
    /// Pre-score hint for HAL. HAL recomputes authoritatively — this value
    /// can never lower the final risk score (HAL is human-written and
    /// cannot be reasoned around; see threat model).
    pub hal_pre_score: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GraphError {
    /// An edge references a node that does not exist.
    UnknownNode(Uuid),
    /// The graph contains a cycle and cannot be ordered.
    CycleDetected,
    /// The graph has no nodes.
    Empty,
}

impl std::fmt::Display for GraphError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownNode(id) => write!(f, "edge references unknown node {}", id),
            Self::CycleDetected => write!(f, "action graph contains a cycle"),
            Self::Empty => write!(f, "action graph has no nodes"),
        }
    }
}

impl std::error::Error for GraphError {}

/// Directed acyclic graph of actions with deterministic execution ordering.
#[derive(Debug, Default)]
pub struct ActionGraph {
    nodes: HashMap<Uuid, ActionNode>,
    /// edges[from] = nodes that depend on `from` completing first (from → to).
    edges: HashMap<Uuid, HashSet<Uuid>>,
}

impl ActionGraph {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a single-node graph from a resolved intent.
    pub fn from_resolved(intent: &ResolvedIntent, schema: &IntentSchema) -> Self {
        let mut g = Self::new();
        g.add_node(ActionNode {
            node_id: Uuid::new_v4(),
            intent_id: intent.intent_id,
            action: intent.selected_action.action.clone(),
            target: intent.selected_action.target.clone(),
            confidence: intent.selected_action.confidence,
            hal_pre_score: schema.hal_pre_score,
        });
        g
    }

    /// Build a graph directly from a parsed [`IntentSchema`], without going
    /// through disambiguation. One node per candidate action (independent, no
    /// edges); if the schema carries no candidates, a single node is derived
    /// from the goal so the graph is never empty. This is the path the
    /// intent-engine binary uses to answer `DispatchIntent`.
    pub fn from_schema(schema: &IntentSchema) -> Self {
        let mut g = Self::new();
        if schema.candidate_actions.is_empty() {
            g.add_node(ActionNode {
                node_id: Uuid::new_v4(),
                intent_id: schema.intent_id,
                action: schema.goal.clone(),
                target: String::new(),
                confidence: schema.confidence,
                hal_pre_score: schema.hal_pre_score,
            });
        } else {
            for cand in &schema.candidate_actions {
                g.add_node(ActionNode {
                    node_id: Uuid::new_v4(),
                    intent_id: schema.intent_id,
                    action: cand.action.clone(),
                    target: cand.target.clone(),
                    confidence: cand.confidence,
                    hal_pre_score: schema.hal_pre_score,
                });
            }
        }
        g
    }

    /// All nodes (unordered). For deterministic ordering use
    /// [`ActionGraph::execution_order`].
    pub fn nodes(&self) -> Vec<ActionNode> {
        self.nodes.values().cloned().collect()
    }

    /// All dependency edges as `(from, to)` pairs, where `to` depends on `from`.
    pub fn dependencies(&self) -> Vec<(Uuid, Uuid)> {
        let mut out = Vec::new();
        for (from, tos) in &self.edges {
            for to in tos {
                out.push((*from, *to));
            }
        }
        out
    }

    /// Add a node, returning its id.
    pub fn add_node(&mut self, node: ActionNode) -> Uuid {
        let id = node.node_id;
        self.nodes.insert(id, node);
        self.edges.entry(id).or_default();
        id
    }

    /// Declare that `to` depends on `from` completing first.
    pub fn add_dependency(&mut self, from: Uuid, to: Uuid) -> Result<(), GraphError> {
        if !self.nodes.contains_key(&from) {
            return Err(GraphError::UnknownNode(from));
        }
        if !self.nodes.contains_key(&to) {
            return Err(GraphError::UnknownNode(to));
        }
        self.edges.entry(from).or_default().insert(to);
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Kahn topological sort with deterministic ordering for equal-rank
    /// nodes (sorted by action + target) so audit logs are reproducible.
    pub fn execution_order(&self) -> Result<Vec<ActionNode>, GraphError> {
        if self.nodes.is_empty() {
            return Err(GraphError::Empty);
        }

        let mut indegree: HashMap<Uuid, usize> =
            self.nodes.keys().map(|k| (*k, 0)).collect();
        for tos in self.edges.values() {
            for to in tos {
                if let Some(d) = indegree.get_mut(to) {
                    *d += 1;
                }
            }
        }

        let mut ready: Vec<Uuid> = indegree
            .iter()
            .filter(|(_, d)| **d == 0)
            .map(|(k, _)| *k)
            .collect();
        self.sort_deterministic(&mut ready);

        let mut queue: VecDeque<Uuid> = ready.into();
        let mut order = Vec::with_capacity(self.nodes.len());

        while let Some(id) = queue.pop_front() {
            if let Some(node) = self.nodes.get(&id) {
                order.push(node.clone());
            }
            let mut newly_ready = Vec::new();
            if let Some(tos) = self.edges.get(&id) {
                for to in tos {
                    if let Some(d) = indegree.get_mut(to) {
                        *d -= 1;
                        if *d == 0 {
                            newly_ready.push(*to);
                        }
                    }
                }
            }
            self.sort_deterministic(&mut newly_ready);
            for n in newly_ready {
                queue.push_back(n);
            }
        }

        if order.len() != self.nodes.len() {
            return Err(GraphError::CycleDetected);
        }
        Ok(order)
    }

    fn sort_deterministic(&self, ids: &mut Vec<Uuid>) {
        ids.sort_by(|a, b| {
            let ka = self
                .nodes
                .get(a)
                .map(|n| (n.action.clone(), n.target.clone()));
            let kb = self
                .nodes
                .get(b)
                .map(|n| (n.action.clone(), n.target.clone()));
            ka.cmp(&kb)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(action: &str, target: &str) -> ActionNode {
        ActionNode {
            node_id: Uuid::new_v4(),
            intent_id: Uuid::new_v4(),
            action: action.into(),
            target: target.into(),
            confidence: 0.9,
            hal_pre_score: 0.1,
        }
    }

    #[test]
    fn chain_executes_in_dependency_order() {
        let mut g = ActionGraph::new();
        let a = g.add_node(node("create_dir", "~/p"));
        let b = g.add_node(node("create_file", "~/p/f"));
        let c = g.add_node(node("open_files", "~/p/f"));
        g.add_dependency(a, b).expect("edge a->b");
        g.add_dependency(b, c).expect("edge b->c");
        let order = g.execution_order().expect("order");
        let actions: Vec<&str> = order.iter().map(|n| n.action.as_str()).collect();
        assert_eq!(actions, vec!["create_dir", "create_file", "open_files"]);
    }

    #[test]
    fn cycle_is_rejected() {
        let mut g = ActionGraph::new();
        let a = g.add_node(node("a", "t"));
        let b = g.add_node(node("b", "t"));
        g.add_dependency(a, b).expect("edge a->b");
        g.add_dependency(b, a).expect("edge b->a");
        assert!(matches!(
            g.execution_order(),
            Err(GraphError::CycleDetected)
        ));
    }

    #[test]
    fn unknown_node_edge_rejected() {
        let mut g = ActionGraph::new();
        let a = g.add_node(node("a", "t"));
        let ghost = Uuid::new_v4();
        assert!(matches!(
            g.add_dependency(a, ghost),
            Err(GraphError::UnknownNode(_))
        ));
    }

    #[test]
    fn empty_graph_rejected() {
        let g = ActionGraph::new();
        assert!(matches!(g.execution_order(), Err(GraphError::Empty)));
    }

    #[test]
    fn from_resolved_builds_single_node() {
        use crate::disambiguation::ResolvedIntent;
        use crate::parser::{CandidateAction, IntentSchema, SessionContext};

        let intent_id = Uuid::new_v4();
        let schema = IntentSchema {
            intent_id,
            raw_input: "open motor".into(),
            goal: "open_files".into(),
            domain: Some("robotics".into()),
            confidence: 0.9,
            ambiguity_score: 0.1,
            risk_estimate: 0.1,
            required_context: vec![],
            candidate_actions: vec![],
            disambiguation_required: false,
            disambiguation_question: None,
            session_context: SessionContext {
                last_active_domain: None,
                last_active_files: vec![],
                current_time: "10:00".into(),
                time_since_last_session: None,
            },
            hal_pre_score: 0.14,
            escalate_to_cloud: false,
            source: None,
        };
        let resolved = ResolvedIntent {
            intent_id,
            selected_action: CandidateAction {
                action: "open_files".into(),
                target: "~/projects/motor.py".into(),
                confidence: 0.9,
                recency_score: 0.8,
            },
            was_disambiguated: false,
            disambiguation_question: None,
            user_response: None,
        };
        let g = ActionGraph::from_resolved(&resolved, &schema);
        let order = g.execution_order().expect("order");
        assert_eq!(order.len(), 1);
        assert!((order[0].hal_pre_score - 0.14).abs() < f32::EPSILON);
    }
}
