//! Temporal trust — time-decay trust model.
//!
//!
//! Trust in the HAL is not a static value; it is earned through repeated
//! good behavior and decays with disuse. The [`TemporalTrust`] table tracks,
//! per agent, a *baseline* trust (set at onboarding), an *earned* delta that
//! accumulates with reinforcements, and a *half-life* over which the earned
//! delta decays toward zero if the agent stops being reinforced.
//!
//! The model is intentionally simple and bounded: every agent's effective
//! trust lives in `[0.0, 1.0]`, with `0.5` being "neutral" (no information).
//! Reinforcements move the earned component up or down; time alone moves it
//! back toward zero. Baseline shifts require explicit operator action and
//! are out of scope for v0.
//!
//! v0: stub implementation. Persistence is TODO(v1); decay math is computed
//! lazily on read in v0 (no background sweeper).

use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, info, warn};

// v0: stub implementation

/// Type alias for an agent identifier (matches the rest of the crate).
pub type AgentId = String;

/// Default half-life for earned trust: 7 days.
const DEFAULT_HALF_LIFE_DAYS: i64 = 7;

// ─── Trust Entry ────────────────────────────────────────────────────────────────

/// Per-agent temporal trust state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustEntry {
    /// Baseline trust set at onboarding (e.g. 0.5 for a new agent).
    pub baseline: f32,
    /// Accumulated earned-trust delta. May be negative.
    pub earned: f32,
    /// When the earned component was last reinforced.
    pub last_reinforced: DateTime<Utc>,
    /// Half-life of the earned component. Configurable per agent.
    pub half_life: Duration,
}

impl TrustEntry {
    /// Create a new entry with the given baseline and the default half-life.
    pub fn new(baseline: f32, now: DateTime<Utc>) -> Self {
        Self {
            baseline,
            earned: 0.0,
            last_reinforced: now,
            half_life: Duration::days(DEFAULT_HALF_LIFE_DAYS),
        }
    }

    /// Effective trust value at time `now`, accounting for decay.
    ///
    /// Decay follows exponential half-life: after `half_life` of disuse, the
    /// earned component has halved; after `2 * half_life`, quartered; etc.
    /// The result is clamped to `[0.0, 1.0]`.
    pub fn trust_at(&self, now: DateTime<Utc>) -> f32 {
        let decayed_earned = self.decayed_earned(now);
        (self.baseline + decayed_earned).clamp(0.0, 1.0)
    }

    /// Compute the decayed earned component at time `now`.
    fn decayed_earned(&self, now: DateTime<Utc>) -> f32 {
        let elapsed = now.signed_duration_since(self.last_reinforced);
        if elapsed <= Duration::zero() {
            return self.earned;
        }
        let half_life_secs = self.half_life.num_seconds().max(1) as f64;
        let elapsed_secs = elapsed.num_seconds().max(0) as f64;
        // 2^(-elapsed / half_life)
        let decay_factor = 2.0_f64.powf(-elapsed_secs / half_life_secs);
        (self.earned as f64 * decay_factor) as f32
    }
}

impl Default for TrustEntry {
    fn default() -> Self {
        Self::new(0.5, Utc::now())
    }
}

// ─── Temporal Trust Table ───────────────────────────────────────────────────────

/// The per-agent temporal trust table.
pub struct TemporalTrust {
    /// Map of agent id → trust entry.
    pub entries: HashMap<AgentId, TrustEntry>,
}

impl TemporalTrust {
    /// Create an empty trust table.
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Effective trust for an agent at time `now`.
    ///
    /// Returns the baseline `0.5` for agents with no entry, so unknown agents
    /// are treated as neutral rather than maximally untrusted.
    pub fn trust_at(&self, agent: &AgentId, now: DateTime<Utc>) -> f32 {
        match self.entries.get(agent) {
            Some(entry) => entry.trust_at(now),
            None => 0.5,
        }
    }

    /// Reinforce an agent's trust by `delta` (may be negative) at time `now`.
    ///
    /// The decayed earned component is first computed at `now`, then `delta`
    /// is added, then the entry is marked as reinforced at `now`. This means
    /// reinforcements always reflect the agent's *current* state, not their
    /// state at the last reinforcement.
    pub fn reinforce(&mut self, agent: &AgentId, delta: f32, now: DateTime<Utc>) {
        let entry = self
            .entries
            .entry(agent.clone())
            .or_insert_with(|| TrustEntry::new(0.5, now));
        let decayed = entry.decayed_earned(now);
        entry.earned = decayed + delta;
        entry.last_reinforced = now;
        debug!(
            agent = %agent,
            delta,
            earned = entry.earned,
            "temporal_trust: reinforced"
        );
    }

    /// Force a global decay pass at time `now`.
    ///
    /// This is a no-op on the *values* (decay is computed lazily on read in
    /// v0), but it does drop agents whose decayed earned component has fallen
    /// below a small epsilon AND whose baseline is exactly the neutral `0.5` —
    /// those agents are indistinguishable from unknown agents and can be
    /// safely forgotten to bound table growth.
    // TODO(v1): persist the trust table to disk via the continuity engine
    // so decay survives reboots, and run decay as a background sweeper
    // rather than lazily on read.
    pub fn decay_all(&mut self, now: DateTime<Utc>) {
        let mut dropped = 0usize;
        self.entries.retain(|_, entry| {
            let decayed = entry.decayed_earned(now);
            if decayed.abs() < 1e-4 && (entry.baseline - 0.5).abs() < 1e-6 {
                dropped += 1;
                false
            } else {
                true
            }
        });
        if dropped > 0 {
            info!(dropped, "temporal_trust: decay_all pruned neutral entries");
        }
    }

    /// Configure a custom half-life for an agent.
    ///
    /// The agent is created with the default baseline if it does not yet
    /// exist.
    pub fn set_half_life(&mut self, agent: &AgentId, half_life: Duration, now: DateTime<Utc>) {
        let entry = self
            .entries
            .entry(agent.clone())
            .or_insert_with(|| TrustEntry::new(0.5, now));
        entry.half_life = half_life;
        warn!(
            agent = %agent,
            half_life_secs = half_life.num_seconds(),
            "temporal_trust: half-life overridden"
        );
    }

    /// Number of tracked agents.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the table is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Default for TemporalTrust {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Errors ─────────────────────────────────────────────────────────────────────

/// Errors returned by the temporal trust module.
#[derive(Debug, Error)]
pub enum TemporalTrustError {
    /// Reinforcement delta would push the agent out of `[0, 1]` even after
    /// clamping. v0 does not surface this; v1 may use it for diagnostics.
    #[error("trust delta out of range: {0}")]
    DeltaOutOfRange(f32),
}
