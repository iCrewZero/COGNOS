//! Recovery kernel — brings the system to a last-known-safe HAL state.
//!
//!
//! When HAL detects system compromise (tamper, audit-chain break, lockdown
//! trigger), it must be able to roll the *governed* state of the system back
//! to a point where HAL's invariants held. The [`RecoveryKernel`] maintains a
//! ordered list of [`RecoveryPoint`]s, each capturing a hash of the relevant
//! HAL-protected state at checkpoint time, plus a human-readable description.
//!
//! Restoring a recovery point is **not** a full filesystem snapshot — it only
//! rewinds HAL's own governed state (policies, trust tables, autonomy level,
//! audit head). Filesystem and process rollback are the OS's job; HAL just
//! needs to be internally consistent.
//!
//! v0: stub implementation. State hashing is a placeholder (uses description
//! string + timestamp); real content-addressed hashing of governed state is
//! TODO(v1).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tracing::{info, warn};
use uuid::Uuid;

// v0: stub implementation

/// Type alias for an agent identifier (matches the rest of the crate).
pub type AgentId = String;

/// Identifier of a recovery point. Stable across reboots (UUID v4).
pub type RecoveryPointId = Uuid;

// ─── Recovery Point ─────────────────────────────────────────────────────────────

/// A snapshot of HAL's governed state at a moment in time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryPoint {
    /// Unique id of this recovery point.
    pub id: RecoveryPointId,
    /// When the point was captured.
    pub timestamp: DateTime<Utc>,
    /// SHA-256 hash of the governed-state blob at capture time.
    pub hash: [u8; 32],
    /// Human-readable description of what HAL state this captures.
    pub description: String,
}

impl RecoveryPoint {
    /// Compute a fresh recovery point from a description and a state blob.
    ///
    /// The state blob in v0 is the description bytes; v1 will replace this
    /// with a canonical serialization of the full governed state (policies,
    /// trust table, autonomy level, audit head).
    pub fn new(description: impl Into<String>, state_blob: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(state_blob);
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&hasher.finalize());
        Self {
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            hash,
            description: description.into(),
        }
    }
}

// ─── Recovery State ─────────────────────────────────────────────────────────────

/// The current governed state of HAL. v0 stub.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RecoveryState {
    /// Human-readable label for the current state.
    pub label: String,
    /// Free-form blob representing the governed state.
    pub blob: Vec<u8>,
}

// ─── Rollback Policy ────────────────────────────────────────────────────────────

/// Policy governing when and how rollback is permitted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RollbackPolicy {
    /// Rollback is permitted without restriction (dev/test only).
    Unrestricted,
    /// Rollback requires an explicit human approval token. v0 default.
    RequiresHumanApproval,
    /// Rollback is forbidden; the system must be rebuilt from a fresh
    /// installation. Used in catastrophic-tamper scenarios.
    Forbidden,
}

impl Default for RollbackPolicy {
    fn default() -> Self {
        Self::RequiresHumanApproval
    }
}

// ─── Recovery Kernel ────────────────────────────────────────────────────────────

/// The recovery kernel: maintains recovery points and mediates rollback.
pub struct RecoveryKernel {
    /// Ordered list of recovery points (oldest first).
    pub recovery_points: Vec<RecoveryPoint>,
    /// Current governed state.
    pub current_state: RecoveryState,
    /// Active rollback policy.
    rollback_policy: RollbackPolicy,
    /// Maximum number of recovery points to retain.
    max_points: usize,
}

impl RecoveryKernel {
    /// Build a new recovery kernel with the default policy and capacity.
    pub fn new() -> Self {
        Self {
            recovery_points: Vec::new(),
            current_state: RecoveryState::default(),
            rollback_policy: RollbackPolicy::default(),
            max_points: 32,
        }
    }

    /// Capture a recovery point at the current state.
    ///
    /// Returns the id of the new recovery point. If retention is exceeded,
    /// the oldest recovery point is dropped.
    pub fn checkpoint(&mut self) -> RecoveryPointId {
        let description = format!(
            "checkpoint @ {} (label={})",
            Utc::now().to_rfc3339(),
            self.current_state.label
        );
        let point = RecoveryPoint::new(description, &self.current_state.blob);
        let id = point.id;
        info!(%id, "recovery_kernel: checkpoint captured");
        self.recovery_points.push(point);
        while self.recovery_points.len() > self.max_points {
            let dropped = self.recovery_points.remove(0);
            warn!(%dropped.id, "recovery_kernel: dropped oldest checkpoint");
        }
        id
    }

    /// Restore HAL governed state to the recovery point with the given id.
    ///
    /// v0 does not actually mutate `current_state`; it just verifies the
    /// point exists. v1 will load the corresponding state blob from disk and
    /// atomically swap it in.
    pub fn restore(&mut self, id: RecoveryPointId) -> Result<(), RecoveryError> {
        if matches!(self.rollback_policy, RollbackPolicy::Forbidden) {
            return Err(RecoveryError::RollbackForbidden);
        }
        let point = self
            .recovery_points
            .iter()
            .find(|p| p.id == id)
            .ok_or(RecoveryError::NotFound { id })?;
        info!(%id, hash = ?point.hash, "recovery_kernel: restoring checkpoint");
        // TODO(v1): actually load and apply the state blob. v0 just logs.
        Ok(())
    }

    /// The active rollback policy.
    pub fn rollback_policy(&self) -> RollbackPolicy {
        self.rollback_policy.clone()
    }

    /// Set the rollback policy.
    pub fn set_rollback_policy(&mut self, policy: RollbackPolicy) {
        self.rollback_policy = policy;
    }

    /// Look up a recovery point by id.
    pub fn point(&self, id: RecoveryPointId) -> Option<&RecoveryPoint> {
        self.recovery_points.iter().find(|p| p.id == id)
    }

    /// Number of recovery points currently retained.
    pub fn len(&self) -> usize {
        self.recovery_points.len()
    }

    /// Whether the kernel holds any recovery points.
    pub fn is_empty(&self) -> bool {
        self.recovery_points.is_empty()
    }
}

impl Default for RecoveryKernel {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Errors ─────────────────────────────────────────────────────────────────────

/// Errors returned by the recovery kernel.
#[derive(Debug, Error)]
pub enum RecoveryError {
    /// No recovery point with the given id was found.
    #[error("recovery point not found: {id}")]
    NotFound { id: RecoveryPointId },
    /// The rollback policy forbids restoration.
    #[error("rollback is forbidden by current policy")]
    RollbackForbidden,
    /// The recovery-point state blob could not be loaded or verified.
    #[error("state blob verification failed: {0}")]
    VerificationFailed(String),
    /// A human-approval token was required but not supplied.
    #[error("human approval required for rollback")]
    HumanApprovalRequired,
}
