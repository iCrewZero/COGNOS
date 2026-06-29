//! COGNOS Orchestrator — coordinates multi-agent task execution.
//!
//! The orchestrator takes a high-level intent, decomposes it into a DAG
//! of tasks, and dispatches them to agents through the scheduler.

pub mod event_bus;
pub mod runtime;
pub mod scheduler;
pub mod task_graph;

pub use runtime::{OrchestratorRuntime, Intent, IntentPriority, IntentId, TaskStatus};
pub use task_graph::{TaskId, AgentId, TaskNode, NodeState};
