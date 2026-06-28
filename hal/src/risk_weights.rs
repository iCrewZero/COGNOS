//! Risk weights — configurable weights for each risk-model component.


use crate::hal_types::{
    IntentSeverity,
    RiskVector,
    SyscallSensitivity,
};

#[derive(Debug, Clone)]
pub struct RiskWeights {
    pub intent_weight: f32,

    pub syscall_weight: f32,

    pub trust_weight: f32,

    pub anomaly_weight: f32,

    pub volatility_weight: f32,

    pub user_confidence_weight:
        f32,

    pub provenance_weight: f32,
}

impl Default for RiskWeights {
    fn default() -> Self {
        Self {
            intent_weight: 0.22,

            syscall_weight: 0.20,

            trust_weight: 0.18,

            anomaly_weight: 0.15,

            volatility_weight: 0.10,

            user_confidence_weight:
                0.08,

            provenance_weight: 0.07,
        }
    }
}

pub struct RiskEngine {
    weights: RiskWeights,
}

impl RiskEngine {
    pub fn new() -> Self {
        Self {
            weights:
                RiskWeights::default(),
        }
    }

    pub fn with_weights(
        weights: RiskWeights,
    ) -> Self {
        Self {
            weights,
        }
    }

    pub fn compute_risk(
        &self,

        vector: &RiskVector,
    ) -> f32 {
        let mut risk = 0.0;

        risk +=
            vector.intent_risk
                * self
                    .weights
                    .intent_weight;

        risk +=
            vector.syscall_risk
                * self
                    .weights
                    .syscall_weight;

        risk +=
            vector.trust_deficit
                * self
                    .weights
                    .trust_weight;

        risk +=
            vector.anomaly_risk
                * self
                    .weights
                    .anomaly_weight;

        risk +=
            vector.volatility_risk
                * self
                    .weights
                    .volatility_weight;

        risk -=
            vector.user_confidence
                * self
                    .weights
                    .user_confidence_weight;

        risk -=
            vector.provenance_confidence
                * self
                    .weights
                    .provenance_weight;

        risk.clamp(0.0, 1.0)
    }

    pub fn severity_score(
        severity:
            &IntentSeverity,
    ) -> f32 {
        match severity {
            IntentSeverity::Low =>
                0.10,

            IntentSeverity::Moderate =>
                0.35,

            IntentSeverity::High =>
                0.70,

            IntentSeverity::Critical =>
                1.00,
        }
    }

    pub fn syscall_score(
        sensitivity:
            &SyscallSensitivity,
    ) -> f32 {
        match sensitivity {
            SyscallSensitivity::Safe =>
                0.05,

            SyscallSensitivity::Sensitive =>
                0.40,

            SyscallSensitivity::Dangerous =>
                0.80,

            SyscallSensitivity::Irreversible =>
                1.00,
        }
    }

    pub fn normalized_trust_deficit(
        trust_score: f32,
    ) -> f32 {
        (1.0 - trust_score)
            .clamp(0.0, 1.0)
    }

    pub fn anomaly_component(
        anomaly_score: f32,
    ) -> f32 {
        anomaly_score
            .clamp(0.0, 1.0)
    }

    pub fn volatility_component(
        volatility_score: f32,
    ) -> f32 {
        volatility_score
            .clamp(0.0, 1.0)
    }

    pub fn provenance_component(
        provenance_confidence:
            f32,
    ) -> f32 {
        provenance_confidence
            .clamp(0.0, 1.0)
    }

    pub fn user_confirmation_component(
        confirmation_strength:
            f32,
    ) -> f32 {
        confirmation_strength
            .clamp(0.0, 1.0)
    }

    pub fn irreversible_action_penalty(
        base_risk: f32,
    ) -> f32 {
        (
            base_risk + 0.35
        )
            .clamp(0.0, 1.0)
    }

    pub fn escalation_penalty(
        base_risk: f32,

        escalation_attempts: u32,
    ) -> f32 {
        let multiplier =
            1.0
                + (
                    escalation_attempts
                        as f32
                        * 0.15
                );

        (
            base_risk
                * multiplier
        )
            .clamp(0.0, 1.0)
    }

    pub fn confidence_score(
        trust_score: f32,

        provenance_confidence:
            f32,

        anomaly_score: f32,
    ) -> f32 {
        let confidence =
            (
                trust_score
                    * 0.50
            )
            + (
                provenance_confidence
                    * 0.35
            )
            - (
                anomaly_score
                    * 0.15
            );

        confidence
            .clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_low_risk() {
        let engine =
            RiskEngine::new();

        let vector =
            RiskVector {
                intent_risk: 0.1,

                syscall_risk: 0.1,

                trust_deficit: 0.1,

                anomaly_risk: 0.0,

                volatility_risk: 0.0,

                user_confidence: 0.9,

                provenance_confidence:
                    0.9,
            };

        let result =
            engine.compute_risk(
                &vector
            );

        assert!(
            result < 0.3
        );
    }

    #[test]
    fn compute_high_risk() {
        let engine =
            RiskEngine::new();

        let vector =
            RiskVector {
                intent_risk: 1.0,

                syscall_risk: 1.0,

                trust_deficit: 1.0,

                anomaly_risk: 1.0,

                volatility_risk: 1.0,

                user_confidence: 0.0,

                provenance_confidence:
                    0.0,
            };

        let result =
            engine.compute_risk(
                &vector
            );

        assert!(
            result > 0.8
        );
    }

    #[test]
    fn irreversible_penalty() {
        let risk =
            RiskEngine::
                irreversible_action_penalty(
                    0.5
                );

        assert!(
            risk > 0.5
        );
    }

    #[test]
    fn escalation_multiplier() {
        let risk =
            RiskEngine::
                escalation_penalty(
                    0.4,
                    5,
                );

        assert!(
            risk > 0.4
        );
    }
}