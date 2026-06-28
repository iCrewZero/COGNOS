//! Model pool — keeps multiple GGUF models resident, shares them across agents
//! via Arc, and supports hot-swapping when memory pressure rises.
//!
//! The [`ModelPool`] is the single owner of every `LlamaModel` handle in the
//! process. Agents and the inference engine acquire [`Arc<LoadedModel>`]
//! handles from the pool; when memory pressure rises, the scheduler can call
//! [`evict_lru`] to reclaim bytes from the least-recently-used unreferenced
//! model.
//!
//! [`evict_lru`]: ModelPool::evict_lru

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, error, info, warn};

use crate::bindings::{free_model, load_model, LlamaModel};

// ─── Types ─────────────────────────────────────────────────────────────────

/// Stable identifier for a loaded model. Usually a short slug like
/// `"llama-3-8b"` or `"qwen-2.5-coder"`.
pub type ModelId = String;

/// Errors returned by [`ModelPool`] operations.
#[derive(Debug, Error, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub enum PoolError {
    /// No model with the requested id is currently loaded.
    #[error("model not found in pool: {0}")]
    NotFound(String),

    /// Loading another model would exceed the pool's byte budget.
    #[error("pool out of memory: needed {needed} bytes, {free} free")]
    Oom { needed: u64, free: u64 },

    /// The model cannot be unloaded because outstanding `Arc` handles still
    /// reference it.
    #[error("model still referenced: {0} outstanding handles")]
    StillReferenced(usize),

    /// The underlying loader (FFI / disk) failed to bring the model in.
    #[error("model load failed: {0}")]
    LoadFailed(String),
}

// ─── Loaded Model ──────────────────────────────────────────────────────────

/// A resident, reference-counted GGUF model.
///
/// The pool owns the canonical `Arc`; every acquirer bumps the
/// [`refcount`](Self::refcount) atomic so that [`ModelPool::evict_lru`] can
/// tell which models are safe to drop.
pub struct LoadedModel {
    /// Stable identifier — also the pool's HashMap key.
    pub id: ModelId,
    /// Absolute filesystem path the GGUF was loaded from.
    pub path: PathBuf,
    /// Raw, non-null handle into the FFI layer. Owned by this struct.
    pub handle: NonNull<LlamaModel>,
    /// Best-effort on-disk size, used for pool accounting.
    pub size_bytes: u64,
    /// Number of outstanding `Arc<LoadedModel>` clones. Bumped on `acquire`
    /// and decremented on `release`.
    pub refcount: AtomicUsize,
    /// Time of the most recent `acquire`; used by LRU eviction.
    pub last_used: std::sync::Mutex<Instant>,
}

// SAFETY: `LlamaModel` is an opaque C type that is safe to share across
// threads — llama.cpp's model handle is immutable after load. The
// `last_used` mutex is `Sync` by construction.
unsafe impl Send for LoadedModel {}
unsafe impl Sync for LoadedModel {}

impl LoadedModel {
    /// Returns the current outstanding reference count.
    pub fn refcount(&self) -> usize {
        self.refcount.load(Ordering::Acquire)
    }

    /// Returns `true` if no `Arc` handles are currently outstanding.
    pub fn is_idle(&self) -> bool {
        self.refcount() == 0
    }

    /// Bump the refcount and refresh `last_used`.
    fn acquire_ref(&self) {
        self.refcount.fetch_add(1, Ordering::AcqRel);
        if let Ok(mut guard) = self.last_used.lock() {
            *guard = Instant::now();
        }
    }

    /// Decrement the refcount, saturating at zero.
    fn release_ref(&self) {
        // Saturating subtract so a stray `release` cannot underflow.
        let _ = self.refcount.fetch_update(Ordering::AcqRel, Ordering::Acquire, |v| {
            if v == 0 { None } else { Some(v - 1) }
        });
    }
}

impl Drop for LoadedModel {
    fn drop(&mut self) {
        debug!(model.id = %self.id, "dropping LoadedModel — freeing FFI handle");
        // SAFETY: `self.handle` was obtained from `load_model` and has not
        // been freed yet — `Drop` is the only place we release it. The pool
        // guarantees no other code path holds a raw pointer to this handle
        // after the `Arc` count reaches zero.
        unsafe { free_model(self.handle) };
    }
}

// ─── Model Pool ────────────────────────────────────────────────────────────

/// A pool of loaded GGUF models with reference counting and LRU eviction.
pub struct ModelPool {
    /// Resident models keyed by id.
    models: HashMap<ModelId, Arc<LoadedModel>>,
    /// Maximum total bytes the pool will hold resident.
    capacity_bytes: u64,
    /// Current total bytes held resident.
    used_bytes: u64,
}

impl ModelPool {
    /// Construct a new, empty pool with the given byte budget.
    pub fn new(capacity_bytes: u64) -> Self {
        info!(capacity_bytes, "creating ModelPool");
        Self {
            models: HashMap::new(),
            capacity_bytes,
            used_bytes: 0,
        }
    }

    /// Returns the pool's byte capacity.
    pub fn capacity_bytes(&self) -> u64 {
        self.capacity_bytes
    }

    /// Returns the bytes currently held resident.
    pub fn used_bytes(&self) -> u64 {
        self.used_bytes
    }

    /// Returns the bytes still free in the pool.
    pub fn free_bytes(&self) -> u64 {
        self.capacity_bytes.saturating_sub(self.used_bytes)
    }

    /// Returns the number of resident models.
    pub fn len(&self) -> usize {
        self.models.len()
    }

    /// `true` if no models are resident.
    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }

    /// Returns `true` if a model with the given id is resident.
    pub fn contains(&self, id: &str) -> bool {
        self.models.contains_key(id)
    }

    /// Load `path` into the pool under `id`.
    ///
    /// If a model with the same id is already resident, this is a no-op (and
    /// returns `Ok(())`). If the new model would overflow the pool's byte
    /// budget, the caller may need to [`evict_lru`] first.
    ///
    /// [`evict_lru`]: Self::evict_lru
    pub fn load(&mut self, id: impl Into<ModelId>, path: impl AsRef<Path>) -> Result<(), PoolError> {
        let id = id.into();
        let path = path.as_ref().to_path_buf();

        if self.models.contains_key(&id) {
            debug!(model.id = %id, "model already resident, skipping load");
            return Ok(());
        }

        let size_bytes = path
            .metadata()
            .map(|m| m.len())
            .unwrap_or(0);

        if size_bytes > self.free_bytes() {
            warn!(
                model.id = %id,
                needed = size_bytes,
                free = self.free_bytes(),
                "not enough free bytes to load model"
            );
            return Err(PoolError::Oom {
                needed: size_bytes,
                free: self.free_bytes(),
            });
        }

        info!(model.id = %id, path = %path.display(), size_bytes, "loading model into pool");

        // TODO(v1): pass real LlamaModelParams sourced from a config struct.
        let handle = load_model(
            path.to_str().unwrap_or(""),
            crate::bindings::LlamaModelParams::default(),
        )
        .map_err(|e| {
            error!(model.id = %id, error = %e, "load_model failed");
            PoolError::LoadFailed(e.to_string())
        })?;

        let loaded = Arc::new(LoadedModel {
            id: id.clone(),
            path,
            handle,
            size_bytes,
            refcount: AtomicUsize::new(0),
            last_used: std::sync::Mutex::new(Instant::now()),
        });

        self.models.insert(id, loaded);
        self.used_bytes += size_bytes;
        Ok(())
    }

    /// Unload a model from the pool.
    ///
    /// Returns [`PoolError::StillReferenced`] if there are outstanding
    /// `Arc<LoadedModel>` handles — the caller must release them first (or
    /// force-evict via [`evict_lru`]).
    ///
    /// [`evict_lru`]: Self::evict_lru
    pub fn unload(&mut self, id: &str) -> Result<(), PoolError> {
        let arc = self
            .models
            .get(id)
            .ok_or_else(|| PoolError::NotFound(id.to_string()))?;

        let outstanding = arc.refcount();
        if outstanding > 0 {
            warn!(model.id = id, outstanding, "cannot unload — still referenced");
            return Err(PoolError::StillReferenced(outstanding));
        }

        // Strong count is 1 (only the pool's HashMap slot) — safe to drop.
        let removed = self.models.remove(id);
        if let Some(removed) = removed {
            self.used_bytes = self.used_bytes.saturating_sub(removed.size_bytes);
            info!(model.id = id, "model unloaded");
        }
        Ok(())
    }

    /// Acquire an `Arc` handle to a resident model, bumping its refcount.
    ///
    /// The caller MUST pair every `acquire` with a [`release`] call to avoid
    /// leaking refcount and pinning the model in memory.
    ///
    /// [`release`]: Self::release
    pub fn acquire(&mut self, id: &str) -> Result<Arc<LoadedModel>, PoolError> {
        let arc = self
            .models
            .get(id)
            .ok_or_else(|| PoolError::NotFound(id.to_string()))?
            .clone();

        arc.acquire_ref();
        debug!(model.id = id, refcount = arc.refcount(), "acquired model");
        Ok(arc)
    }

    /// Release a previously-acquired handle.
    ///
    /// In v0 this simply decrements the model's atomic refcount. The `Arc`
    /// itself is dropped by the caller when it goes out of scope.
    pub fn release(&mut self, id: &str) {
        if let Some(arc) = self.models.get(id) {
            arc.release_ref();
            debug!(model.id = id, refcount = arc.refcount(), "released model");
        } else {
            warn!(model.id = id, "release() called for unknown model");
        }
    }

    /// Evict the least-recently-used *idle* models until at least
    /// `target_bytes` are free, returning the number of bytes freed.
    ///
    /// Models with outstanding references are skipped — eviction is always
    /// safe and never interrupts in-flight inference.
    pub fn evict_lru(&mut self, target_bytes: u64) -> Result<usize, PoolError> {
        let mut freed: usize = 0;

        // Collect candidates (idle models) sorted by last_used ascending.
        let mut candidates: Vec<(ModelId, Instant, u64)> = self
            .models
            .iter()
            .filter_map(|(id, m)| {
                if m.is_idle() {
                    let last = *m.last_used.lock().ok()?;
                    Some((id.clone(), last, m.size_bytes))
                } else {
                    None
                }
            })
            .collect();
        candidates.sort_by_key(|(_, last, _)| *last);

        for (id, _last, size) in candidates {
            if self.free_bytes() >= target_bytes {
                break;
            }
            match self.unload(&id) {
                Ok(()) => {
                    freed = freed.saturating_add(size as usize);
                    info!(model.id = %id, freed_bytes = size, "evicted LRU model");
                }
                Err(PoolError::StillReferenced(n)) => {
                    warn!(model.id = %id, outstanding = n, "skipping LRU candidate — now referenced");
                }
                Err(e) => {
                    error!(model.id = %id, error = %e, "eviction failed");
                    return Err(e);
                }
            }
        }

        Ok(freed)
    }
}

impl Default for ModelPool {
    fn default() -> Self {
        // v0 default: 8 GiB budget.
        Self::new(8 * 1024 * 1024 * 1024)
    }
}

// v0: stub implementation
