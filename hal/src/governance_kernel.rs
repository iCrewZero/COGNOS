//! Governance kernel — the kernel-resident governance enforcer.
//!
//!
//! In production this module is compiled into the HAL kernel module and
//! evaluates every action request before it reaches the syscall dispatch
//! path. It is the *last* line of defense: even if every userspace HAL
//! component is compromised, the governance kernel should still deny
//! forbidden actions.
//!
//! The evaluation rule is simple:
//!   1. Find all policies whose `condition` matches the request.
//!   2. The highest-priority matching policy wins.
//!   3. If no policy matches, default to Deny.
//!
//! "Deny by default" is non-negotiable. Adding a new Allow policy requires
//! human review and a spec update.
//!
//! v0: stub implementation. Policy storage and the matching loop are in
//! place; the condition DSL is a TODO(v1) placeholder (always matches).

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::info;
use uuid::Uuid;

// v0: stub implementation

// ─── Effects ────────────────────────────────────────────────────────────────────

/// The effect a policy has on a matching request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Effect {
    /// Allow the request to proceed.
    Allow,
    /// Deny the request outright.
    Deny,
    /// Escalate to the user for an explicit decision.
    Ask,
    /// Allow but write a mandatory audit entry (no UX interruption).
    Log,
}

// ─── Conditions ─────────────────────────────────────────────────────────────────

/// A policy condition.
///
/// v0 supports a small compositional DSL. v1 will add request-target
/// matching, capability matching, and time-window matching.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Condition {
    /// Always matches (useful for default policies).
    Always,
    /// Matches when the request action type equals the given string.
    ActionEquals(String),
    /// Matches when the request agent equals the given string.
    AgentEquals(String),
    /// Matches when the request risk score is at least the given value.
    RiskAtLeast(f32),
    /// Conjunction of two conditions.
    And(Box<Condition>, Box<Condition>),
    /// Disjunction of two conditions.
    Or(Box<Condition>, Box<Condition>),
}

impl Condition {
    /// Evaluate the condition against a request.
    pub fn matches(&self, request: &ActionRequest) -> bool {
        match self {
            Self::Always => true,
            Self::ActionEquals(s) => request.action_type == *s,
            Self::AgentEquals(s) => request.agent == *s,
            Self::RiskAtLeast(t) => request.risk_score >= *t,
            Self::And(a, b) => a.matches(request) && b.matches(request),
            Self::Or(a, b) => a.matches(request) || b.matches(request),
        }
    }
}

// ─── Policy ─────────────────────────────────────────────────────────────────────

/// A single governance policy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Policy {
    /// Stable policy ID (used for audit and override tracking).
    pub id: Uuid,
    /// Priority — higher wins. Ties broken by insertion order.
    pub priority: i32,
    /// Condition under which this policy applies.
    pub condition: Condition,
    /// Effect when the condition matches.
    pub effect: Effect,
    /// Human-readable description (shown in audit logs).
    pub description: String,
    /// When the policy was installed.
    pub installed_at: DateTime<Utc>,
}

// ─── Action Request / Verdict ───────────────────────────────────────────────────

/// An action request submitted to the governance kernel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionRequest {
    /// Acting agent.
    pub agent: String,
    /// Action type (e.g. "delete_file").
    pub action_type: String,
    /// Target resource path or identifier.
    pub target: String,
    /// HAL risk score ∈ [0.0, 1.0].
    pub risk_score: f32,
}

/// The kernel's verdict on a request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Verdict {
    /// The winning effect.
    pub effect: Effect,
    /// ID of the policy that produced this verdict (None = default-deny).
    pub policy_id: Option<Uuid>,
    /// Human-readable reason for the audit log.
    pub reason: String,
    /// Timestamp of the verdict.
    pub at: DateTime<Utc>,
}

// ─── Policy Errors ──────────────────────────────────────────────────────────────

/// Errors returned by the governance kernel.
#[derive(Debug, Error)]
pub enum PolicyError {
    /// A policy with the same ID already exists.
    #[error("policy {0} already installed")]
    DuplicatePolicy(Uuid),
    /// The policy priority is invalid (e.g. reserved range).
    #[error("policy priority {0} is in a reserved range")]
    InvalidPriority(i32),
}

// ─── Governance Kernel ──────────────────────────────────────────────────────────

/// The governance kernel. Owns the policy table and a verdict cache.
#[derive(Debug)]
pub struct GovernanceKernel {
    policies: Vec<Policy>,
    /// Cache of (request-hash → verdict) for hot-path fast-paths.
    /// TODO(v1): wire an actual hash key; v0 leaves the cache empty.
    verdict_cache: HashMap<String, Verdict>,
}

impl Default for GovernanceKernel {
    fn default() -> Self {
        Self {
            policies: Vec::new(),
            verdict_cache: HashMap::new(),
        }
    }
}

impl GovernanceKernel {
    /// Construct an empty kernel.
    pub fn new() -> Self {
        Self::default()
    }

    /// Install a new policy. Returns an error on duplicate ID or on a
    /// priority in the reserved range.
    pub fn install_policy(&mut self, policy: Policy) -> Result<(), PolicyError> {
        if self.policies.iter().any(|p| p.id == policy.id) {
            return Err(PolicyError::DuplicatePolicy(policy.id));
        }
        // Reserved priority range [-1000, -1] is reserved for kernel defaults.
        if (-1000..=-1).contains(&policy.priority) {
            return Err(PolicyError::InvalidPriority(policy.priority));
        }
        info!(
            policy_id = %policy.id,
            priority = policy.priority,
            "policy installed"
        );
        self.policies.push(policy);
        // Policies are evaluated in priority order; keep sorted descending
        // so the highest-priority match wins on the first iteration.
        self.policies.sort_by(|a, b| b.priority.cmp(&a.priority));
        // Cache is invalidated on any policy change.
        self.verdict_cache.clear();
        Ok(())
    }

    /// Remove a policy by ID. Returns true if a policy was removed.
    pub fn remove_policy(&mut self, id: Uuid) -> bool {
        let before = self.policies.len();
        self.policies.retain(|p| p.id != id);
        let removed = self.policies.len() < before;
        if removed {
            self.verdict_cache.clear();
        }
        removed
    }

    /// Evaluate a request. Highest-priority matching policy wins; deny by
    /// default.
    pub fn evaluate(&self, request: &ActionRequest) -> Verdict {
        for policy in &self.policies {
            if policy.condition.matches(request) {
                return Verdict {
                    effect: policy.effect,
                    policy_id: Some(policy.id),
                    reason: format!(
                        "matched policy '{}' (priority {})",
                        policy.description, policy.priority
                    ),
                    at: Utc::now(),
                };
            }
        }
        // Default-deny — non-negotiable.
        Verdict {
            effect: Effect::Deny,
            policy_id: None,
            reason: "no matching policy — default-deny".to_string(),
            at: Utc::now(),
        }
    }

    /// Borrow the policy table (for inspection / audit).
    pub fn policies(&self) -> &[Policy] {
        &self.policies
    }
}
