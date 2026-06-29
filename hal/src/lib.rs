//! COGNOS Human Approval Layer (HAL).
//!
//!
//! Library surface:
//! - [`risk_scorer`] — deterministic risk formula and hard floors
//! - [`audit_log`] — tamper-evident JSONL audit trail
//! - [`trust_calibration`] — per-user interrupt thresholds
//! - [`approval_flow`] (Unix only) — gate daemon over Unix sockets
//!
//! Policy subsystem (context-rich evaluation pipeline):
//! - [`hal_types`] — shared context/result types ([`hal_types::HALContext`], [`hal_types::HALResult`])
//! - [`risk_weights`] — weighted risk-vector engine
//! - [`confidence_engine`] — trust/provenance/behavior confidence fusion
//! - [`action_validator`] — dangerous-path and rule validation
//! - [`policy_engine`] — boundary evaluation producing a HAL decision
//! - [`anomaly_detection`] — per-agent metric anomaly tracking
//! - [`behavioral_model`] — per-agent behavior history feeding risk inputs
//! - [`provenance`] — signature and hash-chain verification
//! - [`runtime_state`] — shared async state (trust map, decisions, audit chain)
//! - [`session_context`] — session construction helpers
//! - [`permissions`] — closed capability enumeration (capability lattice)
//! - [`restraint_model`] — prediction gating for the cognitive preloader

pub mod action_validator;
pub mod anomaly_detection;
pub mod audit_log;
pub mod behavioral_model;
pub mod confidence_engine;
pub mod hal_types;
pub mod permissions;
pub mod policy_engine;
pub mod provenance;
pub mod restraint_model;
pub mod risk_scorer;
pub mod risk_weights;
pub mod runtime_state;
pub mod session_context;
pub mod trust_calibration;

#[cfg(unix)]
pub mod approval_flow;

pub use audit_log::{AuditEntry, AuditFilter, AuditLog, VerifyResult};
pub use risk_scorer::{
    describe_score, score_action,
    IrreversibilityLevel, ScopeLevel, TrustContextLevel,
    TimeAnomalyLevel, VibeFlagLevel, UserHistoryLevel, PatternMatchLevel,
    ProposedAction, RiskLevel, RiskScore, ComponentScores, SystemContext,
};
pub use trust_calibration::{ActionClass, Feedback, TrustCalibration};

#[cfg(unix)]
pub use approval_flow::{GateReason, GateRequest, GateResponse, HalDaemon};

pub mod audit_chain;
pub mod authority_compressor;
pub mod autonomous_constitution;
pub mod autonomy_controller;
pub mod behavior_monitor;
pub mod capability_lattice;
pub mod cognitive_equilibrium;
pub mod cognitive_firewall;
pub mod continuity_engine;
pub mod existential_governor;
pub mod forensic_replay;
pub mod governance_kernel;
pub mod meta_governance;
pub mod recovery_kernel;
pub mod recursion_limiter;
pub mod recursive_identity;
pub mod reputation_engine;
pub mod restraint_runtime;
pub mod score_fusion;
pub mod self_preservation;
pub mod self_rewrite_monitor;
pub mod syscall_tracker;
pub mod temporal_trust;
pub mod trust_heuristics;
