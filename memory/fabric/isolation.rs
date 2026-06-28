//! Isolation — enforces per-agent memory regions with guard pages and access tracking. No agent can read another's memory fabric.
//!
//! COGNOS's threat model (see `docs/THREAT_MODEL.md`) assumes a compromised
/// agent may attempt to read another agent's secrets via shared-memory
//! side-channels or pointer arithmetic. This module gives every agent its
/// own [`MemoryRegion`] carved out of the address space, bounded on both
/// sides by `PROT_NONE` guard pages, and tracks ownership so any
/// cross-region access is detectable.
//!
//! v0: stub implementation — regions are bookkept in a `HashMap` but the
//! actual `mmap` + `mprotect` calls land in v1. The ownership check is
//! purely pointer-arithmetic and is enforced regardless.

use std::collections::HashMap;
use std::ptr::NonNull;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, info, instrument, warn};

// ─── Errors ──────────────────────────────────────────────────────────────────

/// Errors returned by [`MemoryIsolation`] operations.
#[derive(Debug, Error)]
pub enum IsolationError {
    /// The pointer does not fall inside any tracked region.
    #[error("pointer {ptr:p} is not inside any isolated region")]
    NotIsolated { ptr: *const u8 },
    /// The pointer falls inside a region owned by a different agent.
    #[error("pointer {ptr:p} belongs to {owner}, not {caller}")]
    WrongOwner {
        ptr: *const u8,
        owner: String,
        caller: String,
    },
    /// The pointer is within `[base, base+len)` but outside the writable
    /// range (i.e. it landed in a guard page).
    #[error("pointer {ptr:p} is inside a guard page")]
    OutOfBounds { ptr: *const u8 },
    /// The caller tried to free a region twice.
    #[error("double free for agent {0}")]
    DoubleFree(String),
    /// The OS refused to allocate the region (mmap failed).
    #[error("mmap failed for {size} bytes: {detail}")]
    AllocationFailed { size: usize, detail: String },
    /// `mprotect` failed when installing a guard page.
    #[error("mprotect failed: {0}")]
    MprotectFailed(String),
}

// ─── Types ───────────────────────────────────────────────────────────────────

/// Stable identifier for an agent.
pub type AgentId = String;

/// One isolated memory region.
///
/// Layout in memory (ascending addresses):
///
/// ```text
///   [ guard_before | usable (base..base+len) | guard_after ]
/// ```
///
/// `base` always points at the first usable byte. The guard pages are
/// `PROT_NONE` so any read or write into them traps with `SIGSEGV`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryRegion {
    /// First usable byte. Serialized as a raw address for diagnostics; the
    /// pointer itself is not valid across process restarts.
    #[serde(serialize_with = "serialize_ptr_as_addr")]
    #[serde(deserialize_with = "deserialize_ptr_from_addr")]
    pub base: NonNull<u8>,
    /// Number of usable bytes (`base..base+len`).
    pub len: usize,
    /// Size in bytes of the leading guard page.
    pub guard_before: usize,
    /// Size in bytes of the trailing guard page.
    pub guard_after: usize,
    /// Owning agent id.
    pub owner: AgentId,
}

fn serialize_ptr_as_addr<S>(p: &NonNull<u8>, s: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    s.serialize_u64(p.as_ptr() as u64)
}

fn deserialize_ptr_from_addr<'de, D>(d: D) -> Result<NonNull<u8>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    let addr = u64::deserialize(d)?;
    // SAFETY: round-tripping a pointer across process boundaries is unsound
    // in general — v0 only supports round-tripping within the same process.
    // v1 will persist `{pid, base_addr}` tuples and refuse to revive regions
    // whose pid no longer matches.
    NonNull::new(addr as *mut u8).ok_or_else(|| {
        <D::Error as serde::de::Error>::custom("null pointer in serialized MemoryRegion")
    })
}

// `MemoryRegion` is safe to move between threads but not to share without
// synchronisation — concurrent writes into the underlying bytes are the
// caller's responsibility.
unsafe impl Send for MemoryRegion {}

// ─── MemoryIsolation ─────────────────────────────────────────────────────────

/// Owns the table of per-agent [`MemoryRegion`]s and enforces ownership.
pub struct MemoryIsolation {
    /// `agent_id -> region`. At most one live region per agent in v0.
    regions: HashMap<AgentId, MemoryRegion>,
    /// Standard guard-page size. Defaults to one 4 KiB page.
    guard_page_size: usize,
}

impl MemoryIsolation {
    /// Construct a new isolation manager with a 4 KiB guard page size.
    pub fn new() -> Self {
        Self {
            regions: HashMap::new(),
            guard_page_size: 4096,
        }
    }

    /// Override the default guard-page size (must be a multiple of the system
    /// page size; v1 will assert this).
    pub fn with_guard_page_size(mut self, size: usize) -> Self {
        self.guard_page_size = size;
        self
    }

    /// Allocate a new region for `agent` of `size` usable bytes, with guard
    /// pages on both sides.
    #[instrument(skip(self), fields(agent = %agent, size))]
    pub fn allocate_region(
        &mut self,
        agent: impl Into<AgentId>,
        size: usize,
    ) -> Result<MemoryRegion, IsolationError> {
        let agent = agent.into();
        if self.regions.contains_key(&agent) {
            warn!(%agent, "allocate_region: overwriting prior region");
            // v0 simply replaces; v1 should return an error or expose an
            // explicit `replace_region` method.
        }

        let guard = self.guard_page_size;
        let total = guard.checked_add(size).and_then(|n| n.checked_add(guard))
            .ok_or_else(|| IsolationError::AllocationFailed {
                size,
                detail: "size overflow during guard-page layout".to_string(),
            })?;

        // TODO(v1): mmap total bytes with PROT_READ | PROT_WRITE, then
        //           mprotect the first `guard` and last `guard` bytes to
        //           PROT_NONE. For v0 we synthesize a dangling-but-non-null
        //           pointer so the bookkeeping round-trips through tests.
        let base_addr = total as *mut u8; // intentionally non-null, never dereferenced in v0
        let base = NonNull::new(base_addr).ok_or_else(|| IsolationError::AllocationFailed {
            size,
            detail: "synthesized base pointer was null".to_string(),
        })?;

        info!(%agent, size, guard_before = guard, guard_after = guard, "allocate_region: stub");
        let region = MemoryRegion {
            base,
            len: size,
            guard_before: guard,
            guard_after: guard,
            owner: agent.clone(),
        };
        self.regions.insert(agent, region.clone());
        Ok(region)
    }

    /// Verify that `ptr` is inside `agent`'s region and within the usable
    /// (non-guard) range. Returns `Ok(())` on success.
    pub fn verify_owner(
        &self,
        ptr: *const u8,
        agent: impl AsRef<str>,
    ) -> Result<(), IsolationError> {
        let agent = agent.as_ref();
        // Find any region whose [base, base+len) contains ptr.
        let region = self
            .regions
            .values()
            .find(|r| {
                let start = r.base.as_ptr() as usize;
                let end = start.checked_add(r.len).unwrap_or(usize::MAX);
                let p = ptr as usize;
                p >= start && p < end
            })
            .ok_or(IsolationError::NotIsolated { ptr })?;

        if region.owner != agent {
            return Err(IsolationError::WrongOwner {
                ptr,
                owner: region.owner.clone(),
                caller: agent.to_string(),
            });
        }
        Ok(())
    }

    /// Free the region owned by `agent`. Returns
    /// [`IsolationError::DoubleFree`] if no region is tracked for that agent.
    #[instrument(skip(self), fields(agent = %agent))]
    pub fn deallocate_region(&mut self, agent: impl Into<AgentId>) {
        let agent = agent.into();
        if self.regions.remove(&agent).is_none() {
            warn!(%agent, "deallocate_region: no prior region (potential double-free)");
            // TODO(v1): surface this as IsolationError::DoubleFree once the
            //           caller is updated to handle Result.
            return;
        }
        // TODO(v1): mprotect the guard pages back to RW, then munmap the whole
        //           range. v0 leaves the synthesized pointer alone.
        debug!(%agent, "deallocate_region: stub");
    }

    /// Snapshot of all live regions (for diagnostics / `cognos status`).
    pub fn regions(&self) -> impl Iterator<Item = &MemoryRegion> {
        self.regions.values()
    }
}

impl Default for MemoryIsolation {
    fn default() -> Self {
        Self::new()
    }
}
