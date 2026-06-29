//! Meta-governance — governance of governance.
//!
//!
//! Every other HAL module governs *agent actions*. Meta-governance governs
//! *changes to the governance itself*: installing a new policy, adjusting a
//! risk weight, redefining a hard floor. Without meta-governance, an agent
//! that gained the "install policy" capability could quietly legalize every
//! action it wanted to take.
//!
//! The meta-governance rules in v0 are deliberately conservative. Every
//! policy change proposal must:
///   1. be recorded as a proposal with a unique id,
///   2. receive explicit human approval,
///   3. wait at least 24 hours between approval and effect,
///   4. be recorded in the audit chain, and
///   5. be reversible — every applied change has a recorded revert procedure.
//!
//! v0: stub implementation. The 24h delay is recorded but not enforced (no
//! background scheduler); v1 will wire this to a real timer.

use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{info, warn};
use uuid::Uuid;

// v0: stub implementation

/// Type alias for an agent identifier (matches the rest of the crate).
pub type AgentId = String;

/// Identifier of a policy-change proposal.
pub type ProposalId = Uuid;

/// The mandatory delay between human approval and effect.
const APPROVAL_DELAY: Duration = Duration::hours(24);

// ─── Policy Change ──────────────────────────────────────────────────────────────

/// A proposed change to the governance itself.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyChange {
    /// Human-readable description of the change.
    pub description: String,
    /// The change payload, as an opaque JSON blob. v0 does not interpret this;
    /// v1 will define a proper schema per change type.
    pub payload: serde_json::Value,
    /// The agent proposing the change.
    pub proposer: AgentId,
    /// When the proposal was created.
    pub proposed_at: DateTime<Utc>,
}

/// The policy governing how policy changes themselves are made.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyChangePolicy {
    /// Whether human approval is required (always `true` in v0).
    pub require_human_approval: bool,
    /// The mandatory delay between approval and effect.
    pub approval_delay: Duration,
    /// Whether the change must be reversible (always `true` in v0).
    pub require_reversibility: bool,
    /// Whether an audit entry must be written (always `true` in v0).
    pub require_audit_entry: bool,
}

impl Default for PolicyChangePolicy {
    fn default() -> Self {
        Self {
            require_human_approval: true,
            approval_delay: APPROVAL_DELAY,
            require_reversibility: true,
            require_audit_entry: true,
        }
    }
}

// ─── Human Approval ─────────────────────────────────────────────────────────────

/// A human-approval token accompanying a ratification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HumanApproval {
    /// Identifier of the approving human operator.
    pub approver_id: String,
    /// When the approval was granted.
    pub approved_at: DateTime<Utc>,
    /// Optional free-text justification.
    pub justification: Option<String>,
    /// Opaque signature blob. v0 does not verify this; v1 will.
    pub signature: Option<String>,
}

// ─── Proposal State ─────────────────────────────────────────────────────────────

/// The lifecycle state of a policy-change proposal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProposalState {
    /// Proposed but not yet ratified.
    Pending,
    /// Ratified by a human; waiting for the approval delay to elapse.
    Ratified {
        /// When the proposal was ratified.
        ratified_at: DateTime<Utc>,
        /// When the proposal is eligible to take effect.
        effective_at: DateTime<Utc>,
    },
    /// The change has taken effect.
    Applied {
        /// When the change took effect.
        applied_at: DateTime<Utc>,
    },
    /// The change was reverted.
    Reverted {
        /// When the revert took effect.
        reverted_at: DateTime<Utc>,
    },
    /// The proposal was withdrawn or rejected.
    Withdrawn,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProposalRecord {
    id: ProposalId,
    change: PolicyChange,
    state: ProposalState,
    approval: Option<HumanApproval>,
    audit_entry: Option<String>,
}

// ─── Meta-Governance ────────────────────────────────────────────────────────────

/// The meta-governance engine.
pub struct MetaGovernance {
    /// The active policy-change policy.
    pub change_policy: PolicyChangePolicy,
    /// History of all proposals, keyed by id.
    pub history: HashMap<ProposalId, ProposalRecord>,
}

impl Default for MetaGovernance {
    fn default() -> Self {
        Self::new()
    }
}

impl MetaGovernance {
    /// Build a new meta-governance engine with the default policy.
    pub fn new() -> Self {
        Self {
            change_policy: PolicyChangePolicy::default(),
            history: HashMap::new(),
        }
    }

    /// Propose a new policy change.
    ///
    /// Returns the id of the new proposal. The proposal is recorded in the
    /// `Pending` state and must be ratified by a human before it can take
    /// effect.
    pub fn propose_change(&mut self, proposal: PolicyChange) -> Result<ProposalId, MetaError> {
        let id = Uuid::new_v4();
        let record = ProposalRecord {
            id,
            change: proposal,
            state: ProposalState::Pending,
            approval: None,
            audit_entry: None,
        };
        info!(%id, "meta_governance: proposal recorded");
        self.history.insert(id, record);
        Ok(id)
    }

    /// Ratify a proposal with explicit human approval.
    ///
    /// Records the approval and schedules the change to take effect after
    /// the mandatory delay. v0 does not actually apply the change at the
    /// scheduled time — the caller must poll [`Self::apply_if_ready`].
    pub fn ratify(
        &mut self,
        id: ProposalId,
        human_approval: HumanApproval,
    ) -> Result<(), MetaError> {
        if !self.change_policy.require_human_approval {
            warn!("meta_governance: human approval requirement disabled");
        }
        let record = self
            .history
            .get_mut(&id)
            .ok_or(MetaError::NotFound { id })?;
        if !matches!(record.state, ProposalState::Pending) {
            return Err(MetaError::NotPending { id, state: format!("{:?}", record.state) });
        }
        let ratified_at = human_approval.approved_at;
        let effective_at = ratified_at + self.change_policy.approval_delay;
        record.approval = Some(human_approval);
        record.state = ProposalState::Ratified {
            ratified_at,
            effective_at,
        };
        info!(%id, ?effective_at, "meta_governance: ratified, scheduled");
        Ok(())
    }

    /// Apply a ratified proposal if its delay has elapsed.
    ///
    /// Returns `Ok(true)` if the proposal was applied, `Ok(false)` if it is
    /// still waiting for the delay to elapse.
    pub fn apply_if_ready(&mut self, id: ProposalId, now: DateTime<Utc>) -> Result<bool, MetaError> {
        let record = self
            .history
            .get_mut(&id)
            .ok_or(MetaError::NotFound { id })?;
        let effective_at = match &record.state {
            ProposalState::Ratified { effective_at, .. } => *effective_at,
            _ => return Err(MetaError::NotRatified { id }),
        };
        if now < effective_at {
            return Ok(false);
        }
        record.state = ProposalState::Applied { applied_at: now };
        // TODO(v1): actually execute the change payload against the
        // governance kernel / risk weights / etc. v0 just records the
        // state transition.
        // TODO(v1): write an audit entry to the audit chain.
        record.audit_entry = Some(format!("applied proposal {} at {}", id, now.to_rfc3339()));
        info!(%id, "meta_governance: applied");
        Ok(true)
    }

    /// Revert a previously-applied proposal.
    pub fn revert(&mut self, id: ProposalId) -> Result<(), MetaError> {
        if !self.change_policy.require_reversibility {
            warn!("meta_governance: reversibility requirement disabled");
        }
        let record = self
            .history
            .get_mut(&id)
            .ok_or(MetaError::NotFound { id })?;
        if !matches!(record.state, ProposalState::Applied { .. }) {
            return Err(MetaError::NotApplied { id, state: format!("{:?}", record.state) });
        }
        record.state = ProposalState::Reverted {
            reverted_at: Utc::now(),
        };
        // TODO(v1): actually execute the revert procedure (recorded at apply
        // time) against the governance kernel.
        info!(%id, "meta_governance: reverted");
        Ok(())
    }

    /// Look up the state of a proposal.
    pub fn state(&self, id: ProposalId) -> Option<&ProposalState> {
        self.history.get(&id).map(|r| &r.state)
    }

    /// Number of proposals recorded.
    pub fn len(&self) -> usize {
        self.history.len()
    }

    /// Whether the engine has any proposals recorded.
    pub fn is_empty(&self) -> bool {
        self.history.is_empty()
    }
}

// ─── Errors ─────────────────────────────────────────────────────────────────────

/// Errors returned by the meta-governance engine.
#[derive(Debug, Error)]
pub enum MetaError {
    /// No proposal with the given id was found.
    #[error("proposal not found: {id}")]
    NotFound { id: ProposalId },
    /// The proposal is not in the Pending state (required for ratification).
    #[error("proposal {id} not pending (state: {state})")]
    NotPending { id: ProposalId, state: String },
    /// The proposal is not in the Ratified state (required for application).
    #[error("proposal {id} not ratified (state: {state})")]
    NotRatified { id: ProposalId },
    /// The proposal is not in the Applied state (required for revert).
    #[error("proposal {id} not applied (state: {state})")]
    NotApplied { id: ProposalId, state: String },
    /// The mandatory audit entry could not be written.
    #[error("audit entry write failed: {0}")]
    AuditWriteFailed(String),
}
