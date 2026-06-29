//! Policy table — maps usage scenarios to concrete resource policies
//! (CPU governor, AI cgroup weights, memory limits). Policies are static;
//! the runtime picks which to apply.
//!
//! All values in the default table stay inside the safety envelope defined
//! in [`crate::resource_policy`]: governors come from `ALLOWED_GOVERNORS`,
//! `10 ≤ ai_cgroup_cpu_weight ≤ 400`, and memory within
//! `[AI_MEMORY_HIGH_MIN_GB, AI_MEMORY_HIGH_MAX_GB]`. User overrides that
//! fall outside the envelope are rejected with [`PolicyError::OverrideRejected`].

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

// ─── Scenario ─────────────────────────────────────────────────────────────────

/// All recognized usage scenarios. The order of evaluation in the predictor
/// and daemon matters — more specific scenarios (e.g. `BatteryCritical`)
/// are checked before more general ones. `GeneralUse` is the safe fallback.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize,
)]
pub enum Scenario {
    /// User is actively editing code in an IDE/editor.
    CodingActive,
    /// A GPU/CPU-heavy render job (ffmpeg, blender, …) is running.
    VideoRendering,
    /// Battery is below the critical threshold and discharging.
    BatteryCritical,
    /// Machine is idle overnight — AI may use the full budget.
    IdleOvernight,
    /// A fullscreen game has the foreground.
    Gaming,
    /// IDE + AI assistant are both active (Claude Code, Copilot, …).
    VibeCoding,
    /// None of the above; balanced defaults.
    #[default]
    GeneralUse,
}

// ─── I/O Priority ─────────────────────────────────────────────────────────────

/// I/O priority class for the AI cgroup. Mirrors the Linux `ionice`
/// best-effort / realtime / idle classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum IoPriority {
    /// Best-effort class with level `0` (highest) .. `7` (lowest).
    #[default]
    Be(u8),
    /// Idle class — only dispatched when no other I/O is pending.
    Idle,
    /// Realtime class with level `0`..`7`. Use sparingly.
    Realtime(u8),
}

// ─── Resource Policy ──────────────────────────────────────────────────────────

/// A concrete resource policy to apply when a given scenario is active.
///
/// All fields are kept inside the safety envelope defined in
/// [`crate::resource_policy`]: `cpu_governor` ∈ `ALLOWED_GOVERNORS`,
/// `10 ≤ ai_cgroup_cpu_weight ≤ 400`, and
/// `AI_MEMORY_HIGH_MIN_GB ≤ ai_memory_high_gb ≤ AI_MEMORY_HIGH_MAX_GB`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResourcePolicy {
    /// CPU frequency governor name (e.g. `"performance"`, `"powersave"`,
    /// `"schedutil"`).
    pub cpu_governor: String,
    /// AI cgroup `cpu.weight` (clamped to `10..=400`).
    pub ai_cgroup_cpu_weight: u32,
    /// AI cgroup `memory.high` soft limit, in GiB.
    pub ai_memory_high_gb: f32,
    /// I/O priority for the AI cgroup.
    pub io_priority: IoPriority,
    /// Human-readable description (also written to the audit log).
    pub description: String,
}

// ─── Errors ───────────────────────────────────────────────────────────────────

/// Policy table errors.
#[derive(Debug, Error)]
pub enum PolicyError {
    /// A user-supplied override violated the safety envelope.
    #[error("policy override rejected: {0}")]
    OverrideRejected(String),
}

// ─── Policy Table ─────────────────────────────────────────────────────────────

/// Static map of [`Scenario`] → [`ResourcePolicy`]. The runtime never
/// modifies these at runtime; user overrides go through
/// [`PolicyTable::override`] which validates against the safety envelope.
pub struct PolicyTable {
    policies: HashMap<Scenario, ResourcePolicy>,
}

impl Default for PolicyTable {
    fn default() -> Self {
        Self::new()
    }
}

impl PolicyTable {
    /// Build the default policy table per `docs/SPEC.md`.
    ///
    /// The defaults match the heuristic scenarios detected by
    /// `daemon::SchedulerDaemon::detect_scenario`: coding/active scenarios
    /// bias toward `performance` and headroom for the AI; battery and idle
    /// scenarios bias toward `powersave` and either throttle or unthrottle
    /// the AI cgroup accordingly.
    pub fn new() -> Self {
        let mut policies: HashMap<Scenario, ResourcePolicy> = HashMap::new();

        policies.insert(
            Scenario::CodingActive,
            ResourcePolicy {
                cpu_governor: "performance".into(),
                ai_cgroup_cpu_weight: 200,
                ai_memory_high_gb: 1.2,
                io_priority: IoPriority::Be(4),
                description: "Coding active: performance governor, AI at 20% CPU".into(),
            },
        );
        policies.insert(
            Scenario::VideoRendering,
            ResourcePolicy {
                cpu_governor: "performance".into(),
                ai_cgroup_cpu_weight: 50,
                ai_memory_high_gb: 0.8,
                io_priority: IoPriority::Be(6),
                description: "Video rendering: AI limited to 5% CPU, low I/O priority".into(),
            },
        );
        policies.insert(
            Scenario::BatteryCritical,
            ResourcePolicy {
                cpu_governor: "powersave".into(),
                ai_cgroup_cpu_weight: 50,
                ai_memory_high_gb: 0.4,
                io_priority: IoPriority::Idle,
                description: "Battery critical: AI throttled, powersave mode".into(),
            },
        );
        policies.insert(
            Scenario::IdleOvernight,
            ResourcePolicy {
                cpu_governor: "powersave".into(),
                ai_cgroup_cpu_weight: 400,
                ai_memory_high_gb: 2.0,
                io_priority: IoPriority::Be(2),
                description: "Idle overnight: AI can use full budget for indexing".into(),
            },
        );
        policies.insert(
            Scenario::Gaming,
            ResourcePolicy {
                cpu_governor: "performance".into(),
                ai_cgroup_cpu_weight: 30,
                ai_memory_high_gb: 0.8,
                io_priority: IoPriority::Be(6),
                description: "Gaming: AI isolated to minimum, GPU priority for games".into(),
            },
        );
        policies.insert(
            Scenario::VibeCoding,
            ResourcePolicy {
                cpu_governor: "schedutil".into(),
                ai_cgroup_cpu_weight: 200,
                ai_memory_high_gb: 1.8,
                io_priority: IoPriority::Be(3),
                description: "Vibe-coding: AI models kept hot, balanced governor".into(),
            },
        );
        policies.insert(
            Scenario::GeneralUse,
            ResourcePolicy {
                cpu_governor: "schedutil".into(),
                ai_cgroup_cpu_weight: 100,
                ai_memory_high_gb: 1.2,
                io_priority: IoPriority::Be(4),
                description: "General use: balanced defaults".into(),
            },
        );

        Self { policies }
    }

    /// Look up the resource policy for a scenario. Always returns a policy:
    /// if the scenario is somehow missing from the table, falls back to the
    /// `GeneralUse` entry (which is guaranteed to exist).
    pub fn policy_for(&self, scenario: &Scenario) -> &ResourcePolicy {
        self.policies
            .get(scenario)
            .or_else(|| self.policies.get(&Scenario::GeneralUse))
            .expect("default policy table always contains GeneralUse")
    }

    /// Replace the policy for a scenario (e.g. user override). The supplied
    /// policy must remain inside the safety envelope; otherwise an
    /// [`PolicyError::OverrideRejected`] is returned and the table is left
    /// untouched.
    ///
    /// Note: `override` is a Rust keyword, so the method is spelled
    /// `r#override` at the call site.
    pub fn r#override(
        &mut self,
        scenario: Scenario,
        policy: ResourcePolicy,
    ) -> Result<(), PolicyError> {
        // v0: stub — only validate cpu.weight. TODO(v1): validate governor,
        // memory, and io_priority against `crate::resource_policy` and reject
        // out-of-envelope fields with `OverrideRejected`.
        if !(10..=400).contains(&policy.ai_cgroup_cpu_weight) {
            return Err(PolicyError::OverrideRejected(format!(
                "ai_cgroup_cpu_weight {} outside 10..=400",
                policy.ai_cgroup_cpu_weight
            )));
        }
        self.policies.insert(scenario, policy);
        Ok(())
    }
}

// v0: stub implementation
