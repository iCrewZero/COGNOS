//! ANFS FUSE overlay — translates kernel VFS requests into tag-aware, journaled,
//! access-controlled file operations against the backing directory.
//!
//! The overlay implements `fuser::Filesystem` (the Rust FUSE binding, aliased
//! here as `fuse`) and delegates each operation through three subsystems:
//!
//!   * **journal** — every mutating op is appended to a write-ahead log
//!     *before* it is acknowledged to the kernel, so a crash leaves a
//!     replayable trail.
//!   * **cache** — hot file data and metadata is held in an LRU; dirty pages
//!     are flushed back to the backing dir on eviction or explicit `fsync`.
//!   * **security** — every op is checked against the per-agent capability
//!     lattice; restricted paths (`~/.cognos/`, `~/.ssh/`, `/etc/cognos/`)
//!     are gated and audited.
//!
//! Paths in the FUSE mount are resolved against the *semantic* tag namespace
//! when they begin with `/.tags/` (e.g. `/.tags/work/notes.md` resolves to
//! the most-recently-tagged `notes.md` carrying the `work` tag). All other
//! paths pass through to the backing directory verbatim.
//!
//! v0: stub implementation — every `Filesystem` method logs intent, optionally
//! delegates to the journal/cache/security stubs, and replies `ENOSYS` (or a
//! placeholder entry) to the kernel. No real I/O is performed against the
//! backing store.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use fuser as fuse;
use fuse::{
    FileAttr, FileType, ReplyAttr, ReplyCreate, ReplyData, ReplyDirectory, ReplyEmpty,
    ReplyEntry, ReplyWrite, Request,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, warn};

use crate::cache::AnfsCache;
use crate::journal::{Journal, JournalEntry};
use crate::security::{AnfsSecurity, FileOp};

// ─── Errors ─────────────────────────────────────────────────────────────────

/// Errors raised by the overlay while servicing a FUSE request.
#[derive(Debug, Error)]
pub enum OverlayError {
    /// The backing path could not be resolved.
    #[error("backing path resolution failed: {0}")]
    BackingResolve(String),
    /// A semantic `/.tags/` path did not resolve to any known file.
    #[error("tag path unresolved: {0}")]
    TagUnresolved(String),
    /// The security subsystem denied the operation.
    #[error("security denied: {0}")]
    Denied(String),
    /// The journal could not append an entry.
    #[error("journal append failed: {0}")]
    Journal(String),
}

// ─── Overlay state ───────────────────────────────────────────────────────────

/// ANFS FUSE overlay.
///
/// Owns the backing directory path plus the three subsystems (journal, cache,
/// security) that mediate every VFS operation.
pub struct AnfsOverlay {
    /// Real directory that ANFS overlays.
    pub backing: PathBuf,
    /// Write-ahead journal for crash recovery.
    pub journal: Journal,
    /// Semantic LRU cache for hot data + metadata.
    pub cache: AnfsCache,
    /// Per-agent access-control gate.
    pub security: AnfsSecurity,
}

impl AnfsOverlay {
    /// Construct a new overlay from the daemon config.
    pub fn new(backing: PathBuf, config: crate::AnfsConfig) -> Self {
        let journal = Journal::new(&config.journal_path);
        let cache = AnfsCache::new(config.cache_max_bytes, config.cache_metadata_capacity);
        let security = AnfsSecurity::new(&config.default_lattice, &config.audit_log);
        Self {
            backing,
            journal,
            cache,
            security,
        }
    }

    /// Resolve a FUSE-visible path to a real path in the backing directory.
    ///
    /// Paths starting with `/.tags/` are resolved through the semantic tag
    /// namespace: the most-recently-tagged file carrying the given tag chain
    /// is returned. All other paths are joined under `backing` verbatim.
    ///
    /// v0: tag resolution always returns `Err(OverlayError::TagUnresolved)`.
    /// TODO(v1): query the tag engine (`memory/anfs/src/tag_engine.rs`) for
    /// the best match and cache the result.
    pub fn resolve(&self, fuse_path: &Path) -> Result<PathBuf, OverlayError> {
        let rel = fuse_path.strip_prefix("/").unwrap_or(fuse_path);
        if rel.starts_with(".tags") {
            // TODO(v1): real tag resolution via the tag engine.
            warn!(
                path = %fuse_path.display(),
                "tag-path resolution not implemented in v0"
            );
            return Err(OverlayError::TagUnresolved(fuse_path.display().to_string()));
        }
        Ok(self.backing.join(rel))
    }

    /// Extract the agent identity associated with the current request.
    ///
    /// v0: always returns the placeholder `"anfs:default"` agent.
    /// TODO(v1): read the agent identity from the FUSE request's extended
    /// attributes or the calling process's capability token (set via the
    /// `cognos-anfs` PAM module at session start).
    fn agent_for(&self, _req: &Request) -> String {
        // TODO(v1): real agent resolution from the request.
        "anfs:default".to_string()
    }

    /// Compute a placeholder `FileAttr` for a stubbed reply.
    ///
    /// v0: returns a zero-size regular file at the UNIX epoch. TODO(v1):
    /// `lstat` the real backing inode and convert its metadata.
    fn stub_attr(ino: u64) -> FileAttr {
        FileAttr {
            ino,
            size: 0,
            blocks: 0,
            atime: std::time::UNIX_EPOCH,
            mtime: std::time::UNIX_EPOCH,
            ctime: std::time::UNIX_EPOCH,
            crtime: std::time::UNIX_EPOCH,
            kind: FileType::RegularFile,
            perm: 0o644,
            nlink: 1,
            uid: 0,
            gid: 0,
            rdev: 0,
            flags: 0,
            blksize: 512,
        }
    }
}

// ─── Filesystem trait implementation ─────────────────────────────────────────

/// v0: every method logs intent, optionally delegates to journal/cache/security
/// (each of which is itself a v0 stub), and replies `ENOSYS` or a placeholder
/// entry to the kernel. No real I/O is performed against the backing store.
impl fuse::Filesystem for AnfsOverlay {
    fn lookup(
        &mut self,
        req: &Request,
        parent: u64,
        name: &OsStr,
        reply: ReplyEntry,
    ) {
        let agent = self.agent_for(req);
        debug!(%agent, parent, name = ?name, "lookup");
        // TODO(v1): security.check → cache.metadata → backing.stat
        let _ = self.security.check(&agent, &PathBuf::from(name), FileOp::GetAttr);
        reply.entry(&Duration::from_secs(1), &Self::stub_attr(parent), 0);
    }

    fn getattr(
        &mut self,
        req: &Request,
        ino: u64,
        _fh: Option<u64>,
        reply: ReplyAttr,
    ) {
        let agent = self.agent_for(req);
        debug!(%agent, ino, "getattr");
        let _ = self.security.check(&agent, &PathBuf::from("."), FileOp::GetAttr);
        // TODO(v1): serve from cache.metadata; fall back to backing.stat().
        reply.attr(&Duration::from_secs(1), &Self::stub_attr(ino));
    }

    fn read(
        &mut self,
        req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        size: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyData,
    ) {
        let agent = self.agent_for(req);
        debug!(%agent, ino, offset, size, "read");
        let _ = self.security.check(&agent, &PathBuf::from("."), FileOp::Read);
        // TODO(v1): cache.read → backing.read → reply.data(slice)
        let _ = self.cache.read(&PathBuf::from("."));
        reply.error(libc::ENOSYS);
    }

    fn write(
        &mut self,
        req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        data: &[u8],
        _write_flags: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyWrite,
    ) {
        let agent = self.agent_for(req);
        debug!(%agent, ino, offset, len = data.len(), "write");
        if self
            .security
            .check(&agent, &PathBuf::from("."), FileOp::Write)
            .is_err()
        {
            reply.error(libc::EACCES);
            return;
        }
        // TODO(v1): journal.append(Write{...}) → cache.insert(path, data, tags)
        //           → reply.written(len)
        let entry = JournalEntry::Write {
            path: PathBuf::from("."),
            offset: offset as u64,
            len: data.len() as u64,
            agent: agent.clone(),
            ts: chrono::Utc::now(),
        };
        let _ = self.journal.append(entry);
        reply.error(libc::ENOSYS);
    }

    fn create(
        &mut self,
        req: &Request,
        parent: u64,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        _flags: i32,
        reply: ReplyCreate,
    ) {
        let agent = self.agent_for(req);
        debug!(%agent, parent, name = ?name, "create");
        if self
            .security
            .check(&agent, &PathBuf::from(name), FileOp::Create)
            .is_err()
        {
            reply.error(libc::EACCES);
            return;
        }
        let entry = JournalEntry::Create {
            path: PathBuf::from(name),
            mode: 0o644,
            agent: agent.clone(),
            ts: chrono::Utc::now(),
        };
        let _ = self.journal.append(entry);
        // TODO(v1): backing.create → cache.insert → reply.created(...)
        reply.error(libc::ENOSYS);
    }

    fn unlink(
        &mut self,
        req: &Request,
        parent: u64,
        name: &OsStr,
        reply: ReplyEmpty,
    ) {
        let agent = self.agent_for(req);
        debug!(%agent, parent, name = ?name, "unlink");
        if self
            .security
            .check(&agent, &PathBuf::from(name), FileOp::Delete)
            .is_err()
        {
            reply.error(libc::EACCES);
            return;
        }
        let entry = JournalEntry::Delete {
            path: PathBuf::from(name),
            agent: agent.clone(),
            ts: chrono::Utc::now(),
        };
        let _ = self.journal.append(entry);
        // TODO(v1): backing.unlink → cache.invalidate → reply.ok()
        reply.error(libc::ENOSYS);
    }

    fn mkdir(
        &mut self,
        req: &Request,
        parent: u64,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        reply: ReplyEntry,
    ) {
        let agent = self.agent_for(req);
        debug!(%agent, parent, name = ?name, "mkdir");
        if self
            .security
            .check(&agent, &PathBuf::from(name), FileOp::Create)
            .is_err()
        {
            reply.error(libc::EACCES);
            return;
        }
        let entry = JournalEntry::Create {
            path: PathBuf::from(name),
            mode: 0o755,
            agent: agent.clone(),
            ts: chrono::Utc::now(),
        };
        let _ = self.journal.append(entry);
        // TODO(v1): backing.mkdir → reply.entry(...)
        reply.entry(&Duration::from_secs(1), &Self::stub_attr(parent), 0);
    }

    fn readdir(
        &mut self,
        req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        let agent = self.agent_for(req);
        debug!(%agent, ino, offset, "readdir");
        let _ = self.security.check(&agent, &PathBuf::from("."), FileOp::GetAttr);
        // TODO(v1): backing.readdir → optionally inject /.tags/ virtual dir
        //           → reply.add(...) for each entry.
        let _ = reply;
        reply.ok();
    }

    fn rename(
        &mut self,
        req: &Request,
        parent: u64,
        name: &OsStr,
        new_parent: u64,
        new_name: &OsStr,
        reply: ReplyEmpty,
    ) {
        let agent = self.agent_for(req);
        debug!(
            %agent,
            parent,
            name = ?name,
            new_parent,
            new_name = ?new_name,
            "rename"
        );
        if self
            .security
            .check(&agent, &PathBuf::from(name), FileOp::Rename)
            .is_err()
        {
            reply.error(libc::EACCES);
            return;
        }
        let entry = JournalEntry::Rename {
            from: PathBuf::from(name),
            to: PathBuf::from(new_name),
            agent: agent.clone(),
            ts: chrono::Utc::now(),
        };
        let _ = self.journal.append(entry);
        // TODO(v1): backing.rename → cache.invalidate(from) → cache.invalidate(to)
        //           → reply.ok()
        reply.error(libc::ENOSYS);
    }
}

// ─── Tag-aware path resolution ───────────────────────────────────────────────

/// A tag query extracted from a `/.tags/<tag1>/<tag2>/.../<filename>` path.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TagQuery {
    /// Unordered set of tags that the target file must carry
    /// (e.g. `{"work", "quarterly"}`).
    pub tags: HashSet<String>,
    /// Optional filename suffix; if absent, all matches are returned.
    pub filename: Option<String>,
}

impl TagQuery {
    /// Parse a FUSE path under `/.tags/` into a [`TagQuery`].
    ///
    /// v0: returns an empty query. TODO(v1): split the path components after
    /// `.tags` into tags + final filename.
    pub fn parse(path: &Path) -> Self {
        // TODO(v1): real parsing.
        let _ = path;
        Self::default()
    }

    /// Whether this query matches a file carrying the given tag set.
    ///
    /// v0: always returns `false`. TODO(v1): set-subset check.
    pub fn matches(&self, _file_tags: &HashSet<String>) -> bool {
        // TODO(v1): self.tags ⊆ file_tags && (filename matches or is None).
        false
    }
}

// v0: stub implementation
