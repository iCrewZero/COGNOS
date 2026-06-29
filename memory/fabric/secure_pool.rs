//! Secure pool — pre-allocates a large mlock'd region at startup and carves sub-allocations from it. Eliminates page-fault stalls during inference.
//!
//! Calling `mlock` per secret at runtime works, but it has two problems:
//!   1. `mlock` is a syscall — it can stall for tens of microseconds while
//!      the kernel walks the page tables, which is fatal during LLM
//!      inference where we want sub-ms scheduling.
//!   2. `RLIMIT_MEMLOCK` is often tight; per-call locking can exhaust it
//!      transiently and force a fallback to the swap-prone default allocator.
//!
//! [`SecurePool`] solves both by grabbing one large mlock'd arena at startup
//! and carving sub-allocations out of it using a buddy allocator. Acquire /
//! release are then pure userspace bitmap operations — no syscalls, no page
//! faults.
//!
//! v0: stub implementation — the pool is laid out but the buddy allocator
//! returns the first fit and never splits blocks. v1 lands the real buddy
//! splitting / coalescing logic.

use std::collections::HashMap;
use std::ptr::NonNull;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, info, instrument, warn};

// ─── Errors ──────────────────────────────────────────────────────────────────

/// Errors returned by [`SecurePool`] operations.
#[derive(Debug, Error)]
pub enum PoolError {
    /// The pool was unable to allocate the backing arena at startup.
    #[error("arena allocation failed for {size} bytes: {detail}")]
    ArenaFailed { size: usize, detail: String },
    /// `mlock` of the arena failed (usually `RLIMIT_MEMLOCK` too low).
    #[error("mlock of arena failed: {0}")]
    MlockFailed(String),
    /// No contiguous block large enough to satisfy the request.
    #[error("pool exhausted: requested {requested}, largest free {largest_free}")]
    Exhausted {
        requested: usize,
        largest_free: usize,
    },
    /// The caller passed a stale or unknown [`BlockHandle`].
    #[error("unknown block handle: {0}")]
    UnknownHandle(usize),
    /// The caller passed a zero or absurdly large size.
    #[error("invalid size: {0}")]
    InvalidSize(usize),
}

// ─── Types ───────────────────────────────────────────────────────────────────

/// Opaque handle returned by [`SecurePool::acquire`]. Internally the offset
/// of the block within the arena.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BlockHandle {
    /// Byte offset from `base` where this block starts.
    pub offset: usize,
    /// Log2 of the buddy-block size class. v0 always uses the requested
    /// size rounded up; v1 uses the real buddy order.
    pub order: u32,
}

/// One free block in the buddy free-list.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct FreeBlock {
    /// Offset of the block within the arena.
    pub offset: usize,
    /// Size in bytes (always a power of two in v1).
    pub size: usize,
}

/// Snapshot of pool utilisation.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct PoolStats {
    /// Total arena size in bytes.
    pub total: usize,
    /// Bytes currently handed out via [`SecurePool::acquire`].
    pub allocated: usize,
    /// Bytes still free (sum of all [`FreeBlock`] sizes).
    pub free: usize,
    /// Fragmentation ratio in [0.0, 1.0]: `1.0 - (largest_free / free)`.
    pub fragmentation: f32,
}

// ─── SecurePool ──────────────────────────────────────────────────────────────

/// Pre-allocated, mlock'd arena with a (v0: stubbed) buddy allocator.
pub struct SecurePool {
    /// Base of the arena. The arena is `total` bytes long and entirely
    /// `mlock`'d at construction time.
    base: NonNull<u8>,
    /// Total arena size in bytes.
    total: usize,
    /// Free-list, sorted by `offset`. v0: a single block covering the whole
    /// arena is split lazily; v1: per-order buddy lists.
    free_list: Vec<FreeBlock>,
    /// `handle.offset -> BlockHandle` for every live allocation.
    allocated: HashMap<usize, BlockHandle>,
}

// `SecurePool` owns a raw allocation but never shares `&mut` references to it
// across threads in v0.
unsafe impl Send for SecurePool {}
unsafe impl Sync for SecurePool {}

impl SecurePool {
    /// Allocate and mlock the arena. Must be called once at startup.
    #[instrument(skip_all, fields(total_bytes))]
    pub fn new(total_bytes: usize) -> Result<Self, PoolError> {
        if total_bytes == 0 || !total_bytes.is_power_of_two() {
            // v1 will round up; v0 insists on a power of two so the buddy
            // math is clean.
            return Err(PoolError::InvalidSize(total_bytes));
        }

        // TODO(v1): mmap MAP_PRIVATE | MAP_ANONYMOUS, then mlock the whole
        //           range. For v0 we synthesize a non-null base pointer so
        //           the bookkeeping round-trips through tests.
        let base_addr = total_bytes as *mut u8; // intentionally non-null, never dereferenced in v0
        let base = NonNull::new(base_addr).ok_or(PoolError::ArenaFailed {
            size: total_bytes,
            detail: "synthesized base pointer was null".to_string(),
        })?;

        info!(total_bytes, "secure pool: arena created (stub)");

        Ok(Self {
            base,
            total: total_bytes,
            free_list: vec![FreeBlock {
                offset: 0,
                size: total_bytes,
            }],
            allocated: HashMap::new(),
        })
    }

    /// Total arena size in bytes.
    pub fn total(&self) -> usize {
        self.total
    }

    /// Acquire `size` bytes from the pool. Returns a handle that must later
    /// be passed to [`SecurePool::release`].
    #[instrument(skip(self), fields(size))]
    pub fn acquire(&mut self, size: usize) -> Result<BlockHandle, PoolError> {
        if size == 0 {
            return Err(PoolError::InvalidSize(0));
        }

        // v0: first-fit over the free-list. v1: buddy-allocator split.
        let idx = self
            .free_list
            .iter()
            .position(|b| b.size >= size)
            .ok_or_else(|| {
                let largest = self.free_list.iter().map(|b| b.size).max().unwrap_or(0);
                PoolError::Exhausted {
                    requested: size,
                    largest_free: largest,
                }
            })?;

        let block = self.free_list.swap_remove(idx);
        let handle = BlockHandle {
            offset: block.offset,
            order: order_of(size),
        };

        if block.size > size {
            // Push the remainder back as a free block.
            self.free_list.push(FreeBlock {
                offset: block.offset + size,
                size: block.size - size,
            });
            // Keep the free-list sorted by offset for determinism.
            self.free_list.sort_by_key(|b| b.offset);
        }

        self.allocated.insert(handle.offset, handle);
        debug!(offset = handle.offset, size, "acquire: ok");
        Ok(handle)
    }

    /// Release a previously-acquired block back to the free-list.
    #[instrument(skip(self))]
    pub fn release(&mut self, handle: BlockHandle) {
        if self.allocated.remove(&handle.offset).is_none() {
            warn!(offset = handle.offset, "release: unknown handle");
            // TODO(v1): surface PoolError::UnknownHandle once the caller
            //           signature is updated.
            return;
        }

        // v0: just push back. v1: coalesce with neighbours of the same order.
        let size = size_of_order(handle.order);
        self.free_list.push(FreeBlock {
            offset: handle.offset,
            size,
        });
        self.free_list.sort_by_key(|b| b.offset);
        debug!(offset = handle.offset, size, "release: ok");
    }

    /// Snapshot pool utilisation. Useful for `cognos status` and for the
    /// scheduler's OOM-avoidance heuristic.
    pub fn stats(&self) -> PoolStats {
        let allocated: usize = self
            .allocated
            .values()
            .map(|h| size_of_order(h.order))
            .sum();
        let free: usize = self.free_list.iter().map(|b| b.size).sum();
        let largest_free = self.free_list.iter().map(|b| b.size).max().unwrap_or(0);
        let fragmentation = if free == 0 {
            0.0
        } else {
            1.0 - (largest_free as f32 / free as f32)
        };
        PoolStats {
            total: self.total,
            allocated,
            free,
            fragmentation,
        }
    }

    /// Borrow the base pointer. Used by integration tests and by the
    /// `cognos-memory` service to pass the arena to mmap-backed subsystems.
    #[allow(dead_code)]
    pub fn base(&self) -> NonNull<u8> {
        self.base
    }
}

impl Drop for SecurePool {
    fn drop(&mut self) {
        // TODO(v1): munlock + munmap the arena. v0 holds a synthesized
        // pointer and does nothing here.
        info!(total = self.total, "secure pool: drop (stub)");
    }
}

// ─── Buddy math ──────────────────────────────────────────────────────────────

/// Minimum buddy-block size (bytes). Smaller requests are rounded up.
const MIN_BLOCK: usize = 16;

/// Order of `n` bytes: smallest `k` such that `MIN_BLOCK << k >= n`.
fn order_of(n: usize) -> u32 {
    if n <= MIN_BLOCK {
        return 0;
    }
    let mut k = 0u32;
    let mut s = MIN_BLOCK;
    while s < n {
        s <<= 1;
        k += 1;
    }
    k
}

/// Size of a block of order `k`. `MIN_BLOCK << k`.
fn size_of_order(k: u32) -> usize {
    MIN_BLOCK << k
}

// TODO(v1): replace `acquire`/`release` with a real buddy allocator:
//   - Per-order free-lists `Vec<Vec<FreeBlock>>` indexed by order.
//   - On acquire: find smallest order >= request, split down recursively.
//   - On release: coalesce with buddy if both free, recurse upward.

#[cfg(test)]
mod _compile_only {
    // v0 ships no tests — kept here so future test additions compile cleanly.
    use super::*;
    #[allow(dead_code)]
    fn _ensure_send_sync() {
        fn _assert<T: Send + Sync>() {}
        _assert::<SecurePool>();
    }
}
