//! Telemetry seam for the scheduler daemon.
//!
//! v0 ships a /proc + /sys best-effort reader with no eBPF dependency; the
//! kernel/ebpf `scheduler_telemetry` program replaces it in Phase 4 behind
//! the same [`EbpfReader`] trait. Fields that require compositor or eBPF
//! data (foreground app, GPU, io ops, idle time) default to conservative
//! zeros — the daemon's scenario detection degrades gracefully to
//! `GeneralUse` rather than guessing.

use std::fs;
use std::sync::Mutex;

use chrono::Timelike;

use crate::daemon::{EbpfReader, ProcessInfo, SystemMetrics};

/// Fixed-value reader for tests and simulation.
pub struct FixedReader(pub SystemMetrics);

impl EbpfReader for FixedReader {
    fn read_metrics(&self) -> SystemMetrics {
        self.0.clone()
    }
}

/// (busy, total) jiffies per core, sampled from /proc/stat.
#[derive(Debug, Clone, Default)]
struct CpuSample {
    cores: Vec<(u64, u64)>,
}

fn read_cpu_sample() -> CpuSample {
    let content = fs::read_to_string("/proc/stat").unwrap_or_default();
    let mut cores = Vec::new();
    for line in content.lines() {
        // Per-core lines are "cpu0 ...", "cpu1 ..."; skip the aggregate "cpu ".
        if line.starts_with("cpu") && !line.starts_with("cpu ") {
            let fields: Vec<u64> = line
                .split_whitespace()
                .skip(1)
                .filter_map(|v| v.parse().ok())
                .collect();
            if fields.len() >= 4 {
                let total: u64 = fields.iter().sum();
                let idle = fields[3] + fields.get(4).copied().unwrap_or(0);
                cores.push((total.saturating_sub(idle), total));
            }
        }
    }
    CpuSample { cores }
}

/// Best-effort /proc + /sys telemetry reader.
pub struct ProcfsReader {
    prev: Mutex<CpuSample>,
}

impl Default for ProcfsReader {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcfsReader {
    pub fn new() -> Self {
        Self {
            prev: Mutex::new(read_cpu_sample()),
        }
    }

    /// Per-core usage as the busy-jiffie delta since the previous read.
    fn cpu_usage(&self) -> Vec<f32> {
        let current = read_cpu_sample();
        let mut prev = match self.prev.lock() {
            Ok(guard) => guard,
            // A poisoned lock only means a panic happened mid-sample;
            // the data is still usable for best-effort telemetry.
            Err(poisoned) => poisoned.into_inner(),
        };
        let usage = current
            .cores
            .iter()
            .zip(prev.cores.iter())
            .map(|((busy, total), (prev_busy, prev_total))| {
                let dt = total.saturating_sub(*prev_total);
                if dt == 0 {
                    0.0
                } else {
                    (busy.saturating_sub(*prev_busy)) as f32 / dt as f32
                }
            })
            .map(|u| u.clamp(0.0, 1.0))
            .collect();
        *prev = current;
        usage
    }

    fn ram_used_gb() -> f32 {
        let content = fs::read_to_string("/proc/meminfo").unwrap_or_default();
        let mut total_kb = 0u64;
        let mut avail_kb = 0u64;
        for line in content.lines() {
            if let Some(rest) = line.strip_prefix("MemTotal:") {
                total_kb = parse_kb(rest);
            } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
                avail_kb = parse_kb(rest);
            }
        }
        total_kb.saturating_sub(avail_kb) as f32 / (1024.0 * 1024.0)
    }

    /// (percent, discharging). Machines without a battery report (100, false)
    /// so BatteryCritical can never trigger on desktops.
    fn battery() -> (u8, bool) {
        let capacity = fs::read_to_string("/sys/class/power_supply/BAT0/capacity")
            .ok()
            .and_then(|s| s.trim().parse::<u8>().ok())
            .unwrap_or(100);
        let status = fs::read_to_string("/sys/class/power_supply/BAT0/status")
            .unwrap_or_default();
        (
            capacity.min(100),
            status.trim().eq_ignore_ascii_case("discharging"),
        )
    }

    /// Process names from /proc/<pid>/comm. Per-process CPU attribution
    /// arrives with the eBPF reader; v0 reports 0.0.
    fn process_list() -> Vec<ProcessInfo> {
        let mut procs = Vec::new();
        if let Ok(entries) = fs::read_dir("/proc") {
            for entry in entries.flatten() {
                let is_pid = entry
                    .file_name()
                    .to_string_lossy()
                    .parse::<u32>()
                    .is_ok();
                if is_pid {
                    if let Ok(comm) = fs::read_to_string(entry.path().join("comm")) {
                        procs.push(ProcessInfo {
                            name: comm.trim().to_string(),
                            cpu_percent: 0.0,
                        });
                    }
                }
            }
        }
        procs
    }
}

fn parse_kb(rest: &str) -> u64 {
    rest.trim()
        .trim_end_matches("kB")
        .trim()
        .parse()
        .unwrap_or(0)
}

impl EbpfReader for ProcfsReader {
    fn read_metrics(&self) -> SystemMetrics {
        let (battery_percent, battery_discharging) = Self::battery();
        SystemMetrics {
            cpu_usage_per_core: self.cpu_usage(),
            gpu_usage: 0.0,
            ram_used_gb: Self::ram_used_gb(),
            battery_percent,
            battery_discharging,
            process_list: Self::process_list(),
            foreground_app: String::new(),
            io_ops_per_second: 0,
            user_idle_seconds: 0,
            hour_of_day: chrono::Local::now().hour() as u8,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_reader_returns_its_metrics() {
        let reader = FixedReader(SystemMetrics {
            battery_percent: 42,
            ..Default::default()
        });
        assert_eq!(reader.read_metrics().battery_percent, 42);
    }

    #[test]
    fn parse_kb_handles_meminfo_format() {
        assert_eq!(parse_kb("      16384256 kB"), 16384256);
        assert_eq!(parse_kb("garbage"), 0);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn procfs_reader_produces_sane_values() {
        let reader = ProcfsReader::new();
        let m = reader.read_metrics();
        assert!(m.hour_of_day <= 23);
        assert!(m.battery_percent <= 100);
        for usage in &m.cpu_usage_per_core {
            assert!((0.0..=1.0).contains(usage));
        }
    }
}
