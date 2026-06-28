//! Capability lattice (escalation paths) — models the partial order over
//!
//!
//! capabilities and the explicit escalation paths between them.
//!
//! This module is *different* from [`crate::permissions`]: that module
//! answers "is agent X allowed to use capability Y right now?" (a flat
//! allow-set per agent). This module answers "if an agent has capability
//! A, what is the explicit chain of approvals required to reach capability
//! B?" — the *escalation graph*.
//!
//! The lattice is a partial order: not every pair of capabilities is
//! comparable. [`CapabilityLattice::implies`] decides comparability;
//! [`CapabilityLattice::escalate`] returns the explicit path of
//! intermediate capabilities, each of which may or may not require HAL
//! approval.
//!
//! v0: stub implementation. The lattice edges are a TODO(v1): the v0
//! surface only exposes the API and a trivial identity-escalation.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::permissions::Capability;

// v0: stub implementation

// ─── Lattice Nodes ──────────────────────────────────────────────────────────────

/// A node in the capability lattice.
///
/// A leaf node is a single [`Capability`]. A composite node is the
/// conjunction of two nodes (both must be held to "have" the composite).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LatticeNode {
    /// A leaf capability.
    Capability(Capability),
    /// Conjunction of two sub-nodes.
    Composite(Box<LatticeNode>, Box<LatticeNode>),
}

impl LatticeNode {
    /// Flatten this node into the set of capabilities it implies.
    pub fn flatten(&self) -> Vec<Capability> {
        // TODO(v1): implement properly with HashSet for dedup.
        match self {
            Self::Capability(c) => vec![c.clone()],
            Self::Composite(a, b) => {
                let mut out = a.flatten();
                out.extend(b.flatten());
                out
            }
        }
    }
}

// ─── Escalation Path ────────────────────────────────────────────────────────────

/// An explicit chain of capabilities from `from` to `to`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EscalationPath {
    /// Ordered list of capabilities, starting with `from` and ending with `to`.
    pub steps: Vec<Capability>,
    /// Whether HAL approval is required at any step.
    pub requires_approval: bool,
    /// Human-readable summary, suitable for an audit-log entry.
    pub summary: String,
}

// ─── Lattice Errors ─────────────────────────────────────────────────────────────

/// Errors returned by lattice operations.
#[derive(Debug, Error)]
pub enum LatticeError {
    /// The two capabilities are not comparable in the lattice.
    #[error("capabilities are not comparable: {from:?} does not imply {to:?}")]
    NotComparable {
        /// The capability held.
        from: Capability,
        /// The capability wanted.
        to: Capability,
    },
    /// The escalation path would require crossing a forbidden boundary
    /// (e.g. into `ModifyHal`, which is never grantable to an agent).
    #[error("escalation path crosses forbidden boundary at {0:?}")]
    ForbiddenBoundary(Capability),
    /// The requested escalation is not in the pre-approved path table.
    #[error("no approved escalation path from {from:?} to {to:?}")]
    NoApprovedPath {
        /// The capability held.
        from: Capability,
        /// The capability wanted.
        to: Capability,
    },
}

// ─── Capability Lattice ─────────────────────────────────────────────────────────

/// The escalation-path capability lattice.
///
/// Owns the set of nodes and the directed edges (capability → capability)
/// that represent approved escalation steps. Each edge carries a flag
/// indicating whether HAL approval is required to traverse it.
#[derive(Debug, Default)]
pub struct CapabilityLattice {
    nodes: Vec<LatticeNode>,
    /// Adjacency map: from-capability → list of (to-capability, requires_approval).
    edges: HashMap<Capability, Vec<(Capability, bool)>>,
}

impl CapabilityLattice {
    /// Construct an empty lattice. Production code should call [`Self::with_defaults`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Construct the lattice with the v0 default edge set.
    ///
    /// v0 ships only identity edges (every capability implies itself).
    /// TODO(v1): add the full escalation DAG from the spec.
    pub fn with_defaults() -> Self {
        // Every capability implies itself (reflexivity). We do not need to
        // materialize these as edges — implies() returns true trivially
        // for the identity case.
        Self::new()
    }

    /// Decide whether having `have` implies `want` (reflexive + transitive).
    pub fn implies(&self, have: Capability, want: Capability) -> bool {
        // Identity is always implied.
        if have == want {
            return true;
        }
        // TODO(v1): implement BFS over the edge set.
        false
    }

    /// Compute the explicit escalation path from `from` to `to`.
    ///
    /// Returns an error if no path exists or the path crosses a forbidden
    /// boundary (e.g. one that would grant `ModifyHal`).
    pub fn escalate(
        &self,
        from: Capability,
        to: Capability,
    ) -> Result<EscalationPath, LatticeError> {
        // Forbidden boundary: any escalation targeting ModifyHal is rejected.
        if to == Capability::ModifyHal || from == Capability::ModifyHal {
            return Err(LatticeError::ForbiddenBoundary(Capability::ModifyHal));
        }
        // Identity case.
        if from == to {
            return Ok(EscalationPath {
                steps: vec![from],
                requires_approval: false,
                summary: format!("identity escalation: {:?}", from),
            });
        }
        // TODO(v1): BFS over the edge set with a visited set.
        Err(LatticeError::NoApprovedPath { from, to })
    }

    /// Install a new escalation edge in the lattice. Intended for use by
    /// the policy DSL at startup; not exposed to agents at runtime.
    pub fn add_edge(&mut self, from: Capability, to: Capability, requires_approval: bool) {
        self.edges
            .entry(from)
            .or_default()
            .push((to, requires_approval));
    }

    /// Borrow the edge table (for inspection / audit).
    pub fn edges(&self) -> &HashMap<Capability, Vec<(Capability, bool)>> {
        &self.edges
    }

    /// Borrow the node list (for inspection / audit).
    pub fn nodes(&self) -> &[LatticeNode] {
        &self.nodes
    }
}
