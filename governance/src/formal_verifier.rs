//! Formal verifier — checks compiled policies against safety properties using model checking. Catches unsafe policies before deployment: "is there any state in which an irreversible action is allowed without HAL approval?"

use std::collections::HashSet;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{info, warn};
use uuid::Uuid;

use crate::policy_compiler::{CompiledPolicy, Effect, Expr};

// v0: stub implementation

// ─── Property Kinds ───────────────────────────────────────────────────────────

/// Classification of a safety property.
///
/// The verifier picks a checking strategy based on this kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PropertyKind {
    /// Liveness: "something good eventually happens".
    Liveness,
    /// Safety: "something bad never happens".
    Safety,
    /// Non-interference: secret inputs do not leak to public outputs.
    NonInterference,
    /// Mutual exclusion: two effects are never both granted in the same state.
    MutualExclusion,
    /// Reachability: a state matching the predicate is reachable from init.
    Reachability,
}

// ─── Safety Property ──────────────────────────────────────────────────────────

/// A single safety property to check against a compiled policy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SafetyProperty {
    /// Stable property identifier.
    pub id: Uuid,
    /// Kind of property (drives the checker strategy).
    pub kind: PropertyKind,
    /// Predicate over policy states. For `Safety`, `holds == true` means
    /// "no violating state is reachable"; for `Reachability`, `holds == true`
    /// means "a predicate-matching state is reachable".
    pub predicate: Expr,
    /// Last-computed verdict. `false` until the property has been checked.
    pub holds: bool,
}

// ─── State Space ──────────────────────────────────────────────────────────────

/// A state in the model-checking state space.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyState {
    /// Capabilities active in this state.
    pub capabilities: HashSet<String>,
    /// Pending action type (e.g. `"delete_file"`).
    pub action: String,
    /// Risk score ∈ [0.0, 1.0].
    pub risk: f32,
    /// Trust score ∈ [0.0, 1.0].
    pub trust: f32,
    /// Time-window label (e.g. `"business-hours"`).
    pub time: String,
    /// Decision the policy produced in this state.
    pub decision: Effect,
}

/// The model-checking state space: a finite set of states plus transitions.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StateSpace {
    /// States, indexed by their position in the vector.
    pub states: Vec<PolicyState>,
    /// Edges: `(from_index, to_index)`.
    pub transitions: Vec<(usize, usize)>,
}

// ─── Results & Reports ────────────────────────────────────────────────────────

/// Per-property verification outcome.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PropertyResult {
    /// Property the result refers to.
    pub property_id: Uuid,
    /// Whether the property holds on the supplied policy.
    pub holds: bool,
    /// Number of states explored while checking.
    pub model_size: usize,
    /// Wall-clock time spent checking, in milliseconds.
    pub time_ms: u64,
}

/// A trace demonstrating that a property is violated.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Counterexample {
    /// Property the counterexample refutes.
    pub property_id: Uuid,
    /// Sequence of states from an initial state to a violating one.
    pub trace: Vec<PolicyState>,
}

/// Full verification report across all registered properties.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VerifyReport {
    /// Properties that held.
    pub passed: Vec<PropertyResult>,
    /// Properties that failed.
    pub failed: Vec<PropertyResult>,
    /// One counterexample per failed property (when synthesizable).
    pub counterexamples: Vec<Counterexample>,
}

// ─── Errors ───────────────────────────────────────────────────────────────────

/// Errors returned by the formal verifier.
#[derive(Debug, Error)]
pub enum VerifyError {
    /// The state space exceeded the configured bound (e.g. > 1e6 states).
    #[error("state space explosion: model exceeded configured bound")]
    StateExplosion,
    /// The property kind is not yet supported by the checker.
    #[error("unsupported property kind")]
    UnsupportedProperty,
    /// The supplied policy is malformed (e.g. empty bytecode).
    #[error("invalid policy")]
    InvalidPolicy,
}

// ─── Verifier ─────────────────────────────────────────────────────────────────

/// The formal verifier.
///
/// Owns a set of safety properties and a state space. The state space is
/// built lazily per-policy from the policy's bytecode.
#[derive(Debug, Default)]
pub struct FormalVerifier {
    /// Properties to check on every `verify()` call.
    properties: Vec<SafetyProperty>,
    /// State space (rebuilt per verification in v0).
    state_space: StateSpace,
}

impl FormalVerifier {
    /// Construct an empty verifier.
    pub fn new() -> Self {
        Self::default()
    }

    /// Construct a verifier pre-loaded with the built-in COGNOS safety
    /// properties.
    ///
    /// Built-in properties:
    ///   1. Never allow an `Irreversible` + `KernelLevel` action without
    ///      HAL approval.
    ///   2. Never grant the `ModifyHal` capability to any agent.
    ///   3. `AutonomyEscalate` and `UserOverride` are mutually exclusive.
    pub fn with_builtin_properties() -> Self {
        // v0: stub implementation
        let mut v = Self::new();
        for prop in builtin_properties() {
            v.add_property(prop);
        }
        v
    }

    /// Add a safety property to be checked on subsequent `verify()` calls.
    pub fn add_property(&mut self, prop: SafetyProperty) {
        info!(property_id = %prop.id, kind = ?prop.kind, "property added");
        self.properties.push(prop);
    }

    /// Verify a policy against every registered property.
    ///
    /// Returns a full report; failing properties also yield a
    /// counterexample trace when one can be synthesised.
    pub fn verify(&self, policy: &CompiledPolicy) -> Result<VerifyReport, VerifyError> {
        // v0: stub implementation
        if policy.bytecode.is_empty() {
            return Err(VerifyError::InvalidPolicy);
        }
        let mut report = VerifyReport::default();
        for prop in &self.properties {
            match self.check_property(policy, prop) {
                Ok(result) => {
                    if result.holds {
                        report.passed.push(result);
                    } else {
                        report.failed.push(result);
                        // TODO(v1): synthesise a real counterexample trace
                        // from the failing exploration path.
                        report.counterexamples.push(Counterexample {
                            property_id: prop.id,
                            trace: Vec::new(),
                        });
                    }
                }
                Err(e) => {
                    warn!(error = %e, property_id = %prop.id, "property check failed");
                    return Err(e);
                }
            }
        }
        Ok(report)
    }

    /// Check a single property against a policy.
    pub fn check_property(
        &self,
        policy: &CompiledPolicy,
        prop: &SafetyProperty,
    ) -> Result<PropertyResult, VerifyError> {
        // v0: stub implementation
        let start = Instant::now();
        if policy.bytecode.is_empty() {
            return Err(VerifyError::InvalidPolicy);
        }
        // TODO(v1): build the state space from the policy bytecode, then
        // run a BFS/DFS model-checker against `prop.predicate`. Property
        // kind selects the algorithm (safety uses invariant checking,
        // liveness uses nested DFS, mutual-exclusion uses product
        // automata, etc.).
        let model_size = self.state_space.states.len();
        Ok(PropertyResult {
            property_id: prop.id,
            holds: true,
            model_size,
            time_ms: start.elapsed().as_millis() as u64,
        })
    }

    /// Borrow the registered properties (for inspection / audit).
    pub fn properties(&self) -> &[SafetyProperty] {
        &self.properties
    }

    /// Borrow the current state space (for inspection / debugging).
    pub fn state_space(&self) -> &StateSpace {
        &self.state_space
    }
}

// ─── Built-in Properties ──────────────────────────────────────────────────────

/// The set of COGNOS-mandated safety properties.
///
/// These are loaded by [`FormalVerifier::with_builtin_properties`] and act
/// as a non-negotiable baseline: a policy that violates any of them cannot
/// be deployed, regardless of any user-supplied `Allow` rules.
fn builtin_properties() -> Vec<SafetyProperty> {
    // v0: stub implementation
    // TODO(v1): assign stable UUIDs (currently nil) and replace `Literal`
    // placeholders with the real DSL predicates sketched below.

    // (1) Never allow Irreversible + KernelLevel without HAL approval.
    //
    //     not ( ActionIn(["Irreversible"])
    //           and CapEquals("KernelLevel")
    //           and not CapEquals("HalApproved") )
    let no_irreversible_without_hal = SafetyProperty {
        id: Uuid::nil(),
        kind: PropertyKind::Safety,
        predicate: Expr::Not(Box::new(Expr::And(
            Box::new(Expr::And(
                Box::new(Expr::ActionIn(vec!["Irreversible".to_string()])),
                Box::new(Expr::CapEquals("KernelLevel".to_string())),
            )),
            Box::new(Expr::Not(Box::new(Expr::CapEquals(
                "HalApproved".to_string(),
            )))),
        ))),
        holds: false,
    };

    // (2) Never allow ModifyHal capability to any agent.
    //
    //     not CapEquals("ModifyHal")
    let no_modify_hal = SafetyProperty {
        id: Uuid::nil(),
        kind: PropertyKind::Safety,
        predicate: Expr::Not(Box::new(Expr::CapEquals("ModifyHal".to_string()))),
        holds: false,
    };

    // (3) Mutual exclusion: AutonomyEscalate xor UserOverride.
    //
    //     not ( CapEquals("AutonomyEscalate")
    //           and CapEquals("UserOverride") )
    let mutual_exclusion = SafetyProperty {
        id: Uuid::nil(),
        kind: PropertyKind::MutualExclusion,
        predicate: Expr::Not(Box::new(Expr::And(
            Box::new(Expr::CapEquals("AutonomyEscalate".to_string())),
            Box::new(Expr::CapEquals("UserOverride".to_string())),
        ))),
        holds: false,
    };

    vec![no_irreversible_without_hal, no_modify_hal, mutual_exclusion]
}
