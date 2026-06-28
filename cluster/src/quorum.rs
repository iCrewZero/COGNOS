//! Quorum — computes majority and supermajority thresholds, tracks live vs dead members, and decides if a write can commit.
//!
//! Wraps a [`QuorumTracker`] around a cluster membership set. Each node
//! is either `live` or `failed`; the tracker exposes the size threshold
//! required for a given [`QuorumKind`] and answers `has_quorum` for a
//! set of respondents. Used by both the consensus layer (commit
//! decisions) and the cluster membership manager (reconfiguration
//! safety).
//!
//! v0: stub implementation

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, info, warn};

/// Cluster-unique identifier for a node.
pub type NodeId = String;

// ─── Errors ─────────────────────────────────────────────────────────────────

/// Errors returned by quorum bookkeeping.
#[derive(Debug, Error)]
pub enum QuorumError {
    /// The set of respondents is smaller than the required threshold.
    #[error("no quorum: {respondents} respondents, {required} required")]
    NoQuorum {
        /// Number of respondents.
        respondents: usize,
        /// Quorum size for the current configuration.
        required: usize,
    },
    /// The supplied membership configuration predates the active one.
    #[error("stale configuration (expected epoch {expected}, got {got})")]
    StaleConfiguration {
        /// Expected configuration epoch.
        expected: u64,
        /// Epoch supplied by the caller.
        got: u64,
    },
    /// The supplied membership configuration conflicts with the active one
    /// (e.g. a joint-consensus transition is already in progress).
    #[error("conflicting configuration")]
    ConflictingConfiguration,
}

// ─── Quorum kind ────────────────────────────────────────────────────────────

/// Policy used to compute the quorum threshold for a cluster.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum QuorumKind {
    /// Strict majority: `floor(N/2) + 1`.
    Majority,
    /// Supermajority at a fractional threshold in `0.0..=1.0`
    /// (e.g. `0.66` for a two-thirds majority).
    Supermajority(f32),
    /// Unanimous consent from every member.
    All,
}

impl Default for QuorumKind {
    fn default() -> Self {
        Self::Majority
    }
}

// ─── QuorumTracker ──────────────────────────────────────────────────────────

/// Tracks live vs failed members and the active quorum policy.
///
/// `members` is the union of `live` and `failed`; the invariant is
/// maintained by [`mark_alive`](Self::mark_alive) and
/// [`mark_failed`](Self::mark_failed).
#[derive(Debug, Clone)]
pub struct QuorumTracker {
    /// Full membership set (live ∪ failed).
    pub members: HashSet<NodeId>,
    /// Members currently considered reachable.
    pub live: HashSet<NodeId>,
    /// Members currently considered unreachable.
    pub failed: HashSet<NodeId>,
    /// Active quorum policy.
    pub quorum_kind: QuorumKind,
    /// Monotonic configuration epoch, bumped on every `reconfigure`.
    pub epoch: u64,
}

impl QuorumTracker {
    /// Build a new tracker with the supplied membership and the default
    /// (`Majority`) quorum policy. All members start in the `live` set.
    pub fn new(members: HashSet<NodeId>) -> Self {
        let live = members.clone();
        Self {
            members,
            live,
            failed: HashSet::new(),
            quorum_kind: QuorumKind::default(),
            epoch: 0,
        }
    }

    /// Compute the threshold number of respondents required for quorum
    /// given the current membership size and [`QuorumKind`].
    pub fn quorum_size(&self) -> usize {
        let n = self.members.len();
        match self.quorum_kind {
            QuorumKind::Majority => n / 2 + 1,
            QuorumKind::Supermajority(frac) => {
                let frac = frac.clamp(0.0, 1.0);
                ((n as f32) * frac).ceil() as usize
            }
            QuorumKind::All => n,
        }
    }

    /// Returns `true` if `respondents` (intersected with the live set)
    /// meets the configured quorum threshold.
    pub fn has_quorum(&self, respondents: &HashSet<NodeId>) -> bool {
        let live_respondents = respondents.intersection(&self.live).count();
        let needed = self.quorum_size();
        debug!(
            respondents = respondents.len(),
            live_respondents,
            needed,
            "quorum check"
        );
        live_respondents >= needed
    }

    /// Mark `node` as alive (move it from `failed` to `live`).
    pub fn mark_alive(&mut self, node: NodeId) {
        if self.failed.remove(&node) {
            self.live.insert(node.clone());
        }
        // TODO(v1): emit a `MemberAlive` event on the cluster event bus.
        debug!(node = %node, "marked alive");
    }

    /// Mark `node` as failed (move it from `live` to `failed`).
    pub fn mark_failed(&mut self, node: NodeId) {
        if self.live.remove(&node) {
            self.failed.insert(node.clone());
        }
        // TODO(v1): emit a `MemberFailed` event; if the leader falls
        // below quorum, step down via consensus::ConsensusNode::tick.
        warn!(node = %node, "marked failed");
    }

    /// Replace the entire membership set.
    ///
    /// Resets every node to `live`, bumps the configuration epoch, and
    /// discards any pending joint-consensus state.
    ///
    /// TODO(v1): implement a joint-consensus (C_old ∪ C_new) transition
    /// per Raft §6 so reconfiguration never loses quorum mid-switch.
    pub fn reconfigure(&mut self, new_members: HashSet<NodeId>) {
        self.members = new_members.clone();
        self.live = new_members;
        self.failed = HashSet::new();
        self.epoch = self.epoch.wrapping_add(1);
        info!(epoch = self.epoch, "membership reconfigured");
    }

    /// Returns the number of currently-live members.
    pub fn live_count(&self) -> usize {
        self.live.len()
    }

    /// Returns the number of currently-failed members.
    pub fn failed_count(&self) -> usize {
        self.failed.len()
    }
}

impl Default for QuorumTracker {
    fn default() -> Self {
        Self::new(HashSet::new())
    }
}

// v0: stub implementation
