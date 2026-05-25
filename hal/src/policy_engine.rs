use crate::{
    action_validator::ActionValidator,

    confidence_engine::ConfidenceEngine,

    hal_types::{
        EscalationContext,
        EscalationLevel,
        HALContext,
        HALDecision,
        HALResult,
        IntentSeverity,
        RestraintBoundary,
        SyscallSensitivity,
    },

    risk_weights::RiskEngine,
};

pub struct PolicyEngine;

impl PolicyEngine {
    pub fn evaluate(
        context:
            &HALContext,

        risk: f32,

        confidence: f32,

        boundaries:
            &RestraintBoundary,
    ) -> HALResult {
        let mut violated =
            Vec::new();

        if risk
            > boundaries
                .maximum_allowed_risk
        {
            violated.push(
                "risk_boundary_exceeded"
                    .into()
            );
        }

        if boundaries
            .irreversible_action_lock
            && Self::irreversible(
                context
            )
        {
            violated.push(
                "irreversible_action_locked"
                    .into()
            );
        }

        if boundaries
            .require_user_presence
            && !context
                .session
                .user_present
        {
            violated.push(
                "user_presence_required"
                    .into()
            );
        }

        if boundaries
            .require_multi_factor
            && confidence < 0.92
        {
            violated.push(
                "multi_factor_requirement"
                    .into()
            );
        }

        if context
            .behavioral
            .anomaly_score
            > 0.85
        {
            violated.push(
                "behavioral_anomaly"
                    .into()
            );
        }

        if context
            .behavioral
            .escalation_attempts
            > 3
        {
            violated.push(
                "repeated_escalation"
                    .into()
            );
        }

        let decision =
            Self::policy_decision(
                &violated,
                risk,
                confidence,
            );

        HALResult {
            decision:
                decision.clone(),

            computed_risk:
                risk,

            confidence,

            explanation:
                format!(
                    "policy evaluation with {} rule violations",
                    violated.len()
                ),

            violated_rules:
                violated,

            audit_hash:
                format!(
                    "policy:{}:{}",
                    risk,
                    confidence
                ),

            requires_user_prompt:
                matches!(
                    decision,
                    HALDecision::Confirm
                ),

            escalation_required:
                matches!(
                    decision,
                    HALDecision::Escalate
                        | HALDecision::Block
                ),
        }
    }

    fn policy_decision(
        violations:
            &[String],

        risk: f32,

        confidence: f32,
    ) -> HALDecision {
        if violations.iter()
            .any(
                |v| {
                    v == "irreversible_action_locked"
                },
            )
        {
            return HALDecision::Block;
        }

        if violations.iter()
            .any(
                |v| {
                    v == "behavioral_anomaly"
                },
            )
        {
            return HALDecision::Escalate;
        }

        if violations.iter()
            .any(
                |v| {
                    v == "repeated_escalation"
                },
            )
        {
            return HALDecision::Escalate;
        }

        if risk > 0.95 {
            return HALDecision::Block;
        }

        if confidence < 0.20 {
            return HALDecision::Block;
        }

        if risk > 0.70 {
            return HALDecision::Confirm;
        }

        if !violations
            .is_empty()
        {
            return HALDecision::Notify;
        }

        HALDecision::Allow
    }

    fn irreversible(
        context:
            &HALContext,
    ) -> bool {
        matches!(
            context.severity,
            IntentSeverity::Critical
        ) || matches!(
            context
                .syscall_sensitivity,
            SyscallSensitivity::Irreversible
        )
    }

    pub fn enforce_restraint(
        risk: f32,

        confidence: f32,
    ) -> bool {
        risk < 0.35
            && confidence > 0.85
    }

    pub fn adaptive_risk_boundary(
        base_boundary: f32,

        anomaly_score: f32,

        trust_score: f32,
    ) -> f32 {
        let anomaly_penalty =
            anomaly_score
                * 0.25;

        let trust_bonus =
            trust_score
                * 0.10;

        (
            base_boundary
                - anomaly_penalty
                + trust_bonus
        )
            .clamp(0.10, 0.95)
    }

    pub fn requires_isolation(
        context:
            &HALContext,
    ) -> bool {
        context
            .behavioral
            .anomaly_score
            > 0.90
            || context
                .behavioral
                .escalation_attempts
                > 5
    }

    pub fn escalation_context(
        context:
            &HALContext,
    ) -> EscalationContext {
        if context
            .behavioral
            .anomaly_score
            > 0.90
        {
            return EscalationContext {
                level:
                    EscalationLevel::Critical,

                reason:
                    "critical behavioral anomaly"
                        .into(),

                requires_isolation:
                    true,

                requires_forensics:
                    true,
            };
        }

        if context
            .behavioral
            .escalation_attempts
            > 3
        {
            return EscalationContext {
                level:
                    EscalationLevel::High,

                reason:
                    "persistent escalation attempts"
                        .into(),

                requires_isolation:
                    true,

                requires_forensics:
                    false,
            };
        }

        EscalationContext {
            level:
                EscalationLevel::Low,

            reason:
                "minor policy deviation"
                    .into(),

            requires_isolation:
                false,

            requires_forensics:
                false,
        }
    }

    pub fn authority_continuity(
        trust_score: f32,

        provenance_score: f32,

        capability_valid: bool,
    ) -> bool {
        trust_score > 0.70
            && provenance_score
                > 0.75
            && capability_valid
    }

    pub fn bounded_autonomy(
        risk: f32,

        confidence: f32,

        anomaly_score: f32,
    ) -> bool {
        risk < 0.30
            && confidence > 0.90
            && anomaly_score
                < 0.15
    }

    pub fn irreversible_requires_human(
        context:
            &HALContext,
    ) -> bool {
        Self::irreversible(
            context
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adaptive_boundary_reduces_with_anomaly(
    ) {
        let boundary =
            PolicyEngine::
                adaptive_risk_boundary(
                    0.80,
                    0.90,
                    0.20,
                );

        assert!(
            boundary < 0.80
        );
    }

    #[test]
    fn bounded_autonomy_allowed() {
        assert!(
            PolicyEngine::
                bounded_autonomy(
                    0.10,
                    0.95,
                    0.05,
                )
        );
    }

    #[test]
    fn authority_continuity_valid() {
        assert!(
            PolicyEngine::
                authority_continuity(
                    0.90,
                    0.91,
                    true,
                )
        );
    }
}