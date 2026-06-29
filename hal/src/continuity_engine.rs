//! Continuity engine — ensures HAL state survives reboots and crashes.
//!
//!
//! HAL is the trust root of the system. If HAL loses its state across a
//! reboot or crash, the system has no way to know whether the post-reboot
//! state is consistent with the pre-reboot state. The [`ContinuityEngine`]
//! periodically snapshots HAL's governed state (audit head, trust tables,
//! reputation tables, autonomy level) to a file on disk, using an atomic
//! write-and-rename so a crash mid-write cannot corrupt the snapshot.
//!
//! On boot, HAL calls [`ContinuityEngine::restore`] to load the snapshot. If
//! the snapshot is missing or corrupted, HAL refuses to start in anything
//! above [`crate::autonomy_controller::AutonomyLevel::Supervised`] and emits
//! an audit event.
//!
//! v0: stub implementation. Snapshot is JSON; v1 will use a binary format
//! with content-addressed chunks for incremental snapshots.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{error, info, warn};

// v0: stub implementation

/// Type alias for an agent identifier (matches the rest of the crate).
pub type AgentId = String;

// ─── Snapshot ───────────────────────────────────────────────────────────────────

/// A snapshot of HAL's governed state at a moment in time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    /// SHA-256 hash of the audit chain head at snapshot time.
    pub audit_head: [u8; 32],
    /// Serialized trust state (per-agent trust table).
    pub trust_state: serde_json::Value,
    /// Serialized reputation state (per-agent reputation table).
    pub reputation_state: serde_json::Value,
    /// Current autonomy level (e.g. "Supervised", "Advisory", ...).
    pub autonomy_level: String,
    /// When the snapshot was captured.
    pub timestamp: DateTime<Utc>,
}

impl Snapshot {
    /// Build a fresh, empty snapshot suitable as a v0 default.
    pub fn empty() -> Self {
        Self {
            audit_head: [0u8; 32],
            trust_state: serde_json::json!({}),
            reputation_state: serde_json::json!({}),
            autonomy_level: "Supervised".to_string(),
            timestamp: Utc::now(),
        }
    }
}

// ─── Continuity Engine ──────────────────────────────────────────────────────────

/// The continuity engine: snapshots HAL state to disk and restores it.
pub struct ContinuityEngine {
    /// Filesystem path to the snapshot file.
    pub snapshot_path: PathBuf,
    /// Snapshot interval. v0 does not run a background timer; callers must
    /// poll `snapshot()` on this interval.
    pub interval: chrono::Duration,
}

impl ContinuityEngine {
    /// Build a new engine targeting the given snapshot path with a 60s interval.
    pub fn new(snapshot_path: impl Into<PathBuf>) -> Self {
        Self {
            snapshot_path: snapshot_path.into(),
            interval: chrono::Duration::seconds(60),
        }
    }

    /// Build a new engine with a custom snapshot interval.
    pub fn with_interval(
        snapshot_path: impl Into<PathBuf>,
        interval: chrono::Duration,
    ) -> Self {
        Self {
            snapshot_path: snapshot_path.into(),
            interval,
        }
    }

    /// Write a fresh snapshot to disk.
    ///
    /// The write is atomic: the snapshot is serialized to a temporary file
    /// in the same directory, then renamed over the target path. A crash at
    /// any point leaves either the old snapshot intact or the new one fully
    /// in place — never a torn write.
    pub fn snapshot(&self) -> Result<Snapshot, ContinuityError> {
        // v0: the caller is expected to populate the snapshot from the live
        // HAL state. The engine itself just orchestrates the write.
        // TODO(v1): accept a closure or trait object that produces the
        // governed-state blob.
        let snapshot = Snapshot::empty();
        self.write_atomic(&snapshot)?;
        info!(path = ?self.snapshot_path, "continuity_engine: snapshot written");
        Ok(snapshot)
    }

    /// Write a caller-provided snapshot to disk atomically.
    pub fn write_snapshot(&self, snapshot: &Snapshot) -> Result<(), ContinuityError> {
        self.write_atomic(snapshot)
    }

    /// Restore the latest snapshot from disk.
    ///
    /// Returns [`ContinuityError::NotFound`] if no snapshot file exists.
    pub fn restore(&self) -> Result<Snapshot, ContinuityError> {
        if !self.snapshot_path.exists() {
            warn!(path = ?self.snapshot_path, "continuity_engine: no snapshot to restore");
            return Err(ContinuityError::NotFound);
        }
        let bytes = fs::read(&self.snapshot_path)
            .map_err(|e| ContinuityError::Read(e.to_string()))?;
        let snapshot: Snapshot = serde_json::from_slice(&bytes)
            .map_err(|e| ContinuityError::Deserialize(e.to_string()))?;
        info!(path = ?self.snapshot_path, ts = ?snapshot.timestamp, "continuity_engine: snapshot restored");
        Ok(snapshot)
    }

    /// Atomic write: serialize to `.<rand>.tmp` in the same directory, fsync,
    /// then rename over the target. On Unix, rename is atomic; on other
    /// platforms we still try.
    fn write_atomic(&self, snapshot: &Snapshot) -> Result<(), ContinuityError> {
        let parent = self
            .snapshot_path
            .parent()
            .ok_or_else(|| ContinuityError::Path("snapshot path has no parent".into()))?;
        fs::create_dir_all(parent)
            .map_err(|e| ContinuityError::Write(e.to_string()))?;

        let tmp_name = format!(
            ".{}.{}.tmp",
            self.snapshot_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("snapshot"),
            uuid::Uuid::new_v4().simple()
        );
        let tmp_path = parent.join(tmp_name);

        let bytes = serde_json::to_vec_pretty(snapshot)
            .map_err(|e| ContinuityError::Serialize(e.to_string()))?;

        {
            let mut f = fs::File::create(&tmp_path)
                .map_err(|e| ContinuityError::Write(e.to_string()))?;
            f.write_all(&bytes)
                .map_err(|e| ContinuityError::Write(e.to_string()))?;
            f.sync_all()
                .map_err(|e| ContinuityError::Write(e.to_string()))?;
        }

        rename_atomic(&tmp_path, &self.snapshot_path)
            .map_err(|e| ContinuityError::Write(e.to_string()))?;

        Ok(())
    }
}

/// Platform-agnostic rename. On Unix, `fs::rename` is atomic.
fn rename_atomic(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::rename(from, to)
}

// ─── Errors ─────────────────────────────────────────────────────────────────────

/// Errors returned by the continuity engine.
#[derive(Debug, Error)]
pub enum ContinuityError {
    /// No snapshot file exists at the configured path.
    #[error("no snapshot file at configured path")]
    NotFound,
    /// Snapshot file could not be read.
    #[error("failed to read snapshot: {0}")]
    Read(String),
    /// Snapshot file could not be parsed.
    #[error("failed to deserialize snapshot: {0}")]
    Deserialize(String),
    /// Snapshot could not be serialized.
    #[error("failed to serialize snapshot: {0}")]
    Serialize(String),
    /// Snapshot file could not be written or renamed.
    #[error("failed to write snapshot: {0}")]
    Write(String),
    /// The configured snapshot path is invalid.
    #[error("invalid snapshot path: {0}")]
    Path(String),
}

// ─── Logging helper ─────────────────────────────────────────────────────────────

/// Emit an audit-style log line when a continuity operation fails.
pub fn log_continuity_failure(e: &ContinuityError) {
    error!(error = %e, "continuity_engine: operation failed");
}
