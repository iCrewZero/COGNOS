//! Orchestrator runtime — coordinates multi-agent task execution.
//!
//! Takes a high-level intent, decomposes it into a DAG of tasks
//! using the intent-engine, and dispatches them via the scheduler.

use std::collections::HashMap;
use std::time::Instant;

use cognos_ipc_grpc::client::CognosClient;
use cognos_ipc_grpc::pipeline_metrics::{log_stage, METRICS};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::event_bus::{Event, EventBus};
use crate::executor;
use crate::hal_gate::{self, Decision, SideEffect};
use crate::intent_adapter::{self, DecompositionPlan};
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
    #[error("HAL gate call failed: {0}")]
    HalGate(String),
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

// ─── Dispatch outcome ───────────────────────────────────────────────────────

/// Result of attempting to dispatch a single [`TaskNode`], after the HAL gate.
#[derive(Debug, Clone, PartialEq)]
pub enum DispatchOutcome {
    /// The node was dispatched (marked [`NodeState::Running`]). Read-only nodes
    /// take this path directly; side-effecting nodes only after HAL granted.
    Dispatched,
    /// HAL requires explicit user approval; the node is parked in
    /// [`NodeState::AwaitingHal`] and NOT dispatched.
    AwaitingApproval { risk_score: f64 },
    /// HAL denied the action; the node is marked [`NodeState::Failed`] and NOT
    /// dispatched.
    Denied { reason: String },
}

/// Per-task execution record returned to the CLI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskExecutionRecord {
    pub task_id: String,
    pub action: String,
    pub target: String,
    pub capability: String,
    pub agent: String,
    pub status: String,
    pub hal_decision: Option<String>,
    pub hal_risk_score: Option<f64>,
    pub message: Option<String>,
}

/// Per-stage latency for one intent pipeline run (ms).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PipelineLatency {
    pub total_ms: u64,
    pub parse_ms: u64,
    pub orchestrate_ms: u64,
    pub execute_ms: u64,
}

/// End-to-end result of submit + execute for one user utterance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionReport {
    pub intent_id: String,
    pub trace_id: String,
    pub success: bool,
    pub summary: String,
    pub tasks: Vec<TaskExecutionRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency: Option<PipelineLatency>,
}

// ─── OrchestratorRuntime ────────────────────────────────────────────────────

pub struct OrchestratorRuntime {
    pub graph: TaskGraph,
    pub bus: EventBus,
    pub scheduler: OrchestratorScheduler,
    pub agents: AgentRegistry,
    /// Optional connected HAL gate client. When present, side-effecting nodes
    /// are gated through it before dispatch; when absent, side-effecting nodes
    /// fail closed (see [`OrchestratorRuntime::dispatch_node`]).
    hal_gate: Option<CognosClient>,
    /// Optional connected intent-engine client. When present, intents are
    /// decomposed via `DispatchIntent`; on transport failure the legacy keyword
    /// classifier is used instead (logged at WARNING).
    intent_client: Option<CognosClient>,
}

impl OrchestratorRuntime {
    pub async fn new() -> Self {
        Self {
            graph: TaskGraph::new(),
            bus: EventBus::new(1024),
            scheduler: OrchestratorScheduler::new(),
            agents: AgentRegistry::default(),
            hal_gate: None,
            intent_client: None,
        }
    }

    /// Attach a connected intent-engine client. Once attached, every intent
    /// submitted via [`OrchestratorRuntime::submit`] is sent to the engine for
    /// decomposition; transport errors fall back to the local keyword path.
    pub fn attach_intent_client(&mut self, client: CognosClient) {
        self.intent_client = Some(client);
    }

    /// Whether an intent-engine client is attached.
    pub fn has_intent_client(&self) -> bool {
        self.intent_client.is_some()
    }

    /// Attach a connected HAL gate client. Once attached, every side-effecting
    /// node routed through [`OrchestratorRuntime::dispatch_node`] is gated.
    pub fn attach_hal_gate(&mut self, client: CognosClient) {
        self.hal_gate = Some(client);
    }

    /// Whether a HAL gate client is attached.
    pub fn has_hal_gate(&self) -> bool {
        self.hal_gate.is_some()
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

        // Decompose via intent-engine when a client is attached; fall back to the
        // legacy keyword classifier on transport errors only.
        let plan = self.resolve_decomposition(&intent).await?;

        // Add each node to the graph, wiring dependencies from the plan.
        let mut node_ids: Vec<TaskId> = Vec::new();
        for planned in &plan.nodes {
            let sub = &planned.sub_task;
            let agent = self
                .agents
                .select_for_capability(&sub.capability)
                .unwrap_or_else(|| AgentId("agent.coordinator".to_string()));

            let intent_value = serde_json::json!({
                "description": sub.description,
                "capability": sub.capability,
                "action": sub.action,
                "target": sub.target,
            });

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

            let _ = self.bus.publish(Event::TaskCreated {
                task_id: node_id,
                agent_id: agent,
            });
        }

        for &(from_idx, to_idx) in &plan.edges {
            let from = node_ids[from_idx];
            let to = node_ids[to_idx];
            self.graph
                .add_dependency(from, to, crate::task_graph::EdgeKind::Control)
                .map_err(|e| OrchestratorError::DecompositionFailed(e.to_string()))?;
        }

        let ready_roots: Vec<TaskId> = self.graph.ready_tasks();
        if ready_roots.is_empty() {
            return Err(OrchestratorError::DecompositionFailed(
                "intent produced zero ready sub-tasks".into(),
            ));
        }

        let priority_val = match intent.priority {
            IntentPriority::Background => 0,
            IntentPriority::Normal => 50,
            IntentPriority::High => 75,
            IntentPriority::Critical => 100,
        };

        for root_id in &ready_roots {
            if let Some(node) = self.graph.get_mut(*root_id) {
                node.state = NodeState::Ready;
            }
            let agent = self
                .graph
                .get(*root_id)
                .map(|n| n.agent.clone())
                .unwrap_or_else(|| AgentId("agent.coordinator".to_string()));
            self.scheduler.enqueue(
                crate::scheduler::SchedEntry::new(
                    *root_id,
                    agent.clone(),
                    crate::scheduler::Priority(priority_val),
                ),
            );
            debug!(?root_id, "enqueued root task with scheduler");
            let _ = self.bus.publish(Event::TaskStateChanged {
                task_id: *root_id,
                old_state: NodeState::Pending,
                new_state: NodeState::Ready,
            });
        }

        let root_id = ready_roots[0];
        info!(?root_id, total_nodes = node_ids.len(), roots = ready_roots.len(), "decomposed intent into DAG");
        Ok(root_id)
    }

    /// Resolve an intent into a [`DecompositionPlan`].
    ///
    /// When an intent-engine client is attached, tries `DispatchIntent` first.
    /// Transport / timeout failures log a WARNING and fall back to the legacy
    /// keyword classifier. Semantic failures (bad status, cycle, …) propagate
    /// as [`OrchestratorError::DecompositionFailed`] without falling back.
    async fn resolve_decomposition(
        &self,
        intent: &Intent,
    ) -> Result<DecompositionPlan, OrchestratorError> {
        if let Some(client) = &self.intent_client {
            match intent_adapter::decompose_via_intent_engine(client, intent).await {
                Ok(plan) => {
                    info!(
                        nodes = plan.nodes.len(),
                        edges = plan.edges.len(),
                        "decomposed intent via intent-engine"
                    );
                    return Ok(plan);
                }
                Err(e) if e.is_transport() => {
                    warn!(
                        error = %e,
                        "intent-engine unreachable — falling back to local keyword decomposition"
                    );
                }
                Err(e) => {
                    return Err(OrchestratorError::DecompositionFailed(e.to_string()));
                }
            }
        }
        let plan = intent_adapter::decompose_locally(intent);
        debug!(
            nodes = plan.nodes.len(),
            "decomposed intent via local keyword fallback"
        );
        Ok(plan)
    }

    /// Dispatch a single ready node to its agent, gating side-effecting nodes
    /// through HAL first.
    ///
    /// This is the orchestrator's execution point: **every action with a side
    /// effect passes through [`hal_gate::gate_action`] before it is dispatched
    /// to an agent.** Read-only nodes are dispatched immediately. Side-effecting
    /// nodes are gated:
    ///   * `Granted`           → the node is marked [`NodeState::Running`]
    ///     (dispatched) and the grant proceeds;
    ///   * `ApprovalRequired`  → the node is parked in [`NodeState::AwaitingHal`];
    ///   * `Denied`            → the node is marked [`NodeState::Failed`].
    ///
    /// If a node is side-effecting but no HAL gate client is attached, the
    /// orchestrator **fails closed**: the node is not dispatched and
    /// [`OrchestratorError::HalDenied`] is returned. This guarantees no
    /// side-effecting action is ever dispatched ungated.
    pub async fn dispatch_node(
        &mut self,
        node_id: TaskId,
        trace_id: &str,
    ) -> Result<(DispatchOutcome, Option<String>), OrchestratorError> {
        // Extract the action descriptor from the node's intent blob.
        let (op, capability, path, source_agent) = {
            let node = self
                .graph
                .get(node_id)
                .ok_or(OrchestratorError::Internal)?;
            let field = |key: &str| {
                node.intent
                    .get(key)
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            };
            let op = field("action").unwrap_or_else(|| "execute".to_string());
            let capability = field("capability").unwrap_or_else(|| "general.execute".to_string());
            // SubTasks don't carry a concrete target yet; fall back to the
            // human-readable description so HAL still gets a resource string.
            let path = field("target")
                .or_else(|| field("path"))
                .or_else(|| field("description"))
                .unwrap_or_default();
            (op, capability, path, node.agent.0.clone())
        };

        // Read-only work is dispatched without a gate.
        if !hal_gate::is_side_effecting(&capability) {
            self.graph.mark_running(node_id);
            debug!(?node_id, %capability, "dispatched read-only node (no gate)");
            return Ok((DispatchOutcome::Dispatched, None));
        }

        // Side-effecting work must pass HAL. Fail closed when no gate is wired.
        let Some(client) = self.hal_gate.as_ref() else {
            warn!(?node_id, %capability, "side-effecting node with no HAL gate — failing closed");
            if let Some(node) = self.graph.get_mut(node_id) {
                node.state = NodeState::Failed;
            }
            return Err(OrchestratorError::HalDenied);
        };

        let action = SideEffect::new(op, path, capability, source_agent);
        let decision = hal_gate::gate_action(client, &action, trace_id)
            .await
            .map_err(|e| OrchestratorError::HalGate(e.to_string()))?;

        match decision {
            Decision::Granted { risk_score, .. } => {
                self.graph.mark_running(node_id);
                info!(?node_id, action = %action.op, risk_score, "HAL granted — dispatching");
                Ok((
                    DispatchOutcome::Dispatched,
                    Some(format!("granted (risk={risk_score:.2})")),
                ))
            }
            Decision::ApprovalRequired { risk_score } => {
                if let Some(node) = self.graph.get_mut(node_id) {
                    let old = node.state;
                    node.state = NodeState::AwaitingHal;
                    let _ = self.bus.publish(Event::TaskStateChanged {
                        task_id: node_id,
                        old_state: old,
                        new_state: NodeState::AwaitingHal,
                    });
                }
                info!(
                    ?node_id,
                    action = %action.op,
                    risk_score,
                    "HAL requires approval — waiting on Unix gate"
                );

                #[cfg(unix)]
                {
                    use crate::approval_socket::{blocking_gate, gate_timeout, reason_label};

                    let action_for_gate = action.clone();
                    let trace = trace_id.to_string();
                    let wait = tokio::time::timeout(
                        gate_timeout(),
                        tokio::task::spawn_blocking(move || {
                            blocking_gate(&action_for_gate, &trace)
                        }),
                    )
                    .await;

                    match wait {
                        Ok(Ok(Ok(resp))) if resp.approved => {
                            self.graph.mark_running(node_id);
                            info!(
                                ?node_id,
                                reason = reason_label(&resp.reason),
                                "HAL user approval granted — dispatching"
                            );
                            return Ok((
                                DispatchOutcome::Dispatched,
                                Some(format!(
                                    "user_approved (risk={:.2}, reason={})",
                                    resp.hal_score,
                                    reason_label(&resp.reason)
                                )),
                            ));
                        }
                        Ok(Ok(Ok(resp))) => {
                            if let Some(node) = self.graph.get_mut(node_id) {
                                node.state = NodeState::Failed;
                            }
                            let reason = format!(
                                "user denied (hal_reason={})",
                                reason_label(&resp.reason)
                            );
                            warn!(?node_id, %reason, "HAL user denied after approval wait");
                            return Ok((
                                DispatchOutcome::Denied { reason: reason.clone() },
                                Some(format!("denied: {reason}")),
                            ));
                        }
                        Ok(Ok(Err(e))) => {
                            if let Some(node) = self.graph.get_mut(node_id) {
                                node.state = NodeState::Failed;
                            }
                            let reason = format!("approval gate error: {e}");
                            warn!(?node_id, %reason, "HAL approval gate failed");
                            return Ok((
                                DispatchOutcome::Denied { reason: reason.clone() },
                                Some(format!("denied: {reason}")),
                            ));
                        }
                        Ok(Err(e)) => {
                            if let Some(node) = self.graph.get_mut(node_id) {
                                node.state = NodeState::Failed;
                            }
                            let reason = format!("approval gate task failed: {e}");
                            return Ok((
                                DispatchOutcome::Denied { reason: reason.clone() },
                                Some(format!("denied: {reason}")),
                            ));
                        }
                        Err(_) => {
                            if let Some(node) = self.graph.get_mut(node_id) {
                                node.state = NodeState::Failed;
                            }
                            let secs = crate::approval_socket::approval_timeout_secs();
                            let reason = format!(
                                "approval timeout after {secs}s — auto-denied"
                            );
                            warn!(?node_id, %reason, "orchestrator approval wait timed out");
                            return Ok((
                                DispatchOutcome::Denied { reason: reason.clone() },
                                Some(format!("denied: {reason}")),
                            ));
                        }
                    }
                }

                #[cfg(not(unix))]
                {
                    Ok((
                        DispatchOutcome::AwaitingApproval { risk_score },
                        Some(format!("approval_required (risk={risk_score:.2})")),
                    ))
                }
            }
            Decision::Denied { reason, risk_score } => {
                if let Some(node) = self.graph.get_mut(node_id) {
                    node.state = NodeState::Failed;
                }
                warn!(?node_id, action = %action.op, %reason, "HAL denied — not dispatching");
                Ok((
                    DispatchOutcome::Denied {
                        reason: reason.clone(),
                    },
                    Some(format!("denied: {reason} (risk={risk_score:.2})")),
                ))
            }
        }
    }

    /// Clear the task graph and scheduler between ingress requests.
    pub fn reset_execution_state(&mut self) {
        self.graph = TaskGraph::new();
        self.scheduler = OrchestratorScheduler::new();
    }

    /// Submit an intent, run the scheduler until idle, and return an execution
    /// report suitable for the CLI / ingress handler.
    pub async fn submit_and_execute(
        &mut self,
        utterance: &str,
        user_id: &str,
        trace_id: &str,
    ) -> Result<ExecutionReport, OrchestratorError> {
        self.reset_execution_state();
        let pipeline_started = Instant::now();
        let intent = Intent::new(user_id, utterance);
        let intent_id = intent.id.0.to_string();

        let parse_started = Instant::now();
        let _root = self.submit(intent).await?;
        let parse_ms = parse_started.elapsed().as_millis() as u64;

        let mut tasks = Vec::new();
        let mut execute_ms = 0u64;

        while let Some(entry) = self.scheduler.next() {
            let node_id = entry.task_id;
            let (action, target, capability, agent) = {
                let node = self.graph.get(node_id).ok_or(OrchestratorError::Internal)?;
                let field = |key: &str| {
                    node.intent
                        .get(key)
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                };
                (
                    field("action").unwrap_or_default(),
                    field("target").unwrap_or_default(),
                    field("capability").unwrap_or_default(),
                    node.agent.0.clone(),
                )
            };

            let (outcome, hal_note) = self.dispatch_node(node_id, trace_id).await?;
            let (status, message, success) = match &outcome {
                DispatchOutcome::Dispatched => {
                    let node_snapshot = self
                        .graph
                        .get(node_id)
                        .cloned()
                        .ok_or(OrchestratorError::Internal)?;
                    let exec_started = Instant::now();
                    let result =
                        executor::execute_node(&node_snapshot, "", trace_id).await;
                    execute_ms += exec_started.elapsed().as_millis() as u64;
                    let ok = result.error.is_none();
                    self.graph.mark_completed(node_id, result.clone());
                    self.scheduler.complete(node_id);
                    self.enqueue_ready_dependents(node_id);
                    (
                        if ok { "succeeded" } else { "failed" }.to_string(),
                        result.error.clone().or_else(|| {
                            result
                                .output
                                .get("message")
                                .and_then(|m| m.as_str())
                                .map(|s| s.to_string())
                        }),
                        ok,
                    )
                }
                DispatchOutcome::AwaitingApproval { .. } => {
                    ("awaiting_hal".to_string(), hal_note.clone(), false)
                }
                DispatchOutcome::Denied { reason } => {
                    ("denied".to_string(), Some(reason.clone()), false)
                }
            };

            tasks.push(TaskExecutionRecord {
                task_id: node_id.0.to_string(),
                action,
                target,
                capability,
                agent,
                status: status.clone(),
                hal_decision: hal_note,
                hal_risk_score: None,
                message,
            });

            if !success && status != "awaiting_hal" {
                break;
            }
        }

        let overall = !tasks.is_empty() && tasks.iter().all(|t| t.status == "succeeded");
        let summary = if overall {
            format!("completed {} task(s)", tasks.len())
        } else {
            format!(
                "finished with failures — last status: {}",
                tasks.last().map(|t| t.status.as_str()).unwrap_or("none")
            )
        };

        let total_ms = pipeline_started.elapsed().as_millis() as u64;
        let orchestrate_ms = total_ms
            .saturating_sub(parse_ms)
            .saturating_sub(execute_ms);
        let latency = PipelineLatency {
            total_ms,
            parse_ms,
            orchestrate_ms,
            execute_ms,
        };
        METRICS.record_latency(trace_id, parse_ms, orchestrate_ms, execute_ms);
        log_stage(trace_id, "orchestration", orchestrate_ms);
        log_stage(trace_id, "execution", execute_ms);
        tracing::info!(
            trace_id = %trace_id,
            stage = "pipeline_total",
            latency_ms = total_ms,
            parse_ms = parse_ms,
            orchestrate_ms = orchestrate_ms,
            execute_ms = execute_ms,
            "pipeline stage"
        );

        Ok(ExecutionReport {
            intent_id,
            trace_id: trace_id.to_string(),
            success: overall,
            summary,
            tasks,
            latency: Some(latency),
        })
    }

    fn enqueue_ready_dependents(&mut self, completed: TaskId) {
        let children: Vec<TaskId> = self
            .graph
            .edges
            .iter()
            .filter(|e| e.from == completed)
            .map(|e| e.to)
            .collect();
        for child in children {
            let ready = self
                .graph
                .edges
                .iter()
                .filter(|e| e.to == child)
                .all(|e| {
                    self.graph
                        .get(e.from)
                        .map(|n| n.state == NodeState::Succeeded)
                        .unwrap_or(false)
                });
            if !ready {
                continue;
            }
            let agent = self.graph.get(child).map(|n| n.agent.clone());
            if let Some(node) = self.graph.get_mut(child) {
                if node.state == NodeState::Pending {
                    node.state = NodeState::Ready;
                    if let Some(agent) = agent {
                        self.scheduler.enqueue(crate::scheduler::SchedEntry::new(
                            child,
                            agent,
                            crate::scheduler::Priority(50),
                        ));
                    }
                }
            }
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
pub(crate) fn classify_intent(text: &str) -> String {
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

/// A sub-task produced by decomposing an intent (legacy keyword path).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SubTask {
    pub description: String,
    pub capability: String,
    pub action: String,
}

/// Break a classified intent into an ordered list of sub-tasks.
/// Produces a linear chain; kept as the transport-failure fallback.
pub(crate) fn decompose_into_tasks(action: &str, intent: &Intent) -> Vec<SubTask> {
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
    if graph.get(task_id).is_some() {
        // Walk outgoing edges to find successor task IDs.
        for edge in graph.edges.iter().filter(|e| e.from == task_id) {
            if let Some(dep) = graph.get(edge.to) {
                states.push(dep.state);
                collect_descendant_states(graph, edge.to, states);
            }
        }
    }
}