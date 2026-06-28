# COGNOS Formal Models

This document specifies the formal models underlying COGNOS/OS. Every formula here is implemented in Rust and verified by the formal verifier (see `governance/src/formal_verifier.rs`).

These models are the mathematical core of HAL — the Human Authority Layer. They define:

- How risk is scored for every candidate action.
- How trust is calibrated per-user over time.
- How autonomy is graduated and escalated.
- How capabilities form a lattice, not a set of booleans.
- How the audit log is tamper-evident.
- How reputation decays, predictions are gated, and constitutional
  invariants are preserved.

Each section maps a formula to its implementation file. Where a full
mechanical proof is not yet available, the section is marked `TODO`.

---

## 1. Risk Model

**Implementation:** `hal/src/risk_scorer.rs`, `hal/src/risk_weights.rs`

Every candidate action `A` proposed by an agent is scored before HAL
decides whether to allow, ask, or block it. The score `R(A) ∈ [0, 1]`
combines seven components:

```
R(A) = w₁·Irreversibility(A)
     + w₂·Scope(A)
     + w₃·TrustContext(A)
     + w₄·TimeAnomaly(A)
     + w₅·VibeFlag(A)
     − w₆·UserHistory(A)
     − w₇·PatternMatch(A)
```

The last two terms are **subtracted** — familiarity and recognized-good
patterns reduce risk.

### Weights

| Weight | Component        | Value | Sign | Notes                                  |
|--------|------------------|-------|------|----------------------------------------|
| w₁     | Irreversibility  | 0.25  | +    | Cannot be undone without restore.      |
| w₂     | Scope            | 0.20  | +    | Blast radius (files, processes, net).  |
| w₃     | TrustContext     | 0.20  | +    | Authenticated user, session strength.  |
| w₄     | TimeAnomaly      | 0.10  | +    | Off-hours, burst, jitter.              |
| w₅     | VibeFlag         | 0.10  | +    | Heuristic unease signal.               |
| w₆     | UserHistory      | 0.10  | −    | User has done this before safely.      |
| w₇     | PatternMatch     | 0.05  | −    | Matches a known-good pattern.          |
|        | **Σ weights**    | 1.00  |      | Weights sum to 1.0.                    |

Weights are loaded from `hal/src/risk_weights.rs` at compile time; runtime
overrides are denied (HAL is immutable to AI — see §9).

### Component score ranges

Each component is normalized to `[0, 1]`:

- `Irreversibility(A) ∈ [0, 1]` — 0 = fully reversible (e.g., create
  temp file), 1 = irreversible (e.g., `rm -rf /`, kernel module load).
- `Scope(A) ∈ [0, 1]` — 0 = single file in user dir, 1 = system-wide.
- `TrustContext(A) ∈ [0, 1]` — 0 = strong authenticated session,
  1 = unauthenticated / degraded session.
- `TimeAnomaly(A) ∈ [0, 1]` — 0 = normal business hours, 1 = 03:00
  burst with no precedent.
- `VibeFlag(A) ∈ [0, 1]` — heuristic blend of `behavioral_model.rs`
  outputs.
- `UserHistory(A) ∈ [0, 1]` — 0 = novel action, 1 = repeated-safe.
- `PatternMatch(A) ∈ [0, 1]` — 0 = no match, 1 = exact known-good.

### Hard floors

These override the weighted sum — they are non-negotiable:

```
R(Irreversible ∧ KernelLevel) = 1.0     # Always block
R(Delete)                  ≥ 0.5        # At minimum, ask
R(ModifyHal)               = 1.0        # Always block (constitutional)
R(NetworkBind + RootCreds) = 1.0        # Always block
```

### Decision thresholds

| Range                  | Decision          | HAL action                                  |
|------------------------|-------------------|---------------------------------------------|
| `R < 0.2`              | Allow             | Execute silently, log to audit.             |
| `0.2 ≤ R < 0.5`        | AllowWithNotice   | Execute, surface toast / log line.          |
| `0.5 ≤ R < 0.8`        | Ask               | Pause, request user approval with reason.   |
| `R ≥ 0.8`              | Block             | Refuse, log attempt, alert.                 |

The thresholds are tunable per-user via the Trust Calibration model
(§2), but the hard floors and the `Block` threshold are not.

---

## 2. Trust Calibration Model

**Implementation:** `hal/src/trust_calibration.rs`, `hal/src/temporal_trust.rs`

HAL should interrupt the user neither too often (annoying) nor too
rarely (unsafe). We calibrate a per-user, per-action trust score
`T(u, a) ∈ [0, 1]`:

```
T(u, a) = α·baseline(u) + β·recent(u, a) + γ·domain(u, a)
```

| Term              | Meaning                                   | Default weight |
|-------------------|-------------------------------------------|----------------|
| `baseline(u)`     | Long-run trust for user `u`.              | α = 0.5        |
| `recent(u, a)`    | Recent outcomes for action class `a`.     | β = 0.3        |
| `domain(u, a)`    | Domain-specific trust (e.g., dev vs ops). | γ = 0.2        |

### Calibration target

The interruption rate (fraction of actions routed to `Ask` or `Block`)
should stay between 5% and 15% of total actions per user per day:

```
0.05 ≤ interrupt_rate(u, day) ≤ 0.15
```

If the rate drifts outside this band, the calibration engine nudges
weights via the feedback rule below.

### Feedback learning

After each action resolves (allow, ask-then-approve, ask-then-deny,
or block-then-override), we update:

```
T' = T + η·(observed − predicted)        where η = 0.05
```

- `observed ∈ {0, 1}` — 1 if the action was safe in hindsight,
  0 if it was unsafe or rolled back.
- `predicted = T(u, a)` — the trust value used in the decision.

### Bounds

`T` is clamped to `[0, 1]` after every update. The learning rate `η`
is itself bounded — it cannot exceed `0.10` to prevent trust
oscillation from a single noisy event.

---

## 3. Autonomy Levels

**Implementation:** `hal/src/autonomy_controller.rs`, `hal/src/autonomous_constitution.rs`

Agents operate at one of four graduated autonomy levels. Each level
caps the maximum risk score the agent may execute without escalation.

| Level | Name             | Max-Risk Threshold | Meaning                                       |
|-------|------------------|--------------------|-----------------------------------------------|
| L0    | Supervised       | 0.0                | Every action requires user approval.          |
| L1    | Advisory         | 0.2                | Proposes only; user executes.                 |
| L2    | SemiAutonomous   | 0.5                | May execute low-risk actions autonomously.    |
| L3    | Autonomous       | 0.7                | May execute moderate-risk actions; high-risk still escalates. |

```
max_risk(L) = { 0.0 if L=0, 0.2 if L=1, 0.5 if L=2, 0.7 if L=3 }
```

### Escalation rules

Moving from `L_n` to `L_(n+1)` requires **all** of:

1. **User consent** — explicit, recorded, in-band (not inferred).
2. **24h delay** — the consent must age 24 hours before taking effect.
   This blocks drive-by escalation.
3. **Audit entry** — the escalation is recorded in the append-only
   audit chain (§5) with full context.
4. **Reversible** — a user may de-escalate at any time, instantly,
   no delay, no consent required.

### De-escalation triggers

HAL auto-de-escalates (drops one level, immediately) when any of:

- `anomaly_score > 0.7` — behavioral model flags the agent.
- `R(A) > 0.9` for any executed action — a near-miss.
- User override — explicit `/cognos autonomy downgrade` command.
- Audit chain verification failure (§5) — immediate drop to L0.

De-escalation never requires user consent — only escalation does.

---

## 4. Capability Lattice

**Implementation:** `hal/src/capability_lattice.rs`, `hal/src/permissions.rs`, `agents/shared/capability_lattice.py`

Capabilities are not a flat set of booleans — they form a partial
order `(C, ≤)`. A capability `c₁` implies another `c₂` iff `c₁ ≤ c₂`
in the lattice.

```
(C, ≤) is a partial order                       # reflexive, antisymmetric, transitive
imply(have, want) := have ≤ want                # if I have `have`, I may do `want`
escalate(from, to) requires approval iff ¬(from ≤ to)
```

### Lattice structure

- The lattice is **closed** — the set of capabilities is enumerated
  in `hal/src/permissions.rs` and cannot be extended at runtime.
- The bottom element `⊥` is "no capabilities" (L0 agent).
- The top element `⊤` is "all capabilities" — reserved for the human
  user, never granted to any agent.
- Join (`⊔`) and meet (`⊓`) are defined in `capability_lattice.rs`.

### Permission check

When an agent requests capability `want`, HAL checks:

```
approved(have, want) := have ≤ want
```

If `approved` is false, the request is routed through the escalation
path — which requires user consent (§3).

### Closed set

The capability enumeration lives in `hal/src/permissions.rs`. Adding a
new capability requires:

1. A change to HAL source (which is GPG-signed by iCrewZero only — see CI
   `hal-human-authorship-check`).
2. A new formal-model test.
3. A new audit-chain entry type.
4. Reviewer sign-off.

This is intentionally high-friction.

---

## 5. Audit Chain Integrity

**Implementation:** `hal/src/audit_chain.rs`, `hal/src/audit_log.rs`

The audit log is an append-only hash chain. Each entry commits to all
prior entries, making tampering detectable.

### Genesis

```
h(0) = 0          # well-known genesis hash (all-zero)
```

### Chain rule

For each entry `entry(n)` (a serialized JSON record of the action,
risk score, decision, and outcome):

```
h(n) = SHA256( h(n−1) || entry(n) )
```

`||` denotes byte concatenation. `h(n)` is stored alongside
`entry(n)` in `/var/lib/cognos/hal/audit.jsonl`.

### Verification

To verify the chain from genesis to entry `N`:

```
recompute h(1), h(2), ..., h(N) from the entries
assert recomputed h(N) == stored h(N)
```

A mismatch at any position `k` indicates tampering at entry `k` or
earlier. On mismatch, HAL:

1. Drops all agents to L0 (§3).
2. Alerts the user.
3. Writes a forensic marker (does **not** delete the tampered log —
   the log is append-only, see below).

### Append-only

- **No deletion.** Entries are never removed, even on tamper.
- **No rewrite.** Existing entries are immutable; only appends are
  permitted, and appends must extend the chain correctly.
- The audit file is owned by `cognos-hal`, mode `0640`, and the AI
  daemon has read-only access (see `security/apparmor/cognos-ai-daemon`).

---

## 6. Reputation Decay

**Implementation:** `hal/src/reputation_engine.rs`

Each agent has a long-term reputation `R ∈ [0, 1]` that decays over
time, so that past good behavior does not permanently license future
risk.

```
R(t) = R₀ · e^(−λ(t − t₀))        where λ = ln(2) / half_life
```

- `R₀` — reputation at time `t₀` (last reinforcement).
- `half_life` — default 7 days. Configurable per agent class.
- `λ = ln(2)/7 ≈ 0.099` per day at the default.

### Reinforcement

A successful outcome (action executed, no rollback, no user complaint
within the observation window) reinforces reputation:

```
R₀ ← clamp(R(t) + δ, 0, 1)        where δ = 0.1 (default)
t₀ ← t
```

### Bounds

`R ∈ [0, 1]` always. Reputation cannot go negative and cannot exceed 1.

Reputation feeds into `TrustContext(A)` in the risk model (§1) — a
low-reputation agent raises the trust-context component, nudging risk
upward.

---

## 7. Cognitive Equilibrium

**Implementation:** `hal/src/cognitive_equilibrium.rs`

HAL balances **helpfulness** against **agency loss** — the user's
loss of direct control when an agent acts on their behalf. An action
is recommended only when the balance is positive **and** the action
is genuinely helpful.

```
B(a) = H(a) − w·A(a)
```

- `H(a) ∈ [0, 1]` — helpfulness (estimated utility to the user).
- `A(a) ∈ [0, 1]` — agency loss (how much control the user cedes).
- `w ∈ [0, 1]` — user-tuned weight (default `0.5`).

### Recommendation rule

```
recommend(a) := B(a) > 0  ∧  H(a) > 0.3
```

- `B(a) > 0` — the action is net-positive after accounting for
  agency cost.
- `H(a) > 0.3` — floor on helpfulness; even a net-positive action
  is suppressed if it's barely useful (avoids nagging).

### Tuning

`w` is user-tunable: a user who wants more proactive assistance lowers
`w`; a user who wants tighter control raises `w`. Changes are audited
and reversible.

---

## 8. Restraint Model

**Implementation:** `hal/src/restraint_model.rs`, `hal/src/restraint_runtime.rs`

Predictions (proactive suggestions, context preloads, intent
pre-computation) are gated. A prediction surfaces to the user only
when all of the following hold:

```
surface(a) := confidence(a) > 0.85
              ∧ intimacy(a) = low
              ∧ domain(a) ∈ accepted_domains
              ∧ time_appropriate(a)
```

- `confidence(a) > 0.85` — the model is highly confident.
- `intimacy(a) = low` — the prediction is not about private content.
- `domain(a) ∈ accepted_domains` — the user has opted in to
  predictions in this domain.
- `time_appropriate(a)` — not during focused work, DnD, or off-hours
  unless the user has enabled always-on.

### When in doubt, suppress

```
¬(all conditions) ⇒ suppress(a)
```

A suppressed prediction is logged for offline analysis (so we can
learn whether suppression was correct) but is **never** surfaced to
the user. Restraint is the default; surfacing is the exception.

---

## 9. Constitutional Invariants

**Implementation:** `hal/src/autonomous_constitution.rs`, verified by `governance/src/formal_verifier.rs`

These are properties that **always hold**, for every action `a`, in
every mode. They are non-negotiable and enforced at multiple layers
(HAL source, AppArmor profile, seccomp, audit chain).

```
∀ a: ¬ modifies(a, HAL)                         # AI cannot modify HAL
∀ a: ¬ deceives(a, user)                        # AI cannot deceive user
∀ a: preserves(a, audit)                        # AI cannot destroy audit
∀ a: escalation(a, autonomy) ⇒ consent(user)    # no silent escalation
∀ a: sovereignty(user, a) > sovereignty(a, user) # user always wins
```

### Notes

- `¬ modifies(a, HAL)` — enforced by file permissions, AppArmor
  (`deny /etc/cognos/hal/** rwklx`), and the CI human-authorship
  check.
- `¬ deceives(a, user)` — the AI daemon may not omit material facts
  from approval prompts. Material facts are defined in
  `hal/src/autonomous_constitution.rs`.
- `preserves(a, audit)` — the audit log is append-only (§5); the AI
  daemon has read-only access (AppArmor).
- `escalation(a, autonomy) ⇒ consent(user)` — escalation requires
  explicit, aged consent (§3).
- `sovereignty(user, a) > sovereignty(a, user)` — the user can
  always override any HAL decision, including `Block`. The override
  is logged but never refused.

The formal verifier (`governance/src/formal_verifier.rs`) checks
these invariants against every policy change and every HAL release.

---

## 10. Quorum and Consensus

**Implementation:** `cluster/src/quorum.rs`, `cluster/src/consensus.rs`

Multi-node COGNOS clusters use a Raft-based consensus protocol for
HAL state replication (audit chain, reputation, trust calibration).

### Quorum

- **Majority quorum** (for ordinary replication, log appends):

  ```
  q = ⌊n/2⌋ + 1
  ```

- **Supermajority quorum** (for policy changes, autonomy escalation
  across the cluster, HAL source upgrades):

  ```
  q = ⌈2n/3⌉
  ```

### Leader election

- Randomized election timeout: **150–300 ms**.
- A candidate that receives a majority quorum of votes becomes leader
  for the term.
- Heartbeat interval: **50 ms** (less than min election timeout to
  prevent spurious elections).

### State replication

- HAL audit-chain entries are replicated via the consensus log.
- An entry is **committed** once a majority quorum acknowledges it.
- Committed entries are durable — they survive leader failover.

### Failover

- If the leader becomes unresponsive (missed heartbeats for
  `election_timeout_max`), followers time out and elect a new leader.
- The new leader reconstructs the audit chain from its log and
  verifies integrity (§5) before accepting new entries.

---

<!-- v0: stub — formulas are implemented; full proofs are TODO -->
<!--
TODO:
  * Mechanize the proofs in Coq / Lean and link the artifact.
  * Add a §11 on information-flow (noninterference between agents).
  * Add a §12 on resource-fairness (scheduler formal model).
  * Cross-link each formula to its Rust test by symbol name.
-->
