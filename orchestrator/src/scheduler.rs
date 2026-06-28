//! Orchestrator scheduler — picks which ready task to dispatch next,
//! respects priority, fairness, and capability quotas per agent.
//!
//! v0: stub implementation.

use std::collections::{BinaryHeap, HashMap, HashSet};
use std::cmp::Ordering;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::task_graph::{AgentId, TaskId};

// ─── Priority ───────────────────────────────────────────────────────────────

/// Dispatch priority for a [`SchedEntry`]. Higher numeric values are
/// dispatched first; ties are broken by FIFO order (earliest
/// `submitted_at` wins).
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
)]
pub struct Priority(pub u8);

impl Default for Priority {
    fn default() -> Self {
        Self(1)
    }
}

// ─── Deadline ───────────────────────────────────────────────────────────────

/// Absolute deadline for a [`SchedEntry`]. The scheduler uses this to flag
/// imminent misses; v0 does not yet preempt running tasks.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
)]
pub struct Deadline(pub DateTime<Utc>);

// ─── Scheduler entry ────────────────────────────────────────────────────────

/// A single entry in the scheduler queue. Ordering is:
/// 1. Higher [`Priority`] first.
/// 2. Earlier `submitted_at` first (FIFO within a priority class).
///
/// The custom [`Ord`] impl is what makes a `BinaryHeap<SchedEntry>` a
/// priority + FIFO queue: `BinaryHeap` is a max-heap, so the entry that
/// compares `Greater` is dispatched first.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedEntry {
    /// Task being scheduled.
    pub task_id: TaskId,
    /// Agent the task has been routed to.
    pub agent: AgentId,
    /// Dispatch priority.
    pub priority: Priority,
    /// When the entry was enqueued (UTC).
    pub submitted_at: DateTime<Utc>,
    /// Optional absolute deadline.
    pub deadline: Option<Deadline>,
}

impl SchedEntry {
    /// Convenience constructor that fills `submitted_at` with the current
    /// time and `deadline` with `None`.
    pub fn new(task_id: TaskId, agent: AgentId, priority: Priority) -> Self {
        Self {
            task_id,
            agent,
            priority,
            submitted_at: Utc::now(),
            deadline: None,
        }
    }
}

impl PartialEq for SchedEntry {
    fn eq(&self, other: &Self) -> bool {
        // Identity is task_id; two entries with the same task_id are
        // considered equal even if their other fields differ.
        self.task_id == other.task_id
    }
}

impl Eq for SchedEntry {}

impl PartialOrd for SchedEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SchedEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        // BinaryHeap is a max-heap, so "greater" entries come out first.
        // We want: higher priority first, then earlier submitted_at first.
        // For equal priority, the earlier submitted_at should compare
        // Greater (so it pops first), hence the reversed comparison.
        match self.priority.cmp(&other.priority) {
            Ordering::Equal => other.submitted_at.cmp(&self.submitted_at),
            ord => ord,
        }
    }
}

// ─── OrchestratorScheduler ──────────────────────────────────────────────────

/// Orchestrator's embedded scheduler. Picks which ready task to dispatch
/// next, respecting priority, FIFO fairness within a priority class, and a
/// per-agent fair-share weight that prevents one agent from starving the
/// rest.
///
/// The fair-share table is consulted by [`OrchestratorScheduler::enqueue`]
/// in v1 to scale the effective priority of each entry by its agent's
/// weight; v0 stores the table but does not yet apply it.
pub struct OrchestratorScheduler {
    /// Priority queue of waiting entries.
    pub queue: BinaryHeap<SchedEntry>,
    /// Per-agent fair-share weight in `[0.0, 1.0]`. v0: uniform `1.0`
    /// (absent entries are treated as `1.0`); v1: derived from capability
    /// quotas and observed throughput.
    pub fair_share: HashMap<AgentId, f32>,
    /// Tasks currently dispatched to an agent and awaiting completion.
    pub in_flight: HashSet<TaskId>,
}

impl OrchestratorScheduler {
    /// Construct an empty scheduler.
    pub fn new() -> Self {
        Self {
            queue: BinaryHeap::new(),
            fair_share: HashMap::new(),
            in_flight: HashSet::new(),
        }
    }

    /// Enqueue a new entry for dispatch. v0: does not deduplicate; callers
    /// are responsible for not enqueuing the same task twice.
    pub fn enqueue(&mut self, entry: SchedEntry) {
        // TODO(v1): apply fair-share weighting here — scale the effective
        // priority by `fair_share[&entry.agent]` (defaulting to 1.0) so a
        // saturated agent's entries sink relative to others. The simplest
        // approach is to wrap the entry in an inner type that carries the
        // adjusted priority.
        self.queue.push(entry);
    }

    /// Pop the next entry to dispatch, marking it as in-flight. Returns
    /// `None` if the queue is empty.
    pub fn next(&mut self) -> Option<SchedEntry> {
        let entry = self.queue.pop()?;
        self.in_flight.insert(entry.task_id);
        Some(entry)
    }

    /// Mark an in-flight task as complete. v0: no-op if the task is not in
    /// flight; v1 will surface that as an error.
    pub fn complete(&mut self, task_id: TaskId) {
        // TODO(v1): return Result<(), SchedulerError> and surface
        // NotInFlight when the task isn't tracked.
        self.in_flight.remove(&task_id);
    }

    /// Rebalance the fair-share table. v0: no-op (weights are static and
    /// uniform); v1: recompute weights from observed throughput and
    /// capability quotas so no single agent exceeds its allotted share of
    /// dispatch slots.
    pub fn rebalance(&mut self) {
        // TODO(v1): for each agent with entries in the queue or in flight,
        // measure the completed/queued ratio over the last window and
        // adjust `fair_share` accordingly. A deficit-reduction algorithm
        // (start fair, deduct actual usage, weight by remaining deficit)
        // is the planned approach.
    }

    /// Number of tasks currently dispatched and awaiting completion.
    pub fn in_flight_count(&self) -> usize {
        self.in_flight.len()
    }

    /// Number of tasks waiting in the queue.
    pub fn queue_depth(&self) -> usize {
        self.queue.len()
    }
}

impl Default for OrchestratorScheduler {
    fn default() -> Self {
        Self::new()
    }
}


