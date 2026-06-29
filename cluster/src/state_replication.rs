//! State replication — applies committed log entries to a deterministic state machine. Every node converges to the same state given the same log.
//!
//! The [`StateReplication`] actor wraps a [`ReplicatedState`] value and
//! feeds committed [`LogEntry`]s into it in strict index order. Periodic
//! snapshots compact the log so that slow-joining nodes can catch up
//! without replaying the full history.
//!
//! The state machine is intentionally narrow: policies, trust scores,
//! autonomy level, and the membership set. Every cluster-replicated
//! decision lands here so that all nodes observe an identical view.
//!
//! v0: stub implementation

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Cluster-unique identifier for a node.
pub type NodeId = String;

/// Stable identifier for an agent.
pub type AgentId = String;

/// Term in which a log entry was created.
pub type Term = u64;

/// Index into the replicated log (1-based).
pub type LogIndex = u64;

// ─── Errors ─────────────────────────────────────────────────────────────────

/// Errors returned by state machine replication.
#[derive(Debug, Error)]
pub enum ReplicationError {
    /// The supplied entry's index is not `last_applied + 1`.
    #[error("conflict at index {expected}: got {got}")]
    Conflict {
        /// Expected next index.
        expected: u64,
        /// Index the caller supplied.
        got: u64,
    },
    /// The entry failed validation before being applied.
    #[error("invalid entry: {0}")]
    InvalidEntry(String),
    /// The supplied snapshot predates the local state.
    #[error("snapshot too old (last_included_index {got} <= applied {applied})")]
    SnapshotTooOld {
        /// Snapshot's last included index.
        got: u64,
        /// Locally applied index.
        applied: u64,
    },
    /// Applying the entry to the state machine failed.
    #[error("apply failed: {0}")]
    ApplyFailed(String),
}

// ─── Autonomy level ─────────────────────────────────────────────────────────

/// Discrete autonomy levels for the cluster.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AutonomyLevel {
    /// HAL blocks every state-changing op pending human approval.
    Locked,
    /// Default operating mode — bounded autonomy within the lattice.
    Supervised,
    /// Expanded autonomy for trusted agents under tight telemetry.
    Extended,
}

impl Default for AutonomyLevel {
    fn default() -> Self {
        Self::Supervised
    }
}

// ─── Policy ─────────────────────────────────────────────────────────────────

/// A versioned HAL policy document stored in the replicated state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Policy {
    /// Human-readable policy name (unique within `policies`).
    pub name: String,
    /// Monotonic version counter.
    pub version: u64,
    /// Serialized policy body (format-specific; opaque to this module).
    pub body: Vec<u8>,
}

// ─── Replicated state ───────────────────────────────────────────────────────

/// Deterministic state machine driven by the consensus log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicatedState {
    /// Active policies keyed by name.
    pub policies: HashMap<String, Policy>,
    /// Trust scores keyed by agent id (range `0.0..=1.0`).
    pub trust: HashMap<AgentId, f32>,
    /// Current cluster-wide autonomy level.
    pub autonomy_level: AutonomyLevel,
    /// Current cluster membership.
    pub members: HashSet<NodeId>,
    /// Monotonically increasing version, bumped on every applied entry.
    pub version: u64,
}

impl Default for ReplicatedState {
    fn default() -> Self {
        Self {
            policies: HashMap::new(),
            trust: HashMap::new(),
            autonomy_level: AutonomyLevel::default(),
            members: HashSet::new(),
            version: 0,
        }
    }
}

// ─── Snapshot ───────────────────────────────────────────────────────────────

/// A point-in-time copy of the replicated state used for log compaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    /// Unique id of this snapshot (used as a content-address key in v1).
    pub id: Uuid,
    /// Frozen state at the snapshot point.
    pub state: ReplicatedState,
    /// Index of the last log entry included in the snapshot.
    pub last_included_index: u64,
    /// Term of the last log entry included in the snapshot.
    pub last_included_term: u64,
    /// When the snapshot was captured.
    pub created_at: DateTime<Utc>,
}

// ─── Log entry (mirrors cluster::consensus::LogEntry) ───────────────────────

/// Commands that can be replicated through the consensus log. Mirrors
/// `cluster::consensus::ConsensusCommand` to avoid a circular dep
/// between the two submodules in v0.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConsensusCommand {
    /// No-op used to commit the leader's term.
    NoOp,
    /// Install or replace a HAL policy document.
    InstallPolicy,
    /// Escalate (or de-escalate) the system-wide autonomy level.
    AutonomyEscalate,
    /// Adjust the trust score for a specific agent.
    TrustUpdate,
    /// Add a new node to the cluster configuration.
    AddNode,
    /// Remove an existing node from the cluster configuration.
    RemoveNode,
}

/// A single entry in the replicated log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEntry {
    /// Term in which the entry was created.
    pub term: Term,
    /// Position of the entry in the log (1-based).
    pub index: LogIndex,
    /// Command payload.
    pub command: ConsensusCommand,
}

// ─── StateReplication ───────────────────────────────────────────────────────

/// Applies committed log entries to a [`ReplicatedState`].
pub struct StateReplication {
    /// Current replicated state.
    pub state: ReplicatedState,
    /// Index of the highest log entry applied to `state`.
    pub applied_index: LogIndex,
    /// Number of applied entries between automatic snapshots.
    pub snapshot_interval: u64,
}

impl StateReplication {
    /// Build a new replicator with the default state and snapshot interval.
    pub fn new() -> Self {
        Self {
            state: ReplicatedState::default(),
            applied_index: 0,
            snapshot_interval: 1_000,
        }
    }

    /// Apply a committed log entry to the state machine.
    ///
    /// The entry's index must be exactly `applied_index + 1`; otherwise
    /// a [`ReplicationError::Conflict`] is returned. Application must be
    /// deterministic — every node fed the same log converges to the same
    /// [`ReplicatedState`].
    pub async fn apply(&mut self, entry: &LogEntry) -> Result<(), ReplicationError> {
        let expected = self.applied_index + 1;
        if entry.index != expected {
            return Err(ReplicationError::Conflict {
                expected,
                got: entry.index,
            });
        }

        // TODO(v1): dispatch on entry.command to mutate `state`:
        //   - InstallPolicy    -> insert/replace self.state.policies[name]
        //   - AutonomyEscalate -> set self.state.autonomy_level
        //   - TrustUpdate      -> adjust self.state.trust[agent]
        //   - AddNode/RemoveNode -> mutate self.state.members
        //   - NoOp             -> no state change, just advance the index
        debug!(
            index = entry.index,
            ?entry.command,
            "applying entry (v0 stub — no state mutation)"
        );

        self.applied_index = entry.index;
        self.state.version = self.state.version.wrapping_add(1);
        Ok(())
    }

    /// Capture a snapshot of the current state.
    pub async fn snapshot(&self) -> Result<Snapshot, ReplicationError> {
        // TODO(v1): stream serialized state to a content-addressed store
        // (ANFS) and return a descriptor instead of the full in-memory
        // snapshot; also truncate the log up to last_included_index.
        info!(applied_index = self.applied_index, "snapshot requested (v0 stub)");
        Ok(Snapshot {
            id: Uuid::new_v4(),
            state: self.state.clone(),
            last_included_index: self.applied_index,
            last_included_term: 0,
            created_at: Utc::now(),
        })
    }

    /// Restore state from a snapshot.
    ///
    /// Refuses snapshots whose `last_included_index` is older than the
    /// local `applied_index` to prevent regressions.
    pub async fn restore(&mut self, snap: Snapshot) -> Result<(), ReplicationError> {
        if snap.last_included_index <= self.applied_index {
            return Err(ReplicationError::SnapshotTooOld {
                got: snap.last_included_index,
                applied: self.applied_index,
            });
        }
        // TODO(v1): truncate the local log up to last_included_index and
        // replace the in-memory state atomically under a single lock.
        warn!(idx = snap.last_included_index, "restore from snapshot (v0 stub)");
        self.state = snap.state;
        self.applied_index = snap.last_included_index;
        Ok(())
    }
}

impl Default for StateReplication {
    fn default() -> Self {
        Self::new()
    }
}

// v0: stub implementation
