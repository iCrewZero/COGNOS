//! ANFS semantic cache — holds hot file data and metadata in memory, keyed by
//! path, with LRU eviction and dirty-tracking so that writes can be coalesced
//! before being flushed back to the backing directory.
//!
//! Two caches live side-by-side:
//!
//!   * **data** — file contents (byte vectors) plus their semantic tags.
//!   * **metadata** — `FileAttr`-like summaries (size, mtime, owner, tags).
//!
//! Both are bounded LRU caches. The data cache is bounded by *bytes*, the
//! metadata cache by *entry count*. Eviction of a dirty data entry triggers
//! a flush to the backing store.
//!
//! v0: stub implementation — the LRU is a simple `HashMap`-backed FIFO with
//! no real recency tracking; `flush_dirty` is a no-op that does not touch the
//! backing store.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, warn};

// ─── Errors ─────────────────────────────────────────────────────────────────

/// Errors raised by the cache subsystem.
#[derive(Debug, Error)]
pub enum CacheError {
    /// The backing store rejected a flush.
    #[error("flush to backing failed for {0}: {1}")]
    Flush(String, String),
    /// The cache was asked to evict but the entry was locked.
    #[error("eviction blocked (entry locked): {0}")]
    EvictionBlocked(String),
}

// ─── LRU cache ───────────────────────────────────────────────────────────────

/// A bounded least-recently-used cache.
///
/// v0: backed by a `HashMap` with FIFO-style eviction on insert once
/// `capacity` is exceeded; `get` does not update recency. TODO(v1): replace
/// with the `lru` crate's `LruCache` (or a hand-rolled proper LRU with O(1)
/// touch).
pub struct LruCache<K, V> {
    /// Underlying storage.
    inner: HashMap<K, V>,
    /// Insertion order (oldest first) for FIFO eviction.
    order: Vec<K>,
    /// Maximum number of entries.
    capacity: usize,
}

impl<K, V> LruCache<K, V>
where
    K: std::hash::Hash + Eq + Clone,
{
    /// Create a new LRU cache with the given entry capacity.
    pub fn new(capacity: usize) -> Self {
        let cap = capacity.max(1);
        Self {
            inner: HashMap::with_capacity(cap),
            order: Vec::with_capacity(cap),
            capacity: cap,
        }
    }

    /// Current number of cached entries.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Maximum number of entries.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Look up an entry by key (does not update recency in v0).
    pub fn get(&self, key: &K) -> Option<&V> {
        self.inner.get(key)
    }

    /// Look up an entry by key mutably (does not update recency in v0).
    pub fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        self.inner.get_mut(key)
    }

    /// Insert an entry, evicting the oldest if at capacity.
    ///
    /// Returns the evicted entry (if any). TODO(v1): dirty-eviction
    /// callbacks so the caller can flush before discarding.
    pub fn insert(&mut self, key: K, value: V) -> Option<(K, V)> {
        let mut evicted = None;
        if !self.inner.contains_key(&key) && self.inner.len() >= self.capacity {
            // Evict the oldest entry (FIFO).
            if let Some(old_key) = self.order.first().cloned() {
                if let Some(old_val) = self.inner.remove(&old_key) {
                    evicted = Some((old_key.clone(), old_val));
                }
                self.order.retain(|k| k != &old_key);
            }
        }
        if !self.inner.contains_key(&key) {
            self.order.push(key.clone());
        }
        self.inner.insert(key, value);
        evicted
    }

    /// Remove an entry by key.
    pub fn remove(&mut self, key: &K) -> Option<V> {
        self.order.retain(|k| k != key);
        self.inner.remove(key)
    }

    /// Iterate over all entries (no ordering guarantee).
    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.inner.iter()
    }

    /// Iterate mutably over all entries.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&K, &mut V)> {
        self.inner.iter_mut()
    }

    /// Remove all entries.
    pub fn clear(&mut self) {
        self.inner.clear();
        self.order.clear();
    }
}

// ─── Cached entry ────────────────────────────────────────────────────────────

/// A cached file's data plus its semantic tags and bookkeeping.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedEntry {
    /// The file contents.
    pub data: Vec<u8>,
    /// Semantic tags attached to this file.
    pub tags: HashSet<String>,
    /// When this cache entry expires and must be re-validated.
    pub expiry: DateTime<Utc>,
    /// Whether the data has been modified since it was loaded.
    pub dirty: bool,
}

impl CachedEntry {
    /// Build a fresh, clean cache entry from raw bytes.
    pub fn new(data: Vec<u8>, tags: HashSet<String>, ttl: ChronoDuration) -> Self {
        Self {
            data,
            tags,
            expiry: Utc::now() + ttl,
            dirty: false,
        }
    }

    /// Mark this entry as dirty (modified, needs flush).
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Whether this entry has expired as of `now`.
    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        now >= self.expiry
    }

    /// Length of the cached data in bytes.
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Whether the cached data is empty.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

// ─── Metadata ────────────────────────────────────────────────────────────────

/// Cached metadata summary for a path (a stripped-down `FileAttr`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Metadata {
    /// File size in bytes.
    pub size: u64,
    /// Last modification time (seconds since the UNIX epoch).
    pub mtime_secs: i64,
    /// Owner UID.
    pub uid: u32,
    /// Owner GID.
    pub gid: u32,
    /// Permission bits.
    pub mode: u32,
    /// Semantic tags (mirrored from `CachedEntry::tags`).
    pub tags: HashSet<String>,
}

// ─── AnfsCache ───────────────────────────────────────────────────────────────

/// The ANFS semantic cache.
///
/// Holds hot file data and metadata in two parallel LRU caches. The data
/// cache is bounded by `max_bytes`; the metadata cache is bounded by
/// `metadata_capacity` entries.
pub struct AnfsCache {
    /// LRU of file contents keyed by path.
    pub data: LruCache<PathBuf, CachedEntry>,
    /// LRU of metadata summaries keyed by path.
    pub metadata: LruCache<PathBuf, Metadata>,
    /// Maximum total bytes held in `data`.
    pub max_bytes: u64,
    /// Current bytes held in `data`.
    pub current_bytes: u64,
    /// Default TTL for newly-inserted entries.
    pub default_ttl: ChronoDuration,
}

impl AnfsCache {
    /// Construct a new cache with the given byte budget and metadata capacity.
    pub fn new(max_bytes: u64, metadata_capacity: usize) -> Self {
        Self {
            data: LruCache::new(1024),
            metadata: LruCache::new(metadata_capacity),
            max_bytes,
            current_bytes: 0,
            default_ttl: ChronoDuration::seconds(30),
        }
    }

    /// Look up a cached data entry by path.
    pub fn read(&self, path: &Path) -> Option<&CachedEntry> {
        self.data.get(&path.to_path_buf())
    }

    /// Look up a cached data entry mutably by path.
    pub fn read_mut(&mut self, path: &Path) -> Option<&mut CachedEntry> {
        self.data.get_mut(&path.to_path_buf())
    }

    /// Look up a cached metadata entry by path.
    pub fn metadata(&self, path: &Path) -> Option<&Metadata> {
        self.metadata.get(&path.to_path_buf())
    }

    /// Insert a data entry (and a matching metadata summary) into the cache.
    ///
    /// If inserting would exceed `max_bytes`, entries should be evicted
    /// (oldest first) until there is room; dirty evicted entries are flushed
    /// back to the backing store first. v0 logs the over-commit and does not
    /// actually evict. TODO(v1): real eviction loop with dirty-flush.
    pub fn insert(&mut self, path: PathBuf, data: Vec<u8>, tags: HashSet<String>) {
        let len = data.len() as u64;

        // Evict until we have room.
        // TODO(v1): real eviction loop with dirty-flush callbacks.
        if self.current_bytes + len > self.max_bytes {
            warn!(
                path = %path.display(),
                requested = len,
                current = self.current_bytes,
                max = self.max_bytes,
                "cache over-commit (v0 stub — no eviction performed)"
            );
        }

        let entry = CachedEntry::new(data, tags.clone(), self.default_ttl);
        self.current_bytes = self.current_bytes.saturating_add(len);
        let _evicted = self.data.insert(path.clone(), entry);

        let meta = Metadata {
            size: len,
            mtime_secs: 0,
            uid: 0,
            gid: 0,
            mode: 0o644,
            tags,
        };
        let _ = self.metadata.insert(path, meta);
    }

    /// Invalidate a single path in both caches.
    pub fn invalidate(&mut self, path: &Path) {
        if let Some(removed) = self.data.remove(&path.to_path_buf()) {
            self.current_bytes = self
                .current_bytes
                .saturating_sub(removed.data.len() as u64);
        }
        let _ = self.metadata.remove(&path.to_path_buf());
        debug!(path = %path.display(), "cache invalidated");
    }

    /// Flush all dirty data entries back to the backing directory.
    ///
    /// v0: no-op that only clears the `dirty` flag — it does not touch
    /// `backing`. TODO(v1): walk `data`, for each dirty entry open the
    /// backing path and write the bytes, then clear the dirty flag and
    /// emit a journal entry if appropriate.
    pub fn flush_dirty(&mut self, backing: &Path) -> Result<(), CacheError> {
        let mut flushed = 0usize;
        for (_path, entry) in self.data.iter_mut() {
            if entry.dirty {
                // TODO(v1): std::fs::write(backing.join(_path), &entry.data)
                entry.dirty = false;
                flushed += 1;
            }
        }
        if flushed > 0 {
            debug!(
                flushed,
                backing = %backing.display(),
                "flushed dirty entries (v0 no-op)"
            );
        }
        Ok(())
    }

    /// Current bytes held in the data cache.
    pub fn bytes_used(&self) -> u64 {
        self.current_bytes
    }

    /// Number of entries in the data cache.
    pub fn data_len(&self) -> usize {
        self.data.len()
    }

    /// Number of entries in the metadata cache.
    pub fn metadata_len(&self) -> usize {
        self.metadata.len()
    }
}

impl Default for AnfsCache {
    fn default() -> Self {
        Self::new(64 * 1024 * 1024, 4096)
    }
}

// v0: stub implementation
