# HAL Audit Log

This document records every change made to the `hal/` directory.
HAL is the trust anchor of COGNOS/OS. Every change requires:
1. A human reviewer reading every modified line
2. An entry in this document before the PR merges
3. The GitHub issue created by the CI workflow to be closed

CI enforces: PRs touching `hal/` that do not update this file are blocked.

---

## Format

```
## YYYY-MM-DD — [Commit short SHA] — [Author]

**Files changed:** list of files

**What changed:** plain description of the modification

**Why it changed:** justification — what problem was this solving?

**Security review:** did this change affect any of:
  - Risk scoring formula or weights?
  - Hard floor rules?
  - Threshold values?
  - Trust calibration bounds?
  - Capability lattice deny rules?

**Reviewed by:** GitHub handle of human reviewer
**Audited:** YYYY-MM-DD
```

---

## 2025-03-14 — initial — cognos-maintainer

**Files changed:** All hal/src/ files (initial implementation)

**What changed:** Initial implementation of HAL v0 skeleton (approval_flow.rs)
and HAL v1 formal models (risk_scorer.rs, trust_calibration.rs, permissions.rs,
restraint_model.rs, audit_log.rs).

**Why it changed:** Phase 1 + Phase 3 foundation. No prior HAL existed.

**Security review:**
- Risk scoring formula: matches FORMAL_MODELS.md exactly
- Hard floors: delete ≥ 0.5, kernel ≥ 0.7, AI-unreviewed ≥ 0.8
- Thresholds: Silent/Notify/Confirm/Block boundaries match spec
- Trust calibration: KernelAdjacent and AiGeneratedCode floors cannot go below 0.6
- Capability lattice: ModifyHal is denied for all agents including coordinator

**Reviewed by:** iCrewZero
**Audited:** 2025-05-24

---

## 2026-07-08 — [pending] — cognos-maintainer

**Files changed:**
- `hal/Cargo.toml` — added `cognos-ipc-grpc` dependency; enabled tokio features
  (`rt-multi-thread`, `signal`, `time`, `macros`) required by the binary.
- `hal/src/main.rs` — IPC agent wiring (transport only).
- `hal/src/autonomous_constitution.rs`, `hal/src/cognitive_equilibrium.rs`,
  `hal/src/meta_governance.rs` — doc-comment syntax fix (stray `///` → `//!`).
- `hal/src/meta_governance.rs` — removed a dangling `{state}` reference in the
  `MetaError::NotRatified` `#[error(...)]` format string (that variant has no
  `state` field).
- `hal/src/confidence_engine.rs`, `hal/src/provenance.rs`,
  `hal/src/session_context.rs` — added explicit `f32` type annotations on local
  score accumulators to resolve ambiguous-numeric-type errors on `.clamp()`.
- `hal/src/capability_lattice.rs` — `.clone()` on a `Capability` moved into a
  `vec!` before being formatted (borrow-after-move fix).

**What changed:**
1. HAL now registers itself as an agent of the central IPC server and keeps a
   heartbeat alive on a background tokio task (via the shared
   `cognos_ipc_grpc::agent` bootstrap). The daemon's blocking gate loop still
   runs on the main thread (Unix only), unchanged.
2. Pre-existing compilation errors in the HAL library were fixed with the
   minimum mechanical edits needed to make the crate build (doc-comment syntax,
   a numeric type annotation, a clone, and a format-string reference). These
   were blocking `cargo check` before this change.

**Why it changed:** orchestrator/, scheduler/, and hal/ must each become an
agent registered on the IPC bus (capability registration + heartbeat). Making
HAL an agent requires the HAL crate to compile, which surfaced the pre-existing
errors above.

**Security review:** No effect on any of the following — all verified untouched:
- **Risk scoring formula or weights** — `risk_scorer.rs` / `risk_weights.rs`
  were NOT modified. No formula, coefficient, or weight changed.
- **Hard floor rules** — unchanged (delete ≥ 0.5, kernel ≥ 0.7,
  AI-unreviewed ≥ 0.8).
- **Threshold values** — no Silent/Notify/Confirm/Block boundary changed.
- **Trust calibration bounds** — `trust_calibration.rs` NOT modified.
- **Capability lattice deny rules** — the `capability_lattice.rs` edit is a
  `.clone()` on the *identity* escalation branch only; the `ModifyHal`
  forbidden-boundary check and all deny rules are byte-for-byte unchanged.

The `f32` annotations do not change any computed value: the accumulators were
already used as `f32` (the functions return `f32`); the annotation only removes
inference ambiguity. The doc-comment and error-string edits are non-executable
text. The capability strings HAL declares to the IPC registry
(`hal.gate`, `risk.score`, `audit.append`) are advisory identifiers; they do
not touch the capability lattice or its deny rules.

**Reviewed by:** (pending human review)
**Audited:** 2026-07-08

---

## 2026-07-08 — [pending] — cognos-maintainer

**Files changed:**
- `hal/src/main.rs` — added a `HalGate` RPC handler (`PolicyHalGate`) and start
  a `CognosServer` in the HAL binary so HAL serves its own gate decisions on
  `COGNOS_HAL_BIND` (default `127.0.0.1:7444`). **Binary only — no `hal/src`
  library file was modified.** No `Cargo.toml` change (deps already present).

**What changed:**
1. The HAL binary now serves the `HalGate` RPC. The handler is a pure transport
   *adapter*: it maps the wire `HalGateRequest` onto HAL's existing public
   policy inputs and maps the policy output back onto the wire `HalGateResponse`.
   Concretely it delegates to the **unmodified** `cognos_hal` library:
   - dangerous-path / destructive-pattern classification →
     `action_validator::ActionValidator::validate` (its committed dangerous-path
     list and destructive-pattern regexes are used verbatim; the handler only
     *reads* the `violated_rules` it returns);
   - the risk score and its band → `risk_scorer::score_action`.
2. The `HalGate` decision status is derived purely from HAL's own risk band:
   `Silent`/`Notify` → `granted`, `Confirm` → `approval_required` (or `denied`
   when the caller disabled the approval flow), `Block` → `denied`; a
   destructive-pattern violation → `denied`.

**Why it changed:** the orchestrator must gate every side-effecting action
through HAL before dispatch. HAL previously exposed no `HalGate` RPC (the central
IPC server answered with a conservative `approval_required` stub for *every*
request, which cannot distinguish a benign open from a dangerous delete). Per the
task, the handler is wired **in the HAL binary**, delegating to the existing
policy engine. `cognos-ipc-grpc` cannot depend on `cognos-hal` (that would be a
dependency cycle — HAL already depends on the IPC crate), so the IPC server
exposes a `HalGateHandler` trait that the HAL binary implements and injects.

**Security review:** No effect on any of the following — all verified untouched:
- **Risk scoring formula or weights** — `risk_scorer.rs` / `risk_weights.rs`
  were NOT modified. The handler *calls* `score_action`; it does not reimplement
  or alter the formula, any coefficient, or any weight.
- **Hard floor rules** — unchanged (delete ≥ 0.5, kernel ≥ 0.7,
  AI-unreviewed ≥ 0.8). The mapping relies on those floors but does not change
  them; a dangerous path is mapped to `ScopeLevel::KernelLevel`, letting HAL's
  own kernel floor set the band.
- **Threshold values** — the `RiskLevel → status` mapping consumes HAL's
  existing Silent/Notify/Confirm/Block boundaries; no boundary value changed.
- **Trust calibration bounds** — `trust_calibration.rs` NOT modified.
- **Capability lattice deny rules** — `capability_lattice.rs` NOT modified. The
  capability strings on the wire (`file.read`, `file.delete`, …) are passed
  through as advisory identifiers and are not consulted against the lattice.

The handler is deterministic and side-effect-free from the server's point of
view. The neutral `HALContext` it builds only carries `target_resource` and
`requested_action` (the fields `ActionValidator::validate` inspects for path /
pattern rules); every other field is set to a benign value so no *other*
validator rule is triggered. Verified end-to-end by
`orchestrator/tests/hal_gate_integration.rs`: a benign open is `granted`, a
delete under `/etc` is gated (`approval_required`).

**Reviewed by:** (pending human review)
**Audited:** 2026-07-08

---

## 2026-07-09 — [pending] — cognos-maintainer

**Files changed:**
- `hal/src/approval_flow.rs` — socket path overrides via environment variables
  (`COGNOS_HAL_SOCKET`, `COGNOS_HAL_UI_SOCKET`, `COGNOS_HAL_NOTIFICATIONS_SOCKET`).
  No scoring, policy, or `handle_connection` logic changed.

**What changed:**
Transport wiring only: HAL daemon socket locations are configurable for E2E/CI
without changing v0 risk rules. Documented optional `notice` field on the
`hal-ui.sock` JSON response (consumed by `cognos approval watch`; HAL v0 still
reads only `approved`).

**Why it changed:**
Enable real-time blocking approval (`cognos approval watch`) and orchestrator
resume on `approval_required` in isolated test sockets under `/tmp`.

**Security review:**
- Risk scoring formula or weights — **NOT modified**
- Hard floor rules — **NOT modified**
- Threshold values — **NOT modified**
- Trust calibration bounds — **NOT modified**
- Capability lattice deny rules — **NOT modified**

**Reviewed by:** (pending human review)
**Audited:** 2026-07-09

---

## 2026-07-09 — [pending] — cognos-maintainer

**Files changed:**
- `hal/src/audit_log.rs` — hardened JSONL audit-chain verification.

**What changed:**
`AuditLog::verify()` now treats malformed JSON lines as tampering instead of
silently skipping them, and it compares the recomputed final chain head against
the persisted chain-tip file. This closes the truncation/deletion gap where a
shortened log could previously still verify if the remaining prefix was
internally consistent.

**Why it changed:**
The audit log's immutability claim is a core security argument. It must detect
not only content edits in the middle of the log, but also truncation,
reordering, deletion, and on-disk corruption. External security tests now rely
on this verifier to flag those negative cases with a broken index.

**Security review:**
- Risk scoring formula or weights — **NOT modified**
- Hard floor rules — **NOT modified**
- Threshold values — **NOT modified**
- Trust calibration bounds — **NOT modified**
- Capability lattice deny rules — **NOT modified**

**Reviewed by:** (pending human review)
**Audited:** 2026-07-09