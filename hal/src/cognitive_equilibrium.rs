//! Cognitive equilibrium — balances AI helpfulness against user agency.
//!
//!
//! A pure helpfulness-maximizer would silently do everything for the user,
//! eroding their skill and attention. A pure agency-maximizer would never do
//! anything without explicit instruction, defeating the purpose of an AI. The
//! [`CognitiveEquilibrium`] module computes a per-action balance between these
//! forces and produces a recommendation: act, ask, or abstain.
//!
//! The model is deliberately simple:
///   `net_benefit = helpfulness - (agency_loss * agency_weight)`
///
/// An action is recommended only if `net_benefit > 0`. The weights are
/// recalibrated from explicit user feedback (override, confirmation, undo).
//!
//! v0: stub implementation. Helpful / agency scores are computed from a few
//! hand-written heuristics; a learned model is TODO(v1).

use std::collections::VecDeque;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, info};

// v0: stub implementation

/// Type alias for an agent identifier (matches the rest of the crate).
pub type AgentId = String;

/// Default weight for helpfulness in the equilibrium formula.
const DEFAULT_HELPFULNESS_WEIGHT: f32 = 1.0;

/// Default weight for agency loss in the equilibrium formula.
const DEFAULT_AGENCY_WEIGHT: f32 = 1.5;

/// Maximum number of feedback events to retain for recalibration.
const HISTORY_CAPACITY: usize = 256;

// ─── Action Descriptor ──────────────────────────────────────────────────────────

/// Minimal descriptor of an action proposed for equilibrium evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EquilibriumAction {
    /// Agent proposing the action.
    pub agent: AgentId,
    /// Action name (e.g. "open_file", "refactor_function").
    pub action: String,
    /// Estimated user effort saved by this action, in `[0.0, 1.0]`.
    pub estimated_helpfulness: f32,
    /// Estimated loss of user agency / skill / attention, in `[0.0, 1.0]`.
    pub estimated_agency_loss: f32,
    /// Whether the user has explicitly requested this action.
    pub user_requested: bool,
}

// ─── Equilibrium Score ──────────────────────────────────────────────────────────

/// The result of an equilibrium evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EquilibriumScore {
    /// Raw helpfulness score in `[0.0, 1.0]`.
    pub helpfulness: f32,
    /// Raw agency-loss score in `[0.0, 1.0]`.
    pub agency_loss: f32,
    /// Net benefit: `helpfulness - (agency_loss * agency_weight)`.
    pub net_benefit: f32,
    /// The recommendation derived from the net benefit.
    pub recommendation: Recommendation,
}

/// What HAL should do with the action, per the equilibrium calculation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Recommendation {
    /// Net benefit is clearly positive — act silently.
    Act,
    /// Net benefit is positive but small — act and notify the user.
    ActWithNotice,
    /// Net benefit is near zero — ask the user before acting.
    Ask,
    /// Net benefit is negative — abstain from the action.
    Abstain,
}

// ─── User Feedback ──────────────────────────────────────────────────────────────

/// Feedback signals used to recalibrate the equilibrium weights.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UserFeedback {
    /// User explicitly accepted a proposed action.
    Accepted {
        /// Action that was accepted.
        action: String,
        /// When the feedback was received.
        at: DateTime<Utc>,
    },
    /// User explicitly rejected a proposed action.
    Rejected {
        /// Action that was rejected.
        action: String,
        /// When the feedback was received.
        at: DateTime<Utc>,
    },
    /// User undid an action HAL took.
    Undid {
        /// Action that was undone.
        action: String,
        /// When the feedback was received.
        at: DateTime<Utc>,
    },
    /// User overrode HAL's recommendation (either direction).
    Overrode {
        /// Action that was overridden.
        action: String,
        /// Whether HAL had recommended acting.
        hal_recommended_act: bool,
        /// When the feedback was received.
        at: DateTime<Utc>,
    },
}

// ─── Cognitive Equilibrium ──────────────────────────────────────────────────────

/// The equilibrium evaluator.
pub struct CognitiveEquilibrium {
    /// Weight applied to the helpfulness term.
    pub helpfulness_weight: f32,
    /// Weight applied to the agency-loss term.
    pub agency_weight: f32,
    /// Rolling history of feedback events used for recalibration.
    pub history: VecDeque<UserFeedback>,
}

impl CognitiveEquilibrium {
    /// Build a new evaluator with default weights.
    pub fn new() -> Self {
        Self {
            helpfulness_weight: DEFAULT_HELPFULNESS_WEIGHT,
            agency_weight: DEFAULT_AGENCY_WEIGHT,
            history: VecDeque::with_capacity(HISTORY_CAPACITY),
        }
    }

    /// Evaluate the equilibrium for an action.
    ///
    /// The formula follows the spec:
    ///   `net_benefit = helpfulness - (agency_loss * agency_weight)`
    ///
    /// An action is recommended only if `net_benefit > 0`. The
    /// `helpfulness_weight` is recorded for future recalibration use but does
    /// not enter the v0 formula; v1 may use it as a multiplicative factor.
    ///
    /// If the user explicitly requested the action, helpfulness is boosted to
    /// `1.0` (the maximum) and agency loss is zeroed — the user has already
    /// decided to delegate, so the equilibrium calculation should respect
    /// that.
    pub fn evaluate(&self, action: &EquilibriumAction) -> EquilibriumScore {
        let (helpfulness, agency_loss) = if action.user_requested {
            (1.0_f32.max(action.estimated_helpfulness), 0.0)
        } else {
            (action.estimated_helpfulness, action.estimated_agency_loss)
        };

        let net_benefit = helpfulness - (agency_loss * self.agency_weight);

        let recommendation = if net_benefit > 0.5 {
            Recommendation::Act
        } else if net_benefit > 0.0 {
            Recommendation::ActWithNotice
        } else if net_benefit > -0.25 {
            Recommendation::Ask
        } else {
            Recommendation::Abstain
        };

        debug!(
            agent = %action.agent,
            action = %action.action,
            helpfulness,
            agency_loss,
            net_benefit,
            ?recommendation,
            "cognitive_equilibrium: evaluated"
        );

        EquilibriumScore {
            helpfulness,
            agency_loss,
            net_benefit,
            recommendation,
        }
    }

    /// Recalibrate weights from a feedback event.
    ///
    /// v0 uses simple additive nudges; v1 will fit a small online model.
    /// Accepted actions slightly raise `helpfulness_weight`; rejected/undone
    /// actions slightly raise `agency_weight`. Overrides nudge both.
    // TODO(v1): replace additive nudges with a proper online regression
    // over (helpfulness, agency_loss, user_feedback) tuples so the weights
    // converge to the user's actual preference frontier.
    pub fn recalibrate(&mut self, feedback: UserFeedback) {
        const NUDGE: f32 = 0.01;
        match &feedback {
            UserFeedback::Accepted { .. } => {
                self.helpfulness_weight += NUDGE;
            }
            UserFeedback::Rejected { .. } | UserFeedback::Undid { .. } => {
                self.agency_weight += NUDGE;
            }
            UserFeedback::Overrode {
                hal_recommended_act: true,
                ..
            } => {
                // HAL said "act", user said "don't" — agency matters more.
                self.agency_weight += NUDGE;
            }
            UserFeedback::Overrode {
                hal_recommended_act: false,
                ..
            } => {
                // HAL said "abstain", user said "act" — helpfulness matters more.
                self.helpfulness_weight += NUDGE;
            }
        }
        self.history.push_back(feedback);
        while self.history.len() > HISTORY_CAPACITY {
            self.history.pop_front();
        }
        info!(
            helpfulness_weight = self.helpfulness_weight,
            agency_weight = self.agency_weight,
            "cognitive_equilibrium: recalibrated"
        );
    }

    /// Number of feedback events currently retained.
    pub fn history_len(&self) -> usize {
        self.history.len()
    }
}

impl Default for CognitiveEquilibrium {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Errors ─────────────────────────────────────────────────────────────────────

/// Errors returned by the equilibrium module.
#[derive(Debug, Error)]
pub enum CognitiveEquilibriumError {
    /// Weights drifted out of their valid range during recalibration.
    #[error("weight out of range: helpfulness={helpfulness} agency={agency}")]
    WeightOutOfRange {
        /// Current helpfulness weight.
        helpfulness: f32,
        /// Current agency weight.
        agency: f32,
    },
}
