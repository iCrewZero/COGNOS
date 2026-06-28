//! Autonomy controller — controls the level of autonomy granted to AI agents.
//!
//!
//! Autonomy is graduated: an agent starts at `Supervised` and can earn its
//! way up to `Autonomous` through sustained good behavior. The controller
//! is per-user (each user has their own calibration) and reverts to
//! `Supervised` immediately on any anomaly — failures of over-trust are
//! more costly than failures of under-trust.
//!
//! The four levels map to HAL behavior as follows:
//!   - `Supervised`: every gated action requires explicit user approval.
//!   - `Advisory`: agent may propose; user must approve before execution.
//!   - `SemiAutonomous`: low-risk actions auto-execute; high-risk escalate.
//!   - `Autonomous`: agent may execute anything not on the hard-block list,
//!     subject to budget caps.
//!
//! v0: stub implementation. Level transitions are in place; budget
//! accounting is TODO(v1).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{info, warn};

// v0: stub implementation

/// Type alias for an agent identifier.
pub type AgentId = String;

// ─── Autonomy Levels ────────────────────────────────────────────────────────────

/// The four levels of autonomy an agent may be granted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum AutonomyLevel {
    /// Every gated action requires explicit user approval.
    Supervised,
    /// Agent may propose; user must approve before execution.
    Advisory,
    /// Low-risk actions auto-execute; high-risk escalate to user.
    SemiAutonomous,
    /// Agent may execute anything not on the hard-block list.
    Autonomous,
}

impl AutonomyLevel {
    /// Rank used for escalation/de-escalation comparisons.
    pub fn rank(self) -> u8 {
        match self {
            Self::Supervised => 0,
            Self::Advisory => 1,
            Self::SemiAutonomous => 2,
            Self::Autonomous => 3,
        }
    }
}

impl Default for AutonomyLevel {
    fn default() -> Self {
        Self::Supervised
    }
}

// ─── Action Budget ──────────────────────────────────────────────────────────────

/// Per-agent action budget. Caps how many auto-executed actions an agent
/// may take per period before requiring re-approval.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionBudget {
    /// Maximum auto-executed actions per period.
    pub max_per_period: u32,
    /// Current count in the period.
    pub current: u32,
    /// Period length in seconds.
    pub period_secs: u64,
}

impl Default for ActionBudget {
    fn default() -> Self {
        Self {
            max_per_period: 100,
            current: 0,
            period_secs: 3600,
        }
    }
}

impl ActionBudget {
    /// Whether the budget has been exhausted.
    pub fn exhausted(&self) -> bool {
        self.current >= self.max_per_period
    }

    /// Consume one unit of budget. Returns false if exhausted.
    pub fn consume(&mut self) -> bool {
        if self.exhausted() {
            return false;
        }
        self.current += 1;
        true
    }

    /// Reset the period counter.
    pub fn reset_period(&mut self) {
        self.current = 0;
    }
}

// ─── Escalation Errors ──────────────────────────────────────────────────────────

/// Errors returned by the autonomy controller.
#[derive(Debug, Error)]
pub enum EscalationError {
    /// The requested escalation skips a level (must be one step at a time).
    #[error("escalation skips levels: from {from:?} to {to:?}")]
    SkipsLevels {
        /// The current level.
        from: AutonomyLevel,
        /// The requested level.
        to: AutonomyLevel,
    },
    /// The agent's reputation is too low to escalate.
    #[error("reputation {reputation:.2} below threshold {threshold:.2} for {to:?}")]
    InsufficientReputation {
        /// The agent's current reputation.
        reputation: f32,
        /// The threshold required for the target level.
        threshold: f32,
        /// The level the agent was trying to escalate to.
        to: AutonomyLevel,
    },
    /// The action budget is exhausted.
    #[error("action budget exhausted ({current}/{max})")]
    BudgetExhausted {
        /// Current consumed count.
        current: u32,
        /// Maximum allowed per period.
        max: u32,
    },
}

// ─── Action Classification ──────────────────────────────────────────────────────

/// Lightweight classification of an action for autonomy decisions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionRequest {
    /// The agent requesting.
    pub agent: AgentId,
    /// Action type (e.g. "delete_file", "open_app").
    pub action_type: String,
    /// Risk score from the HAL risk engine ∈ [0.0, 1.0].
    pub risk_score: f32,
    /// Whether the action is on the hard-block list.
    pub on_block_list: bool,
}

// ─── Autonomy Controller ────────────────────────────────────────────────────────

/// The autonomy controller. Per-user; one instance per active user session.
#[derive(Debug, Default)]
pub struct AutonomyController {
    /// Current global autonomy level. Starts at Supervised.
    level: AutonomyLevel,
    /// Current action budget.
    budget: ActionBudget,
    /// Per-agent level overrides (when an agent earns more or less trust
    /// than the user-wide baseline).
    per_agent: HashMap<AgentId, AutonomyLevel>,
    /// Per-agent reputation, used to gate escalations.
    reputation: HashMap<AgentId, f32>,
}

impl AutonomyController {
    /// Construct a new controller at the Supervised level.
    pub fn new() -> Self {
        Self::default()
    }

    /// Current global autonomy level.
    pub fn level(&self) -> AutonomyLevel {
        self.level
    }

    /// Per-agent autonomy level (falls back to the global level).
    pub fn level_for(&self, agent: &AgentId) -> AutonomyLevel {
        self.per_agent.get(agent).copied().unwrap_or(self.level)
    }

    /// Escalate the global autonomy level. Must be a single-step escalation.
    pub fn escalate(&mut self, to: AutonomyLevel) -> Result<(), EscalationError> {
        if to.rank() != self.level.rank() + 1 {
            return Err(EscalationError::SkipsLevels {
                from: self.level,
                to,
            });
        }
        info!(from = ?self.level, to = ?to, "autonomy escalated");
        self.level = to;
        Ok(())
    }

    /// De-escalate the global autonomy level. May jump multiple levels (e.g.
    /// straight to Supervised on anomaly).
    pub fn deescalate(&mut self, to: AutonomyLevel) {
        if to.rank() < self.level.rank() {
            warn!(from = ?self.level, to = ?to, "autonomy de-escalated");
            self.level = to;
        }
    }

    /// Force-revert to Supervised (called on anomaly).
    pub fn revert_to_supervised(&mut self) {
        self.deescalate(AutonomyLevel::Supervised);
    }

    /// Decide whether the given action may auto-execute under the current
    /// autonomy level. Does NOT consume budget — call [`Self::consume_budget`]
    /// after a positive decision.
    pub fn can_auto_execute(&self, action: &ActionRequest) -> bool {
        if action.on_block_list {
            return false;
        }
        let level = self.level_for(&action.agent);
        match level {
            AutonomyLevel::Supervised => false,
            AutonomyLevel::Advisory => false,
            AutonomyLevel::SemiAutonomous => action.risk_score < 0.3,
            AutonomyLevel::Autonomous => {
                action.risk_score < 0.6 && !self.budget.exhausted()
            }
        }
    }

    /// Consume one unit of budget for an auto-executed action.
    pub fn consume_budget(&mut self) -> bool {
        self.budget.consume()
    }

    /// Set the per-agent reputation score (informed by
    /// [`crate::reputation_engine`]).
    pub fn set_reputation(&mut self, agent: &AgentId, score: f32) {
        self.reputation.insert(agent.clone(), score.clamp(0.0, 1.0));
    }

    /// Reputation score for an agent (0.5 = neutral if unknown).
    pub fn reputation_of(&self, agent: &AgentId) -> f32 {
        self.reputation.get(agent).copied().unwrap_or(0.5)
    }

    /// Borrow the action budget (for inspection / reset).
    pub fn budget(&self) -> &ActionBudget {
        &self.budget
    }

    /// Mutably borrow the action budget (for period resets).
    pub fn budget_mut(&mut self) -> &mut ActionBudget {
        &mut self.budget
    }
}
