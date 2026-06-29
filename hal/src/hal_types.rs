//! Shared HAL types — context, result, decisions, and component scores.
//!
//!
//! This module collects the data types that flow through the HAL pipeline.
//! It is intentionally the single shared vocabulary that other HAL modules
//! (policy_engine, confidence_engine, runtime_state, ...) import from, so
//! that those modules do not need to depend on each other directly.
//!
//! The original v0 types (HALContext, HALResult, HALDecision, etc.) are
//! preserved for backward compatibility. The supplemental types added in
//! Task 2-b (HALContextV2, HALResultV2, HalComponentScores, BlockReason,
//! TimeContext, TrustSnapshot) reflect the spec's preferred v0 shape and
//! are intended to supersede the originals in v1.
//!
//! v0: stub implementation.

use serde::{
    Deserialize,
    Serialize,
};

use std::collections::HashMap;

// v0: stub implementation

// Re-export the canonical ComponentScores from the risk scorer so callers
// can pull every shared HAL type from `hal_types` if they prefer.
pub use crate::risk_scorer::ComponentScores as HalComponentScores;

#[derive(
    Debug,
    Clone,
    Serialize,
    Deserialize,
    PartialEq,
)]
pub enum HALDecision {
    Allow,
    /// Allow the action and surface a non-blocking notice to the user.
    AllowWithNotice,
    /// Defer the action and ask the user for confirmation.
    Ask,
    Notify,
    Confirm,
    Block,
    /// Block the action and emit a high-priority alert (operator-visible).
    BlockAndAlert,
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

// ─── v0 Supplemental Types (Task 2-b) ───────────────────────────────────────────
//
// The types below were added in Task 2-b to reflect the spec's preferred v0
// shape for shared HAL types. The original HALContext, HALResult, and
// HALDecision above are preserved for backward compatibility; v1 should
// migrate all callers to the V2 shapes and delete the originals.

/// V2 HAL context: the full bundle of inputs that HAL consults when deciding
/// what to do with an action. Spec'd by Task 2-b with the fields:
/// `user_id, session_id, agent_id, source, action, system_state, user_history,
/// time_context, trust_snapshot`.
#[derive(
    Debug,
    Clone,
    Serialize,
    Deserialize,
)]
pub struct HALContextV2 {
    /// The user on whose behalf the action is being taken.
    pub user_id: String,

    /// The session the action belongs to.
    pub session_id: String,

    /// The agent proposing the action.
    pub agent_id: String,

    /// The origin of the action (e.g. "user_request", "planner",
    /// "cognitive_preloader").
    pub source: String,

    /// The action descriptor itself, as an opaque JSON blob. v0 keeps this
    /// as `Value` to avoid coupling to any one action schema; v1 will define
    /// a proper `ActionDescriptor` type.
    pub action: serde_json::Value,

    /// Snapshot of relevant system state at decision time.
    pub system_state: SystemStateSnapshot,

    /// The user's recent action history, summarized.
    pub user_history: UserHistorySummary,

    /// Time-of-day and temporal-anomaly context.
    pub time_context: TimeContext,

    /// Snapshot of the trust state for the acting agent.
    pub trust_snapshot: TrustSnapshot,
}

/// V2 HAL result: the decision HAL reached, plus the supporting scores and
/// metadata. Spec'd by Task 2-b with the fields:
/// `decision, risk_score, components, gate_reason, audit_id, expires_at`.
#[derive(
    Debug,
    Clone,
    Serialize,
    Deserialize,
)]
pub struct HALResultV2 {
    /// The decision HAL reached.
    pub decision: HALDecision,

    /// The final risk score in `[0.0, 1.0]`.
    pub risk_score: f32,

    /// Per-component scores, for transparency and audit.
    pub components: HalComponentScores,

    /// If the decision was Block or Ask, the reason that triggered the gate.
    pub gate_reason: Option<BlockReason>,

    /// Identifier of the audit entry recording this decision.
    pub audit_id: String,

    /// When this result expires and must be re-evaluated. Cached HAL results
    /// must not be used past this timestamp.
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

/// Minimal per-component scores mirror matching the Task 2-b spec (all f32).
///
/// This is a thin re-export of the canonical `ComponentScores` from
/// `risk_scorer`; the canonical type has the same seven f32 fields, plus two
/// extras (`hard_floor_applied`, `hard_floor_reason`) that do not affect the
/// score formula.
pub use crate::risk_scorer::ComponentScores as ComponentScoresV2;

/// Reasons HAL may gate or block an action. Spec'd by Task 2-b as the
/// `GateReason` enum with variants `RiskFloor, UserHistoryInsufficient,
/// DangerousPath, PolicyDeny, ...`.
///
/// This is distinct from `crate::approval_flow::GateReason`, which records
/// the *outcome* of a gate decision (AutoApproved, UserDenied, ...). This
/// enum records the *reason* a gate was triggered.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum BlockReason {
    /// A hard risk floor was triggered (e.g. delete actions always ≥ 0.5).
    RiskFloor,
    /// The user has insufficient history with this action to auto-approve.
    UserHistoryInsufficient,
    /// The action targets a dangerous path (kernel, system, etc.).
    DangerousPath,
    /// An explicit policy in the governance kernel denies this action.
    PolicyDeny,
    /// The action violates a constitutional article.
    ConstitutionViolation,
    /// The action threatens a protected resource (existential governor).
    ExistentialThreat,
    /// The acting agent is currently rate-limited by the recursion limiter.
    RecursionLimited,
    /// The action is irreversible and the user has not opted into auto-approve
    /// for irreversible actions.
    IrreversibleWithoutOptIn,
    /// The proposed action is a self-rewrite that requires human review.
    SelfRewriteRequiresReview,
    /// A meta-governance rule (e.g. autonomy escalation) requires human
    /// consent.
    RequiresConsent,
    /// Catch-all for any other gate reason not yet enumerated. v1 should
    /// remove this variant once the enum is complete.
    Other,
}

/// Snapshot of relevant system state at decision time.
#[derive(
    Debug,
    Clone,
    Serialize,
    Deserialize,
)]
pub struct SystemStateSnapshot {
    /// Current autonomy level (e.g. "Supervised", "Advisory", ...).
    pub autonomy_level: String,
    /// Whether HAL is currently in lockdown.
    pub lockdown: bool,
    /// Whether the audit chain has been verified since boot.
    pub audit_chain_verified: bool,
    /// Free-form metadata.
    pub metadata: HashMap<String, String>,
}

impl Default for SystemStateSnapshot {
    fn default() -> Self {
        Self {
            autonomy_level: "Supervised".to_string(),
            lockdown: false,
            audit_chain_verified: false,
            metadata: HashMap::new(),
        }
    }
}

/// Summary of the user's recent action history.
#[derive(
    Debug,
    Clone,
    Serialize,
    Deserialize,
)]
pub struct UserHistorySummary {
    /// Number of times the user has performed this exact action.
    pub count: u32,
    /// Whether this action is in the user's routine set (>100 occurrences).
    pub is_routine: bool,
    /// The last time the user performed this action, if ever.
    pub last_performed: Option<chrono::DateTime<chrono::Utc>>,
}

impl Default for UserHistorySummary {
    fn default() -> Self {
        Self {
            count: 0,
            is_routine: false,
            last_performed: None,
        }
    }
}

/// Time-of-day and temporal-anomaly context.
#[derive(
    Debug,
    Clone,
    Serialize,
    Deserialize,
)]
pub struct TimeContext {
    /// The wall-clock time of the action.
    pub now: chrono::DateTime<chrono::Utc>,
    /// The local-time hour bucket (0-23) the action falls in.
    pub local_hour: u32,
    /// Whether the action is occurring outside the user's normal hours.
    pub is_unusual_hour: bool,
    /// Whether the action's scope is also unusual for this time.
    pub unusual_time_and_scope: bool,
}

impl Default for TimeContext {
    fn default() -> Self {
        Self {
            now: chrono::Utc::now(),
            local_hour: 0,
            is_unusual_hour: false,
            unusual_time_and_scope: false,
        }
    }
}

/// Snapshot of the trust state for the acting agent.
#[derive(
    Debug,
    Clone,
    Serialize,
    Deserialize,
)]
pub struct TrustSnapshot {
    /// The agent's effective trust score in `[0.0, 1.0]`.
    pub trust_score: f32,
    /// The agent's reputation score in `[0.0, 1.0]`.
    pub reputation_score: f32,
    /// Whether the agent is currently under any trust penalty.
    pub penalized: bool,
    /// When the trust score was last updated.
    pub last_updated: chrono::DateTime<chrono::Utc>,
}

impl Default for TrustSnapshot {
    fn default() -> Self {
        Self {
            trust_score: 0.5,
            reputation_score: 0.5,
            penalized: false,
            last_updated: chrono::Utc::now(),
        }
    }
}

// TODO(v1): migrate all callers from HALContext -> HALContextV2,
// HALResult -> HALResultV2, and from BlockReason::Other to specific
// variants. The original types should be removed once the migration is
// complete.