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