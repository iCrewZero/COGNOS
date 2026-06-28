//! Policy compiler — translates high-level declarative policies (a small DSL) into executable rule sets that the governance kernel evaluates. Compilation is total: malformed policies fail to compile rather than fail at runtime.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, info, warn};
use uuid::Uuid;

// v0: stub implementation

// ─── Types & IDs ──────────────────────────────────────────────────────────────

/// Stable identifier for a compiled policy.
pub type PolicyId = Uuid;

/// 32-byte content hash of a policy source; used as the AST-cache key.
pub type PolicyHash = [u8; 32];

// ─── Policy Source ────────────────────────────────────────────────────────────

/// Where a policy comes from. The compiler treats every source uniformly:
/// by the time it reaches the verifier it has been reduced to bytecode.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PolicySource {
    /// Inline source text (e.g. supplied via a CLI flag or RPC body).
    Inline(String),
    /// Loaded from a file on disk.
    File(PathBuf),
    /// Already-compiled bytecode (e.g. cached on disk or shipped by a vendor).
    Compiled(Vec<u8>),
}

// ─── Effects ──────────────────────────────────────────────────────────────────

/// The decision a rule produces when its condition matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Effect {
    /// Allow the request to proceed.
    Allow,
    /// Deny the request outright.
    Deny,
    /// Escalate to the user / HAL for an explicit decision.
    Ask,
    /// Allow but emit a mandatory audit entry (no UX interruption).
    Log,
    /// Allow exactly once, then auto-expire.
    AllowOnce,
}

// ─── Expression DSL ───────────────────────────────────────────────────────────

/// A boolean expression over the request context.
///
/// The grammar is deliberately small so the compiler can statically verify
/// type-correctness and so the runtime evaluator is `O(rules × expr-size)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Expr {
    /// The request's capability set equals the given capability.
    CapEquals(String),
    /// The request's action is one of the listed actions.
    ActionIn(Vec<String>),
    /// The request's target path matches the given glob/regex pattern.
    PathMatches(String),
    /// The request's risk score is strictly greater than the threshold.
    RiskGt(f32),
    /// The request's trust score is strictly less than the threshold.
    TrustLt(f32),
    /// The request's wall-clock time falls in the named window
    /// (e.g. `"business-hours"`).
    TimeIn(String),
    /// Logical conjunction.
    And(Box<Expr>, Box<Expr>),
    /// Logical disjunction.
    Or(Box<Expr>, Box<Expr>),
    /// Logical negation.
    Not(Box<Expr>),
    /// Constant boolean literal.
    Literal(bool),
}

// ─── Rule & Policy AST ────────────────────────────────────────────────────────

/// A single rule: when `condition` holds, apply `effect`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    /// Boolean condition that gates the rule.
    pub condition: Expr,
    /// Effect to apply when the condition matches.
    pub effect: Effect,
    /// Rule priority — higher wins. Ties broken by source order.
    pub priority: i32,
}

/// A parsed-but-not-yet-compiled policy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Policy {
    /// Stable policy identifier.
    pub id: PolicyId,
    /// Monotonically increasing version.
    pub version: u32,
    /// Where the policy came from.
    pub source: PolicySource,
    /// Rules declared in the policy, in source order.
    pub rules: Vec<Rule>,
}

/// Internal parsed AST.
///
/// The compiler caches these by source hash so that re-compiling unchanged
/// policies is O(1). The cache is purely a performance optimisation;
/// correctness does not depend on cache hits.
#[derive(Debug, Clone)]
pub struct PolicyAst {
    /// Policy id parsed from the source header.
    pub id: PolicyId,
    /// Version parsed from the source header.
    pub version: u32,
    /// Parsed rules, in source order.
    pub rules: Vec<Rule>,
}

// ─── Bytecode ─────────────────────────────────────────────────────────────────

/// Stack-machine bytecode for a single rule.
///
/// Evaluation pushes operands onto a stack and ends with an `Effect`
/// instruction. `JumpIfFalse` implements short-circuit evaluation of `And`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RuleBytecode {
    /// Push the request's capability set onto the stack.
    LoadCap,
    /// Push the request's action onto the stack.
    LoadAction,
    /// Push the request's target path onto the stack.
    LoadPath,
    /// Push the request's risk score onto the stack.
    LoadRisk,
    /// Push the request's trust score onto the stack.
    LoadTrust,
    /// Push the request's wall-clock time onto the stack.
    LoadTime,
    /// Equality test (pops two operands, pushes bool).
    Eq,
    /// Set-membership test (pops two operands, pushes bool).
    In,
    /// Pattern-match test (pops two operands, pushes bool).
    Match,
    /// Numeric greater-than test (pops two operands, pushes bool).
    Gt,
    /// Numeric less-than test (pops two operands, pushes bool).
    Lt,
    /// Boolean AND (short-circuit via `JumpIfFalse`).
    And,
    /// Boolean OR.
    Or,
    /// Boolean NOT.
    Not,
    /// Pop the top of the stack; if false, jump to the given instruction.
    JumpIfFalse(u32),
    /// Emit the effect and end the rule.
    Effect(Effect),
}

/// The output of a successful compilation: bytecode per rule plus the hash
/// of the source the bytecode was derived from.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CompiledPolicy {
    /// Policy id (echoed from source).
    pub id: PolicyId,
    /// Bytecode for each rule, in evaluation order.
    pub bytecode: Vec<RuleBytecode>,
    /// Hash of the source text used to compile this policy.
    pub source_hash: PolicyHash,
}

// ─── Errors ───────────────────────────────────────────────────────────────────

/// Errors returned by the policy compiler.
///
/// Every variant is constructed with enough context to point the user at the
/// exact location in the source that caused the failure.
#[derive(Debug, Error)]
pub enum CompileError {
    /// The source could not be parsed.
    #[error("parse error: {0}")]
    ParseError(String),
    /// The source referenced an effect the compiler does not recognise.
    #[error("unknown effect")]
    UnknownEffect,
    /// The source referenced an undefined capability name.
    #[error("undefined capability")]
    UndefinedCap,
    /// The source had a type error (e.g. comparing a string to a number).
    #[error("type mismatch")]
    TypeMismatch,
    /// The rule graph contained a cycle (priority or override loop).
    #[error("cycle in rules")]
    CycleInRules,
}

// ─── Compiler ─────────────────────────────────────────────────────────────────

/// The policy compiler.
///
/// Holds an AST cache keyed by source hash so that re-compiling unchanged
/// policies is O(1). The cache is purely a performance optimisation;
/// correctness does not depend on cache hits.
#[derive(Debug, Default)]
pub struct PolicyCompiler {
    /// Cache of (source hash → parsed AST).
    ast_cache: HashMap<PolicyHash, PolicyAst>,
}

impl PolicyCompiler {
    /// Construct a new compiler with an empty AST cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Compile a policy from source text.
    ///
    /// Compilation is total: the only way for a malformed policy to surface
    /// at runtime is to be loaded directly from a `PolicySource::Compiled`
    /// blob, which bypasses the parser. Anything that goes through this
    /// method is guaranteed syntactically valid and type-correct.
    pub fn compile(&mut self, source: &str) -> Result<CompiledPolicy, CompileError> {
        // v0: stub implementation
        let hash = hash_source(source);
        if let Some(_cached) = self.ast_cache.get(&hash) {
            debug!(?hash, "policy ast cache hit");
            // TODO(v1): re-emit bytecode from the cached AST instead of
            // re-parsing.
        }
        let _ast = self.parse(source)?;
        self.verify_syntax(source)?;
        let mut compiled = CompiledPolicy {
            id: Uuid::nil(),
            bytecode: Vec::new(),
            source_hash: hash,
        };
        self.optimize(&mut compiled);
        info!(policy_id = ?compiled.id, "policy compiled");
        Ok(compiled)
    }

    /// Verify the syntax of a policy source without producing bytecode.
    ///
    /// Used by editors, linters and CI to fail fast on malformed policies.
    pub fn verify_syntax(&self, source: &str) -> Result<(), CompileError> {
        // v0: stub implementation
        let _ = source;
        // TODO(v1): real lexer + grammar check (reserved keywords, balanced
        // braces, well-formed predicates).
        Ok(())
    }

    /// Run peephole + dead-rule optimisations on a compiled policy.
    ///
    /// Optimisations are sound: they never change the observable decision
    /// for any request. In v0 this is a no-op.
    pub fn optimize(&self, policy: &mut CompiledPolicy) {
        // v0: stub implementation
        // TODO(v1): constant folding for `Literal`, dead-rule elimination,
        // short-circuit rewriting for nested `And`/`Or`, and common-
        // subexpression elimination across rules.
        let _ = policy;
        warn!("optimize() is a no-op in v0");
    }

    /// Borrow the AST cache (for inspection / debugging).
    pub fn ast_cache(&self) -> &HashMap<PolicyHash, PolicyAst> {
        &self.ast_cache
    }

    // ─── Recursive-descent parser (public functions only, bodies stubbed) ─────

    /// Top-level parser entry point.
    ///
    /// Tokenises `source` and dispatches to [`Self::parse_policy`]. Caches
    /// the resulting AST under its source hash.
    pub fn parse(&self, source: &str) -> Result<PolicyAst, CompileError> {
        // v0: stub implementation
        // TODO(v1): tokenise, then dispatch to parse_policy.
        let _ = source;
        Err(CompileError::ParseError(
            "v0 parser not implemented".to_string(),
        ))
    }

    /// Parse a single policy header + body.
    ///
    /// Grammar (v1 target):
    /// ```text
    /// policy <id> v<version> {
    ///     <rule>*
    /// }
    /// ```
    pub fn parse_policy(&self) -> Result<PolicyAst, CompileError> {
        // v0: stub implementation
        // TODO(v1): parse header, then loop parse_rule until `}`.
        Err(CompileError::ParseError(
            "v0 parser not implemented".to_string(),
        ))
    }

    /// Parse a single rule.
    ///
    /// Grammar (v1 target):
    /// ```text
    /// <priority> <effect> when <expr>
    /// ```
    pub fn parse_rule(&self) -> Result<Rule, CompileError> {
        // v0: stub implementation
        // TODO(v1): parse priority int, dispatch to parse_effect, then
        // parse_expr after the `when` keyword.
        Err(CompileError::ParseError(
            "v0 parser not implemented".to_string(),
        ))
    }

    /// Parse an expression.
    ///
    /// Implemented as a Pratt parser over the DSL grammar with the usual
    /// precedence: `Not` > `And` > `Or`.
    pub fn parse_expr(&self) -> Result<Expr, CompileError> {
        // v0: stub implementation
        // TODO(v1): Pratt parser over the DSL grammar.
        Err(CompileError::ParseError(
            "v0 parser not implemented".to_string(),
        ))
    }

    /// Parse an effect keyword.
    ///
    /// Maps the source keywords `allow`, `deny`, `ask`, `log`, `allow-once`
    /// onto the corresponding [`Effect`] variant.
    pub fn parse_effect(&self) -> Result<Effect, CompileError> {
        // v0: stub implementation
        // TODO(v1): keyword table.
        Err(CompileError::UnknownEffect)
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Compute a content hash of a policy source.
///
/// v0 uses a truncated byte-prefix; v1 will replace this with blake3 so the
/// hash is collision-resistant and suitable for content-addressed storage.
fn hash_source(source: &str) -> PolicyHash {
    let mut h = [0u8; 32];
    for (i, b) in source.bytes().take(32).enumerate() {
        h[i] = b;
    }
    h
}
