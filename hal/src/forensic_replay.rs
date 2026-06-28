//! Forensic replay — reconstructs past system state from the audit chain.
//!
//!
//! Given an [`AuditChain`], this module can:
//!   - replay a time range as a flat event stream,
//!   - reconstruct a synthetic [`SystemSnapshot`] at any past timestamp,
//!   - flag anomalies (bursts, novel paths, escalation attempts) in a window.
//!
//! This is the time-travel debugging surface for HAL decisions: when a user
//! asks "why did the HAL approve X at 14:32?", the answer comes from a
//! forensic replay of the surrounding window.
//!
//! v0: stub implementation. The replay engine returns empty results; the
//! snapshot type is defined but reconstruction is TODO(v1).

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::audit_chain::{AuditChain, ChainedEntry};

// v0: stub implementation

// ─── Replay Event ───────────────────────────────────────────────────────────────

/// A single event materialized during a replay window.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayEvent {
    /// Sequence number from the audit chain.
    pub seq: u64,
    /// Timestamp of the original audit entry.
    pub ts: DateTime<Utc>,
    /// Acting agent.
    pub agent: String,
    /// Action performed.
    pub action: String,
    /// HAL risk score at the time (if recorded).
    pub hal_score: Option<f32>,
    /// HAL level at the time (if recorded).
    pub hal_level: Option<String>,
}

impl ReplayEvent {
    /// Materialize a replay event from a chained audit entry.
    fn from_chained(chained: &ChainedEntry) -> Self {
        Self {
            seq: chained.seq,
            ts: chained.entry.ts,
            agent: chained.entry.agent.clone(),
            action: chained.entry.action.clone(),
            hal_score: chained.entry.hal_score,
            hal_level: chained.entry.hal_level.clone(),
        }
    }
}

// ─── System Snapshot ────────────────────────────────────────────────────────────

/// A synthetic reconstruction of system state at a point in time.
///
/// This is *not* a literal memory image — it is a summary of the
/// HAL-relevant state derived from the audit chain up to `at`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemSnapshot {
    /// The timestamp this snapshot corresponds to.
    pub at: DateTime<Utc>,
    /// Per-agent action counts up to `at`.
    pub agent_action_counts: HashMap<String, u64>,
    /// Last action performed by each agent (if any) up to `at`.
    pub last_action_per_agent: HashMap<String, ReplayEvent>,
    /// Highest HAL score observed in the (at-1h, at] window.
    pub peak_score_last_hour: Option<f32>,
    /// Number of HAL-confirm-or-block actions in the last hour.
    pub confirm_or_block_last_hour: u64,
}

// ─── Anomaly ────────────────────────────────────────────────────────────────────

/// A flagged anomaly found during a forensic replay.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Anomaly {
    /// Timestamp at which the anomaly was detected.
    pub ts: DateTime<Utc>,
    /// Categorical type of the anomaly (e.g. "burst", "novel_path").
    pub kind: String,
    /// Severity ∈ [0.0, 1.0].
    pub severity: f32,
    /// Human-readable description suitable for an incident report.
    pub description: String,
    /// Sequence numbers of the entries that triggered the detection.
    pub evidence_seqs: Vec<u64>,
}

// ─── Replay Window ──────────────────────────────────────────────────────────────

/// A replay window specification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayWindow {
    /// Inclusive start.
    pub from: DateTime<Utc>,
    /// Inclusive end.
    pub to: DateTime<Utc>,
    /// Optional agent filter.
    pub agent: Option<String>,
}

// ─── Forensic Replay Engine ─────────────────────────────────────────────────────

/// The forensic replay engine. Owns a reference to an audit chain.
#[derive(Debug)]
pub struct ForensicReplay {
    chain: AuditChain,
}

impl ForensicReplay {
    /// Construct a replay engine over an audit chain.
    pub fn new(chain: AuditChain) -> Self {
        Self { chain }
    }

    /// Borrow the underlying chain.
    pub fn chain(&self) -> &AuditChain {
        &self.chain
    }

    /// Replay a time range, returning the materialized event stream.
    pub fn replay_range(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> Vec<ReplayEvent> {
        self.chain
            .entries()
            .iter()
            .filter(|c| c.entry.ts >= from && c.entry.ts <= to)
            .map(ReplayEvent::from_chained)
            .collect()
    }

    /// Reconstruct a synthetic system snapshot at `at`.
    pub fn reconstruct_state(&self, at: DateTime<Utc>) -> SystemSnapshot {
        let mut agent_action_counts: HashMap<String, u64> = HashMap::new();
        let mut last_action_per_agent: HashMap<String, ReplayEvent> = HashMap::new();
        let mut peak_score_last_hour: Option<f32> = None;
        let mut confirm_or_block_last_hour = 0u64;

        let one_hour_ago = at - chrono::Duration::hours(1);

        for chained in self.chain.entries() {
            if chained.entry.ts > at {
                break;
            }
            let agent = chained.entry.agent.clone();
            *agent_action_counts.entry(agent.clone()).or_insert(0) += 1;
            last_action_per_agent.insert(agent, ReplayEvent::from_chained(chained));

            if chained.entry.ts > one_hour_ago {
                if let Some(score) = chained.entry.hal_score {
                    peak_score_last_hour =
                        Some(peak_score_last_hour.map_or(score, |p| p.max(score)));
                }
                if let Some(level) = &chained.entry.hal_level {
                    if level == "confirm" || level == "block" {
                        confirm_or_block_last_hour += 1;
                    }
                }
            }
        }

        SystemSnapshot {
            at,
            agent_action_counts,
            last_action_per_agent,
            peak_score_last_hour,
            confirm_or_block_last_hour,
        }
    }

    /// Find anomalies in a window. v0 returns an empty vec — the detection
    /// heuristics are TODO(v1).
    pub fn find_anomalies(&self, _window: ReplayWindow) -> Vec<Anomaly> {
        // TODO(v1): implement burst detection, novel-path detection, and
        // escalation-attempt detection over the window's events.
        debug!("find_anomalies called (v0 returns empty)");
        Vec::new()
    }
}
