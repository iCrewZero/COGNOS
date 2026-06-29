//! Trust heuristics for HAL — observable-pattern adjustments to per-agent trust.
//!
//!
//! Each [`HeuristicSignal`] maps to a small signed delta ∈ [-1.0, +1.0] that
//! is combined with the baseline trust score before HAL fusion. The goal is
//! to surface *cheaply observable* anomalies (time-of-day, path novelty,
//! rapid sequences, geography drift) without running the full LSTM-based
//! behavior model. Heuristics are deliberately conservative: the cost of a
//! false "untrusted" signal is a frictional UX, but the cost of a missed
//! signal is catastrophic. When in doubt, the heuristic leans towards
//! lowering trust.
//!
//! v0: stub implementation. All `evaluate` returns are placeholder
//! constants that match the documented rationale; v1 will calibrate them
//! from real traffic.

use serde::{Deserialize, Serialize};
use tracing::debug;

// v0: stub implementation

// ─── Heuristic Signals ──────────────────────────────────────────────────────────

/// Observable signals that adjust an agent's trust score.
///
/// Each variant documents the rationale for its trust delta. The closed
/// list is intentional: adding a new signal requires a spec update and a
/// human review of the trust-fusion formula.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HeuristicSignal {
    /// Action occurred outside the agent's established time-of-day pattern.
    ///
    /// Rationale: a memory agent that has only ever run between 08:00–18:00
    /// suddenly firing at 03:14 is a strong anomaly signal. Even if the
    /// action itself is benign, the temporal drift warrants scrutiny.
    TimeOfDayAnomaly,

    /// Action targets a path that the agent has never touched before.
    ///
    /// Rationale: novel paths are how data exfiltration and lateral
    /// movement begin. We don't block, but we lower trust so the HAL
    /// fusion engine requires more confidence before auto-approving.
    UnusualPath,

    /// Agent attempted to use a capability it has never used before.
    ///
    /// Rationale: capability novelty is the strongest single signal of an
    /// agent attempting to escalate. Treat with the largest delta.
    NovelCapability,

    /// Two or more HAL-gated actions from the same agent within 500ms.
    ///
    /// Rationale: a burst of actions is consistent with both legitimate
    /// batch workflows AND with a compromised agent racing to act before
    /// the user notices. We lower trust to force serial review.
    RapidSequence,

    /// Action originates from a geographic location inconsistent with the
    /// user's profile (e.g. sudden country change without a travel signal).
    ///
    /// Rationale: credential theft often manifests as geo-anomalous
    /// access. We treat this as a hard trust reduction even when the
    /// action is otherwise routine.
    GeographicAnomaly,
}

// ─── Heuristic Engine ───────────────────────────────────────────────────────────

/// Container for heuristic evaluation.
///
/// Stateless in v0 — the deltas are constant per signal. v1 may add
/// per-agent history that adjusts them.
#[derive(Debug, Default, Clone)]
pub struct TrustHeuristics {
    // TODO(v1): per-agent baseline windows for adaptive thresholds.
    _private: (),
}

impl TrustHeuristics {
    /// Construct a new heuristic evaluator with default configuration.
    pub fn new() -> Self {
        Self::default()
    }

    /// Evaluate a single signal against a baseline trust score and return
    /// the trust delta ∈ [-1.0, +1.0]. Negative deltas *reduce* trust.
    ///
    /// The `baseline` parameter is accepted for forward-compatibility (v1
    /// will use it to scale deltas for already-low-trust agents). v0
    /// ignores it and returns the documented constant.
    pub fn evaluate(signal: HeuristicSignal, _baseline: f32) -> f32 {
        // TODO(v1): calibrate against real traffic. v0 values are
        // conservative hand-tuned constants matching the rationale in
        // each variant.
        match signal {
            // Mild: temporal drift alone is rarely malicious.
            HeuristicSignal::TimeOfDayAnomaly => -0.10,
            // Stronger: a novel path is a leading indicator of exfiltration.
            HeuristicSignal::UnusualPath => -0.25,
            // Strongest single signal: capability novelty = escalation.
            HeuristicSignal::NovelCapability => -0.50,
            // Moderate: bursts are common in batch flows, don't overreact.
            HeuristicSignal::RapidSequence => -0.20,
            // Severe: geo-anomaly is treated as possible account takeover.
            HeuristicSignal::GeographicAnomaly => -0.75,
        }
    }

    /// Aggregate a slice of signals into a single trust delta.
    ///
    /// The aggregation uses a bounded sum (sum then clamp to [-1.0, 1.0]).
    /// This means a single worst-case signal can saturate trust reduction,
    /// which matches the "when in doubt, lower trust" principle.
    pub fn aggregate(signals: &[HeuristicSignal]) -> f32 {
        if signals.is_empty() {
            debug!("aggregate called with no signals — returning 0.0");
            return 0.0;
        }
        let sum: f32 = signals
            .iter()
            .map(|s| Self::evaluate(s.clone(), 0.0))
            .sum();
        sum.clamp(-1.0, 1.0)
    }
}
