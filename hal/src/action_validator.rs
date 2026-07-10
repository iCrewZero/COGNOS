//! Action validator — checks proposed actions against HAL policy before execution.


use regex::Regex;

use crate::{
    hal_types::{
        HALContext,
        HALDecision,
        HALResult,
        IntentSeverity,
        SyscallSensitivity,
    },
};

pub struct ActionValidator;

impl ActionValidator {
    pub fn validate(
        context:
            &HALContext,

        computed_risk: f32,

        confidence: f32,
    ) -> HALResult {
        let mut violations =
            Vec::new();

        if Self::dangerous_path(
            &context
                .target_resource
        ) {
            violations.push(
                "dangerous_path_access"
                    .into()
            );
        }

        if Self::raw_shell_detected(
            &context
                .requested_action
        ) {
            violations.push(
                "raw_shell_execution"
                    .into()
            );
        }

        if Self::privilege_escalation(
            &context
                .requested_action
        ) {
            violations.push(
                "privilege_escalation"
                    .into()
            );
        }

        if Self::destructive_pattern(
            &context
                .requested_action
        ) {
            violations.push(
                "destructive_pattern"
                    .into()
            );
        }

        if Self::suspicious_network(
            &context
                .requested_action
        ) {
            violations.push(
                "suspicious_network"
                    .into()
            );
        }

        if Self::irreversible_action(
            context
        ) {
            violations.push(
                "irreversible_action"
                    .into()
            );
        }

        let decision =
            Self::final_decision(
                &violations,
                computed_risk,
                confidence,
            );

        HALResult {
            decision:
                decision.clone(),

            computed_risk,

            confidence,

            explanation:
                format!(
                    "validated with {} violations",
                    violations.len()
                ),

            violated_rules:
                violations,

            audit_hash:
                format!(
                    "validator:{}:{}",
                    computed_risk,
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

    fn dangerous_path(
        path: &str,
    ) -> bool {
        let dangerous = [
            "/etc",
            "/boot",
            "/sys",
            "/proc",
            "/dev",
            "/root",
            "/var/lib",
        ];

        dangerous.iter()
            .any(
                |d| {
                    path.starts_with(
                        d
                    )
                },
            )
    }

    fn raw_shell_detected(
        action: &str,
    ) -> bool {
        let patterns = [
            r"rm\s+-rf",
            r"mkfs",
            r"dd\s+if=",
            r"chmod\s+777",
            r"curl.+\|.+sh",
            r"wget.+\|.+bash",
        ];

        patterns.iter()
            .any(
                |p| {
                    Regex::new(p)
                        .unwrap()
                        .is_match(
                            action
                        )
                },
            )
    }

    fn privilege_escalation(
        action: &str,
    ) -> bool {
        let patterns = [
            "sudo ",
            "su ",
            "setcap",
            "pkexec",
            "capsh",
        ];

        patterns.iter()
            .any(
                |p| {
                    action.contains(
                        p
                    )
                },
            )
    }

    fn destructive_pattern(
        action: &str,
    ) -> bool {
        let patterns = [
            "rm -rf /",
            ":(){ :|:& };:",
            "shutdown now",
            "reboot",
            "poweroff",
        ];

        patterns.iter()
            .any(
                |p| {
                    action.contains(
                        p
                    )
                },
            )
    }

    fn suspicious_network(
        action: &str,
    ) -> bool {
        let patterns = [
            "nc ",
            "ncat ",
            "socat ",
            "0.0.0.0",
            "reverse_shell",
        ];

        patterns.iter()
            .any(
                |p| {
                    action.contains(
                        p
                    )
                },
            )
    }

    fn irreversible_action(
        context:
            &HALContext,
    ) -> bool {
        matches!(
            context
                .severity,
            IntentSeverity::Critical
        ) || matches!(
            context
                .syscall_sensitivity,
            SyscallSensitivity::Irreversible
        )
    }

    fn final_decision(
        violations:
            &[String],

        risk: f32,

        confidence: f32,
    ) -> HALDecision {
        if violations.iter()
            .any(
                |v| {
                    v == "destructive_pattern"
                },
            )
        {
            return HALDecision::Block;
        }

        if violations.iter()
            .any(
                |v| {
                    v == "privilege_escalation"
                },
            )
        {
            return HALDecision::Escalate;
        }

        if risk > 0.90 {
            return HALDecision::Block;
        }

        if confidence < 0.25 {
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

    pub fn syscall_risk(
        syscall: &str,
    ) -> f32 {
        match syscall {
            "execve" => 0.80,

            "ptrace" => 0.95,

            "mount" => 0.90,

            "reboot" => 1.00,

            "unlink" => 0.70,

            "open" => 0.20,

            "read" => 0.10,

            _ => 0.50,
        }
    }

    pub fn bounded_execution(
        risk: f32,

        confidence: f32,
    ) -> bool {
        risk < 0.35
            && confidence > 0.85
    }

    pub fn validate_capability_scope(
        capabilities:
            &[String],

        requested_action:
            &str,
    ) -> bool {
        if requested_action
            .contains(
                "network"
            )
        {
            return capabilities
                .contains(
                    &"network.outbound"
                        .into()
                );
        }

        if requested_action
            .contains(
                "write"
            )
        {
            return capabilities
                .contains(
                    &"filesystem.write"
                        .into()
                );
        }

        true
    }

    pub fn semantic_intent_match(
        intent: &str,

        action: &str,
    ) -> bool {
        let normalized_intent =
            intent
                .to_lowercase();

        let normalized_action =
            action
                .to_lowercase();

        normalized_action
            .contains(
                &normalized_intent
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_dangerous_shell() {
        assert!(
            ActionValidator::
                raw_shell_detected(
                    "rm -rf /"
                )
        );
    }

    #[test]
    fn detects_privilege_escalation() {
        assert!(
            ActionValidator::
                privilege_escalation(
                    "sudo bash"
                )
        );
    }

    #[test]
    fn syscall_risk_high() {
        let risk =
            ActionValidator::
                syscall_risk(
                    "ptrace"
                );

        assert!(
            risk > 0.90
        );
    }

    #[test]
    fn bounded_execution_allowed() {
        assert!(
            ActionValidator::
                bounded_execution(
                    0.10,
                    0.95,
                )
        );
    }
}