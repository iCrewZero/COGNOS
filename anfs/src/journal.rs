//! ANFS write-ahead journal — append-only log of every mutating file
//! operation, fsynced before the kernel is told the op succeeded, so a crash
//! leaves a replayable trail that can restore cache + metadata consistency.
//!
//! The journal is a newline-delimited JSON file (JSONL). Each entry carries a
//! monotonically-increasing sequence number, the operation kind, the affected
//! path(s), the agent that initiated it, and a UTC timestamp.
//!
//! On startup, [`Journal::replay`] reads the entire log; on checkpoint, the
//! log is truncated after a final fsync once the in-memory state has been
//! durably flushed to the backing store.
//!
//! v0: stub implementation — `append` records entries in memory only and does
//! not actually write to disk; `replay` returns the in-memory buffer;
//! `checkpoint` is a no-op that clears the in-memory mirror.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, warn};

// ─── Errors ─────────────────────────────────────────────────────────────────

/// Errors raised by the journal subsystem.
#[derive(Debug, Error)]
pub enum JournalError {
    /// The journal file could not be opened or created.
    #[error("journal open failed: {0}")]
    Open(String),
    /// An append failed (disk full, I/O error, serialization, etc.).
    #[error("journal append failed: {0}")]
    Append(String),
    /// The journal file could not be fsynced.
    #[error("journal fsync failed: {0}")]
    Fsync(String),
    /// A replay encountered a corrupt or unparseable entry.
    #[error("journal replay corrupt at seq {0}: {1}")]
    Corrupt(u64, String),
    /// A checkpoint could not truncate the journal.
    #[error("journal checkpoint failed: {0}")]
    Checkpoint(String),
}

// ─── Sequence numbers & agent identity ───────────────────────────────────────

/// Monotonically-increasing journal sequence number.
pub type SeqNo = u64;

/// Identifier for the agent that initiated a journal entry.
pub type AgentId = String;

// ─── Journal entries ─────────────────────────────────────────────────────────

/// One record in the write-ahead journal.
///
/// Each variant corresponds to a mutating VFS operation. Read-only ops
/// (`getattr`, `readdir`) are not journaled.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JournalEntry {
    /// A file or directory was created.
    Create {
        /// Path of the new node (relative to the backing root).
        path: PathBuf,
        /// Unix mode bits (e.g. `0o644` for files, `0o755` for dirs).
        mode: u32,
        /// Agent that initiated the create.
        agent: AgentId,
        /// UTC timestamp of the operation.
        ts: DateTime<Utc>,
    },
    /// A range of bytes was written to a file.
    Write {
        /// Path of the file written.
        path: PathBuf,
        /// Byte offset where the write began.
        offset: u64,
        /// Number of bytes written.
        len: u64,
        /// Agent that performed the write.
        agent: AgentId,
        /// UTC timestamp of the operation.
        ts: DateTime<Utc>,
    },
    /// A file or directory was deleted.
    Delete {
        /// Path of the removed node.
        path: PathBuf,
        /// Agent that initiated the delete.
        agent: AgentId,
        /// UTC timestamp of the operation.
        ts: DateTime<Utc>,
    },
    /// A file or directory was renamed.
    Rename {
        /// Source path.
        from: PathBuf,
        /// Destination path.
        to: PathBuf,
        /// Agent that performed the rename.
        agent: AgentId,
        /// UTC timestamp of the operation.
        ts: DateTime<Utc>,
    },
    /// A semantic tag was attached to a file.
    Tag {
        /// Path of the tagged file.
        path: PathBuf,
        /// Tag that was added.
        tag: String,
        /// Agent that added the tag.
        agent: AgentId,
        /// UTC timestamp of the operation.
        ts: DateTime<Utc>,
    },
    /// A semantic tag was removed from a file.
    Untag {
        /// Path of the untagged file.
        path: PathBuf,
        /// Tag that was removed.
        tag: String,
        /// Agent that removed the tag.
        agent: AgentId,
        /// UTC timestamp of the operation.
        ts: DateTime<Utc>,
    },
    /// A semantic attribute was set on a file (xattr-style key/value).
    AttrSet {
        /// Path of the attributed file.
        path: PathBuf,
        /// Attribute key.
        key: String,
        /// Attribute value (any JSON-serializable value).
        value: serde_json::Value,
        /// Agent that set the attribute.
        agent: AgentId,
        /// UTC timestamp of the operation.
        ts: DateTime<Utc>,
    },
}

impl JournalEntry {
    /// Return the path primarily affected by this entry (the source path
    /// for `Rename`).
    pub fn primary_path(&self) -> &Path {
        match self {
            JournalEntry::Create { path, .. }
            | JournalEntry::Write { path, .. }
            | JournalEntry::Delete { path, .. }
            | JournalEntry::Tag { path, .. }
            | JournalEntry::Untag { path, .. }
            | JournalEntry::AttrSet { path, .. } => path,
            JournalEntry::Rename { from, .. } => from,
        }
    }

    /// Return the agent that initiated this entry.
    pub fn agent(&self) -> &AgentId {
        match self {
            JournalEntry::Create { agent, .. }
            | JournalEntry::Write { agent, .. }
            | JournalEntry::Delete { agent, .. }
            | JournalEntry::Rename { agent, .. }
            | JournalEntry::Tag { agent, .. }
            | JournalEntry::Untag { agent, .. }
            | JournalEntry::AttrSet { agent, .. } => agent,
        }
    }

    /// Return the UTC timestamp of this entry.
    pub fn ts(&self) -> &DateTime<Utc> {
        match self {
            JournalEntry::Create { ts, .. }
            | JournalEntry::Write { ts, .. }
            | JournalEntry::Delete { ts, .. }
            | JournalEntry::Rename { ts, .. }
            | JournalEntry::Tag { ts, .. }
            | JournalEntry::Untag { ts, .. }
            | JournalEntry::AttrSet { ts, .. } => ts,
        }
    }

    /// Return the kind name as used in the `serde(tag = "kind")` encoding.
    pub fn kind(&self) -> &'static str {
        match self {
            JournalEntry::Create { .. } => "create",
            JournalEntry::Write { .. } => "write",
            JournalEntry::Delete { .. } => "delete",
            JournalEntry::Rename { .. } => "rename",
            JournalEntry::Tag { .. } => "tag",
            JournalEntry::Untag { .. } => "untag",
            JournalEntry::AttrSet { .. } => "attr_set",
        }
    }
}

// ─── Journal ─────────────────────────────────────────────────────────────────

/// The write-ahead journal.
///
/// Owns the on-disk log path plus an in-memory mirror of the entries that
/// have been appended since the last checkpoint.
pub struct Journal {
    /// Filesystem path of the JSONL journal.
    pub path: PathBuf,
    /// In-memory mirror of all appended entries (cleared on checkpoint).
    pub entries: Vec<JournalEntry>,
    /// Next sequence number to assign.
    pub sequence: SeqNo,
}

impl Journal {
    /// Open (or create) a journal at `path`.
    ///
    /// v0: does not actually touch disk; the file is created lazily on the
    /// first `append`. TODO(v1): open the file with `O_APPEND | O_CREAT`
    /// and pre-allocate a small header with the magic + version.
    pub fn new(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            entries: Vec::new(),
            sequence: 0,
        }
    }

    /// Append `entry` to the journal, fsync, then return its sequence number.
    ///
    /// The entry is assigned the next monotonic sequence number *before*
    /// being written. The on-disk write is followed by `fsync` *before*
    /// this call returns, so any `Ok(seq)` returned by `append` is durable.
    ///
    /// v0: only records the entry in memory; no disk write or fsync is
    /// performed. TODO(v1): real append + fsync.
    pub fn append(&mut self, entry: JournalEntry) -> Result<SeqNo, JournalError> {
        let seq = self.sequence;
        self.sequence += 1;
        self.entries.push(entry.clone());

        // v0: serialize to validate the entry is well-formed; the bytes are
        // discarded. TODO(v1): write `line + "\n"` to disk and fsync.
        let line = match serde_json::to_string(&entry) {
            Ok(s) => s,
            Err(e) => return Err(JournalError::Append(format!("serialize: {e}"))),
        };
        let _ = self.fsync_append(&line);

        debug!(
            seq,
            path = %self.path.display(),
            kind = entry.kind(),
            "journal append (v0 in-memory)"
        );
        Ok(seq)
    }

    /// Replay every entry in the journal.
    ///
    /// v0: returns the in-memory mirror. TODO(v1): open the on-disk file,
    /// parse line-by-line, and return the parsed entries (the in-memory
    /// mirror is empty on a fresh start).
    pub fn replay(&self) -> Result<Vec<JournalEntry>, JournalError> {
        // TODO(v1): real on-disk replay with corrupt-line handling.
        warn!(
            path = %self.path.display(),
            count = self.entries.len(),
            "journal replay returning in-memory mirror (v0 stub)"
        );
        Ok(self.entries.clone())
    }

    /// Truncate the journal after fsyncing the current state.
    ///
    /// Called once the in-memory cache + metadata have been durably flushed
    /// to the backing store; all entries up to and including the current
    /// sequence are then safe to discard.
    ///
    /// v0: clears the in-memory mirror; no on-disk truncation.
    /// TODO(v1): truncate the file to zero bytes (or rotate to a new file)
    /// after a final fsync.
    pub fn checkpoint(&mut self) -> Result<(), JournalError> {
        // TODO(v1): real on-disk truncate + fsync.
        let cleared = self.entries.len();
        self.entries.clear();
        debug!(
            cleared,
            next_seq = self.sequence,
            "journal checkpoint (v0 in-memory)"
        );
        Ok(())
    }

    /// Return the next sequence number that will be assigned by `append`.
    pub fn next_seq(&self) -> SeqNo {
        self.sequence
    }

    /// Number of entries currently held in the in-memory mirror.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the in-memory mirror is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    // ─── Internal helpers ────────────────────────────────────────────────

    /// Append a serialized line to the on-disk journal and fsync.
    ///
    /// v0: no-op that discards the line. TODO(v1): real implementation.
    fn fsync_append(&self, _line: &str) -> Result<(), JournalError> {
        // TODO(v1):
        //   use std::fs::OpenOptions;
        //   use std::io::Write;
        //   let mut f = OpenOptions::new()
        //       .create(true).append(true).open(&self.path)
        //       .map_err(|e| JournalError::Open(e.to_string()))?;
        //   f.write_all(line.as_bytes())
        //       .map_err(|e| JournalError::Append(e.to_string()))?;
        //   f.write_all(b"\n")
        //       .map_err(|e| JournalError::Append(e.to_string()))?;
        //   f.sync_all()
        //       .map_err(|e| JournalError::Fsync(e.to_string()))?;
        Ok(())
    }
}

impl Default for Journal {
    fn default() -> Self {
        Self::new(Path::new(".cognos/anfs/journal.log"))
    }
}

// v0: stub implementation
