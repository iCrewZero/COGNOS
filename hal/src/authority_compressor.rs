//! Authority compressor — caches redundant HAL authority checks.
//!
//!
//! HAL is consulted on every agent action that crosses a capability boundary.
//! Many of these are identical, low-risk, and repeat thousands of times per
//! session (e.g. "open file in workspace", "read X config"). Re-evaluating the
//! full risk formula and policy lattice for each one would dominate HAL's
//! latency budget and starve actually-novel decisions.
//!
//! The [`AuthorityCompressor`] maintains a small LRU cache keyed by a stable
//! digest of the action request. Only **reversible, low-risk** actions are
//! eligible for compression; irreversible actions are never cached and always
//! re-evaluated. The cache is invalidated wholesale whenever the policy set,
//! trust state, or capability lattice changes — partial invalidation is a
//! v1 concern.
//!
//! v0: stub implementation. The LRU is a `HashMap` with manual eviction; a
//! real bounded LRU (`lru` crate) is TODO(v1). TTL is stored but not yet
//! enforced on read; TODO(v1).

use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tracing::{debug, info, warn};
use uuid::Uuid;

// v0: stub implementation

/// Type alias for an agent identifier (matches the rest of the crate).
pub type AgentId = String;

/// Maximum number of cached verdicts before manual eviction kicks in.
const DEFAULT_CACHE_CAPACITY: usize = 1024;

/// Default TTL for cached verdicts: 60 seconds.
const DEFAULT_TTL_SECS: u64 = 60;

// ─── Action Request & Digest ───────────────────────────────────────────────────

/// A HAL action request, in the minimal shape needed for compression.
///
/// This is intentionally a smaller struct than the full [`crate::hal_types::HALContext`];
/// only the fields that affect the verdict are digested.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionRequest {
    /// Agent proposing the action.
    pub agent: AgentId,
    /// Action name, e.g. "open_file", "query_memory".
    pub action: String,
    /// Target resource path or identifier.
    pub target: String,
    /// Whether the action is irreversible (deletes, kernel writes, etc.).
    pub irreversible: bool,
    /// Opaque parameters blob included in the digest.
    pub parameters: serde_json::Value,
}

/// A 32-byte SHA-256 digest of an [`ActionRequest`], used as the cache key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ActionDigest(pub [u8; 32]);

impl ActionDigest {
    /// Compute the canonical digest for an action request.
    ///
    /// The digest covers `agent || action || target || parameters_json`.
    /// Irreversibility is intentionally excluded so that a reversible and an
    /// irreversible variant of the same action get different cache entries
    /// only if the action name itself differs.
    pub fn from_request(req: &ActionRequest) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(req.agent.as_bytes());
        hasher.update(b"\x00");
        hasher.update(req.action.as_bytes());
        hasher.update(b"\x00");
        hasher.update(req.target.as_bytes());
        hasher.update(b"\x00");
        // Canonical JSON would be ideal; v0 uses serde_json::to_string which
        // is deterministic for object keys only via `preserve_order = false`.
        // TODO(v1): use a canonical JSON serializer.
        let params = serde_json::to_string(&req.parameters).unwrap_or_default();
        hasher.update(params.as_bytes());
        let mut out = [0u8; 32];
        out.copy_from_slice(&hasher.finalize());
        Self(out)
    }
}

// ─── Verdict & Cached Verdict ──────────────────────────────────────────────────

/// A HAL verdict on an action. v0 stub; v1 will reference `crate::policy_engine`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Verdict {
    /// `true` if HAL permits the action.
    pub allowed: bool,
    /// Risk score at the time of evaluation.
    pub risk_score: f32,
    /// Human-readable reason for the verdict.
    pub reason: String,
    /// When the verdict was issued.
    pub issued_at: DateTime<Utc>,
}

/// A cached verdict returned by [`AuthorityCompressor::compress`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedVerdict {
    /// The underlying verdict.
    pub verdict: Verdict,
    /// The digest that was matched.
    pub digest: ActionDigest,
    /// When this cached entry was stored.
    pub cached_at: DateTime<Utc>,
    /// Number of times this cached entry has been hit.
    pub hit_count: u64,
}

// ─── Invalidation ───────────────────────────────────────────────────────────────

/// Reasons the authority cache may be invalidated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum InvalidationReason {
    /// The active policy set changed.
    PolicyChanged,
    /// The global trust state changed (e.g. user calibration update).
    TrustChanged,
    /// A capability was revoked from some agent.
    CapabilityRevoked,
    /// Operator-initiated manual flush.
    ManualFlush,
}

// ─── Compressor ─────────────────────────────────────────────────────────────────

/// Stub LRU cache used by the compressor.
///
/// v0: a thin wrapper around `HashMap` with a soft capacity and FIFO-style
/// eviction. v1 will replace this with a proper bounded LRU.
pub struct LruCache<K, V>
where
    K: std::hash::Hash + Eq + Clone,
{
    inner: HashMap<K, V>,
    capacity: usize,
    insert_order: Vec<K>,
}

impl<K, V> LruCache<K, V>
where
    K: std::hash::Hash + Eq + Clone,
{
    /// Create a new cache with the given capacity.
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: HashMap::with_capacity(capacity),
            capacity,
            insert_order: Vec::with_capacity(capacity),
        }
    }

    /// Insert a key/value pair, evicting the oldest entry if at capacity.
    pub fn insert(&mut self, key: K, value: V) {
        if !self.inner.contains_key(&key) {
            if self.inner.len() >= self.capacity && !self.insert_order.is_empty() {
                let evict = self.insert_order.remove(0);
                self.inner.remove(&evict);
            }
            self.insert_order.push(key.clone());
        }
        self.inner.insert(key, value);
    }

    /// Look up a value by key.
    pub fn get(&self, key: &K) -> Option<&V> {
        self.inner.get(key)
    }

    /// Look up a value by key, mutably.
    pub fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        self.inner.get_mut(key)
    }

    /// Number of entries currently cached.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Remove all entries.
    pub fn clear(&mut self) {
        self.inner.clear();
        self.insert_order.clear();
    }
}

/// The authority compressor itself.
///
/// Stores cached verdicts keyed by [`ActionDigest`] in a small LRU. Hit
/// counts are tracked in a separate map so the cache value can stay a plain
/// [`Verdict`] per the spec.
pub struct AuthorityCompressor {
    /// LRU of cached verdicts keyed by action digest.
    cache: LruCache<ActionDigest, Verdict>,
    /// Per-digest hit counts, kept in sync with `cache`.
    hits: HashMap<ActionDigest, u64>,
    /// Time-to-live for cached entries.
    ttl: Duration,
}

impl AuthorityCompressor {
    /// Build a new compressor with the default capacity and TTL.
    pub fn new() -> Self {
        Self {
            cache: LruCache::new(DEFAULT_CACHE_CAPACITY),
            hits: HashMap::new(),
            ttl: Duration::from_secs(DEFAULT_TTL_SECS),
        }
    }

    /// Build a compressor with a custom cache capacity and TTL.
    pub fn with_capacity_and_ttl(capacity: usize, ttl: Duration) -> Self {
        Self {
            cache: LruCache::new(capacity),
            hits: HashMap::new(),
            ttl,
        }
    }

    /// Attempt to compress a request by returning a cached verdict if one is
    /// still fresh and the action is eligible for caching.
    ///
    /// Returns `None` if:
    ///   * the action is irreversible (never cached),
    ///   * no cached verdict exists for this digest,
    ///   * the cached verdict has expired (TTL elapsed).
    ///
    /// On a cache hit, the hit counter is incremented and the verdict is
    /// returned. On a miss the caller is expected to evaluate the action
    /// normally and call [`Self::store`] to populate the cache.
    pub fn compress(&mut self, request: &ActionRequest) -> Option<CachedVerdict> {
        // Irreversible actions are never compressed.
        if request.irreversible {
            debug!(
                agent = %request.agent,
                action = %request.action,
                "authority_compressor: irreversible action, skipping cache"
            );
            return None;
        }

        let digest = ActionDigest::from_request(request);
        let now = Utc::now();

        // Clone the verdict out so we don't hold an immutable borrow of
        // `self.cache` across the mutable borrow of `self.hits` below.
        let cached = self.cache.get(&digest)?.clone();
        let age = now.signed_duration_since(cached.issued_at);
        if age.num_seconds() as u64 > self.ttl.as_secs() {
            debug!(?digest, "authority_compressor: cache entry expired");
            // TODO(v1): actually evict the expired entry here. v0 leaves it
            // to be overwritten naturally on next store.
            return None;
        }

        let hit_count = {
            let counter = self.hits.entry(digest).or_insert(0);
            *counter += 1;
            *counter
        };
        let hit = CachedVerdict {
            verdict: cached.clone(),
            digest,
            cached_at: cached.issued_at,
            hit_count,
        };
        debug!(?digest, hits = hit.hit_count, "authority_compressor: cache hit");
        Some(hit)
    }

    /// Store a freshly-computed verdict in the cache.
    ///
    /// Irreversible actions are silently ignored (never cached).
    pub fn store(&mut self, request: &ActionRequest, verdict: Verdict) {
        if request.irreversible {
            return;
        }
        let digest = ActionDigest::from_request(request);
        debug!(?digest, "authority_compressor: storing verdict");
        self.cache.insert(digest, verdict);
        self.hits.insert(digest, 0);
    }

    /// Invalidate the cache. The reason is logged but does not affect the
    /// scope of invalidation in v0 — the entire cache is always dropped.
    /// TODO(v1): targeted invalidation by agent or capability scope.
    pub fn invalidate(&mut self, reason: InvalidationReason) {
        let count = self.cache.len();
        match reason {
            InvalidationReason::PolicyChanged => {
                info!(count, "authority_compressor: flushed (policy changed)");
            }
            InvalidationReason::TrustChanged => {
                info!(count, "authority_compressor: flushed (trust changed)");
            }
            InvalidationReason::CapabilityRevoked => {
                warn!(count, "authority_compressor: flushed (capability revoked)");
            }
            InvalidationReason::ManualFlush => {
                info!(count, "authority_compressor: flushed (manual)");
            }
        }
        self.cache.clear();
        self.hits.clear();
    }

    /// Current number of cached entries.
    pub fn len(&self) -> usize {
        self.cache.len()
    }

    /// Whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.cache.is_empty()
    }
}

impl Default for AuthorityCompressor {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Errors ─────────────────────────────────────────────────────────────────────

/// Errors produced by the authority compressor.
#[derive(Debug, Error)]
pub enum AuthorityCompressorError {
    /// Action could not be serialized for digesting.
    #[error("failed to serialize action parameters: {0}")]
    Serialization(#[from] serde_json::Error),
}

// ─── Helpers ────────────────────────────────────────────────────────────────────

/// Generate a fresh correlation id for a cache-miss flow. v0 stub.
pub fn new_correlation_id() -> Uuid {
    Uuid::new_v4()
}
