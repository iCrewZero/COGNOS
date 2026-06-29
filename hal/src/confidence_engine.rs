//! Confidence engine — estimates how sure the system is about a risk score.


use crate::{
    hal_types::{
        BehavioralMetrics,
        HALDecision,
        HALResult,
        ProvenanceData,
        SessionContext,
        TrustState,
    },

    risk_weights::RiskEngine,
};

pub struct ConfidenceEngine;

impl ConfidenceEngine {
    pub fn compute_confidence(
        trust: &TrustState,

        provenance:
            &ProvenanceData,

        behavior:
            &BehavioralMetrics,

        session:
            &SessionContext,
    ) -> f32 {
        let mut confidence = 0.0;

        confidence +=
            trust.current_score
                * 0.40;

        confidence +=
            Self::provenance_component(
                provenance
            )
                * 0.30;

        confidence +=
            Self::behavioral_component(
                behavior
            )
                * 0.20;

        confidence +=
            Self::session_component(
                session
            )
                * 0.10;

        confidence
            .clamp(0.0, 1.0)
    }

    pub fn provenance_component(
        provenance:
            &ProvenanceData,
    ) -> f32 {
        let mut score = 0.0;

        if provenance
            .signature_verified
        {
            score += 0.50;
        }

        if provenance
            .replay_checked
        {
            score += 0.25;
        }

        if !provenance
            .trust_chain_hash
            .is_empty()
        {
            score += 0.25;
        }

        score
            .clamp(0.0, 1.0)
    }

    pub fn behavioral_component(
        behavior:
            &BehavioralMetrics,
    ) -> f32 {
        let stability =
            behavior
                .historical_stability;

        let anomaly_penalty =
            behavior
                .anomaly_score
                * 0.40;

        let volatility_penalty =
            behavior
                .volatility_score
                * 0.30;

        let escalation_penalty =
            (
                behavior
                    .escalation_attempts
                    as f32
            )
                * 0.05;

        (
            stability
                - anomaly_penalty
                - volatility_penalty
                - escalation_penalty
        )
            .clamp(0.0, 1.0)
    }

    pub fn session_component(
        session:
            &SessionContext,
    ) -> f32 {
        let mut score = 1.0;

        if !session.user_present {
            score -= 0.45;
        }

        if session
            .requires_confirmation
        {
            score -= 0.20;
        }

        score -=
            (
                1.0
                    - session
                        .user_attention_score
            )
                * 0.35;

        score
            .clamp(0.0, 1.0)
    }

    pub fn confidence_decay(
        base_confidence: f32,

        elapsed_ms: i64,
    ) -> f32 {
        let decay =
            (
                elapsed_ms
                    as f32
                    / 1000.0
            )
                * 0.00015;

        (
            base_confidence
                - decay
        )
            .clamp(0.0, 1.0)
    }

    pub fn confidence_from_risk(
        risk: f32,
    ) -> f32 {
        (
            1.0 - risk
        )
            .clamp(0.0, 1.0)
    }

    pub fn requires_human_confirmation(
        confidence: f32,

        risk: f32,
    ) -> bool {
        confidence < 0.55
            || risk > 0.65
    }

    pub fn decision_from_confidence(
        confidence: f32,

        risk: f32,
    ) -> HALDecision {
        if risk > 0.90 {
            return HALDecision::Block;
        }

        if confidence < 0.20 {
            return HALDecision::Block;
        }

        if risk > 0.70 {
            return HALDecision::Confirm;
        }

        if confidence < 0.45 {
            return HALDecision::Notify;
        }

        HALDecision::Allow
    }

    pub fn consistency_score(
        historical_average: f32,

        current_behavior: f32,
    ) -> f32 {
        let delta =
            (
                historical_average
                    - current_behavior
            )
                .abs();

        (
            1.0 - delta
        )
            .clamp(0.0, 1.0)
    }

    pub fn irreversible_action_allowed(
        confidence: f32,

        risk: f32,

        user_present: bool,
    ) -> bool {
        confidence > 0.92
            && risk < 0.30
            && user_present
    }

    pub fn finalize_result(
        risk: f32,

        confidence: f32,

        explanation: String,
    ) -> HALResult {
        let decision =
            Self::decision_from_confidence(
                confidence,
                risk,
            );

        HALResult {
            decision:
                decision.clone(),

            computed_risk:
                risk,

            confidence,

            explanation,

            violated_rules:
                vec![],

            audit_hash:
                format!(
                    "hal:{}:{}",
                    risk,
                    confidence
                ),

            requires_user_prompt:
                Self::requires_human_confirmation(
                    confidence,
                    risk,
                ),

            escalation_required:
                matches!(
                    decision,
                    HALDecision::Block
                        | HALDecision::Escalate
                ),
        }
    }

    pub fn combined_assurance(
        trust_score: f32,

        provenance_score: f32,

        behavioral_score: f32,

        risk_score: f32,
    ) -> f32 {
        let confidence =
            (
                trust_score
                    * 0.35
            )
                + (
                    provenance_score
                        * 0.30
                )
                + (
                    behavioral_score
                        * 0.20
                )
                + (
                    (
                        1.0
                            - risk_score
                    )
                        * 0.15
                );

        confidence
            .clamp(0.0, 1.0)
    }

    pub fn bounded_autonomy(
        confidence: f32,
    ) -> bool {
        confidence >= 0.85
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::hal_types::{
        ProvenanceConfidence,
    };

    #[test]
    fn confidence_computation() {
        let trust =
            TrustState {
                current_score:
                    0.91,

                historical_average:
                    0.88,

                decay_rate:
                    0.01,

                recovery_rate:
                    0.02,

                compromise_suspected:
                    false,
            };

        let provenance =
            ProvenanceData {
                source_agent:
                    "planner"
                        .into(),

                certificate_fingerprint:
                    "abc".into(),

                trust_chain_hash:
                    "def".into(),

                signature_verified:
                    true,

                replay_checked:
                    true,

                confidence:
                    ProvenanceConfidence::Verified,
            };

        let behavior =
            BehavioralMetrics {
                anomaly_score:
                    0.05,

                volatility_score:
                    0.10,

                escalation_attempts:
                    0,

                historical_stability:
                    0.94,

                recent_failures:
                    0,
            };

        let session =
            SessionContext {
                session_id:
                    "session"
                        .into(),

                user_present:
                    true,

                active_workspace:
                    "dev".into(),

                active_window_title:
                    "terminal"
                        .into(),

                requires_confirmation:
                    false,

                user_attention_score:
                    0.95,
            };

        let confidence =
            ConfidenceEngine::
                compute_confidence(
                    &trust,
                    &provenance,
                    &behavior,
                    &session,
                );

        assert!(
            confidence > 0.75
        );
    }

    #[test]
    fn high_risk_requires_confirmation() {
        assert!(
            ConfidenceEngine::
                requires_human_confirmation(
                    0.90,
                    0.80,
                )
        );
    }

    #[test]
    fn irreversible_action_denied() {
        assert!(
            !ConfidenceEngine::
                irreversible_action_allowed(
                    0.50,
                    0.80,
                    false,
                )
        );
    }
}