//! COGNOS Human Approval Layer (HAL).
//!
//! Library surface:
//! - [`risk_scorer`] — deterministic risk formula and hard floors
//! - [`audit_log`] — tamper-evident JSONL audit trail
//! - [`trust_calibration`] — per-user interrupt thresholds
//! - [`approval_flow`] (Unix only) — gate daemon over Unix sockets

pub mod audit_log;
pub mod risk_scorer;
pub mod trust_calibration;

#[cfg(unix)]
pub mod approval_flow;

pub use audit_log::{AuditEntry, AuditFilter, AuditLog, VerifyResult};
pub use risk_scorer::{
    describe_score, risk_level_from_score, score_action, ActionCategory, FloorRule,
    PatternMatchLevel, ProposedAction, RiskLevel, RiskScore, ScoreComponents, SystemContext,
    TimeAnomalyLevel, VibeCodeStatus,
};
pub use trust_calibration::{ActionClass, Feedback, TrustCalibration};

#[cfg(unix)]
pub use approval_flow::{GateReason, GateRequest, GateResponse, HalDaemon};
