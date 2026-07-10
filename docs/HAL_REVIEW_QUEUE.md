# HAL Review Queue

Human maintainer review is required for every change under `hal/`.
See [CONTRIBUTING.md](CONTRIBUTING.md) and [HAL_AUDIT.md](HAL_AUDIT.md).

| Status | HAL_AUDIT entry | Files | Reviewed by |
|--------|-----------------|-------|-------------|
| **pending** | 2026-07-08 — IPC agent wiring + compile fixes | `hal/Cargo.toml`, `hal/src/main.rs` (agent bootstrap), `hal/src/autonomous_constitution.rs`, `hal/src/cognitive_equilibrium.rs`, `hal/src/meta_governance.rs`, `hal/src/confidence_engine.rs`, `hal/src/provenance.rs`, `hal/src/session_context.rs`, `hal/src/capability_lattice.rs`, `hal/src/risk_scorer.rs` (unused-param prefix only), `hal/src/action_validator.rs`, `hal/src/policy_engine.rs`, `hal/src/self_rewrite_monitor.rs` | *(awaiting iCrewZero)* |
| **pending** | 2026-07-08 — HalGate RPC handler | `hal/src/main.rs` (`PolicyHalGate`, `COGNOS_HAL_BIND` default `127.0.0.1:7444`) | *(awaiting iCrewZero)* |
| **pending** | 2026-07-09 — approval socket path overrides | `hal/src/approval_flow.rs` | *(awaiting iCrewZero)* |
| **pending** | 2026-07-09 — audit chain verification hardening | `hal/src/audit_log.rs` | *(awaiting iCrewZero)* |

**PR:** `feat/vllm-intent-manager` — intent manager vLLM integration; HAL changes are transport/wiring and audit integrity only (no risk formula, floor, threshold, trust-bound, or lattice-deny edits per HAL_AUDIT security review).
