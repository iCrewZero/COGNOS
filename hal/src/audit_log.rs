/// Audit Log System for COGNOS/OS.
///
/// Every AI action is logged here in plain text + JSON lines.
/// Tamper-evident via chained SHA-256 hashes.
/// The user can read, verify, export, or wipe it at any time.

use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

const MAX_LOG_SIZE_BYTES: u64 = 50 * 1024 * 1024; // 50 MB
const MAX_ROTATED_FILES: u32 = 5;
const FLUSH_INTERVAL: Duration = Duration::from_millis(100);
const FLUSH_ENTRY_COUNT: usize = 50;
const CHAIN_FILE: &str = ".cognos/audit.chain";
const INITIAL_HASH_INPUT: &[u8] = b"cognos-audit-v1";

// ─── Types ────────────────────────────────────────────────────────────────────

/// A single audit log entry. All fields are optional except the required set.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    /// ISO 8601 timestamp (required)
    pub ts: DateTime<Utc>,
    /// Agent that performed the action (required)
    pub agent: String,
    /// Action performed (required)
    pub action: String,
    /// Outcome: "success" | "denied" | "error" (required)
    pub outcome: String,
    /// Target file, path, or resource (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// HAL risk score for this action (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hal_score: Option<f32>,
    /// HAL level: "silent" | "notify" | "confirm" | "block" (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hal_level: Option<String>,
    /// Intent ID that triggered this action (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intent_id: Option<Uuid>,
    /// Whether the action can be undone (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reversible: Option<bool>,
    /// Human-readable note (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Error message if outcome is "error" (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    /// Session ID (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// Tamper-evident chain hash
    pub chain_hash: String,
}

/// Filter for querying log entries.
#[derive(Debug, Default)]
pub struct AuditFilter {
    pub agent: Option<String>,
    pub action: Option<String>,
    pub since: Option<DateTime<Utc>>,
    pub intent_id: Option<Uuid>,
    pub hal_level: Option<String>,
    pub target_contains: Option<String>,
}

// ─── AuditLog ─────────────────────────────────────────────────────────────────

struct Inner {
    writer: BufWriter<std::fs::File>,
    pending: Vec<AuditEntry>,
    last_flush: std::time::Instant,
    previous_hash: String,
    log_path: PathBuf,
    chain_path: PathBuf,
}

/// The main audit log. Thread-safe via Arc<Mutex>.
#[derive(Clone)]
pub struct AuditLog {
    inner: Arc<Mutex<Inner>>,
}

impl AuditLog {
    /// Open or create the audit log at the default location.
    pub fn open() -> std::io::Result<Self> {
        let home = dirs::home_dir().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "No home directory")
        })?;
        Self::open_at(home.join(".cognos/audit.log"))
    }

    /// Open at a specific path (useful for testing).
    pub fn open_at(log_path: PathBuf) -> std::io::Result<Self> {
        if let Some(parent) = log_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let chain_path = log_path
            .parent()
            .unwrap_or(Path::new("/tmp"))
            .join(CHAIN_FILE.split('/').last().unwrap_or("audit.chain"));

        // Load previous chain tip or initialise
        let previous_hash = Self::load_chain_tip(&chain_path);

        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)?;

        Ok(Self {
            inner: Arc::new(Mutex::new(Inner {
                writer: BufWriter::new(file),
                pending: Vec::new(),
                last_flush: std::time::Instant::now(),
                previous_hash,
                log_path,
                chain_path,
            })),
        })
    }

    // ─── Writing ─────────────────────────────────────────────────────────────

    /// Append an entry to the log. Non-blocking; entries are buffered.
    pub fn append(
        &self,
        agent: &str,
        action: &str,
        outcome: &str,
        target: Option<&str>,
        hal_score: Option<f32>,
        hal_level: Option<&str>,
        intent_id: Option<Uuid>,
        reversible: Option<bool>,
        note: Option<&str>,
        session: Option<&str>,
    ) {
        let entry = self.build_entry(
            agent, action, outcome, target, hal_score,
            hal_level, intent_id, reversible, note, session,
        );

        if let Ok(mut inner) = self.inner.lock() {
            inner.pending.push(entry);
            let should_flush = inner.pending.len() >= FLUSH_ENTRY_COUNT
                || inner.last_flush.elapsed() >= FLUSH_INTERVAL;
            if should_flush {
                Self::flush_inner(&mut inner);
            }
        } else {
            eprintln!("[audit] Lock poisoned — entry lost");
        }
    }

    /// Force-flush all pending entries immediately (e.g. on shutdown).
    pub fn flush(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            Self::flush_inner(&mut inner);
        }
    }

    // ─── Querying ────────────────────────────────────────────────────────────

    /// Read all entries matching the filter.
    pub fn query(&self, filter: &AuditFilter) -> Vec<AuditEntry> {
        self.flush(); // ensure pending entries are on disk

        let path = {
            let inner = self.inner.lock().unwrap();
            inner.log_path.clone()
        };

        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => return vec![],
        };

        content
            .lines()
            .filter_map(|line| serde_json::from_str::<AuditEntry>(line).ok())
            .filter(|entry| Self::matches_filter(entry, filter))
            .collect()
    }

    // ─── User commands ────────────────────────────────────────────────────────

    /// Print formatted table of matching entries to stdout.
    pub fn show(&self, filter: &AuditFilter) {
        let entries = self.query(filter);
        println!(
            "{:<26} {:<12} {:<20} {:<12} {}",
            "TIMESTAMP", "AGENT", "ACTION", "OUTCOME", "TARGET"
        );
        println!("{}", "-".repeat(90));
        for e in &entries {
            println!(
                "{:<26} {:<12} {:<20} {:<12} {}",
                e.ts.format("%Y-%m-%dT%H:%M:%S"),
                e.agent,
                e.action,
                e.outcome,
                e.target.as_deref().unwrap_or("—"),
            );
        }
        println!("\n{} entries", entries.len());
    }

    /// Verify the chain integrity. Reports any tampered entries.
    pub fn verify(&self) -> VerifyResult {
        self.flush();

        let path = {
            let inner = self.inner.lock().unwrap();
            inner.log_path.clone()
        };

        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                return VerifyResult {
                    valid: false,
                    broken_at: None,
                    error: Some(e.to_string()),
                }
            }
        };

        let mut prev_hash = hex::encode(Sha256::digest(INITIAL_HASH_INPUT));
        let mut line_num = 0u64;

        for raw_line in content.lines() {
            line_num += 1;
            let entry: AuditEntry = match serde_json::from_str(raw_line) {
                Ok(e) => e,
                Err(_) => continue,
            };

            // Recompute expected hash
            let entry_without_hash = {
                let mut e = entry.clone();
                e.chain_hash = String::new();
                serde_json::to_string(&e).unwrap_or_default()
            };
            let expected = hex::encode(Sha256::digest(
                format!("{}{}", prev_hash, entry_without_hash).as_bytes(),
            ));

            if entry.chain_hash != expected {
                return VerifyResult {
                    valid: false,
                    broken_at: Some(line_num),
                    error: Some(format!(
                        "Chain broken at entry {} (agent={}, action={})",
                        line_num, entry.agent, entry.action
                    )),
                };
            }
            prev_hash = entry.chain_hash.clone();
        }

        VerifyResult { valid: true, broken_at: None, error: None }
    }

    /// Export log to a user-specified path.
    pub fn export(&self, dest: &Path) -> std::io::Result<()> {
        self.flush();
        let src = {
            let inner = self.inner.lock().unwrap();
            inner.log_path.clone()
        };
        std::fs::copy(&src, dest)?;
        Ok(())
    }

    /// Wipe all logs. Should only be called after HAL confirmation (score 0.9).
    pub fn wipe(&self) -> std::io::Result<()> {
        if let Ok(mut inner) = self.inner.lock() {
            inner.pending.clear();

            // Rotate existing file
            let log = inner.log_path.clone();
            if log.exists() {
                let dest = log.with_extension("wiped");
                let _ = std::fs::rename(&log, dest);
            }

            // Open fresh file
            let file = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .open(&inner.log_path)?;
            inner.writer = BufWriter::new(file);
            inner.previous_hash =
                hex::encode(Sha256::digest(INITIAL_HASH_INPUT));

            let _ = std::fs::remove_file(&inner.chain_path);
        }
        Ok(())
    }

    // ─── Private ─────────────────────────────────────────────────────────────

    fn build_entry(
        &self,
        agent: &str,
        action: &str,
        outcome: &str,
        target: Option<&str>,
        hal_score: Option<f32>,
        hal_level: Option<&str>,
        intent_id: Option<Uuid>,
        reversible: Option<bool>,
        note: Option<&str>,
        session: Option<&str>,
    ) -> AuditEntry {
        let mut inner = self.inner.lock().unwrap();

        // Build entry without chain_hash first
        let mut entry = AuditEntry {
            ts: Utc::now(),
            agent: agent.to_string(),
            action: action.to_string(),
            outcome: outcome.to_string(),
            target: target.map(str::to_string),
            hal_score,
            hal_level: hal_level.map(str::to_string),
            intent_id,
            reversible,
            note: note.map(str::to_string),
            error_message: None,
            session: session.map(str::to_string),
            chain_hash: String::new(),
        };

        // Compute chain hash
        let entry_json = serde_json::to_string(&entry).unwrap_or_default();
        let chain_hash = hex::encode(Sha256::digest(
            format!("{}{}", inner.previous_hash, entry_json).as_bytes(),
        ));
        entry.chain_hash = chain_hash.clone();
        inner.previous_hash = chain_hash;

        entry
    }

    fn flush_inner(inner: &mut Inner) {
        if inner.pending.is_empty() {
            return;
        }

        // Rotate if needed
        if let Ok(meta) = inner.log_path.metadata() {
            if meta.len() > MAX_LOG_SIZE_BYTES {
                Self::rotate_static(inner);
            }
        }

        for entry in inner.pending.drain(..) {
            if let Ok(line) = serde_json::to_string(&entry) {
                let _ = writeln!(inner.writer, "{}", line);
            }
        }
        let _ = inner.writer.flush();

        // Persist chain tip
        let _ = std::fs::write(&inner.chain_path, &inner.previous_hash);
        inner.last_flush = std::time::Instant::now();
    }

    fn rotate_static(inner: &mut Inner) {
        let log = &inner.log_path;

        // Shift existing rotated files: .4 → deleted, .3→.4, .2→.3, etc.
        for i in (1..MAX_ROTATED_FILES).rev() {
            let old = log.with_extension(format!("log.{}", i));
            let new = log.with_extension(format!("log.{}", i + 1));
            if old.exists() {
                if i + 1 > MAX_ROTATED_FILES {
                    let _ = std::fs::remove_file(&old);
                } else {
                    let _ = std::fs::rename(&old, &new);
                }
            }
        }
        let _ = std::fs::rename(log, log.with_extension("log.1"));

        // Open fresh file
        if let Ok(file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log)
        {
            inner.writer = BufWriter::new(file);
        }
    }

    fn matches_filter(entry: &AuditEntry, filter: &AuditFilter) -> bool {
        if let Some(ref agent) = filter.agent {
            if &entry.agent != agent {
                return false;
            }
        }
        if let Some(ref action) = filter.action {
            if &entry.action != action {
                return false;
            }
        }
        if let Some(since) = filter.since {
            if entry.ts < since {
                return false;
            }
        }
        if let Some(ref id) = filter.intent_id {
            if entry.intent_id.as_ref() != Some(id) {
                return false;
            }
        }
        if let Some(ref level) = filter.hal_level {
            if entry.hal_level.as_ref() != Some(level) {
                return false;
            }
        }
        if let Some(ref needle) = filter.target_contains {
            if !entry.target.as_deref().unwrap_or("").contains(needle.as_str()) {
                return false;
            }
        }
        true
    }

    fn load_chain_tip(path: &Path) -> String {
        std::fs::read_to_string(path)
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| hex::encode(Sha256::digest(INITIAL_HASH_INPUT)))
    }
}

#[derive(Debug)]
pub struct VerifyResult {
    pub valid: bool,
    pub broken_at: Option<u64>,
    pub error: Option<String>,
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn test_log() -> (AuditLog, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let log = AuditLog::open_at(dir.path().join("audit.log")).unwrap();
        (log, dir)
    }

    #[test]
    fn write_and_read_entry() {
        let (log, _dir) = test_log();
        log.append("file_agent", "open_file", "success",
                   Some("~/motor.py"), Some(0.12), Some("silent"),
                   None, Some(true), Some("opened at line 47"), None);
        log.flush();

        let entries = log.query(&AuditFilter::default());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].agent, "file_agent");
        assert_eq!(entries[0].action, "open_file");
    }

    #[test]
    fn chain_integrity_passes_on_fresh_log() {
        let (log, _dir) = test_log();
        log.append("memory", "index_file", "success",
                   Some("~/motor.py"), None, None, None, None, None, None);
        log.append("hal", "gate_request", "approved",
                   Some("install_pkg"), Some(0.65), Some("notify"),
                   None, Some(false), None, None);
        log.flush();

        let result = log.verify();
        assert!(result.valid, "Chain verification failed: {:?}", result.error);
    }

    #[test]
    fn tampered_entry_detected() {
        let (log, dir) = test_log();
        log.append("file_agent", "delete_file", "denied",
                   Some("~/motor.py"), Some(0.9), Some("block"),
                   None, None, None, None);
        log.flush();

        // Tamper with the log file
        let log_path = dir.path().join("audit.log");
        let content = std::fs::read_to_string(&log_path).unwrap();
        let tampered = content.replace("denied", "success");
        std::fs::write(&log_path, tampered).unwrap();

        let result = log.verify();
        assert!(!result.valid, "Tampered log should fail verification");
    }

    #[test]
    fn filter_by_agent() {
        let (log, _dir) = test_log();
        log.append("file_agent", "open", "success", None, None, None, None, None, None, None);
        log.append("memory", "index", "success", None, None, None, None, None, None, None);
        log.append("file_agent", "move", "success", None, None, None, None, None, None, None);
        log.flush();

        let entries = log.query(&AuditFilter {
            agent: Some("file_agent".to_string()),
            ..Default::default()
        });
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|e| e.agent == "file_agent"));
    }

    #[test]
    fn wipe_clears_all_entries() {
        let (log, _dir) = test_log();
        log.append("agent", "action", "success", None, None, None, None, None, None, None);
        log.flush();
        log.wipe().unwrap();

        let entries = log.query(&AuditFilter::default());
        assert!(entries.is_empty());
    }
}
