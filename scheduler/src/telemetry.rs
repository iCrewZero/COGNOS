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

    /// Detect the currently focused window's process name.
    ///
    /// Tries Wayland first (via org_kde_kwin Surface or the sway-ipc /
    /// wlr-foreign-toplevel protocol exposed as a D-Bus property), then
    /// falls back to X11 _NET_ACTIVE_WINDOW. If neither compositor is
    /// running, or the window title cannot be resolved to a pid, returns
    /// an empty string.
    fn foreground_app() -> String {
        // --- Wayland path ---
        // Try sway-ipc first (fastest, no D-Bus round-trip).
        if let Some(name) = foreground_via_sway() {
            return name;
        }

        // Try GNOME/KDE via D-Bus (org.kde.KWin / org.gnome.Shell).
        if let Some(name) = foreground_via_dbus() {
            return name;
        }

        // --- X11 fallback ---
        if let Some(name) = foreground_via_x11() {
            return name;
        }

        String::new()
    }
}

// ─── Foreground app detection helpers ─────────────────────────────────────────

/// Non-Unix fallback: sway/Wayland IPC uses a Unix domain socket, which does
/// not exist on non-Unix hosts. Detection is unavailable there.
#[cfg(not(unix))]
fn foreground_via_sway() -> Option<String> {
    None
}

/// Try to get the focused window's app name from sway via sway-ipc.
/// sway-ipc is the fastest path: a single UNIX socket message, no D-Bus.
#[cfg(unix)]
fn foreground_via_sway() -> Option<String> {
    use std::os::unix::net::UnixStream;
    use std::io::{Read, Write};

    // sway stores its IPC socket path in SWAYSOCK.
    let socket_path = std::env::var("SWAYSOCK").ok()?;

    let mut stream = UnixStream::connect(&socket_path).ok()?;

    // IPC magic header + message type 7 (GET_TREE) + payload length 0.
    let header: [u8; 13] = [
        b's', b'w', b'a', b'y', b'i', b'p', b'c', // magic
        0, 0, 0, 0, // payload length (little-endian u32)
        0, 0, // message type 7 = GET_TREE
    ];
    stream.write_all(&header).ok()?;
    stream.flush().ok()?;

    // Read the response. The first 6 bytes are the IPC header (magic +
    // payload length + type). We read up to 64 KiB of payload — the full
    // tree JSON is typically much smaller.
    let mut buf = [0u8; 65536];
    let n = stream.read(&mut buf).ok()?;
    if n < 6 {
        return None;
    }

    let payload = &buf[6..n];
    let tree: serde_json::Value = serde_json::from_slice(payload).ok()?;

    // Walk the tree to find the focused node.
    find_focused_in_sway_tree(&tree)
}

/// Recursively walk the sway IPC tree JSON to find the focused node's
/// app_id (which is the Wayland app-id / .desktop file name).
#[cfg(unix)]
fn find_focused_in_sway_tree(node: &serde_json::Value) -> Option<String> {
    // Check if this node is focused.
    if node.get("focused").and_then(|f| f.as_bool()).unwrap_or(false) {
        // Prefer app_id (Wayland app-id), fall back to window title class/name.
        if let Some(app_id) = node.get("app_id").and_then(|v| v.as_str()) {
            if !app_id.is_empty() {
                return Some(app_id.to_lowercase());
            }
        }
        if let Some(name) = node.get("name").and_then(|v| v.as_str()) {
            if !name.is_empty() {
                return Some(name.to_lowercase());
            }
        }
        // Try window_properties class
        if let Some(cls) = node
            .get("window_properties")
            .and_then(|wp| wp.get("class"))
            .and_then(|v| v.as_str())
        {
            if !cls.is_empty() {
                return Some(cls.to_lowercase());
            }
        }
    }

    // Recurse into children (floating, tiled, etc.).
    if let Some(nodes) = node.get("nodes").and_then(|n| n.as_array()) {
        for child in nodes {
            if let Some(found) = find_focused_in_sway_tree(child) {
                return Some(found);
            }
        }
    }
    // Also check "floating_nodes" — sway puts floating windows here.
    if let Some(nodes) = node.get("floating_nodes").and_then(|n| n.as_array()) {
        for child in nodes {
            if let Some(found) = find_focused_in_sway_tree(child) {
                return Some(found);
            }
        }
    }

    None
}

/// Try to get the focused window's app name via D-Bus.
/// Supports KDE KWin and GNOME Shell.
fn foreground_via_dbus() -> Option<String> {
    // Attempt KDE KWin first.
    if let Some(name) = foreground_via_kde() {
        return Some(name);
    }
    // Attempt GNOME Shell.
    if let Some(name) = foreground_via_gnome() {
        return Some(name);
    }
    None
}

/// Query KDE KWin for the active window's caption.
fn foreground_via_kde() -> Option<String> {
    let output = std::process::Command::new("dbus-send")
        .args([
            "--print-reply",
            "--dest=org.kde.KWin",
            "/KWin",
            "org.kde.KWin.queryWindowInfo",
        ])
        .output()
        .ok()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    // The reply contains lines like:
    //   string "caption"
    //   variant       string "Firefox — Some Title"
    // We parse the first "caption" value we find.
    let lines: Vec<&str> = stdout.lines().collect();
    for i in 0..lines.len().saturating_sub(1) {
        if lines[i].contains("caption") {
            // Next non-empty line should have the value.
            for j in (i + 1)..lines.len() {
                if let Some(rest) = lines[j].strip_prefix("      string \"") {
                    if let Some(end) = rest.find('"') {
                        let caption = &rest[..end];
                        // Extract just the app name (first word or before " — ").
                        let app = caption.split(" — ").next().unwrap_or(caption);
                        if !app.is_empty() {
                            return Some(app.to_lowercase());
                        }
                    }
                }
            }
        }
    }
    None
}

/// Query GNOME Shell for the active window's title via its D-Bus interface.
fn foreground_via_gnome() -> Option<String> {
    let output = std::process::Command::new("dbus-send")
        .args([
            "--print-reply",
            "--dest=org.gnome.Shell",
            "/org/gnome/Shell",
            "org.gnome.Shell.Eval",
            "string:global.display.get_focus_window().get_title()",
        ])
        .output()
        .ok()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    // The reply looks like:
    //   variant       string "\"Firefox — Some Title\""
    // Strip the outer quotes and JSON escaping.
    if let Some(start) = stdout.find("\"") {
        let rest = &stdout[start + 1..];
        if let Some(end) = rest.find('"') {
            let title = &rest[..end];
            // GNOME returns JSON-escaped strings; unescape the bare quotes.
            let title = title.replace("\\\"", "\"");
            let app = title.split(" — ").next().unwrap_or(&title);
            if !app.is_empty() {
                return Some(app.to_lowercase());
            }
        }
    }
    None
}

/// X11 fallback: use xdotool or xprop to get the active window's class.
fn foreground_via_x11() -> Option<String> {
    // Try xdotool getactivewindow getwindowname first.
    if let Ok(output) = std::process::Command::new("xdotool")
        .args(["getactivewindow", "getwindowname"])
        .output()
    {
        let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !name.is_empty() {
            let app = name.split(" — ").next().unwrap_or(&name);
            if !app.is_empty() {
                return Some(app.to_lowercase());
            }
        }
    }

    // Fallback: xprop _NET_ACTIVE_WINDOW → WM_CLASS.
    if let Ok(output) = std::process::Command::new("xprop")
        .args(["-root", "_NET_ACTIVE_WINDOW", "notype"])
        .output()
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        // Parse the window id from: _NET_ACTIVE_WINDOW(WINDOW): window id # 0x1234567
        if let Some(hex) = stdout.split("0x").nth(1) {
            let hex = hex.split_whitespace().next().unwrap_or("");
            if let Ok(_) = u64::from_str_radix(hex, 16) {
                let win_id = format!("0x{}", hex);
                // Now get WM_CLASS for this window.
                if let Ok(class_output) = std::process::Command::new("xprop")
                    .args(["-id", &win_id, "WM_CLASS"])
                    .output()
                {
                    let class_str = String::from_utf8_lossy(&class_output.stdout);
                    // WM_CLASS(STRING) = "firefox", "Firefox"
                    if let Some(rest) = class_str.split('"').nth(1) {
                        let class = rest.split('"').next().unwrap_or("");
                        if !class.is_empty() {
                            return Some(class.to_lowercase());
                        }
                    }
                }
            }
        }
    }

    None
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
            foreground_app: Self::foreground_app(),
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
