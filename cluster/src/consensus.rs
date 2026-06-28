//! Consensus — Raft-based leader election and log replication for the COGNOS cluster. Single leader per term; majority quorum for commits.
//!
//! This module implements a minimal Raft-style consensus layer so that
//! COGNOS deployments can agree on a single ordered log of cluster-level
//! commands (policy installs, autonomy escalations, trust updates,
//! membership changes). At most one leader exists per term, and an entry
//! is only committed once a majority of nodes have acknowledged it.
//!
//! The v0 surface defines the data model, RPC message shapes, and the
//! public API of [`ConsensusNode`]; the network transport, persistence,
//! and the full state-machine safety checks land in v1.
//!
//! v0: stub implementation

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::time::sleep;
use tracing::{debug, info, warn};
use uuid::Uuid;

// TODO(v1): pull `NodeId` from a shared `cluster::types` module once the
// workspace crate structure lands. For now each cluster submodule
// re-declares it as a `String` newtype.

/// Cluster-unique identifier for a node.
pub type NodeId = String;

/// Raft term. Monotonically increasing across the cluster's lifetime.
pub type Term = u64;

/// Index into the replicated log (1-based, per Raft convention).
pub type LogIndex = u64;

// ─── Errors ─────────────────────────────────────────────────────────────────

/// Errors returned by the consensus protocol.
#[derive(Debug, Error)]
pub enum ConsensusError {
    /// Operation requires this node to be the leader, but it is not.
    #[error("not leader (current state: {0:?})")]
    NotLeader(NodeState),
    /// No leader was elected before the randomized election timeout fired.
    #[error("election timeout (term {0})")]
    ElectionTimeout(Term),
    /// Quorum was lost mid-operation (too many peers unreachable).
    #[error("quorum lost: {respondents} respondents, {required} required")]
    QuorumLost {
        /// Number of nodes that did respond.
        respondents: usize,
        /// Quorum size required to commit.
        required: usize,
    },
    /// The leader's log did not match the follower's at the requested index.
    #[error("log mismatch at index {index}: leader term {leader_term}, follower term {follower_term}")]
    LogMismatch {
        /// Index at which the logs diverged.
        index: LogIndex,
        /// Term of the leader's entry at that index.
        leader_term: Term,
        /// Term of the follower's entry at that index.
        follower_term: Term,
    },
    /// A network failure prevented the RPC from completing.
    #[error("network failure: {0}")]
    NetworkFailure(String),
}

// ─── Node state ─────────────────────────────────────────────────────────────

/// Raft role assumed by a [`ConsensusNode`] at any given time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeState {
    /// Passive; responds to RequestVote and AppendEntries RPCs.
    Follower,
    /// Actively soliciting votes for a new term.
    Candidate,
    /// Elected leader for the current term; drives heartbeats.
    Leader,
}

impl Default for NodeState {
    fn default() -> Self {
        Self::Follower
    }
}

// ─── Log types ──────────────────────────────────────────────────────────────

/// Commands that can be replicated through the consensus log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConsensusCommand {
    /// No-op used to commit the leader's term (Raft §5.4.2).
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

// ─── RPC messages ───────────────────────────────────────────────────────────

/// Response to a `RequestVote` RPC.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoteResponse {
    /// Term the responder saw; used to stale-out the candidate.
    pub term: Term,
    /// Whether the vote was granted.
    pub vote_granted: bool,
}

/// Response to an `AppendEntries` RPC.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppendResponse {
    /// Term the responder saw.
    pub term: Term,
    /// Whether the follower accepted the entries.
    pub success: bool,
    /// Index of the follower's last matching entry (for fast back-off).
    pub match_index: LogIndex,
}

// ─── Timing constants ───────────────────────────────────────────────────────

/// Lower bound of the randomized election timeout window (150 ms).
pub const ELECTION_TIMEOUT_MIN: Duration = Duration::from_millis(150);

/// Upper bound of the randomized election timeout window (300 ms).
pub const ELECTION_TIMEOUT_MAX: Duration = Duration::from_millis(300);

/// Interval at which the leader emits heartbeats (50 ms).
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_millis(50);

// ─── ConsensusNode ──────────────────────────────────────────────────────────

/// A Raft-style consensus participant.
///
/// Owns the local log bookkeeping, current term/vote, and the indices
/// that track replication progress. Network transport is abstracted out
/// in v0 — RPCs are stubbed and never actually sent over the wire.
pub struct ConsensusNode {
    /// Stable identity of this node within the cluster.
    pub id: NodeId,
    /// Peer node identities (excluding self).
    pub peers: Vec<NodeId>,
    /// Current Raft role.
    pub state: NodeState,
    /// Latest term this node has observed.
    pub current_term: Term,
    /// Candidate id that received this node's vote in the current term
    /// (`None` means "not yet voted").
    pub voted_for: Option<NodeId>,
    /// Replicated log.
    pub log: Vec<LogEntry>,
    /// Index of the highest log entry known to be committed.
    pub commit_index: LogIndex,
    /// Index of the highest log entry applied to the state machine.
    pub last_applied: LogIndex,
    /// Timestamp of the last heartbeat observed from a leader.
    pub last_heartbeat: DateTime<Utc>,
    /// Unique id of the current election (set when transitioning to Candidate).
    pub election_id: Option<Uuid>,
}

impl ConsensusNode {
    /// Build a new node with the supplied id and peer set.
    pub fn new(id: impl Into<NodeId>, peers: Vec<NodeId>) -> Self {
        Self {
            id: id.into(),
            peers,
            state: NodeState::Follower,
            current_term: 0,
            voted_for: None,
            log: Vec::new(),
            commit_index: 0,
            last_applied: 0,
            last_heartbeat: Utc::now(),
            election_id: None,
        }
    }

    /// Drive one tick of the protocol loop.
    ///
    /// Followers and candidates check for election-timeout expiry and may
    /// start a new election. Leaders emit heartbeats to all peers.
    pub async fn tick(&mut self) -> Result<(), ConsensusError> {
        // TODO(v1): real per-peer timer state, randomized backoff drawn
        // uniformly from [ELECTION_TIMEOUT_MIN, ELECTION_TIMEOUT_MAX].
        debug!(
            node = %self.id,
            state = ?self.state,
            term = self.current_term,
            "tick"
        );
        match self.state {
            NodeState::Follower | NodeState::Candidate => {
                // TODO(v1): if now - last_heartbeat > election_timeout ->
                // become Candidate, bump term, vote for self, issue
                // RequestVote to every peer.
            }
            NodeState::Leader => {
                // TODO(v1): emit empty AppendEntries (heartbeat) to each
                // peer in `self.peers` over the cluster transport.
                sleep(HEARTBEAT_INTERVAL).await;
            }
        }
        Ok(())
    }

    /// Handle an inbound `RequestVote` RPC.
    ///
    /// Implements the vote-granting rules from Raft §5.4.1: reject if the
    /// candidate's term is stale, reject if we already voted this term,
    /// reject if the candidate's log is not at least as up-to-date as ours.
    pub async fn request_vote(
        &self,
        _candidate: NodeId,
    ) -> Result<VoteResponse, ConsensusError> {
        // TODO(v1): full §5.4.1 logic. v0 always refuses the vote.
        warn!(node = %self.id, "RequestVote received — v0 stub");
        Ok(VoteResponse {
            term: self.current_term,
            vote_granted: false,
        })
    }

    /// Handle an inbound `AppendEntries` RPC.
    ///
    /// Implements the log-matching checks from Raft §5.3: reject if the
    /// leader's term is stale, reject if `prev_log_index`/`prev_log_term`
    /// do not match the local log, otherwise append/overwrite entries.
    pub async fn append_entries(
        &self,
        _leader: NodeId,
        _entries: Vec<LogEntry>,
    ) -> Result<AppendResponse, ConsensusError> {
        // TODO(v1): full §5.3 logic. v0 always refuses the append.
        warn!(node = %self.id, "AppendEntries received — v0 stub");
        Ok(AppendResponse {
            term: self.current_term,
            success: false,
            match_index: self.last_applied,
        })
    }

    /// Propose a new command.
    ///
    /// Only the leader may propose. The entry is appended to the local
    /// log at `log.len() + 1` with the current term, then replicated to
    /// peers on the next heartbeat batch (or immediately, in v1). Returns
    /// the assigned log index. The entry is *not* considered committed
    /// until a majority of peers acknowledge it.
    pub async fn propose(
        &mut self,
        cmd: ConsensusCommand,
    ) -> Result<LogIndex, ConsensusError> {
        if self.state != NodeState::Leader {
            return Err(ConsensusError::NotLeader(self.state));
        }
        // TODO(v1): append entry, trigger an immediate replication round,
        // register an in-flight waiter that resolves when commit_index
        // reaches the assigned index.
        let index = (self.log.len() as LogIndex) + 1;
        info!(
            node = %self.id,
            ?cmd,
            index,
            "propose received — v0 stub (entry not appended)"
        );
        Ok(index)
    }

    /// Convenience accessor: id of the current leader, if known.
    ///
    /// v0 always returns `None` since leader election is not wired up.
    pub fn leader_id(&self) -> Option<&NodeId> {
        // TODO(v1): track leader_id observed from the most recent
        // AppendEntries RPC whose term matched current_term.
        None
    }
}

impl Default for ConsensusNode {
    fn default() -> Self {
        Self::new("node-0", Vec::new())
    }
}

// v0: stub implementation
