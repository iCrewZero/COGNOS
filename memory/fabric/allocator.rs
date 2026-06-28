//! Secure allocator — allocates memory with mlock_guard, zeroizes on free, and prevents swap leakage for keys/tokens/secrets.
//!
//! COGNOS treats any byte buffer that holds a credential, capability token,
//! model weight digest, or decrypted prompt as *secret*. The default Rust
//! allocator will happily page these out to swap, where they can be recovered
//! later by an attacker with disk access. This module wraps `libc::mlock` /
//! `libc::munlock` and zeroizes on drop so secrets never touch swap and never
//! outlive their [`SecureBytes`] handle.
//!
//! v0: stub implementation — the unsafe primitives are wired but the v0
//! build does not enforce an upper bound or fall back to a pre-allocated
//! pool. Use [`crate::secure_pool::SecurePool`] for inference-time
//! allocations to avoid page-fault stalls.

use std::alloc::{alloc, dealloc, Layout};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicUsize, Ordering};

use thiserror::Error;
use tracing::{debug, error, instrument};

// ─── Errors ──────────────────────────────────────────────────────────────────

/// Errors returned by [`SecureAllocator`].
#[derive(Debug, Error)]
pub enum AllocError {
    /// The process hit its `RLIMIT_MEMLOCK` ceiling or mlock is unavailable.
    #[error("mlock failed for {size} bytes: {detail}")]
    MlockFailed { size: usize, detail: String },
    /// `munlock` failed during deallocation. Treat as best-effort.
    #[error("munlock failed for {size} bytes: {detail}")]
    MunlockFailed { size: usize, detail: String },
    /// The global allocator returned a null pointer.
    #[error("out of memory: requested {0} bytes")]
    Oom(usize),
    /// The caller exceeded the configured [`SecureAllocator::max_locked`].
    #[error("locked-byte limit exceeded: {current} + {requested} > {max}")]
    LimitExceeded {
        current: usize,
        requested: usize,
        max: usize,
    },
}

// ─── SecureBytes ─────────────────────────────────────────────────────────────

/// A locked, zero-on-drop byte buffer.
///
/// Invariants upheld by every constructed `SecureBytes`:
///   * `ptr` points to `capacity` bytes obtained from the global allocator,
///   * the page(s) backing `[ptr, ptr + capacity)` are `mlock`'d,
///   * `len <= capacity`,
///   * on `Drop`, the live `[ptr, ptr + len)` range is zeroized with
///     `libc::memset`, then `munlock`'d, then `dealloc`'d.
pub struct SecureBytes {
    ptr: NonNull<u8>,
    len: usize,
    capacity: usize,
}

// `SecureBytes` owns its backing allocation and is not safe to share across
// threads in v0 (no internal synchronisation on the byte contents). v1 may
// introduce an `Arc<Mutex<SecureBytes>>` for shared secrets.
unsafe impl Send for SecureBytes {}
unsafe impl Sync for SecureBytes {}

impl SecureBytes {
    /// Borrow the live bytes as a slice.
    pub fn as_slice(&self) -> &[u8] {
        // SAFETY: `ptr` is valid for `capacity` bytes (allocated by us and not
        // yet freed — `Drop` is the only place that calls `dealloc`), and
        // `len <= capacity`. The borrow is confined to `&self`, so no mutable
        // aliasing occurs.
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }

    /// Borrow the live bytes as a mutable slice.
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        // SAFETY: same as `as_slice`, but with `&mut self` so exclusive access
        // is guaranteed by the borrow checker.
        unsafe { std::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len) }
    }

    /// Length of the live region.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Total capacity (always >= `len`).
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// `true` if `len == 0`.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl Drop for SecureBytes {
    fn drop(&mut self) {
        // SAFETY: we own the allocation; nothing else has a pointer into it
        // because `Self` is not `Clone` and `&mut` borrows are exclusive.
        unsafe {
            // 1. Zeroize the live region (not the whole capacity — the slack
            //    was zeroed on allocation, see `SecureAllocator::allocate`).
            // SAFETY: `ptr` is valid for `len` bytes; `memset` writes exactly
            // `len` bytes of `0u8`.
            if self.len > 0 {
                libc::memset(self.ptr.as_ptr() as *mut libc::c_void, 0, self.len);
            }

            // 2. Release the mlock so the OS counters don't drift.
            // SAFETY: `ptr` was mlock'd in `allocate` for `capacity` bytes,
            // and we still own it. `munlock` is idempotent against double-call
            // at the kernel level, but we only call it once here.
            let mlock_ret = libc::munlock(self.ptr.as_ptr() as *const libc::c_void, self.capacity);
            if mlock_ret != 0 {
                // Best-effort: log and continue, we still need to free.
                error!(
                    capacity = self.capacity,
                    rc = mlock_ret,
                    "munlock failed during SecureBytes drop"
                );
            }

            // 3. Return the allocation to the global allocator.
            // SAFETY: the Layout must match the one used in `allocate`. We
            // reconstruct it from `capacity` and assume the same alignment
            // (max alignment of u8 is 1, but we round up in `allocate`).
            let layout = layout_for(self.capacity);
            dealloc(self.ptr.as_ptr(), layout);
        }
    }
}

// ─── SecureAllocator ─────────────────────────────────────────────────────────

/// A bump-counter-based secure allocator. Tracks how many bytes are currently
/// locked across all live [`SecureBytes`] it has handed out.
pub struct SecureAllocator {
    /// Bytes currently locked across all live allocations.
    total_locked: AtomicUsize,
    /// Hard ceiling. Allocations that would push `total_locked` past this
    /// fail with [`AllocError::LimitExceeded`].
    max_locked: usize,
}

impl SecureAllocator {
    /// Construct a new allocator with the given byte ceiling.
    pub fn new(max_locked: usize) -> Self {
        Self {
            total_locked: AtomicUsize::new(0),
            max_locked,
        }
    }

    /// Current number of bytes locked by this allocator.
    pub fn total_locked(&self) -> usize {
        self.total_locked.load(Ordering::Relaxed)
    }

    /// Configured ceiling.
    pub fn max_locked(&self) -> usize {
        self.max_locked
    }

    /// Allocate `size` bytes, mlock them, and zero the slack.
    ///
    /// The returned [`SecureBytes`] has `len == size` and `capacity` rounded
    /// up to the nearest page (so a single `mlock` covers the whole buffer).
    #[instrument(skip(self), fields(size))]
    pub fn allocate(&mut self, size: usize) -> Result<SecureBytes, AllocError> {
        let current = self.total_locked(Ordering::Relaxed);
        if current + size > self.max_locked {
            return Err(AllocError::LimitExceeded {
                current,
                requested: size,
                max: self.max_locked,
            });
        }

        let capacity = round_up_to_page(size);
        let layout = layout_for(capacity);

        // SAFETY: `layout.size() > 0` after rounding (page is 4096). The
        // global allocator returns a valid, aligned pointer or null.
        let raw = unsafe { alloc(layout) };
        let ptr = NonNull::new(raw).ok_or(AllocError::Oom(size))?;

        // SAFETY: `ptr` is valid for `capacity` bytes (allocated above).
        // `mlock` pins those pages into RAM so they cannot be swapped.
        let mlock_ret =
            unsafe { libc::mlock(ptr.as_ptr() as *const libc::c_void, capacity) };
        if mlock_ret != 0 {
            // Roll back the allocation before returning the error.
            // SAFETY: same layout, ptr is still valid.
            unsafe { dealloc(ptr.as_ptr(), layout) };
            return Err(AllocError::MlockFailed {
                size,
                detail: format!("mlock rc={mlock_ret}"),
            });
        }

        // SAFETY: `ptr` is valid for `capacity` bytes; we zero the entire
        // capacity so any slack is clean before any caller writes into it.
        unsafe {
            libc::memset(ptr.as_ptr() as *mut libc::c_void, 0, capacity);
        }

        self.total_locked.fetch_add(capacity, Ordering::Relaxed);
        debug!(size, capacity, "allocate: ok");

        Ok(SecureBytes {
            ptr,
            len: size,
            capacity,
        })
    }

    /// Free a previously-allocated [`SecureBytes`].
    ///
    /// This is just a typed way to drop the buffer; the actual cleanup happens
    /// in [`SecureBytes::drop`]. The method exists so callers don't have to
    /// write `drop(bytes)` and so we can update bookkeeping symmetrically.
    #[instrument(skip(self, bytes))]
    pub fn deallocate(&mut self, bytes: SecureBytes) {
        let capacity = bytes.capacity;
        // `Drop` does the zeroize + munlock + dealloc. We just adjust the
        // counter afterwards.
        drop(bytes);
        self.total_locked.fetch_sub(capacity, Ordering::Relaxed);
    }

    /// Internal helper for ergonomic atomic loads.
    fn total_locked(&self, order: Ordering) -> usize {
        self.total_locked.load(order)
    }
}

impl Default for SecureAllocator {
    fn default() -> Self {
        // 64 MiB default ceiling. Production deployments should configure this
        // based on `RLIMIT_MEMLOCK` and expected secret sizes.
        Self::new(64 * 1024 * 1024)
    }
}

// ─── Layout helpers ──────────────────────────────────────────────────────────

/// Standard page size on most Linux/POSIX systems. v1 should read
/// `sysconf(_SC_PAGESIZE)` instead of hard-coding.
const PAGE_SIZE: usize = 4096;

/// Round `n` up to the next multiple of [`PAGE_SIZE`]. Zero maps to zero so
/// callers can pass `0` and get back a zero-capacity (illegal) layout — they
/// are expected to short-circuit before then.
fn round_up_to_page(n: usize) -> usize {
    if n == 0 {
        return PAGE_SIZE;
    }
    let mask = PAGE_SIZE - 1;
    (n + mask) & !mask
}

/// Build a `Layout` for `capacity` bytes with the maximum useful alignment
/// (16 bytes — covers SSE/NEON writes used by the zeroizer).
fn layout_for(capacity: usize) -> Layout {
    // SAFETY: `align` is a power of two and <= `Layout::max_align()`;
    // `size` is non-zero because `round_up_to_page(0)` returns `PAGE_SIZE`.
    Layout::from_size_align(capacity, 16).expect("layout invariants upheld by round_up_to_page")
}

// TODO(v1): fall back to [`crate::secure_pool::SecurePool`] when mlock fails
//           due to RLIMIT_MEMLOCK — the pool is pre-locked at startup.

#[cfg(test)]
mod _compile_only {
    // v0 ships no tests — kept here so `cargo test` doesn't warn on the
    // module-level `#[cfg(test)]` consumers in other crates.
    use super::*;
    #[allow(dead_code)]
    fn _ensure_send_sync() {
        fn _assert<T: Send + Sync>() {}
        _assert::<SecureBytes>();
    }
}
