//! Behavior monitor — real-time per-agent behavior monitoring.
//!
//!
//! The behavior monitor is the *fast* signal to HAL's risk scorer. It
//! tracks syscall rate, file-op rate, network-op rate, and capability-use
//! rate per agent over a rolling 60-second window, and exposes an anomaly
//! score ∈ [0, 1] computed as an EWMA deviation from a per-agent baseline.
//!
//! The monitor is intentionally cheap: it must process thousands of events
//! per second without blocking the agent runtime. Heavy statistical work
//! is delegated to [`crate::anomaly_detection`] when a deeper read is needed.
//!
//! v0: stub implementation. EWMA math is in place; baseline learning is
//! TODO(v1).

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// v0: stub implementation

/// Type alias for an agent identifier.
pub type AgentId = String;

/// Length of the rolling window in seconds.
const WINDOW_SECS: i64 = 60;

/// EWMA smoothing factor (alpha). Higher = more reactive, less stable.
const EWMA_ALPHA: f32 = 0.20;

// ─── Behavior Events ────────────────────────────────────────────────────────────

/// A single observed behavior event from an agent.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BehaviorEvent {
    /// A syscall was made (any kind — see [`crate::syscall_tracker`] for
    /// per-syscall classification).
    Syscall,
    /// A file operation was performed (open, read, write, move, delete).
    FileOp,
    /// A network operation was performed (connect, send, recv).
    NetOp,
    /// A HAL-tracked capability was exercised.
    CapabilityUse,
}

// ─── Agent Behavior State ───────────────────────────────────────────────────────

/// Rolling per-agent behavior state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentBehavior {
    /// EWMA of syscalls per second.
    pub syscall_rate: f32,
    /// EWMA of file operations per second.
    pub file_ops_rate: f32,
    /// EWMA of network operations per second.
    pub net_ops_rate: f32,
    /// Aggregate anomaly score ∈ [0.0, 1.0].
    pub anomaly_score: f32,
    /// Start of the current rolling window.
    pub last_window: DateTime<Utc>,
    /// Counters for the current window (reset on rollover).
    pub syscall_count: u64,
    /// File-op counter for the current window.
    pub file_op_count: u64,
    /// Network-op counter for the current window.
    pub net_op_count: u64,
    /// Capability-use counter for the current window.
    pub capability_count: u64,
}

impl Default for AgentBehavior {
    fn default() -> Self {
        Self {
            syscall_rate: 0.0,
            file_ops_rate: 0.0,
            net_ops_rate: 0.0,
            anomaly_score: 0.0,
            last_window: Utc::now(),
            syscall_count: 0,
            file_op_count: 0,
            net_op_count: 0,
            capability_count: 0,
        }
    }
}

impl AgentBehavior {
    /// Observe a single behavior event, updating the rolling counters.
    pub fn observe(&mut self, event: BehaviorEvent) {
        self.maybe_rollover();
        match event {
            BehaviorEvent::Syscall => self.syscall_count += 1,
            BehaviorEvent::FileOp => self.file_op_count += 1,
            BehaviorEvent::NetOp => self.net_op_count += 1,
            BehaviorEvent::CapabilityUse => self.capability_count += 1,
        }
    }

    /// Roll over the window if WINDOW_SECS has elapsed, updating the EWMA
    /// rates from the per-window counters.
    pub fn maybe_rollover(&mut self) {
        let now = Utc::now();
        let elapsed = (now - self.last_window).num_seconds();
        if elapsed < WINDOW_SECS {
            return;
        }
        let secs = elapsed as f32;
        let new_syscall = self.syscall_count as f32 / secs;
        let new_file = self.file_op_count as f32 / secs;
        let new_net = self.net_op_count as f32 / secs;

        self.syscall_rate = ewma(self.syscall_rate, new_syscall);
        self.file_ops_rate = ewma(self.file_ops_rate, new_file);
        self.net_ops_rate = ewma(self.net_ops_rate, new_net);

        // Anomaly score is a placeholder combination in v0.
        // TODO(v1): replace with a z-score against a learned per-agent baseline.
        self.anomaly_score = ((new_syscall / 100.0)
            + (new_file / 10.0)
            + (new_net / 5.0))
            .clamp(0.0, 1.0);

        self.syscall_count = 0;
        self.file_op_count = 0;
        self.net_op_count = 0;
        self.capability_count = 0;
        self.last_window = now;
    }
}

/// Compute one step of an EWMA: `new = α·sample + (1-α)·old`.
fn ewma(old: f32, sample: f32) -> f32 {
    EWMA_ALPHA * sample + (1.0 - EWMA_ALPHA) * old
}

// ─── Behavior Monitor ───────────────────────────────────────────────────────────

/// The behavior monitor. Owns the per-agent state tables.
#[derive(Debug, Default)]
pub struct BehaviorMonitor {
    agents: HashMap<AgentId, AgentBehavior>,
}

impl BehaviorMonitor {
    /// Construct an empty monitor.
    pub fn new() -> Self {
        Self::default()
    }

    /// Observe a behavior event for an agent.
    pub fn observe(&mut self, agent: &AgentId, event: BehaviorEvent) {
        let entry = self.agents.entry(agent.clone()).or_default();
        entry.observe(event);
    }

    /// Return the current anomaly score for an agent (0.0 if unknown).
    pub fn anomaly_score(&self, agent: &AgentId) -> f32 {
        self.agents
            .get(agent)
            .map(|b| b.anomaly_score)
            .unwrap_or(0.0)
    }

    /// Return a snapshot of the full behavior record for an agent.
    pub fn behavior_of(&self, agent: &AgentId) -> Option<&AgentBehavior> {
        self.agents.get(agent)
    }

    /// Force a window rollover for all agents (e.g. on shutdown).
    pub fn rollover_all(&mut self) {
        for b in self.agents.values_mut() {
            b.maybe_rollover();
        }
    }
}
