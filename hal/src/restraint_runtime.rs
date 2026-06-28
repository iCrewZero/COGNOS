//! Restraint runtime — enforces the restraint model at runtime, gating the
//!
//!
//! cognitive preloader.
//!
//! This module is the runtime counterpart to [`crate::restraint_model`].
//! Where `restraint_model` is the pure decision function (stateless), this
//! module owns the per-action gate state and exposes a single `evaluate`
//! entry point that the cognitive preloader calls before surfacing a
//! prediction to the user.
//!
//! The decision is the conjunction of three conditions:
//!   1. Model confidence > 0.85
//!   2. The prediction is low-intimacy (not in a private domain)
//!   3. The prediction's domain is in the user's accepted-domain set
//!
//! If any condition fails, the prediction is either suppressed (silently
//! held back) or escalated to the user via `AskUser`. When in doubt, the
//! runtime suppresses — "stay invisible when in doubt" is the core HAL
//! UX principle.
//!
//! v0: stub implementation. The decision logic is in place; per-action
//! gate state persistence is TODO(v1).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::restraint_model::{
    ContextPrediction, DomainAcceptance, PreloadDecision, RestraintModel,
};

// v0: stub implementation

/// Confidence threshold below which a prediction is never surfaced.
pub const CONFIDENCE_THRESHOLD: f32 = 0.85;

// ─── Decision ───────────────────────────────────────────────────────────────────

/// The runtime's decision for a prediction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RestraintDecision {
    /// Surface the prediction (preload the context).
    Allow,
    /// Silently hold the prediction back. The user is not notified.
    Suppress,
    /// Escalate to the user — ask whether to surface this kind of prediction.
    AskUser,
}

impl RestraintDecision {
    /// Whether this decision allows the prediction to surface.
    pub fn is_allow(&self) -> bool {
        matches!(self, Self::Allow)
    }
}

// ─── Gate State ─────────────────────────────────────────────────────────────────

/// Per-action gate state, persisted across evaluations.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GateState {
    /// Number of times this action has been allowed.
    pub allow_count: u64,
    /// Number of times this action has been suppressed.
    pub suppress_count: u64,
    /// Number of times the user has been asked about this action.
    pub ask_count: u64,
    /// Whether the user has explicitly disabled this action.
    pub user_disabled: bool,
}

// ─── Prediction Wrapper ─────────────────────────────────────────────────────────

/// A prediction wrapper used by the runtime. Adds the action key (used for
/// per-action gate state) to the underlying [`ContextPrediction`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prediction {
    /// The underlying restraint-model prediction.
    pub inner: ContextPrediction,
    /// Stable action key for gate-state lookup (e.g. "open_workspace").
    pub action_key: String,
}

impl Prediction {
    /// Convenience accessor for the confidence field.
    pub fn confidence(&self) -> f32 {
        self.inner.confidence
    }
}

// ─── Restraint Runtime ──────────────────────────────────────────────────────────

/// The restraint runtime. Owns the per-action gate state and the domain
/// acceptance table (single source of truth for "is this domain unlocked?").
#[derive(Debug, Default)]
pub struct RestraintRuntime {
    /// Per-action gate state, keyed by action key.
    gates: HashMap<String, GateState>,
    /// Domain acceptance table — same instance as the one used by the
    /// cognitive preloader.
    acceptance: DomainAcceptance,
}

impl RestraintRuntime {
    /// Construct an empty runtime.
    pub fn new() -> Self {
        Self::default()
    }

    /// Evaluate a prediction. Returns the decision.
    ///
    /// Surfaces predictions only when confidence > 0.85 AND low-intimacy
    /// AND in an accepted domain (delegated to [`RestraintModel`]).
    pub fn evaluate(&self, prediction: &Prediction) -> RestraintDecision {
        // 1. Confidence threshold — hard floor.
        if prediction.confidence() < CONFIDENCE_THRESHOLD {
            debug!(
                confidence = prediction.confidence(),
                "suppressed: low confidence"
            );
            return RestraintDecision::Suppress;
        }

        // 2. Delegate the stateless checks (intimacy, path, time,
        //    acceptance) to the underlying model.
        let decision = RestraintModel::should_preload(&prediction.inner, &self.acceptance);
        match decision {
            PreloadDecision::Preload => RestraintDecision::Allow,
            PreloadDecision::HoldBack { reason } => {
                debug!(reason = %reason, "suppressed by restraint model");
                // TODO(v1): if the same reason recurs N times, escalate
                // to AskUser instead of silently suppressing.
                RestraintDecision::Suppress
            }
        }
    }

    /// Record a user-disabled action (after AskUser → "no").
    pub fn disable_action(&mut self, action_key: &str) {
        self.gates
            .entry(action_key.to_string())
            .or_default()
            .user_disabled = true;
    }

    /// Record a user-allowed action (after AskUser → "yes").
    pub fn enable_action(&mut self, action_key: &str) {
        let gate = self.gates.entry(action_key.to_string()).or_default();
        gate.user_disabled = false;
        gate.allow_count += 1;
    }

    /// Record a positive acceptance for a domain (delegated to DomainAcceptance).
    pub fn record_acceptance(&mut self, domain: &str) {
        self.acceptance.record_acceptance(domain);
    }

    /// Borrow the gate state for an action.
    pub fn gate_state(&self, action_key: &str) -> Option<&GateState> {
        self.gates.get(action_key)
    }

    /// Borrow the domain acceptance table.
    pub fn acceptance(&self) -> &DomainAcceptance {
        &self.acceptance
    }

    /// Mutably borrow the domain acceptance table.
    pub fn acceptance_mut(&mut self) -> &mut DomainAcceptance {
        &mut self.acceptance
    }
}
