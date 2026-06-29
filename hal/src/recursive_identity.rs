//! Recursive identity — maintains stable AI identity across recursive self-improvement.
//!
//!
//! A recursively self-improving system can, in principle, produce descendants
//! that diverge arbitrarily from their parent. Without an identity anchor,
//! there is no way to tell whether a "new" agent is the same agent the user
//! originally trusted, or a fork that has drifted into a different (possibly
//! hostile) policy regime.
//!
//! The [`RecursiveIdentity`] module assigns each agent an *identity hash*
//! computed as `SHA-256(constitution || capabilities || audit_head)`. The hash
//! changes if and only if the agent's constitution, capability set, or audit
//! history changes. Forking the agent (e.g. for a parallel exploration)
//! produces a new identity with the parent's hash recorded in its *lineage*,
//! so the full ancestry of any agent can be inspected and audited.
//!
//! v0: stub implementation. The actual computation of the identity hash
//! assumes the caller provides canonical serializations; v1 will define a
//! proper canonical-form serializer.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tracing::{info, warn};

// v0: stub implementation

/// Type alias for an agent identifier (matches the rest of the crate).
pub type AgentId = String;

/// A 32-byte SHA-256 identity hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IdentityHash(pub [u8; 32]);

impl IdentityHash {
    /// The all-zero identity hash, used as a sentinel for "uninitialized".
    pub fn zero() -> Self {
        Self([0u8; 32])
    }

    /// Compute an identity hash from the three identity-defining blobs.
    ///
    /// The hash is `SHA-256(constitution || capabilities || audit_head)`.
    /// Any change to any of the three inputs produces a different hash.
    pub fn compute(
        constitution_blob: &[u8],
        capabilities_blob: &[u8],
        audit_head: &[u8; 32],
    ) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(b"cognos-identity-v1\x00");
        hasher.update(constitution_blob);
        hasher.update(b"\x00");
        hasher.update(capabilities_blob);
        hasher.update(b"\x00");
        hasher.update(audit_head);
        let mut out = [0u8; 32];
        out.copy_from_slice(&hasher.finalize());
        Self(out)
    }
}

impl std::fmt::Display for IdentityHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for byte in &self.0 {
            write!(f, "{:02x}", byte)?;
        }
        Ok(())
    }
}

// ─── Recursive Identity ─────────────────────────────────────────────────────────

/// The identity anchor for an agent, plus its full lineage.
pub struct RecursiveIdentity {
    /// This agent's identity hash.
    pub identity_root: IdentityHash,
    /// The chain of ancestor identity hashes, oldest first. The immediate
    /// parent is the last element (if any).
    pub lineage: Vec<IdentityHash>,
    /// When this identity was established (i.e. when the root hash was
    /// last computed).
    pub established_at: DateTime<Utc>,
}

impl RecursiveIdentity {
    /// Construct a new root identity (no ancestors).
    pub fn root(identity_root: IdentityHash) -> Self {
        Self {
            identity_root,
            lineage: Vec::new(),
            established_at: Utc::now(),
        }
    }

    /// Verify that a candidate identity hash matches this agent's identity.
    ///
    /// Returns `Ok(())` if the candidate equals `identity_root`, or
    /// [`IdentityError::Mismatch`] otherwise.
    pub fn verify_identity(&self, candidate: IdentityHash) -> Result<(), IdentityError> {
        if candidate == self.identity_root {
            Ok(())
        } else {
            warn!(
                expected = %self.identity_root,
                actual = %candidate,
                "recursive_identity: MISMATCH"
            );
            Err(IdentityError::Mismatch {
                expected: self.identity_root,
                actual: candidate,
            })
        }
    }

    /// Fork this identity, producing a new identity whose lineage is this
    /// identity's lineage plus this identity's root.
    ///
    /// The forked identity's root is the provided hash (computed by the
    /// caller from the fork's new constitution/capabilities/audit state).
    pub fn fork(&mut self) -> RecursiveIdentity {
        let mut child_lineage = self.lineage.clone();
        child_lineage.push(self.identity_root);
        info!(
            root = %self.identity_root,
            lineage_depth = child_lineage.len(),
            "recursive_identity: forked"
        );
        // The forked identity's root is left as the parent's root in v0; the
        // caller is expected to recompute it from the fork's new state and
        // call `set_root` (TODO(v1): make this explicit).
        RecursiveIdentity {
            identity_root: self.identity_root,
            lineage: child_lineage,
            established_at: Utc::now(),
        }
    }

    /// Replace this identity's root with a freshly-computed hash. Used after
    /// `fork()` to install the fork's new identity.
    pub fn set_root(&mut self, new_root: IdentityHash) {
        if new_root != self.identity_root {
            info!(
                old = %self.identity_root,
                new = %new_root,
                "recursive_identity: root updated"
            );
            self.identity_root = new_root;
            self.established_at = Utc::now();
        }
    }

    /// The depth of this identity's lineage. A root identity has depth 0;
    /// its first fork has depth 1; etc.
    pub fn lineage_depth(&self) -> usize {
        self.lineage.len()
    }

    /// The immediate parent of this identity, if any.
    pub fn parent(&self) -> Option<IdentityHash> {
        self.lineage.last().copied()
    }

    /// Whether this identity is descended from the given ancestor.
    pub fn is_descended_from(&self, ancestor: IdentityHash) -> bool {
        self.lineage.contains(&ancestor) || self.identity_root == ancestor
    }
}

// ─── Errors ─────────────────────────────────────────────────────────────────────

/// Errors returned by the recursive identity module.
#[derive(Debug, Error)]
pub enum IdentityError {
    /// The candidate identity hash did not match the expected one.
    #[error(
        "identity mismatch: expected {expected}, got {actual}"
    )]
    Mismatch {
        /// The expected identity hash.
        expected: IdentityHash,
        /// The candidate identity hash.
        actual: IdentityHash,
    },
    /// The lineage chain is too deep (exceeds the configured maximum).
    #[error("lineage too deep: {depth} >= max {max}")]
    LineageTooDeep {
        /// Current lineage depth.
        depth: usize,
        /// Configured maximum.
        max: usize,
    },
}
