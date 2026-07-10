//! Scheduler runtime — ties predictor, policy table, and cgroup writers
//! together. Runs a 1Hz control loop: sample → predict → apply policy → log.
//!
//! The runtime is the orchestrator: each tick it asks the predictor for a
//! forecast, looks up the matching [`ResourcePolicy`] in the [`PolicyTable`],
//! and asks the [`CgroupWriter`] to apply it. Hysteresis prevents thrash
//! between adjacent scenarios — a scenario must be stable for at least
//! `HYSTERESIS_MIN_DURATION` before the runtime will switch.
//!
//! Owner: iCrewZero

use std::path::PathBuf;
use std::time::{Duration, Instant};

use thiserror::Error;
use tokio::time::interval;
use tracing::{info, warn};

// NOTE: CancellationToken requires tokio-util. The Cargo.toml already has
// tokio-util = "0.7" so this import works. If it's not there, add it.
use tokio_util::sync::CancellationToken;

// Scenario is defined in daemon.rs (the canonical location).
// We re-use it here so the runtime control loop can reference
// scenario types without pulling in the full daemon dependency.
use crate::daemon::Scenario;
use crate::predictor::{Predictor, PredictorError};

// ─── Resource Policy (local definition so runtime.rs compiles standalone) ─────
//
// In v1 these types will move to resource_policy.rs. For now they live
// here so the runtime control loop can compile without pulling in the
// full daemon module.

/// A resource policy to apply for a given scenario.
/// Mirrors daemon::ResourcePolicy but without the full daemon dependency.
#[derive(Debug, Clone)]
pub struct ResourcePolicy {
    /// CPU governor name, e.g. "performance", "powersave", "schedutil".
    pub cpu_governor: String,
    /// AI cgroup CPU weight (10–400, out of 1000).
    pub ai_cgroup_cpu_weight: u32,
    /// Soft memory limit for the AI cgroup in GB.
    pub ai_memory_high_gb: f32,
    /// Human-readable description of why this policy was chosen.
    pub description: String,
}

/// A table that maps scenarios to resource policies.
/// In v1 this will be loaded from /etc/cognos/scheduler-policy.toml.
#[derive(Debug, Clone)]
pub struct PolicyTable {
    policies: std::collections::HashMap<String, ResourcePolicy>,
}

impl PolicyTable {
    /// Create a policy table with sensible defaults for every scenario.
    pub fn new() -> Self {
        let mut policies = std::collections::HashMap::new();

        // Each scenario gets a policy that limits AI resource usage.
        // The values here are conservative defaults; v1 will load from config.
        // Values must stay within resource_policy.rs bounds:
        //   CPU weight: 10–400 (AI_CPU_WEIGHT_MIN..MAX)
        //   Memory high: 0.2–4.0 GB (AI_MEMORY_HIGH_MIN_GB..MAX_GB)
        let defaults: &[(&str, &str, u32, f32)] = &[
            // scenario, governor, cpu_weight, memory_high_gb
            ("CodingActive",   "performance", 200, 4.0),
            ("VideoRendering", "performance", 100, 2.0),
            ("BatteryCritical","powersave",  50, 1.0),
            ("IdleOvernight",  "powersave",  30, 0.5),
            ("Gaming",         "performance", 50, 1.0),
            ("VibeCoding",     "performance", 250, 6.0),
            ("GeneralUse",     "schedutil",  100, 3.0),
        ];

        for (scenario, governor, weight, mem) in defaults {
            policies.insert(
                scenario.to_string(),
                ResourcePolicy {
                    cpu_governor: governor.to_string(),
                    ai_cgroup_cpu_weight: *weight,
                    ai_memory_high_gb: *mem,
                    description: format!("Auto policy for {}", scenario),
                },
            );
        }

        Self { policies }
    }

    /// Look up the policy for a scenario. Falls back to GeneralUse.
    pub fn policy_for(&self, scenario: &Scenario) -> &ResourcePolicy {
        let key = format!("{:?}", scenario);
        self.policies
            .get(&key)
            .unwrap_or_else(|| self.policies.get("GeneralUse").unwrap())
    }
}

impl Default for PolicyTable {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Cgroup Writer ────────────────────────────────────────────────────────────

/// Errors returned by [`CgroupWriter::apply`].
#[derive(Debug, Error)]
pub enum CgroupError {
    /// The cgroup root path does not exist or is not writable.
    #[error("cgroup root not accessible: {0}")]
    RootNotAccessible(String),
    /// A write to a cgroup control file failed.
    #[error("cgroup write failed at {path}: {reason}")]
    WriteFailed {
        /// Path of the control file that rejected the write.
        path: String,
        /// Underlying I/O error message.
        reason: String,
    },
    /// The supplied policy violated the safety envelope.
    #[error("policy rejected by safety envelope: {0}")]
    PolicyRejected(String),
}

/// Writes a [`ResourcePolicy`] to the AI cgroup slice. The writer is scoped
/// to a single root (typically
/// `/sys/fs/cgroup/cognos.slice/cognos-ai.slice`) and refuses to write
/// outside it — see `crate::resource_policy::AI_SLICE`.
#[derive(Debug, Clone)]
pub struct CgroupWriter {
    /// Absolute path to the cgroup root this writer is allowed to touch.
    pub root: PathBuf,
}

impl CgroupWriter {
    /// Build a writer rooted at `root`.
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Apply a resource policy by writing `cpu.weight`, `memory.high`, and
    /// the I/O priority files to the cgroup root. CPU governor is delegated
    /// to systemd (see `daemon::set_cpu_governor`) and not handled here.
    pub async fn apply(&self, policy: &ResourcePolicy) -> Result<(), CgroupError> {
        // v0: stub — no real /sys/fs/cgroup writes.
        // In v1 this will:
        //   1. Validate policy via crate::resource_policy::validate_*
        //   2. Write cpu.weight, memory.high, io.latency through cgroup v2
        //   3. All writes go through tokio::fs to stay async-friendly
        let _ = policy;
        let _ = &self.root;
        Ok(())
    }
}

// ─── Runtime Errors ───────────────────────────────────────────────────────────

/// Errors raised by the scheduler runtime control loop.
#[derive(Debug, Error)]
pub enum RuntimeError {
    /// The predictor returned an error.
    #[error("predictor error: {0}")]
    Predictor(#[from] PredictorError),
    /// The cgroup writer returned an error.
    #[error("cgroup error: {0}")]
    Cgroup(#[from] CgroupError),
    /// Telemetry sampling failed (e.g. the eBPF reader went away).
    #[error("telemetry error: {0}")]
    Telemetry(String),
}

// ─── Scheduler Runtime ────────────────────────────────────────────────────────

/// Minimum time the runtime must remain in a scenario before switching to a
/// different one. Prevents thrash between adjacent scenarios.
const HYSTERESIS_MIN_DURATION: Duration = Duration::from_secs(30);

/// 1Hz control loop period.
const TICK_PERIOD: Duration = Duration::from_secs(1);

/// Orchestrates the predictor + policy table + cgroup writer. Created via
/// [`SchedulerRuntime::new`] and driven by [`SchedulerRuntime::run`].
pub struct SchedulerRuntime {
    /// Workload predictor — fed telemetry by the caller via
    /// [`Predictor::push_sample`].
    pub predictor: Predictor,
    /// Static scenario → policy map.
    pub policies: PolicyTable,
    /// Currently active scenario.
    pub current_scenario: Scenario,
    /// Most recently applied policy, if any.
    pub current_policy: Option<ResourcePolicy>,
    /// Cgroup writer used to materialize policies.
    pub cgroup_writer: CgroupWriter,
    /// When the current policy was last applied. Used by the hysteresis
    /// check to decide whether a scenario switch is allowed.
    pub last_applied: Option<Instant>,
}

impl SchedulerRuntime {
    /// Build a runtime from a predictor and a policy table. The cgroup
    /// writer is constructed with the default AI slice path.
    pub fn new(predictor: Predictor, policies: PolicyTable) -> Self {
        let cgroup_writer =
            CgroupWriter::new(PathBuf::from("/sys/fs/cgroup/cognos.slice/cognos-ai.slice"));
        Self {
            predictor,
            policies,
            current_scenario: Scenario::GeneralUse,
            current_policy: None,
            cgroup_writer,
            last_applied: None,
        }
    }

    /// Run the 1Hz control loop until `shutdown` is cancelled.
    ///
    /// Tick errors are logged at `WARN` and the loop continues — the runtime
    /// never exits the loop on its own except for an explicit shutdown.
    pub async fn run(&mut self, shutdown: CancellationToken) -> Result<(), RuntimeError> {
        let mut ticker = interval(TICK_PERIOD);
        // The first `tick()` completes immediately; skip it so we don't
        // apply a policy before any telemetry has been pushed.
        ticker.tick().await;

        loop {
            tokio::select! {
                _ = shutdown.cancelled() => {
                    info!("scheduler runtime shutting down");
                    return Ok(());
                }
                _ = ticker.tick() => {
                    if let Err(e) = self.tick().await {
                        warn!(error = %e, "scheduler tick failed");
                        // v0: keep the loop running. v1: classify and
                        // escalate persistent errors.
                    }
                }
            }
        }
    }

    /// One control-loop iteration: predict → look up policy → apply (subject
    /// to hysteresis) → log. The "sample" step happens outside the runtime:
    /// the caller pushes telemetry into the predictor via
    /// [`Predictor::push_sample`] before each tick.
    pub async fn tick(&mut self) -> Result<(), RuntimeError> {
        let prediction = self.predictor.predict().await?;
        let new_scenario = prediction.scenario;

        // Hysteresis: don't thrash between scenarios. Only switch if we've
        // been in the current scenario for at least 30 seconds, or if the
        // new scenario is the same as the current one.
        let now = Instant::now();
        let stable = match self.last_applied {
            None => true,
            Some(last) => now.duration_since(last) >= HYSTERESIS_MIN_DURATION,
        };

        if new_scenario != self.current_scenario && !stable {
            // Keep current scenario; not yet stable enough to switch.
            return Ok(());
        }

        if new_scenario != self.current_scenario || self.current_policy.is_none() {
            let policy = self.policies.policy_for(&new_scenario).clone();
            self.cgroup_writer.apply(&policy).await?;
            self.current_scenario = new_scenario;
            self.current_policy = Some(policy);
            self.last_applied = Some(now);
            info!(scenario = ?self.current_scenario, "applied new policy");
        }

        Ok(())
    }

    /// Return the currently active scenario.
    pub fn current_scenario(&self) -> Scenario {
        self.current_scenario
    }
}
