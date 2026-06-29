//! Self-preservation — protects the HAL itself from tampering.
//!
//!
//! The HAL is the trust root of the system. If an attacker (or a
//! compromised agent) can modify HAL's policy files, binary, or audit
//! chain, every other guarantee collapses. This module enforces three
//! invariants:
//!
//!   1. **Integrity**: HAL's on-disk files hash to their known-good values.
//!      [`SelfPreservation::verify_integrity`] walks every registered file
//!      and re-hashes it.
//!
//!   2. **Watch**: critical paths are watched for in-flight modification.
//!      [`SelfPreservation::watch_path`] registers a path for inotify-style
//!      monitoring (v0: stub; v1 wires inotify).
//!
//!   3. **Lockdown**: if tampering is detected,
//!      [`SelfPreservation::emergency_lockdown`] freezes the HAL into a
//!      deny-by-default state until a human reviews. AI agents CANNOT
//!      lift a lockdown — by construction.
//!
//! HAL modifications require human-only review. The AI cannot modify HAL:
//! this is enforced structurally by the capability lattice (`ModifyHal`
//! is denied to every agent) and defensively by this module.
//!
//! v0: stub implementation. Integrity hashes are stored and verified;
//! inotify watchers are TODO(v1).

use std::collections::HashMap;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tracing::{debug, error, info, warn};

// v0: stub implementation

// ─── Tamper Errors ──────────────────────────────────────────────────────────────

/// Errors returned by the self-preservation module.
#[derive(Debug, Error)]
pub enum TamperError {
    /// A watched file's hash does not match its baseline.
    #[error("integrity check failed for {path}: expected {expected}, got {actual}")]
    HashMismatch {
        /// Path of the modified file.
        path: PathBuf,
        /// Expected SHA-256 (hex).
        expected: String,
        /// Actual SHA-256 (hex).
        actual: String,
    },
    /// A watched file is missing.
    #[error("watched file missing: {0}")]
    MissingFile(PathBuf),
    /// A watcher could not be installed.
    #[error("watcher install failed for {0}: {1}")]
    WatcherInstall(PathBuf, String),
    /// The HAL is in lockdown and cannot process requests.
    #[error("HAL is in lockdown since {0} — human review required")]
    LockedDown(DateTime<Utc>),
}

// ─── Lockdown State ─────────────────────────────────────────────────────────────

/// The lockdown state of the HAL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LockdownState {
    /// HAL is operating normally.
    Normal,
    /// HAL is in lockdown. All gated actions are denied; only
    /// human-initiated review operations may proceed.
    LockedDown,
}

impl Default for LockdownState {
    fn default() -> Self {
        Self::Normal
    }
}

// ─── Integrity Record ───────────────────────────────────────────────────────────

/// Recorded hash for a watched path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrityRecord {
    /// The watched path.
    pub path: PathBuf,
    /// Expected SHA-256 hash (hex).
    pub expected_hash: String,
    /// When the baseline was recorded.
    pub baseline_at: DateTime<Utc>,
    /// Whether a watcher (inotify) is installed for this path.
    pub watched: bool,
}

// ─── Self-Preservation Engine ───────────────────────────────────────────────────

/// The self-preservation engine. Owns integrity baselines and watchers.
#[derive(Debug, Default)]
pub struct SelfPreservation {
    /// Known-good hashes for HAL-critical files.
    integrity_hashes: HashMap<PathBuf, IntegrityRecord>,
    /// Paths currently being watched (v0: stored, not actually watched).
    watchers: Vec<PathBuf>,
    /// Current lockdown state.
    lockdown: LockdownState,
}

impl SelfPreservation {
    /// Construct a new engine with no baselines.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a path for integrity monitoring. The current hash is
    /// captured as the baseline.
    pub fn register_path(&mut self, path: PathBuf) -> Result<(), TamperError> {
        let hash = Self::hash_file(&path)?;
        let record = IntegrityRecord {
            path: path.clone(),
            expected_hash: hash,
            baseline_at: Utc::now(),
            watched: false,
        };
        info!(path = %path.display(), "integrity baseline recorded");
        self.integrity_hashes.insert(path, record);
        Ok(())
    }

    /// Watch a path for in-flight modification (v0: stub).
    pub fn watch_path(&mut self, path: PathBuf) {
        // TODO(v1): wire inotify (or fanotify) for real-time tamper alerts.
        if !self.watchers.contains(&path) {
            self.watchers.push(path.clone());
            debug!(path = %path.display(), "watcher registered (v0 stub)");
        }
    }

    /// Verify the integrity of all registered paths. Returns the first
    /// failure (or Ok(()) if all hashes match).
    pub fn verify_integrity(&self) -> Result<(), TamperError> {
        for record in self.integrity_hashes.values() {
            let actual = Self::hash_file(&record.path)?;
            if actual != record.expected_hash {
                error!(path = %record.path.display(), "integrity check failed");
                return Err(TamperError::HashMismatch {
                    path: record.path.clone(),
                    expected: record.expected_hash.clone(),
                    actual,
                });
            }
        }
        Ok(())
    }

    /// Trigger an emergency lockdown. Returns the new state.
    pub fn emergency_lockdown(&mut self) -> LockdownState {
        if self.lockdown == LockdownState::Normal {
            error!("HAL entering emergency lockdown");
            self.lockdown = LockdownState::LockedDown;
        } else {
            warn!("lockdown already active");
        }
        self.lockdown
    }

    /// Release the lockdown. This MUST be gated by a human-only review
    /// path — the HAL must never accept a release request from an AI agent.
    ///
    /// v0: this function is callable from anywhere; v1 will require a
    /// hardware-attested human-confirmation token.
    pub fn release_lockdown(&mut self) -> LockdownState {
        // TODO(v1): require a human-only confirmation token (e.g. signed
        // by a hardware key whose private key never leaves the device).
        warn!("lockdown release requested — v0 accepts (v1 will require human token)");
        self.lockdown = LockdownState::Normal;
        self.lockdown
    }

    /// Current lockdown state.
    pub fn lockdown_state(&self) -> LockdownState {
        self.lockdown
    }

    /// Borrow the integrity records (for audit / display).
    pub fn integrity_records(&self) -> &HashMap<PathBuf, IntegrityRecord> {
        &self.integrity_hashes
    }

    /// Borrow the watcher list (for audit / display).
    pub fn watchers(&self) -> &[PathBuf] {
        &self.watchers
    }

    /// SHA-256 a file. Returns hex string. Errors if the file is missing
    /// or unreadable.
    fn hash_file(path: &PathBuf) -> Result<String, TamperError> {
        let bytes =
            std::fs::read(path).map_err(|_| TamperError::MissingFile(path.clone()))?;
        let digest = Sha256::digest(&bytes);
        Ok(hex::encode(digest))
    }
}
