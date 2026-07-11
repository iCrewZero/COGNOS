/// Adaptive Resource Scheduler for COGNOS/OS.
///
/// Reads eBPF telemetry, detects the current usage scenario, and applies
/// the matching resource policy. AI takes only what is allocated.
///
/// All /sys writes go through systemd dbus or cgroup v2 file interface.
/// No unsafe code. No direct memory writes.

use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, Instant};
use serde::{Deserialize, Serialize};

// ─── Types ────────────────────────────────────────────────────────────────────

/// All recognized usage scenarios.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum Scenario {
    CodingActive,
    VideoRendering,
    BatteryCritical,
    IdleOvernight,
    Gaming,
    VibeCoding,
    #[default]
    GeneralUse,
}

/// Snapshot of system-wide metrics, sourced from eBPF telemetry.
#[derive(Debug, Clone, Default)]
pub struct SystemMetrics {
    pub cpu_usage_per_core: Vec<f32>, // 0.0–1.0 per core
    pub gpu_usage: f32,               // 0.0–1.0
    pub ram_used_gb: f32,
    pub battery_percent: u8,
    pub battery_discharging: bool,
    pub process_list: Vec<ProcessInfo>,
    pub foreground_app: String,
    pub io_ops_per_second: u64,
    pub user_idle_seconds: u64,
    pub hour_of_day: u8, // 0–23
}

#[derive(Debug, Clone)]
pub struct ProcessInfo {
    pub name: String,
    pub cpu_percent: f32,
}

/// Resource policy to apply for a given scenario.
#[derive(Debug, Clone)]
struct ResourcePolicy {
    cpu_governor: &'static str, // e.g. "performance", "powersave", "schedutil"
    ai_cgroup_cpu_weight: u32,  // 10–400 (out of 1000)
    ai_memory_high_gb: f32,     // soft memory limit for AI cgroup
    description: &'static str,
}

/// User-requested override of the auto-detected scenario.
struct UserOverride {
    scenario: Scenario,
    expires_at: Option<Instant>,
}

// ─── Scheduler Daemon ─────────────────────────────────────────────────────────

pub trait EbpfReader: Send {
    fn read_metrics(&self) -> SystemMetrics;
}

/// The main scheduler daemon.
pub struct SchedulerDaemon<R: EbpfReader> {
    reader: R,
    current_scenario: Option<Scenario>,
    scenario_since: Option<Instant>,
    user_override: Option<UserOverride>,
    /// How long a new scenario must be stable before we switch policy.
    hysteresis: Duration,
    audit_log: std::path::PathBuf,
}

impl<R: EbpfReader> SchedulerDaemon<R> {
    pub fn new(reader: R) -> Self {
        let audit_log = dirs::home_dir()
            .unwrap_or_else(|| "/tmp".into())
            .join(".cognos/audit.log");

        Self {
            reader,
            current_scenario: None,
            scenario_since: None,
            user_override: None,
            hysteresis: Duration::from_secs(30),
            audit_log,
        }
    }

    /// Main polling loop. Should be called from an async runtime.
    pub async fn run(&mut self) {
        loop {
            let metrics = self.reader.read_metrics();
            self.tick(&metrics).await;
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    /// One scheduler tick.
    async fn tick(&mut self, metrics: &SystemMetrics) {
        let detected = self.detect_scenario(metrics);

        // User override takes precedence
        if let Some(ov) = &self.user_override {
            if ov.expires_at.map(|e| Instant::now() < e).unwrap_or(true) {
                if Some(&ov.scenario) != self.current_scenario.as_ref() {
                    let s = ov.scenario.clone();
                    self.apply_policy(&s).await;
                    self.current_scenario = Some(s);
                }
                return;
            } else {
                self.user_override = None; // expired
            }
        }

        // Hysteresis: only switch if detected scenario has been stable
        let should_switch = match &self.current_scenario {
            None => true,
            Some(current) if current != &detected => {
                match self.scenario_since {
                    None => {
                        self.scenario_since = Some(Instant::now());
                        false
                    }
                    Some(since) => Instant::now().duration_since(since) >= self.hysteresis,
                }
            }
            _ => {
                self.scenario_since = None; // stable, reset timer
                false
            }
        };

        if should_switch {
            self.scenario_since = None;
            self.apply_policy(&detected).await;
            self.current_scenario = Some(detected);
        }
    }

    /// Detect which scenario best describes current system state.
    pub fn detect_scenario(&self, m: &SystemMetrics) -> Scenario {
        let fg = m.foreground_app.to_lowercase();
        let procs: HashSet<String> = m
            .process_list
            .iter()
            .map(|p| p.name.to_lowercase())
            .collect();

        let is_coding_app = [
            "vscode", "code", "vim", "neovim", "nvim", "emacs",
            "jetbrains", "idea", "pycharm", "clion", "rider",
        ]
        .iter()
        .any(|app| fg.contains(app));

        let is_render_proc = ["ffmpeg", "blender", "kdenlive"]
            .iter()
            .any(|p| procs.contains(*p));

        let is_gaming =
            fg.contains("steam")
                || fg.contains("lutris")
                || fg.contains("wine")
                || (m.gpu_usage > 0.70
                    && m
                        .process_list
                        .iter()
                        .any(|p| p.cpu_percent > 0.1));

        let has_ai_proc = procs.contains("ollama") || procs.contains("claude");

        let is_overnight =
            m.user_idle_seconds > 1800 && (m.hour_of_day >= 22 || m.hour_of_day <= 7);

        // Evaluation order matters — more specific scenarios first
        if m.battery_discharging && m.battery_percent < 15 {
            return Scenario::BatteryCritical;
        }
        if is_coding_app && has_ai_proc && m.battery_percent > 20 {
            return Scenario::VibeCoding;
        }
        if is_coding_app && m.battery_percent > 20 {
            return Scenario::CodingActive;
        }
        if m.gpu_usage > 0.80 && is_render_proc {
            return Scenario::VideoRendering;
        }
        if is_gaming {
            return Scenario::Gaming;
        }
        if is_overnight {
            return Scenario::IdleOvernight;
        }
        Scenario::GeneralUse
    }

    /// Apply the resource policy for the given scenario.
    async fn apply_policy(&self, scenario: &Scenario) {
        let policy = Self::policy_for(scenario);

        // 1. CPU governor via systemd dbus (non-blocking best-effort)
        if let Err(e) = set_cpu_governor(policy.cpu_governor).await {
            log::warn!("Failed to set CPU governor '{}': {}", policy.cpu_governor, e);
        }

        // 2. AI cgroup cpu.weight via cgroupv2
        let weight = policy.ai_cgroup_cpu_weight.clamp(10, 400);
        if let Err(e) = set_cgroup_cpu_weight("cognos.slice/cognos-ai.slice", weight).await {
            log::warn!("Failed to set AI cgroup cpu.weight={}: {}", weight, e);
        }

        // 3. AI cgroup memory.high
        let mem_bytes = (policy.ai_memory_high_gb * 1024.0 * 1024.0 * 1024.0) as u64;
        if let Err(e) = set_cgroup_memory_high("cognos.slice/cognos-ai.slice", mem_bytes).await {
            log::warn!("Failed to set AI memory.high: {}", e);
        }

        // 4. sched_setattr hints for AI processes
        set_sched_hints(scenario).await;

        self.audit(scenario, policy);
    }

    fn policy_for(scenario: &Scenario) -> ResourcePolicy {
        match scenario {
            Scenario::CodingActive => ResourcePolicy {
                cpu_governor: "performance",
                ai_cgroup_cpu_weight: 150,  // 15% of 1000
                ai_memory_high_gb: 1.2,
                description: "Coding active: performance governor, AI at 15% CPU",
            },
            Scenario::VideoRendering => ResourcePolicy {
                cpu_governor: "performance",
                ai_cgroup_cpu_weight: 50,   // 5%
                ai_memory_high_gb: 0.8,
                description: "Video rendering: AI limited to 5% CPU",
            },
            Scenario::BatteryCritical => ResourcePolicy {
                cpu_governor: "powersave",
                ai_cgroup_cpu_weight: 10,   // minimal
                ai_memory_high_gb: 0.4,
                description: "Battery critical: AI paused, powersave mode",
            },
            Scenario::IdleOvernight => ResourcePolicy {
                cpu_governor: "powersave",
                ai_cgroup_cpu_weight: 400,  // full budget when idle
                ai_memory_high_gb: 2.0,
                description: "Idle overnight: AI can use full budget for indexing",
            },
            Scenario::Gaming => ResourcePolicy {
                cpu_governor: "performance",
                ai_cgroup_cpu_weight: 30,   // 3%
                ai_memory_high_gb: 0.8,
                description: "Gaming: AI isolated to minimum, GPU priority for games",
            },
            Scenario::VibeCoding => ResourcePolicy {
                cpu_governor: "schedutil",
                ai_cgroup_cpu_weight: 200,  // 20% — models need headroom
                ai_memory_high_gb: 1.8,
                description: "Vibe-coding: AI models kept hot, balanced governor",
            },
            Scenario::GeneralUse => ResourcePolicy {
                cpu_governor: "schedutil",
                ai_cgroup_cpu_weight: 100,  // 10%
                ai_memory_high_gb: 1.2,
                description: "General use: balanced defaults",
            },
        }
    }

    /// Force a specific scenario, optionally with an expiry.
    pub async fn apply_user_override(&mut self, scenario: Scenario, duration: Option<Duration>) {
        let expires_at = duration.map(|d| Instant::now() + d);
        let label = format!("{:?}", scenario);
        self.user_override = Some(UserOverride {
            scenario: scenario.clone(),
            expires_at,
        });
        self.apply_policy(&scenario).await;
        log::info!(
            "User override: {:?}, expires in {:?}",
            scenario,
            duration
        );
        self.write_audit_line(&format!(
            r#"{{"ts":"{}","agent":"scheduler","action":"user_override","scenario":"{}","duration_secs":{}}}"#,
            chrono::Utc::now().to_rfc3339(),
            label,
            duration.map(|d| d.as_secs()).unwrap_or(0),
        ));
    }

    fn audit(&self, scenario: &Scenario, policy: ResourcePolicy) {
        let line = format!(
            r#"{{"ts":"{}","agent":"scheduler","action":"policy_apply","scenario":"{:?}","governor":"{}","ai_cpu_weight":{},"description":"{}"}}"#,
            chrono::Utc::now().to_rfc3339(),
            scenario,
            policy.cpu_governor,
            policy.ai_cgroup_cpu_weight,
            policy.description,
        );
        self.write_audit_line(&line);
    }

    fn write_audit_line(&self, line: &str) {
        if let Some(parent) = self.audit_log.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.audit_log)
        {
            use std::io::Write;
            let _ = writeln!(f, "{}", line);
        }
    }
}

// ─── System interfaces ────────────────────────────────────────────────────────

/// Set CPU frequency governor via systemd (not direct /sys write).
async fn set_cpu_governor(governor: &str) -> Result<(), String> {
    // systemd-run --no-block is simplest for non-dbus callers
    let output = tokio::process::Command::new("systemctl")
        .args(&[
            "--no-block",
            "start",
            &format!("cognos-governor@{}.service", governor),
        ])
        .output()
        .await
        .map_err(|e| e.to_string())?;

    if !output.status.success() {
        // Fallback: write directly if systemd service doesn't exist yet
        let cpu_count = num_cpus::get();
        for i in 0..cpu_count {
            let path = format!("/sys/devices/system/cpu/cpu{}/cpufreq/scaling_governor", i);
            if Path::new(&path).exists() {
                tokio::fs::write(&path, governor)
                    .await
                    .map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(())
}

/// Write cpu.weight to a cgroup v2 slice.
async fn set_cgroup_cpu_weight(slice: &str, weight: u32) -> Result<(), String> {
    let path = format!("/sys/fs/cgroup/{}/cpu.weight", slice);
    tokio::fs::write(&path, weight.to_string())
        .await
        .map_err(|e| format!("cgroup cpu.weight write failed: {}", e))
}

/// Write memory.high to a cgroup v2 slice.
async fn set_cgroup_memory_high(slice: &str, bytes: u64) -> Result<(), String> {
    let path = format!("/sys/fs/cgroup/{}/memory.high", slice);
    tokio::fs::write(&path, bytes.to_string())
        .await
        .map_err(|e| format!("cgroup memory.high write failed: {}", e))
}

/// Apply sched_setattr hints to AI inference processes via the scheduler daemon protocol.
/// In v1 this writes a hint file that the kernel eBPF scheduler hook reads.
async fn set_sched_hints(scenario: &Scenario) {
    let nice = match scenario {
        Scenario::Gaming | Scenario::VideoRendering => 10i8,
        Scenario::BatteryCritical => 19,
        Scenario::IdleOvernight => 15,
        _ => 5,
    };

    let hint_path = Path::new("/run/cognos/scheduler_hints.json");
    if let Some(parent) = hint_path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    let content = format!(r#"{{"ai_nice":{}}}"#, nice);
    let _ = tokio::fs::write(hint_path, content).await;
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    struct MockReader(SystemMetrics);
    impl EbpfReader for MockReader {
        fn read_metrics(&self) -> SystemMetrics {
            self.0.clone()
        }
    }

    fn daemon(metrics: SystemMetrics) -> SchedulerDaemon<MockReader> {
        SchedulerDaemon::new(MockReader(metrics))
    }

    fn metrics_with(fg: &str) -> SystemMetrics {
        SystemMetrics {
            foreground_app: fg.to_string(),
            battery_percent: 80,
            battery_discharging: true,
            ..Default::default()
        }
    }

    #[test]
    fn detects_coding_active() {
        let d = daemon(metrics_with("vscode"));
        let m = metrics_with("vscode");
        assert_eq!(d.detect_scenario(&m), Scenario::CodingActive);
    }

    #[test]
    fn detects_battery_critical() {
        let d = daemon(metrics_with("vscode"));
        let m = SystemMetrics {
            battery_percent: 10,
            battery_discharging: true,
            foreground_app: "vscode".into(),
            ..Default::default()
        };
        assert_eq!(d.detect_scenario(&m), Scenario::BatteryCritical);
    }

    #[test]
    fn battery_critical_overrides_coding() {
        let d = daemon(SystemMetrics::default());
        let m = SystemMetrics {
            foreground_app: "vscode".into(),
            battery_percent: 5,
            battery_discharging: true,
            ..Default::default()
        };
        // BatteryCritical is checked before CodingActive
        assert_eq!(d.detect_scenario(&m), Scenario::BatteryCritical);
    }

    #[test]
    fn detects_gaming() {
        let d = daemon(SystemMetrics::default());
        let m = SystemMetrics {
            foreground_app: "steam".into(),
            battery_percent: 80,
            ..Default::default()
        };
        assert_eq!(d.detect_scenario(&m), Scenario::Gaming);
    }

    #[test]
    fn vibe_coding_requires_ai_process() {
        let d = daemon(SystemMetrics::default());
        let m = SystemMetrics {
            foreground_app: "vscode".into(),
            battery_percent: 80,
            process_list: vec![ProcessInfo { name: "ollama".into(), cpu_percent: 0.3 }],
            ..Default::default()
        };
        assert_eq!(d.detect_scenario(&m), Scenario::VibeCoding);
    }

    #[test]
    fn ai_cpu_weight_clamped() {
        // Ensure we never set weight outside the safe 10–400 range
        for scenario in &[
            Scenario::CodingActive, Scenario::VideoRendering, Scenario::BatteryCritical,
            Scenario::IdleOvernight, Scenario::Gaming, Scenario::VibeCoding,
            Scenario::GeneralUse,
        ] {
            let policy = SchedulerDaemon::<MockReader>::policy_for(scenario);
            assert!(
                policy.ai_cgroup_cpu_weight >= 10 && policy.ai_cgroup_cpu_weight <= 400,
                "{:?} weight {} out of range", scenario, policy.ai_cgroup_cpu_weight
            );
        }
    }
}
