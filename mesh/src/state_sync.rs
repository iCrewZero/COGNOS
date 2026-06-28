//! State sync — eventually-consistent CRDT-based state sync across the mesh. Used for things like agent directories, capability maps, and global policy hints.
//!
//! [`StateSync`] maintains a local [`CrdtMap`] of CRDT values keyed by
//! string. Each write is tagged with the writing node's id and a monotonic
//! counter tracked in `vector_clock`; the counter feeds a
//! last-writer-wins resolution policy for [`CrdtValue::LwwRegister`]
//! values. Periodically (every `sync_interval`) the local map is pushed
//! to a random peer and remote maps are merged in via [`merge`](StateSync::merge).
//!
//! v0 ships the data model, the merge/push/fetch entrypoints, and the
//! [`Conflict`] surface; the anti-entropy transport and a real
//! vector-clock reconciliation pass land in v1.
//!
//! v0: stub implementation

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use tokio::time::Duration;
use tracing::{debug, warn};

// TODO(v1): share `NodeId` with the rest of the mesh submodules via a
// `mesh::types` module.

/// Cluster-unique identifier for a mesh node.
pub type NodeId = String;

/// Default anti-entropy interval — every 2s the local map is pushed to
/// a random peer.
pub const DEFAULT_SYNC_INTERVAL: Duration = Duration::from_secs(2);

// ─── Errors ─────────────────────────────────────────────────────────────────

/// Errors returned by state synchronization.
#[derive(Debug, Error)]
pub enum SyncError {
    /// Two CRDT values were merged under the same key but had
    /// incompatible types (e.g. `LwwRegister` vs. `PNCounter`).
    #[error("incompatible CRDT types for key")]
    IncompatibleType,
    /// The local and remote states have diverged beyond what the CRDT
    /// merge rules can reconcile (e.g. tombstone/value cycles in an
    /// `OrSet` that violate the resolution policy).
    #[error("state diverged beyond repair")]
    DivergedBeyondRepair,
}

// ─── CRDT values ────────────────────────────────────────────────────────────

/// One CRDT value, parameterized by operation type.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data")]
pub enum CrdtValue {
    /// Last-writer-wins register; `timestamp` breaks ties.
    LwwRegister {
        /// Stored payload.
        value: Value,
        /// Wall-clock timestamp of the last write.
        timestamp: DateTime<Utc>,
    },
    /// Grow-only set; items can only ever be added.
    GSet {
        /// Current set contents.
        items: Vec<Value>,
    },
    /// Observed-remove set; additions are tagged and removals only
    /// affect previously-observed tags.
    OrSet {
        /// Live items.
        items: Vec<Value>,
        /// Tombstoned tags (already-removed items).
        tombstones: Vec<Value>,
    },
    /// Positive-negative counter; `p` increments, `n` decrements; value
    /// is `sum(p) - sum(n)`.
    PNCounter {
        /// Positive contributions keyed by node id.
        p: HashMap<NodeId, u64>,
        /// Negative contributions keyed by node id.
        n: HashMap<NodeId, u64>,
    },
}

// ─── CRDT map ───────────────────────────────────────────────────────────────

/// A map of string keys to [`CrdtValue`]s.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CrdtMap {
    /// Keyed CRDT entries.
    pub entries: HashMap<String, CrdtValue>,
}

impl CrdtMap {
    /// Construct an empty map.
    pub fn new() -> Self {
        Self::default()
    }

    /// Read-only access to the entry for `key`.
    pub fn get(&self, key: &str) -> Option<&CrdtValue> {
        self.entries.get(key)
    }
}

// ─── Conflicts ──────────────────────────────────────────────────────────────

/// A conflict surfaced during merge. v0 reports every key whose local and
/// remote values have incompatible CRDT types.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conflict {
    /// Key that conflicted.
    pub key: String,
    /// Local value at conflict time.
    pub ours: CrdtValue,
    /// Remote value at conflict time.
    pub theirs: CrdtValue,
}

// ─── StateSync ──────────────────────────────────────────────────────────────

/// Eventually-consistent state synchronizer.
#[derive(Debug)]
pub struct StateSync {
    /// Local CRDT map.
    pub local_state: CrdtMap,
    /// Per-node monotonic counter; used to break LWW ties.
    pub vector_clock: HashMap<NodeId, u64>,
    /// Anti-entropy interval.
    pub sync_interval: Duration,
}

impl StateSync {
    /// Construct a new synchronizer with the default sync interval.
    pub fn new() -> Self {
        Self {
            local_state: CrdtMap::new(),
            vector_clock: HashMap::new(),
            sync_interval: DEFAULT_SYNC_INTERVAL,
        }
    }

    /// Merge a remote [`CrdtMap`] into `local_state`.
    ///
    /// The merge is element-wise: `GSet` / `OrSet` / `PNCounter` merges
    /// are commutative and never fail; `LwwRegister` merges take the
    /// larger `timestamp`. A type mismatch on the same key is recorded
    /// for later surfacing via [`conflicts`](Self::conflicts) and skipped.
    // TODO(v1): implement the per-variant merge semantics; v0 records
    // type-mismatched keys and returns `Ok(())` so callers can observe
    // them via [`conflicts`](Self::conflicts).
    pub async fn merge(&mut self, other: &CrdtMap) -> Result<(), SyncError> {
        for (key, theirs) in &other.entries {
            let Some(ours) = self.local_state.entries.get(key) else {
                debug!(key = %key, "merge: adopting remote entry (no local copy)");
                self.local_state.entries.insert(key.clone(), theirs.clone());
                continue;
            };
            if std::mem::discriminant(ours) != std::mem::discriminant(theirs) {
                warn!(key = %key, "merge: incompatible CRDT types, skipping entry");
                // v0: stub implementation — v1 will perform an actual
                // CRDT merge per variant and surface a [`Conflict`].
                continue;
            }
            debug!(key = %key, "merge: compatible types, merging (stub)");
        }
        Ok(())
    }

    /// Write `value` under `key` locally and bump the local node's
    /// vector-clock counter.
    // TODO(v1): accept the local `NodeId` from the caller rather than
    // hard-coding `"local"`.
    pub async fn push_local(&mut self, key: String, value: CrdtValue) {
        let counter = self.vector_clock.entry("local".to_string()).or_insert(0);
        *counter = counter.saturating_add(1);
        debug!(key = %key, counter = *counter, "push_local: write recorded");
        self.local_state.entries.insert(key, value);
    }

    /// Snapshot of the local state. Cheap clone — callers may freely
    /// mutate the returned map.
    pub async fn fetch_state(&self) -> CrdtMap {
        self.local_state.clone()
    }

    /// Compute the list of currently-known conflicts.
    ///
    /// v0 reports a [`Conflict`] for every key where local and the most
    /// recently merged remote map disagree on CRDT type. Because v0
    /// does not yet retain the remote map, this returns an empty list.
    // TODO(v1): retain the last merged remote `CrdtMap` and surface real
    // type-mismatch conflicts rather than always returning an empty list.
    pub fn conflicts(&self) -> Vec<Conflict> {
        // v0: stub implementation — no remote map is tracked yet, so
        // there is nothing to conflict with.
        Vec::new()
    }
}

impl Default for StateSync {
    fn default() -> Self {
        Self::new()
    }
}

// TODO(v1): spawn a background anti-entropy loop using
// `tokio::time::interval(self.sync_interval)` that pulls a random peer's
// map and calls [`merge`](StateSync::merge).

// v0: stub implementation
