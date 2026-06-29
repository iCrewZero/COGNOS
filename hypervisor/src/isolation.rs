//! Hypervisor isolation — enforces hard boundaries between VMs and between VMs and the host. Each VM gets its own address space, capability set, and resource quota. No direct memory sharing.
//!
//! The isolation layer is the single enforcement point for three invariants:
//!
//! 1. **Spatial isolation** — each VM has a private [`AddressSpace`]; no VM
//!    can read or write another VM's memory, and the host cannot reach into
//!    VM memory without going through a host call.
//! 2. **Capability isolation** — every privileged operation is gated by a
//!    [`Capability`](crate::authority_vm::Capability) token held by the VM;
//!    the lattice is consulted to decide implication.
//! 3. **Resource isolation** — each VM operates under a [`ResourceQuota`]
//!    bounding memory, CPU time, syscall count, and open file descriptors.
//!
//! v0: stub implementation. The page table, allocator, and capability lattice
//! are stubbed; only the public surface and error types are present.

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, trace};
use uuid::Uuid;

// TODO(v1): once real capability enforcement lands, re-import `warn` from
// tracing for the denial-path log line. v0 keeps only the log levels in use.
use crate::authority_vm::{Capability, VmId};

// v0: stub implementation

// ─── Page Table ─────────────────────────────────────────────────────────────────

/// Page-granularity permissions for a region of VM memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PagePerms {
    /// Page may be read.
    pub read: bool,
    /// Page may be written.
    pub write: bool,
    /// Page may be executed (i.e. fetched as bytecode).
    pub execute: bool,
}

impl PagePerms {
    /// Read-Write-Execute (dangerous; reserved for trusted loaders).
    pub const RWX: Self = Self {
        read: true,
        write: true,
        execute: true,
    };

    /// Read-Only (e.g. constant pool).
    pub const RO: Self = Self {
        read: true,
        write: false,
        execute: false,
    };

    /// Read-Write (e.g. heap, stack).
    pub const RW: Self = Self {
        read: true,
        write: true,
        execute: false,
    };

    /// Read-Execute (e.g. bytecode segment, no self-modification).
    pub const RX: Self = Self {
        read: true,
        write: false,
        execute: true,
    };
}

/// A single page in a VM's [`AddressSpace`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page {
    /// Base address of the page (byte offset within the address space).
    pub base: usize,
    /// Length of the page in bytes.
    pub len: usize,
    /// Permissions for this page.
    pub perms: PagePerms,
    /// Whether the page is currently backed by host memory.
    ///
    /// `false` pages are demand-paged and trap on first access.
    pub mapped: bool,
}

/// A VM's private address space — a flat list of [`Page`]s.
///
/// In v0 this is a simple `Vec<Page>`; v1 will likely move to a real page
/// table or slab allocator. There is intentionally no notion of *shared*
/// pages here: cross-VM communication goes through host calls, not memory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddressSpace {
    /// Pages in the address space, ordered by `base`.
    pub pages: Vec<Page>,
    /// Page size in bytes (typically 4096).
    pub page_size: usize,
}

impl Default for AddressSpace {
    fn default() -> Self {
        // TODO(v1): build initial code/data/stack segments from the VmProgram.
        Self {
            pages: Vec::new(),
            page_size: 4096,
        }
    }
}

impl AddressSpace {
    /// Look up the page containing `addr`, if any.
    pub fn page_for(&self, addr: usize) -> Option<&Page> {
        // TODO(v1): binary search once pages are sorted at construction.
        self.pages.iter().find(|p| addr >= p.base && addr < p.base + p.len)
    }

    /// Check whether `addr..addr+len` is accessible with the given `perms`.
    pub fn check_access(
        &self,
        _addr: usize,
        _len: usize,
        _required: PagePerms,
    ) -> Result<(), IsolationError> {
        // TODO(v1): implement; v0 always permits.
        Ok(())
    }
}

// ─── Resource Quota ─────────────────────────────────────────────────────────────

/// Per-VM resource quota. Enforced by [`VmIsolation::check_quota`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceQuota {
    /// Maximum resident memory in bytes.
    pub max_memory_bytes: u64,
    /// Maximum CPU time in seconds (wall-clock, billed to the VM).
    pub max_cpu_seconds: u64,
    /// Maximum number of syscalls (host calls) the VM may issue.
    pub max_syscalls: u64,
    /// Maximum number of open file descriptors the VM may hold.
    pub max_fd: u64,
}

impl Default for ResourceQuota {
    fn default() -> Self {
        // TODO(v1): pull defaults from a config file rather than hard-coding.
        Self {
            max_memory_bytes: 256 * 1024 * 1024, // 256 MiB
            max_cpu_seconds: 60,                  // 1 minute
            max_syscalls: 10_000,
            max_fd: 64,
        }
    }
}

/// A resource tracked by the quota system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Resource {
    /// Resident memory in bytes.
    Memory,
    /// CPU seconds consumed.
    Cpu,
    /// Syscalls issued.
    Syscalls,
    /// Open file descriptors held.
    Fd,
}

// ─── Capability Lattice ─────────────────────────────────────────────────────────

/// A partial order over [`Capability`]s. The lattice decides whether holding
/// capability `A` implies capability `B`.
///
/// This is a stub mirror of `hal::capability_lattice::CapabilityLattice`;
/// TODO(v1): share the real implementation rather than redefining it here.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CapabilityLattice {
    // TODO(v1): edges: HashMap<Capability, HashSet<Capability>>.
}

impl CapabilityLattice {
    /// Returns `true` if holding `a` implies holding `b`.
    ///
    /// TODO(v1): traverse the real lattice.
    pub fn implies(&self, a: &Capability, b: &Capability) -> bool {
        a == b
    }
}

// ─── Errors ─────────────────────────────────────────────────────────────────────

/// Errors returned by isolation enforcement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Error)]
pub enum IsolationError {
    /// The requested operation would exceed the VM's resource quota.
    #[error("out of quota: {0}")]
    OutOfQuota(String),
    /// The VM lacks the required capability.
    #[error("capability denied: {0}")]
    CapabilityDenied(String),
    /// The provided pointer does not refer to a valid mapping in this VM.
    #[error("invalid pointer: {0:#x}")]
    InvalidPointer(usize),
    /// The operation would cross a VM boundary (e.g. access another VM's memory).
    #[error("cross-VM access denied")]
    CrossVmAccess,
}

// ─── VmIsolation ────────────────────────────────────────────────────────────────

/// Per-VM isolation context. Owns the VM's address space, capability lattice
/// view, and resource quota. All sensitive operations route through this
/// struct so that enforcement cannot be bypassed by accident.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmIsolation {
    /// The VM this context belongs to.
    pub vm_id: VmId,
    /// The VM's private address space.
    pub address_space: AddressSpace,
    /// The capability lattice used to decide implication.
    pub capability_lattice: CapabilityLattice,
    /// The resource quota bound to this VM.
    pub resource_quota: ResourceQuota,
}

impl VmIsolation {
    /// Construct a new isolation context for `vm_id` with the given quota.
    pub fn new(vm_id: VmId, resource_quota: ResourceQuota) -> Self {
        Self {
            vm_id,
            address_space: AddressSpace::default(),
            capability_lattice: CapabilityLattice::default(),
            resource_quota,
        }
    }

    /// Allocate `size` bytes of host memory and map them into the VM's
    /// address space.
    ///
    /// # Returns
    ///
    /// A raw pointer into host memory. The pointer is *only* valid for use
    /// by the host side of a host-call handler; it must never be exposed to
    /// bytecode. The VM sees only the offset within its address space.
    ///
    /// # Safety (caller obligations)
    ///
    /// TODO(v1): once `allocate` is real, callers must:
    /// - never read/write the returned pointer after `deallocate` is called,
    /// - never share the pointer with another VM,
    /// - always pair every `allocate` with exactly one `deallocate`.
    pub fn allocate(&mut self, _size: usize) -> Result<*mut u8, IsolationError> {
        debug!(vm = %self.vm_id, size = _size, "allocate requested");

        // TODO(v1): back this with mmap / a slab; for v0 return null.
        // SAFETY: the null pointer is never dereferenced in v0 — callers must
        // treat an `Ok` return as a no-op reservation, not a usable pointer.
        // v1 will replace this with a real allocator returning a valid pointer.
        trace!(vm = %self.vm_id, "v0 stub: returning null pointer");
        Ok(std::ptr::null_mut())
    }

    /// Deallocate a previously-allocated pointer and unmap its pages.
    ///
    /// # Safety (caller obligations)
    ///
    /// `ptr` must have been returned by a prior call to [`Self::allocate`]
    /// on this same `VmIsolation`, and must not already have been
    /// deallocated. Use-after-free is undefined behavior.
    ///
    /// TODO(v1): the v0 stub is a no-op; the v1 implementation will `munmap`
    /// or return the slab to its pool.
    pub fn deallocate(&mut self, _ptr: *mut u8) {
        debug!(vm = %self.vm_id, ptr = ?_ptr, "deallocate requested");
        // SAFETY: v0 stub — `allocate` returned null and no real memory was
        // ever mapped, so there is nothing to free. v1 will perform a real
        // deallocation here under the safety contract documented above.
        trace!(vm = %self.vm_id, "v0 stub: deallocate is a no-op");
    }

    /// Check that this VM holds the given capability.
    ///
    /// Returns `Ok(())` if held, or [`IsolationError::CapabilityDenied`]
    /// otherwise. TODO(v1): consult the real capability set on the VM, not
    /// just the lattice.
    pub fn check_capability(&self, _cap: &Capability) -> Result<(), IsolationError> {
        // TODO(v1): look up the VM's capability set and ask the lattice whether
        // any held capability implies `_cap`.
        debug!(vm = %self.vm_id, cap = ?_cap, "capability check (v0: allow)");
        Ok(())
    }

    /// Check that the VM has not exceeded its quota for `resource`.
    ///
    /// TODO(v1): track per-VM usage counters and check them here. v0 always
    /// permits.
    pub fn check_quota(&self, resource: Resource) -> Result<(), IsolationError> {
        debug!(vm = %self.vm_id, ?resource, "quota check (v0: allow)");
        // TODO(v1): maintain a usage map and reject when over-quota.
        let _ = resource;
        Ok(())
    }
}

impl Default for VmIsolation {
    fn default() -> Self {
        Self::new(Uuid::nil(), ResourceQuota::default())
    }
}

// ─── SAFETY ─────────────────────────────────────────────────────────────────────
//
// The only `unsafe`-adjacent surface in this module is the raw `*mut u8`
// returned by `VmIsolation::allocate`. In v0 the stub returns `null_mut()`,
// which is safe to construct and never dereferenced. The v1 implementation
// will introduce a real allocator; at that point the safety obligations
// documented on `allocate`/`deallocate` become load-bearing.

// v0: stub implementation
