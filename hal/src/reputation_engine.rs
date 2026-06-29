//! Reputation engine — long-term per-agent reputation tracking.
//!
//!
//! Reputation is a *slow* signal: it accumulates over many decisions and
//! decays slowly over time. It is intentionally distinct from the fast
//! behavior-monitor signal (which reacts within seconds). Reputation
//! informs the trust component of the risk formula; agents with high
//! reputation get more slack, agents with low reputation get more scrutiny.
//!
//! Reputation is bounded [0.0, 1.0]. 0.5 is "neutral" (a brand-new agent
//! with no history). 1.0 means "long track record of approved actions
//! and no escalations". 0.0 means "compromised or consistently
//! overridden".
//!
//! Decay: every hour, reputation moves 5% of the way back toward 0.5
//! (neutral). This prevents both eternal punishment and eternal trust.
//!
//! v0: stub implementation. Persistence and decay math are TODO(v1).

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::debug;

// v0: stub implementation

/// Type alias for an agent identifier. Matches the agent names used across
/// the codebase (e.g. "planner", "memory", "file").
pub type AgentId = String;

// ─── Action Outcomes ────────────────────────────────────────────────────────────

/// The outcome of a HAL-gated action, as observed by the reputation engine.
///
/// Each variant maps to a signed delta applied to the acting agent's
/// reputation. The deltas are asymmetric: bad outcomes cost more than good
/// outcomes reward, because trust is harder to rebuild than to lose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActionOutcome {
    /// The HAL approved the action and it completed without incident.
    Approved,
    /// The HAL rejected the action.
    Rejected,
    /// The HAL auto-executed (Silent level) and it succeeded.
    AutoSucceeded,
    /// The HAL auto-executed (Silent level) and it failed.
    AutoFailed,
    /// The user explicitly overrode the HAL's decision (either direction).
    UserOverrode,
}

impl ActionOutcome {
    /// Reputation delta for this outcome. Positive = trust gain.
    fn delta(&self) -> f32 {
        match self {
            Self::Approved => 0.02,
            Self::Rejected => -0.05,
            Self::AutoSucceeded => 0.01,
            Self::AutoFailed => -0.10,
            Self::UserOverrode => -0.15,
        }
    }

    /// Whether this outcome should be counted as an "escalation" — a
    /// negative event that future risk scoring should weight heavily.
    pub fn is_escalation(&self) -> bool {
        matches!(self, Self::AutoFailed | Self::UserOverrode)
    }
}

// ─── Reputation Record ──────────────────────────────────────────────────────────

/// Per-agent reputation record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reputation {
    /// Current reputation score ∈ [0.0, 1.0]. 0.5 = neutral.
    pub score: f32,
    /// Total number of decisions observed for this agent.
    pub decisions_observed: u64,
    /// Total number of escalations (AutoFailed + UserOverrode).
    pub escalations: u64,
    /// Last time this record was updated.
    pub last_updated: DateTime<Utc>,
}

impl Default for Reputation {
    fn default() -> Self {
        Self {
            score: 0.5,
            decisions_observed: 0,
            escalations: 0,
            last_updated: Utc::now(),
        }
    }
}

impl Reputation {
    /// Apply an outcome to this reputation record.
    pub fn apply(&mut self, outcome: ActionOutcome) {
        self.score = (self.score + outcome.delta()).clamp(0.0, 1.0);
        self.decisions_observed += 1;
        if outcome.is_escalation() {
            self.escalations += 1;
        }
        self.last_updated = Utc::now();
    }

    /// Apply time-based decay toward neutral (0.5).
    ///
    /// Decay rate: 5% of the distance to 0.5 per hour. Idempotent for the
    /// same timestamp — calling twice with the same `now` is a no-op.
    pub fn decay(&mut self, now: DateTime<Utc>) {
        // TODO(v1): persist last_updated and compute decay on read.
        let elapsed_hours = (now - self.last_updated).num_seconds() as f32 / 3600.0;
        if elapsed_hours <= 0.0 {
            return;
        }
        let decay = 0.05_f32 * elapsed_hours; // 5% per hour
        let neutral = 0.5_f32;
        self.score = self.score + (neutral - self.score) * decay.min(1.0);
        self.last_updated = now;
    }
}

// ─── Reputation Engine ──────────────────────────────────────────────────────────

/// The reputation engine. Owns the per-agent reputation table.
#[derive(Debug, Default)]
pub struct ReputationEngine {
    scores: HashMap<AgentId, Reputation>,
}

impl ReputationEngine {
    /// Construct an empty reputation engine.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record an action outcome for an agent, updating its reputation.
    pub fn update(&mut self, agent: &AgentId, outcome: ActionOutcome) {
        let entry = self.scores.entry(agent.clone()).or_default();
        entry.apply(outcome);
        debug!(agent = %agent, score = entry.score, "reputation updated");
    }

    /// Return the current reputation score for an agent, defaulting to 0.5
    /// (neutral) for unknown agents.
    pub fn reputation_of(&self, agent: &AgentId) -> f32 {
        self.scores.get(agent).map(|r| r.score).unwrap_or(0.5)
    }

    /// Return a full reputation record for an agent, if present.
    pub fn record(&self, agent: &AgentId) -> Option<&Reputation> {
        self.scores.get(agent)
    }

    /// Apply time-based decay to all agents. Should be called periodically
    /// (e.g. once per minute) by the HAL daemon.
    pub fn decay_all(&mut self) {
        let now = Utc::now();
        for r in self.scores.values_mut() {
            r.decay(now);
        }
    }

    /// Snapshot the full reputation table (for persistence / audit).
    pub fn snapshot(&self) -> &HashMap<AgentId, Reputation> {
        &self.scores
    }
}

// ─── Errors ─────────────────────────────────────────────────────────────────────

/// Errors returned by the reputation engine.
#[derive(Debug, Error)]
pub enum ReputationError {
    /// The agent was not found in the reputation table.
    #[error("agent '{0}' not found in reputation table")]
    UnknownAgent(AgentId),
}
