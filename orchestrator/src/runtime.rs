//! Orchestrator runtime — coordinates multi-agent task execution.
//!
//! Takes a high-level intent, decomposes it into a DAG of tasks
//! using the intent-engine, and dispatches them via the scheduler.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::event_bus::{Event, EventBus};
use crate::scheduler::OrchestratorScheduler;
use crate::task_graph::{NodeState, TaskGraph, TaskNode};

pub use crate::task_graph::{AgentId, TaskId};

// ─── Identifiers ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IntentId(pub Uuid);

// ─── Intent ─────────────────────────────────────────────────────────────────

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize,
)]
#[repr(u8)]
pub enum IntentPriority {
    Background = 0,
    Normal = 1,
    High = 2,
    Critical = 3,
}

impl Default for IntentPriority {
    fn default() -> Self {
        Self::Normal
    }
}

/// A high-level user intent submitted to the orchestrator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Intent {
    pub id: IntentId,
    pub user_id: String,
    pub text: String,
    pub context: HashMap<String, serde_json::Value>,
    pub priority: IntentPriority,
}

impl Intent {
    pub fn new(user_id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            id: IntentId(Uuid::new_v4()),
            user_id: user_id.into(),
            text: text.into(),
            context: HashMap::new(),
            priority: IntentPriority::default(),
        }
    }
}

// ─── Task status ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskStatus {
    Pending,
    Running,
    AwaitingHal,
    Succeeded,
    Failed,
    Cancelled,
}

// ─── Errors ─────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum OrchestratorError {
    #[error("intent decomposition failed: {0}")]
    DecompositionFailed(String),
    #[error("no agent available for required capability: {0}")]
    NoAgent(String),
    #[error("HAL denied required capability")]
    HalDenied,
    #[error("internal orchestrator error")]
    Internal,
}

// ─── Agent registry ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDescriptor {
    pub id: AgentId,
    pub name: String,
    pub capabilities: Vec<String>,
    pub available: bool,
}

#[derive(Debug, Default)]
pub struct AgentRegistry {
    pub agents: HashMap<AgentId, AgentDescriptor>,
}

impl AgentRegistry {
    pub fn register(&mut self, descriptor: AgentDescriptor) {
        self.agents.insert(descriptor.id.clone(), descriptor);
    }

    pub fn get(&self, id: &AgentId) -> Option<&AgentDescriptor> {
        self.agents.get(id)
    }

    /// Pick an available agent that advertises `capability`.
    /// Falls back to "agent.coordinator" if no match is found.
    pub fn select_for_capability(&self, capability: &str) -> Option<AgentId> {
        self.agents
            .values()
            .find(|a| a.available && a.capabilities.iter().any(|c| c == capability))
            .map(|a| a.id.clone())
    }
}

// ─── OrchestratorRuntime ────────────────────────────────────────────────────

pub struct OrchestratorRuntime {
    pub graph: TaskGraph,
    pub bus: EventBus,
    pub scheduler: OrchestratorScheduler,
    pub agents: AgentRegistry,
}

impl OrchestratorRuntime {
    pub async fn new() -> Self {
        Self {
            graph: TaskGraph::new(),
            bus: EventBus::new(1024),
            scheduler: OrchestratorScheduler::new(),
            agents: AgentRegistry::default(),
        }
    }

    /// Submit a high-level intent. Decomposes it into a multi-node DAG
    /// by parsing the intent text into a canonical action, then building
    /// a graph of sub-tasks. Each node is routed to an agent via the
    /// registry. The ready set is enqueued with the scheduler.
    pub async fn submit(
        &mut self,
        intent: Intent,
    ) -> Result<TaskId, OrchestratorError> {
        info!(intent_id = ?intent.id, priority = ?intent.priority, text = %intent.text, "submitting intent");

        // Step 1: Classify the intent into a canonical action type.
        //
        // In production, the orchestrator sends the raw intent text to the
        // intent-engine (a separate Rust crate) via the IPC gRPC layer.
        // The intent-engine runs its parser, disambiguator, and action graph
        // builder, and returns a structured ActionGraph proto.
        //
        // For now, we use the local fallback classifier. When the
        // intent-engine crate is wired as a workspace dependency, replace
        // the call below with:
        //
        //   let ipc = self.ipc_client.as_ref()
        //       .ok_or(OrchestratorError::Internal)?;
        //   let response = ipc.dispatch_intent(Intent {
        //       intent_id: intent.id.0.to_string(),
        //       utterance: intent.text.clone(),
        //       ..Default::default()
        //   }).await.map_err(|e| OrchestratorError::DecompositionFailed(e.to_string()))?;
        //   let action_graph: ActionGraph = parse_action_graph(&response.result_json);
        //   let sub_tasks = action_graph_to_sub_tasks(&action_graph);
        //
        let action = classify_intent(&intent.text);

        // Step 2: Decompose into a DAG of sub-tasks.
        let sub_tasks = decompose_into_tasks(&action, &intent);

        // Step 3: Add each node to the graph, wiring dependencies.
        let mut node_ids: Vec<TaskId> = Vec::new();
        for (i, sub) in sub_tasks.iter().enumerate() {
            // Pick the best agent for this sub-task's required capability.
            let agent = self
                .agents
                .select_for_capability(&sub.capability)
                .unwrap_or_else(|| AgentId("agent.coordinator".to_string()));

            let intent_value = serde_json::to_value(sub)
                .map_err(|e| OrchestratorError::DecompositionFailed(e.to_string()))?;

            let node_id = self.graph.add_task(TaskNode {
                id: TaskId(Uuid::nil()),
                agent: agent.clone(),
                intent: intent_value,
                state: NodeState::Pending,
                inputs: Vec::new(),
                outputs: Vec::new(),
                retry_count: 0,
            });
            node_ids.push(node_id);

            // Wire edges: each node depends on the previous one (linear chain).
            // The intent-engine would produce a real DAG in production;
            // this is a reasonable default for simple sequential intents.
            if i > 0 {
                self.graph
                    .add_dependency(
                        node_ids[i - 1],
                        node_id,
                        crate::task_graph::EdgeKind::Control,
                    )
                    .map_err(|e| OrchestratorError::DecompositionFailed(e.to_string()))?;
            }

            // Publish node creation event.
            let _ = self.bus.publish(Event::TaskCreated {
                task_id: node_id,
                agent_id: agent,
            });
        }

        // Step 4: Mark the root node as Ready and enqueue it.
        // Nodes with no predecessors (i == 0) are immediately ready.
        if let Some(&root_id) = node_ids.first() {
            if let Some(node) = self.graph.get_mut(root_id) {
                node.state = NodeState::Ready;
            }

            let priority_val = match intent.priority {
                IntentPriority::Background => 0,
                IntentPriority::Normal => 50,
                IntentPriority::High => 75,
                IntentPriority::Critical => 100,
            };

            self.scheduler.enqueue(
                crate::scheduler::SchedEntry::new(
                    root_id,
                    self.agents
                        .select_for_capability("general.execute")
                        .unwrap_or_else(|| AgentId("agent.coordinator".to_string())),
                    crate::scheduler::Priority(priority_val),
                ),
            );
            debug!(?root_id, "enqueued root task with scheduler");
            let _ = self.bus.publish(Event::TaskStateChanged {
                task_id: root_id,
                old_state: NodeState::Pending,
                new_state: NodeState::Ready,
            });

            info!(?root_id, total_nodes = node_ids.len(), "decomposed intent into DAG");
            Ok(root_id)
        } else {
            Err(OrchestratorError::DecompositionFailed(
                "intent produced zero sub-tasks".into(),
            ))
        }
    }

    /// Cancel a task and all its descendants (nodes that depend on it).
    pub async fn cancel(&mut self, task: TaskId) -> Result<(), OrchestratorError> {
        info!(?task, "cancel requested");

        // Collect this task and all its descendants by walking the edge list.
        let mut to_cancel = vec![task];
        let edges = &self.graph.edges;
        let mut frontier = vec![task];
        while let Some(current) = frontier.pop() {
            for edge in edges.iter().filter(|e| e.from == current) {
                to_cancel.push(edge.to);
                frontier.push(edge.to);
            }
        }

        for nid in &to_cancel {
            if let Some(node) = self.graph.get_mut(*nid) {
                let old = node.state;
                node.state = NodeState::Skipped;
                let _ = self.bus.publish(Event::TaskStateChanged {
                    task_id: *nid,
                    old_state: old,
                    new_state: NodeState::Skipped,
                });
            }
        }

        info!(cancelled = to_cancel.len(), "tasks cancelled");
        Ok(())
    }

    /// Query the aggregate status of a task and its descendants.
    pub async fn status(&self, task: TaskId) -> Result<TaskStatus, OrchestratorError> {
        // Walk the task and all its dependents.
        let mut states = Vec::new();
        if let Some(node) = self.graph.get(task) {
            states.push(node.state);
            collect_descendant_states(&self.graph, task, &mut states);
        }

        if states.is_empty() {
            return Ok(TaskStatus::Pending);
        }

        // Reduce: if any Failed → Failed. If any Running/Ready → Running.
        // If any AwaitingHal → AwaitingHal. If all Succeeded → Succeeded.
        if states.contains(&NodeState::Failed) {
            Ok(TaskStatus::Failed)
        } else if states.contains(&NodeState::Skipped) {
            Ok(TaskStatus::Cancelled)
        } else if states.iter().any(|s| matches!(s, NodeState::Running | NodeState::Ready)) {
            Ok(TaskStatus::Running)
        } else if states.contains(&NodeState::AwaitingHal) {
            Ok(TaskStatus::AwaitingHal)
        } else if states.iter().all(|s| *s == NodeState::Succeeded) {
            Ok(TaskStatus::Succeeded)
        } else {
            Ok(TaskStatus::Pending)
        }
    }

    /// Gracefully shut down.
    pub async fn shutdown(self) -> Result<(), OrchestratorError> {
        info!("orchestrator runtime shutting down");
        if let Err(e) = self.bus.publish(Event::Shutdown) {
            warn!(error = ?e, "failed to publish Shutdown event");
        }
        Ok(())
    }
}

impl Default for OrchestratorRuntime {
    fn default() -> Self {
        // Block on async in default — callers should use `new()` directly.
        // This is a convenience for testing.
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(Self::new())
        })
    }
}

// ─── Intent classification ──────────────────────────────────────────────────

/// Map natural-language intent text to a canonical action type.
fn classify_intent(text: &str) -> String {
    let lower = text.to_lowercase();

    let rules: &[(&str, &str)] = &[
        ("open", "file.open"),
        ("find", "file.find"),
        ("search", "file.search"),
        ("move", "file.move"),
        ("rename", "file.rename"),
        ("delete", "file.delete"),
        ("install", "pkg.install"),
        ("uninstall", "pkg.uninstall"),
        ("update", "pkg.update"),
        ("config", "system.config"),
        ("settings", "system.config"),
        ("permission", "security.permission"),
        ("audit", "security.audit"),
        ("check", "security.check"),
        ("code", "coding.task"),
        ("implement", "coding.implement"),
        ("refactor", "coding.refactor"),
        ("debug", "coding.debug"),
        ("fix", "coding.fix"),
        ("write", "coding.write"),
    ];

    for (keyword, action) in rules {
        if lower.contains(keyword) {
            return action.to_string();
        }
    }

    // Default: treat as a general intent that needs disambiguation.
    "intent.general".to_string()
}

// ─── Task decomposition ─────────────────────────────────────────────────────

/// A sub-task produced by decomposing an intent.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SubTask {
    pub description: String,
    pub capability: String,
    pub action: String,
}

/// Break a classified intent into an ordered list of sub-tasks.
/// Produces a linear chain; the intent-engine would produce a real DAG.
fn decompose_into_tasks(action: &str, intent: &Intent) -> Vec<SubTask> {
    match action {
        "file.open" => vec![
            SubTask {
                description: format!("Resolve target for: {}", intent.text),
                capability: "memory.read".into(),
                action: "resolve_target".into(),
            },
            SubTask {
                description: format!("Open: {}", intent.text),
                capability: "file.read".into(),
                action: "execute_open".into(),
            },
        ],
        "file.find" | "file.search" => vec![
            SubTask {
                description: format!("Search memory for context: {}", intent.text),
                capability: "memory.read".into(),
                action: "memory_search".into(),
            },
            SubTask {
                description: format!("Execute file search: {}", intent.text),
                capability: "file.read".into(),
                action: "file_search".into(),
            },
        ],
        "coding.task" | "coding.implement" | "coding.write" | "coding.fix"
        | "coding.refactor" | "coding.debug" => vec![
            SubTask {
                description: format!("Plan: {}", intent.text),
                capability: "coding.plan".into(),
                action: "create_plan".into(),
            },
            SubTask {
                description: format!("Implement: {}", intent.text),
                capability: "coding.execute".into(),
                action: "execute_code".into(),
            },
            SubTask {
                description: "Run tests and validate output".into(),
                capability: "coding.validate".into(),
                action: "validate".into(),
            },
        ],
        "pkg.install" | "pkg.uninstall" | "pkg.update" => vec![
            SubTask {
                description: format!("Security review: {}", intent.text),
                capability: "security.review".into(),
                action: "security_check".into(),
            },
            SubTask {
                description: format!("Execute package operation: {}", intent.text),
                capability: "pkg.execute".into(),
                action: "pkg_op".into(),
            },
        ],
        "security.permission" | "security.audit" | "security.check" => vec![
            SubTask {
                description: format!("Gather system state for: {}", intent.text),
                capability: "security.read".into(),
                action: "gather_state".into(),
            },
            SubTask {
                description: format!("Run security analysis: {}", intent.text),
                capability: "security.analyze".into(),
                action: "analyze".into(),
            },
        ],
        _ => vec![
            SubTask {
                description: format!("Disambiguate intent: {}", intent.text),
                capability: "intent.disambiguate".into(),
                action: "disambiguate".into(),
            },
            SubTask {
                description: format!("Execute: {}", intent.text),
                capability: "general.execute".into(),
                action: "execute".into(),
            },
        ],
    }
}

// ─── Helpers ────────────────────────────────────────────────────────────────

fn collect_descendant_states(
    graph: &TaskGraph,
    task_id: TaskId,
    states: &mut Vec<NodeState>,
) {
    if let Some(node) = graph.get(task_id) {
        // Walk outgoing edges to find successor task IDs.
        for edge in graph.edges.iter().filter(|e| e.from == task_id) {
            if let Some(dep) = graph.get(edge.to) {
                states.push(dep.state);
                collect_descendant_states(graph, edge.to, states);
            }
        }
    }
}