//! Audit chain — tamper-evident hash-chained audit log.
//!
//!
//! Extends [`crate::audit_log`] with an in-memory hash chain over a richer
//! entry type. While `audit_log` writes JSONL to disk and chains via the
//! `chain_hash` field, this module adds:
//!   - a typed [`ChainedEntry`] with explicit `prev_hash` and `entry_hash`,
//!   - a `verify()` that returns a structured [`VerifyResult`] with the
//!     broken-at index,
//!   - an `export()` that writes a JSONL file suitable for offline review.
//!
//! The chain uses SHA-256 over the canonical JSON of the entry plus the
//! previous entry's hash. Genesis hash is the SHA-256 of
//! `b"cognos-audit-v1"`, matching `audit_log::INITIAL_HASH_INPUT`.
//!
//! v0: stub implementation. Hash computation is in place; persistence
//! across restarts is TODO(v1).

use std::io::Write;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tracing::{debug, info};
use uuid::Uuid;

// v0: stub implementation

/// Genesis hash seed. Matches `audit_log::INITIAL_HASH_INPUT`.
const GENESIS_SEED: &[u8] = b"cognos-audit-v1";

// ─── Audit Entry ────────────────────────────────────────────────────────────────

/// A single audit entry, before chaining.
///
/// This is a typed surface for the chain module; the on-disk
/// `audit_log::AuditEntry` is a strict superset and conversion is trivial.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    /// Unique entry ID.
    pub id: Uuid,
    /// ISO 8601 timestamp.
    pub ts: DateTime<Utc>,
    /// Acting agent.
    pub agent: String,
    /// Action performed.
    pub action: String,
    /// HAL risk score for this action, if scored.
    pub hal_score: Option<f32>,
    /// HAL level for this action, if scored.
    pub hal_level: Option<String>,
    /// Free-form metadata (target, intent_id, etc.).
    pub metadata: std::collections::HashMap<String, String>,
}

// ─── Chained Entry ──────────────────────────────────────────────────────────────

/// An audit entry plus its chaining fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainedEntry {
    /// Monotonic sequence number, starting at 0.
    pub seq: u64,
    /// Hash of the previous entry (genesis hash for seq=0).
    pub prev_hash: [u8; 32],
    /// The wrapped audit entry.
    pub entry: AuditEntry,
    /// SHA-256 of `prev_hash || canonical_json(entry)`.
    pub entry_hash: [u8; 32],
}

// ─── Verify Result ──────────────────────────────────────────────────────────────

/// Result of a chain verification pass.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyResult {
    /// True iff every link in the chain hashes correctly.
    pub valid: bool,
    /// If invalid, the sequence number of the first broken link.
    pub broken_at: Option<u64>,
    /// Human-readable error message, if invalid.
    pub error: Option<String>,
    /// Total number of entries inspected.
    pub entries_inspected: u64,
}

// ─── Chain Errors ───────────────────────────────────────────────────────────────

/// Errors returned by the audit chain.
#[derive(Debug, Error)]
pub enum ChainError {
    /// An I/O error occurred while appending or exporting.
    #[error("audit chain I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// An entry failed to serialize.
    #[error("audit chain serialization error: {0}")]
    Serialize(String),
    /// The chain head hash did not match the expected value.
    #[error("chain head hash mismatch")]
    HeadHashMismatch,
}

// ─── Audit Chain ────────────────────────────────────────────────────────────────

/// The in-memory hash-chained audit log.
#[derive(Debug, Default)]
pub struct AuditChain {
    entries: Vec<ChainedEntry>,
    head_hash: [u8; 32],
}

impl AuditChain {
    /// Construct a new empty chain with the genesis head.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            head_hash: genesis_hash(),
        }
    }

    /// Append an entry to the chain. Returns the assigned sequence number.
    pub fn append(&mut self, entry: AuditEntry) -> Result<u64, ChainError> {
        let seq = self.entries.len() as u64;
        let prev_hash = self.head_hash;
        let entry_hash = compute_hash(&prev_hash, &entry)?;
        let chained = ChainedEntry {
            seq,
            prev_hash,
            entry,
            entry_hash,
        };
        self.head_hash = entry_hash;
        self.entries.push(chained);
        debug!(seq, "audit entry appended");
        Ok(seq)
    }

    /// Verify the full chain end-to-end. Returns a structured result.
    pub fn verify(&self) -> VerifyResult {
        let mut prev_hash = genesis_hash();
        let mut inspected = 0u64;

        for chained in &self.entries {
            inspected += 1;
            // 1. The stored prev_hash must equal the running hash.
            if chained.prev_hash != prev_hash {
                return VerifyResult {
                    valid: false,
                    broken_at: Some(chained.seq),
                    error: Some(format!(
                        "prev_hash mismatch at seq={} (expected {}, got {})",
                        chained.seq,
                        hex_hash(&prev_hash),
                        hex_hash(&chained.prev_hash)
                    )),
                    entries_inspected: inspected,
                };
            }
            // 2. The stored entry_hash must match a recomputation.
            match compute_hash(&chained.prev_hash, &chained.entry) {
                Ok(recomputed) if recomputed == chained.entry_hash => {
                    prev_hash = chained.entry_hash;
                }
                Ok(_recomputed) => {
                    return VerifyResult {
                        valid: false,
                        broken_at: Some(chained.seq),
                        error: Some(format!(
                            "entry_hash mismatch at seq={}",
                            chained.seq
                        )),
                        entries_inspected: inspected,
                    };
                }
                Err(e) => {
                    return VerifyResult {
                        valid: false,
                        broken_at: Some(chained.seq),
                        error: Some(format!(
                            "serialize error at seq={}: {}",
                            chained.seq, e
                        )),
                        entries_inspected: inspected,
                    };
                }
            }
        }

        VerifyResult {
            valid: true,
            broken_at: None,
            error: None,
            entries_inspected: inspected,
        }
    }

    /// Export the chain to a JSONL file at `path`.
    pub fn export(&self, path: &Path) -> Result<(), ChainError> {
        let mut file = std::fs::File::create(path)?;
        for chained in &self.entries {
            let line = serde_json::to_string(chained)
                .map_err(|e| ChainError::Serialize(e.to_string()))?;
            writeln!(file, "{}", line)?;
        }
        info!(path = %path.display(), entries = self.entries.len(), "audit chain exported");
        Ok(())
    }

    /// Borrow the entries (for in-memory replay/forensics).
    pub fn entries(&self) -> &[ChainedEntry] {
        &self.entries
    }

    /// Current head hash (for cross-component attestation).
    pub fn head_hash(&self) -> [u8; 32] {
        self.head_hash
    }

    /// Number of entries in the chain.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the chain is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

// ─── Hash Helpers ───────────────────────────────────────────────────────────────

/// Compute the genesis hash: SHA-256 of the genesis seed.
fn genesis_hash() -> [u8; 32] {
    let digest = Sha256::digest(GENESIS_SEED);
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

/// Compute the entry hash: SHA-256 of `prev_hash || canonical_json(entry)`.
fn compute_hash(prev_hash: &[u8; 32], entry: &AuditEntry) -> Result<[u8; 32], ChainError> {
    let json = serde_json::to_string(entry)
        .map_err(|e| ChainError::Serialize(e.to_string()))?;
    let mut hasher = Sha256::new();
    hasher.update(prev_hash);
    hasher.update(json.as_bytes());
    let digest = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    Ok(out)
}

/// Hex-encode a 32-byte hash (for error messages).
fn hex_hash(hash: &[u8; 32]) -> String {
    hex::encode(hash)
}
