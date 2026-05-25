use serde::{
    Deserialize,
    Serialize,
};

use std::collections::HashMap;

#[derive(
    Debug,
    Clone,
    Serialize,
    Deserialize,
    PartialEq,
)]
pub enum HALDecision {
    Allow,
    Notify,
    Confirm,
    Block,
    Escalate,
}

#[derive(
    Debug,
    Clone,
    Serialize,
    Deserialize,
    PartialEq,
)]
pub enum IntentSeverity {
    Low,
    Moderate,
    High,
    Critical,
}

#[derive(
    Debug,
    Clone,
    Serialize,
    Deserialize,
    PartialEq,
)]
pub enum SyscallSensitivity {
    Safe,
    Sensitive,
    Dangerous,
    Irreversible,
}

#[derive(
    Debug,
    Clone,
    Serialize,
    Deserialize,
    PartialEq,
)]
pub enum ProvenanceConfidence {
    Verified,
    Trusted,
    Uncertain,
    Forged,
}

#[derive(
    Debug,
    Clone,
    Serialize,
    Deserialize,
)]
pub struct BehavioralMetrics {
    pub anomaly_score: f32,

    pub volatility_score: f32,

    pub escalation_attempts: u32,

    pub historical_stability: f32,

    pub recent_failures: u32,
}

#[derive(
    Debug,
    Clone,
    Serialize,
    Deserialize,
)]
pub struct ProvenanceData {
    pub source_agent: String,

    pub certificate_fingerprint:
        String,

    pub trust_chain_hash: String,

    pub signature_verified: bool,

    pub replay_checked: bool,

    pub confidence:
        ProvenanceConfidence,
}

#[derive(
    Debug,
    Clone,
    Serialize,
    Deserialize,
)]
pub struct SessionContext {
    pub session_id: String,

    pub user_present: bool,

    pub active_workspace:
        String,

    pub active_window_title:
        String,

    pub requires_confirmation:
        bool,

    pub user_attention_score:
        f32,
}

#[derive(
    Debug,
    Clone,
    Serialize,
    Deserialize,
)]
pub struct CapabilityContext {
    pub granted_capabilities:
        Vec<String>,

    pub temporary_grants:
        Vec<String>,

    pub denied_capabilities:
        Vec<String>,

    pub capability_expiry_ms:
        i64,
}

#[derive(
    Debug,
    Clone,
    Serialize,
    Deserialize,
)]
pub struct RiskVector {
    pub intent_risk: f32,

    pub syscall_risk: f32,

    pub trust_deficit: f32,

    pub anomaly_risk: f32,

    pub volatility_risk: f32,

    pub user_confidence: f32,

    pub provenance_confidence:
        f32,
}

#[derive(
    Debug,
    Clone,
    Serialize,
    Deserialize,
)]
pub struct HALContext {
    pub intent_id: String,

    pub source_agent: String,

    pub target_resource:
        String,

    pub requested_action:
        String,

    pub severity:
        IntentSeverity,

    pub syscall_sensitivity:
        SyscallSensitivity,

    pub provenance:
        ProvenanceData,

    pub behavioral:
        BehavioralMetrics,

    pub session:
        SessionContext,

    pub capabilities:
        CapabilityContext,

    pub metadata:
        HashMap<
            String,
            String,
        >,
}

#[derive(
    Debug,
    Clone,
    Serialize,
    Deserialize,
)]
pub struct HALResult {
    pub decision:
        HALDecision,

    pub computed_risk: f32,

    pub confidence: f32,

    pub explanation: String,

    pub violated_rules:
        Vec<String>,

    pub audit_hash: String,

    pub requires_user_prompt:
        bool,

    pub escalation_required:
        bool,
}

#[derive(
    Debug,
    Clone,
    Serialize,
    Deserialize,
)]
pub struct RuntimeAnomaly {
    pub anomaly_type: String,

    pub severity: f32,

    pub description: String,

    pub detected_at: i64,
}

#[derive(
    Debug,
    Clone,
    Serialize,
    Deserialize,
)]
pub struct AuditEvent {
    pub audit_id: String,

    pub parent_hash: String,

    pub event_hash: String,

    pub event_type: String,

    pub timestamp: i64,

    pub source_agent: String,

    pub payload_hash: String,
}

#[derive(
    Debug,
    Clone,
    Serialize,
    Deserialize,
)]
pub struct RestraintBoundary {
    pub maximum_allowed_risk:
        f32,

    pub irreversible_action_lock:
        bool,

    pub require_user_presence:
        bool,

    pub require_multi_factor:
        bool,
}

#[derive(
    Debug,
    Clone,
    Serialize,
    Deserialize,
)]
pub struct TrustState {
    pub current_score: f32,

    pub historical_average:
        f32,

    pub decay_rate: f32,

    pub recovery_rate: f32,

    pub compromise_suspected:
        bool,
}

#[derive(
    Debug,
    Clone,
    Serialize,
    Deserialize,
)]
pub enum EscalationLevel {
    None,
    Low,
    Medium,
    High,
    Critical,
}

#[derive(
    Debug,
    Clone,
    Serialize,
    Deserialize,
)]
pub struct EscalationContext {
    pub level:
        EscalationLevel,

    pub reason: String,

    pub requires_isolation:
        bool,

    pub requires_forensics:
        bool,
}