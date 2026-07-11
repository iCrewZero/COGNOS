//! COGNOS Orchestrator — coordinates multi-agent task execution.
//!
//! The orchestrator takes a high-level intent, decomposes it into a DAG
//! of tasks, and dispatches them to agents through the scheduler.

pub mod approval_socket;
pub mod event_bus;
pub mod executor;
pub mod hal_gate;
pub mod intent_adapter;
pub mod runtime;
pub mod scheduler;
pub mod serve;
pub mod task_graph;

pub use hal_gate::{gate_action, Decision, GateError, SideEffect};
pub use runtime::{
    DispatchOutcome, ExecutionReport, OrchestratorError, OrchestratorRuntime, Intent,
    IntentPriority, IntentId, TaskStatus, TaskExecutionRecord,
};
pub use task_graph::{TaskId, AgentId, TaskNode, NodeState};
