//! Score fusion — combines multiple risk-component scores into a final HAL decision.
//!
//!
//! The HAL risk model produces a vector of component scores (irreversibility,
//! scope, trust, time-anomaly, vibe, user-history, pattern). This module is
//! the last step before a [`RiskScore`] is handed to the gate daemon: it
//! fuses those components with tunable [`FusionWeights`], clamps the result
//! to [0, 1], and finally applies *hard floors* — non-overridable minimums
//! for actions whose failure modes are catastrophic.
//!
//! Hard floors are the safety net beneath the weighted formula. No amount
//! of user-history or pattern-match can bring a kernel-adjacent delete
//! below its floor. This is by design: floors are the only thing standing
//! between a fully-trained agent and a "trained-into-complacency" failure.
//!
//! v0: stub implementation. Fusion math is in place; floors are stubbed
//! with TODO(v1) markers until the policy DSL lands.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::permissions::Capability;
use crate::risk_scorer::{ComponentScores, RiskLevel, RiskScore};

// v0: stub implementation

// ─── Fusion Weights ─────────────────────────────────────────────────────────────

/// Weights applied to each component during fusion. Must sum to 1.0.
///
/// The defaults below match the v1 formal model documented in
/// `risk_scorer.rs`. They are exposed as a struct (rather than consts) so
/// that future calibration tooling can persist them to disk and load them
/// per-user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FusionWeights {
    /// Weight for the irreversibility component.
    pub irreversibility: f32,
    /// Weight for the scope (blast radius) component.
    pub scope: f32,
    /// Weight for the trust-context component.
    pub trust: f32,
    /// Weight for the time-anomaly component.
    pub time_anomaly: f32,
    /// Weight for the vibe (AI-generated-code) component.
    pub vibe: f32,
    /// Weight for the user-history component.
    pub user_history: f32,
    /// Weight for the pattern-match component.
    pub pattern: f32,
}

impl Default for FusionWeights {
    fn default() -> Self {
        // Must sum to 1.0 — see risk_scorer.rs for rationale.
        Self {
            irreversibility: 0.25,
            scope: 0.20,
            trust: 0.20,
            time_anomaly: 0.10,
            vibe: 0.10,
            user_history: 0.10,
            pattern: 0.05,
        }
    }
}

impl FusionWeights {
    /// Returns true iff the weights sum to ~1.0 (within f32 tolerance).
    pub fn is_valid(&self) -> bool {
        let sum = self.irreversibility
            + self.scope
            + self.trust
            + self.time_anomaly
            + self.vibe
            + self.user_history
            + self.pattern;
        (sum - 1.0_f32).abs() < 0.001
    }
}

// ─── Fusion Errors ──────────────────────────────────────────────────────────────

/// Errors returned by the fusion engine.
#[derive(Debug, Error)]
pub enum FusionError {
    /// The supplied weights do not sum to 1.0 within tolerance.
    #[error("fusion weights do not sum to 1.0 (got {actual})")]
    InvalidWeights { actual: f32 },
}

// ─── Hard Floors ────────────────────────────────────────────────────────────────

/// Marker for actions that trigger a hard floor.
///
/// Hard floors are non-overridable minimums applied *after* fusion. The
/// enum is closed because every floor is a security-critical decision
/// and must be human-reviewed.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HardFloorTrigger {
    /// Action deletes a resource. Floor: 0.5
    Delete,
    /// Action is kernel-adjacent or kernel-level scope. Floor: 0.7
    KernelAdjacent,
    /// AI-generated unreviewed code is involved. Floor: 0.8
    AiUnreviewed,
    /// Combination of irreversible + kernel. Floor: 1.0 (always block)
    IrreversibleKernel,
}

impl HardFloorTrigger {
    /// The minimum score this trigger enforces.
    pub fn floor(&self) -> f32 {
        match self {
            Self::Delete => 0.5,
            Self::KernelAdjacent => 0.7,
            Self::AiUnreviewed => 0.8,
            Self::IrreversibleKernel => 1.0,
        }
    }

    /// Human-readable reason for the audit log.
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Delete => "delete actions always >= 0.5 regardless of history",
            Self::KernelAdjacent => "kernel-adjacent actions always >= 0.7",
            Self::AiUnreviewed => "AI-generated unreviewed code always >= 0.8",
            Self::IrreversibleKernel => "irreversible + kernel-adjacent = mandatory block (1.0)",
        }
    }
}

// ─── Hard-Floor Action Description ──────────────────────────────────────────────

/// Lightweight description of an action used to decide which hard floors
/// apply. We do not pass the full [`crate::risk_scorer::ProposedAction`]
/// here to keep the fusion engine decoupled from the action-model module.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HardFloorAction {
    /// Whether the action deletes a resource.
    pub is_delete: bool,
    /// Whether the action is kernel-adjacent.
    pub is_kernel_adjacent: bool,
    /// Whether the action is irreversible.
    pub is_irreversible: bool,
    /// Whether the action involves AI-generated unreviewed code.
    pub is_ai_unreviewed: bool,
    /// Capabilities requested — used for future kernel-adjacent inference.
    pub capabilities: Vec<Capability>,
}

impl HardFloorAction {
    /// Compute the set of triggers that apply to this action.
    ///
    /// Order matters: more-severe floors come first so the highest floor
    /// wins when multiple apply (the fusion loop raises the score, never
    /// lowers, so first-writer-wins among triggers above the current score).
    pub fn triggers(&self) -> Vec<HardFloorTrigger> {
        let mut out = Vec::new();
        if self.is_irreversible && self.is_kernel_adjacent {
            out.push(HardFloorTrigger::IrreversibleKernel);
        }
        if self.is_ai_unreviewed {
            out.push(HardFloorTrigger::AiUnreviewed);
        }
        if self.is_kernel_adjacent {
            out.push(HardFloorTrigger::KernelAdjacent);
        }
        if self.is_delete {
            out.push(HardFloorTrigger::Delete);
        }
        out
    }
}

// ─── Score Fusion Engine ────────────────────────────────────────────────────────

/// The fusion engine. Stateless aside from weights; safe to share.
#[derive(Debug, Clone)]
pub struct ScoreFusion {
    /// The fusion weights used by [`Self::fuse`].
    pub weights: FusionWeights,
}

impl Default for ScoreFusion {
    fn default() -> Self {
        Self {
            weights: FusionWeights::default(),
        }
    }
}

impl ScoreFusion {
    /// Construct with explicit weights. Returns an error if invalid.
    pub fn with_weights(weights: FusionWeights) -> Result<Self, FusionError> {
        if !weights.is_valid() {
            let actual = weights.irreversibility
                + weights.scope
                + weights.trust
                + weights.time_anomaly
                + weights.vibe
                + weights.user_history
                + weights.pattern;
            return Err(FusionError::InvalidWeights { actual });
        }
        Ok(Self { weights })
    }

    /// Fuse a set of component scores into a final [`RiskScore`] using the
    /// configured weights. Does NOT apply hard floors; call
    /// [`Self::apply_hard_floors`] afterwards.
    pub fn fuse(&self, components: ComponentScores) -> RiskScore {
        // TODO(v1): this re-implements the math in risk_scorer::score_action.
        // v1 should refactor so risk_scorer delegates here, not the reverse.
        let raw = self.weights.irreversibility * components.irreversibility
            + self.weights.scope * components.scope
            + self.weights.trust * components.trust_context
            + self.weights.time_anomaly * components.time_anomaly
            + self.weights.vibe * components.vibe_flag
            - self.weights.user_history * components.user_history
            - self.weights.pattern * components.pattern_match;

        let score = Self::clamp(raw);
        let level = Self::level_for(score);
        RiskScore {
            score,
            level,
            // TODO(v1): generate a plain-English explanation (see
            // risk_scorer::describe_score_internal).
            explanation: String::new(),
            components,
        }
    }

    /// Clamp a raw score to [0.0, 1.0].
    pub fn clamp(score: f32) -> f32 {
        score.clamp(0.0, 1.0)
    }

    /// Map a clamped score to its HAL level.
    fn level_for(score: f32) -> RiskLevel {
        if score < 0.3 {
            RiskLevel::Silent
        } else if score < 0.6 {
            RiskLevel::Notify
        } else if score < 0.8 {
            RiskLevel::Confirm
        } else {
            RiskLevel::Block
        }
    }

    /// Apply hard floors to an existing [`RiskScore`].
    ///
    /// The supplied [`HardFloorAction`] is consulted to determine which
    /// floors, if any, apply. Floors cannot be overridden by fusion — if
    /// a floor applies, the score is raised (never lowered) to the floor.
    pub fn apply_hard_floors(score: RiskScore, action: &HardFloorAction) -> RiskScore {
        let mut out = score.clone();
        let mut floor_applied = false;
        let mut floor_reason: Option<String> = None;

        for trigger in action.triggers() {
            if out.score < trigger.floor() {
                out.score = trigger.floor();
                floor_applied = true;
                floor_reason = Some(trigger.reason().to_string());
            }
        }

        out.level = Self::level_for(out.score);
        out.components.hard_floor_applied = floor_applied;
        out.components.hard_floor_reason = floor_reason;
        out
    }
}
