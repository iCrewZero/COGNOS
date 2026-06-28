//! Event bus — fan-out pub/sub for orchestrator events (task state changes,
//! agent heartbeats, HAL decisions). Backed by tokio broadcast channels.
//!
//! v0: stub implementation.

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::broadcast;

use crate::task_graph::{AgentId, NodeState, TaskId};

// ─── Hal verdict ────────────────────────────────────────────────────────────

/// Decision returned by the HAL for a capability gate. v0: a stand-in for
/// the canonical `hal::HALDecision` enum; v1 will re-export the real type
/// once the orchestrator crate depends on the HAL crate directly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HalVerdict {
    /// Action permitted.
    Allow,
    /// Action permitted with a non-blocking user notice.
    AllowWithNotice,
    /// Defer pending user confirmation.
    Ask,
    /// Surface a notification to the user (no defer).
    Notify,
    /// Block at standard severity.
    Block,
    /// Block and raise an operator-visible alert.
    BlockAndAlert,
}

// ─── Event ──────────────────────────────────────────────────────────────────

/// Events published on the orchestrator [`EventBus`]. All variants are
/// [`Clone`] so lagging subscribers can drop a copy without losing the
/// canonical event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Event {
    /// A new task node was created and added to the graph.
    TaskCreated { task_id: TaskId, agent_id: AgentId },
    /// A task node changed state.
    TaskStateChanged { task_id: TaskId, old_state: NodeState, new_state: NodeState },
    /// An agent heartbeat was received.
    AgentHeartbeat(AgentId),
    /// The HAL returned a verdict for a capability gate.
    HalDecision(HalVerdict),
    /// A task node failed terminally with the given error message.
    TaskFailed(TaskId, String),
    /// A task node succeeded with the given JSON output.
    TaskSucceeded(TaskId, serde_json::Value),
    /// The orchestrator is shutting down; subscribers should drain.
    Shutdown,
}

// ─── Errors ─────────────────────────────────────────────────────────────────

/// Errors returned by [`EventBus::publish`] and surfaced by lagging
/// receivers.
#[derive(Debug, Error)]
pub enum BusError {
    /// The broadcast channel is closed — there are no live receivers and
    /// the publisher side was dropped, or the bus was shut down.
    #[error("event bus channel closed")]
    ChannelClosed,
    /// A subscriber is lagging behind the configured buffer size and is
    /// dropping messages. This is reported by the receiver side
    /// (`broadcast::Receiver::recv` returns `RecvError::Lagged`); the
    /// variant is surfaced here so callers can centralise bus error
    /// handling.
    #[error("slow subscriber dropped messages")]
    SlowSubscriber,
}

// ─── EventBus ───────────────────────────────────────────────────────────────

/// Fan-out pub/sub bus for orchestrator events, backed by a
/// `tokio::sync::broadcast` channel. The bus is cheaply cloneable: each
/// call to [`EventBus::subscribe`] returns an independent receiver with
/// its own lag queue of size `rx_buffer_size`.
pub struct EventBus {
    /// Broadcast sender. Cloning is cheap (Arc-backed).
    pub tx: broadcast::Sender<Event>,
    /// Configured per-subscriber buffer size.
    pub rx_buffer_size: usize,
}

impl EventBus {
    /// Construct a new bus with the given per-subscriber buffer size.
    pub fn new(buffer_size: usize) -> Self {
        let (tx, _rx) = broadcast::channel(buffer_size);
        // Drop the bootstrap receiver — subscribers are created on demand
        // via `subscribe()`. Keeping it would prevent `publish` from ever
        // observing `ChannelClosed`.
        drop(_rx);
        Self {
            tx,
            rx_buffer_size: buffer_size,
        }
    }

    /// Subscribe to the bus. Each returned receiver gets its own lag queue
    /// of size `rx_buffer_size`.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }

    /// Publish an event to all current subscribers. Returns
    /// [`BusError::ChannelClosed`] if there are no live receivers.
    ///
    /// Slow subscribers are handled by tokio's broadcast channel itself:
    /// a receiver that falls behind `rx_buffer_size` skips the oldest
    /// events and surfaces a `RecvError::Lagged` on its next `recv()`.
    /// v0 does not aggregate or replay those skipped events.
    pub fn publish(&self, event: Event) -> Result<(), BusError> {
        // TODO(v1): on `Lagged`, publish a synthetic `SlowSubscriber` metric
        // and optionally enqueue the dropped events on a per-subscriber
        // replay queue for at-least-once delivery of `Shutdown` and
        // `HalDecision` variants.
        match self.tx.send(event) {
            Ok(_n_receivers) => Ok(()),
            Err(_send_err) => Err(BusError::ChannelClosed),
        }
    }
}

impl Clone for EventBus {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            rx_buffer_size: self.rx_buffer_size,
        }
    }
}


