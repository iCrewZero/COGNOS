//! Hypervisor runtime — manages a fleet of authority VMs, schedules them, and handles host calls. Acts as the VMM for COGNOS AI agents.
//!
//! The runtime is the top-level entry point for the hypervisor crate. It owns
//! the set of live [`AuthorityVm`]s, the [`VmScheduler`] that decides which VM
//! runs next, and the registry of host-call handlers. It is the only component
//! allowed to transition a VM between [`VmState::Running`] and
//! [`VmState::AwaitingHostCall`], which keeps the scheduling boundary in one
//! place.
//!
//! v0: stub implementation. The scheduler loop, host-call dispatch, and quota
//! enforcement are stubbed; only the public surface is present.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::watch;
use tracing::{debug, info, trace, warn};
use uuid::Uuid;

use crate::authority_vm::{AgentId, AuthorityVm, Capability, HostCall, HostCallResult, VmId, VmProgram, VmState};

// v0: stub implementation

// ─── Scheduler ──────────────────────────────────────────────────────────────────

/// Scheduling policy for the [`VmScheduler`].
///
/// TODO(v1): wire `FairShare` and `Priority` into the real scheduler.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SchedPolicy {
    /// Cycle through VMs in spawn order.
    #[default]
    RoundRobin,
    /// Allocate CPU proportionally to per-VM weights.
    FairShare,
    /// Strict priority ordering; lower-priority VMs are starved if higher are ready.
    Priority,
}

/// Per-VM scheduling metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VmSchedEntry {
    /// The VM being scheduled.
    pub vm_id: VmId,
    /// Priority (higher = more important). Used by `Priority` and `FairShare`.
    pub priority: u8,
    /// Time slices consumed so far (for fair-share accounting).
    pub slices_consumed: u64,
}

/// Decides which VM runs next.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmScheduler {
    /// Scheduling policy in use.
    pub policy: SchedPolicy,
    /// Length of a single time slice, in milliseconds.
    pub time_slice: u64,
    /// The scheduling queue (ordered per policy).
    // TODO(v1): use a real priority queue; v0 keeps it as a Vec for simplicity.
    pub queue: Vec<VmSchedEntry>,
}

impl Default for VmScheduler {
    fn default() -> Self {
        Self {
            policy: SchedPolicy::RoundRobin,
            time_slice: 10, // 10 ms
            queue: Vec::new(),
        }
    }
}

impl VmScheduler {
    /// Pick the next VM to run, or `None` if the queue is empty.
    // TODO(v1): honor `policy` (RR / FairShare / Priority). v0 always picks the head.
    pub fn next(&mut self) -> Option<VmSchedEntry> {
        if self.queue.is_empty() {
            None
        } else {
            // Rotate for round-robin.
            let head = self.queue.remove(0);
            self.queue.push(head.clone());
            Some(head)
        }
    }

    /// Add a VM to the scheduler queue.
    pub fn enqueue(&mut self, vm_id: VmId, priority: u8) {
        self.queue.push(VmSchedEntry {
            vm_id,
            priority,
            slices_consumed: 0,
        });
    }

    /// Remove a VM from the queue.
    pub fn dequeue(&mut self, vm_id: VmId) {
        self.queue.retain(|e| e.vm_id != vm_id);
    }
}

// ─── Host Call Handlers ─────────────────────────────────────────────────────────

/// Discriminant for a [`HostCall`] — used as a key in the handler registry
/// so handlers can be looked up without cloning the full call.
pub type HostCallKind = u8;

/// A function that handles a host call issued by a VM.
///
/// TODO(v1): make this `async` and pass a `&HypervisorRuntime` context so
/// handlers can route to other VMs, the HAL, memory, etc. v0 keeps it as a
/// plain function pointer for simplicity.
pub type HostHandler = fn(&HostCall) -> HostCallResult;

// ─── VM Info ────────────────────────────────────────────────────────────────────

/// A snapshot of a VM's state, exposed via [`HypervisorRuntime::list_vms`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct VmInfo {
    /// The VM's identifier.
    pub id: VmId,
    /// The owning agent.
    pub agent: AgentId,
    /// Current execution state.
    pub state: VmState,
    /// CPU seconds consumed so far.
    pub cpu_seconds: u64,
    /// Resident memory in bytes.
    pub memory_bytes: u64,
    /// Number of syscalls issued.
    pub syscalls: u64,
}

impl From<&AuthorityVm> for VmInfo {
    fn from(vm: &AuthorityVm) -> Self {
        // TODO(v1): pull cpu_seconds/memory_bytes/syscalls from the quota counters
        // maintained by VmIsolation, not from the VM struct.
        Self {
            id: vm.id,
            agent: vm.agent,
            state: vm.state.clone(),
            cpu_seconds: 0,
            memory_bytes: vm.memory.bytes.len() as u64,
            syscalls: 0,
        }
    }
}

// ─── Errors ─────────────────────────────────────────────────────────────────────

/// Errors returned by the hypervisor runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Error)]
pub enum HypervisorError {
    /// A VM could not be spawned (e.g. quota exhausted at spawn time).
    #[error("spawn failed: {0}")]
    SpawnFailed(String),
    /// The requested VM does not exist.
    #[error("VM not found: {0}")]
    VmNotFound(VmId),
    /// The VM exceeded its resource quota.
    #[error("quota exceeded: {0}")]
    QuotaExceeded(String),
    /// The VM issued a host call with no registered handler.
    #[error("host call unregistered: {0}")]
    HostCallUnregistered(String),
}

// ─── HypervisorRuntime ──────────────────────────────────────────────────────────

/// The hypervisor runtime. Owns all live VMs and the scheduler, and is the
/// single point of truth for VM lifecycle.
#[derive(Debug, Serialize, Deserialize)]
pub struct HypervisorRuntime {
    /// Live VMs, keyed by `VmId`.
    pub vms: HashMap<VmId, AuthorityVm>,
    /// The scheduler that decides which VM runs next.
    pub scheduler: VmScheduler,
    /// Registry of host-call handlers, keyed by [`HostCallKind`].
    pub host_call_handlers: HashMap<HostCallKind, HostHandler>,
}

impl Default for HypervisorRuntime {
    fn default() -> Self {
        Self {
            vms: HashMap::new(),
            scheduler: VmScheduler::default(),
            host_call_handlers: HashMap::new(),
        }
    }
}

impl HypervisorRuntime {
    /// Construct a new, empty runtime.
    pub fn new() -> Self {
        Self::default()
    }

    /// Spawn a new VM for the given agent.
    ///
    /// The VM is created in [`VmState::Halted`] and enqueued on the
    /// scheduler; it will not actually run until [`Self::run`] is called.
    pub async fn spawn(
        &mut self,
        agent: AgentId,
        program: VmProgram,
        caps: HashSet<Capability>,
    ) -> Result<VmId, HypervisorError> {
        info!(%agent, "spawning authority VM");

        // TODO(v1): consult VmIsolation to enforce spawn-time quota and to
        // validate that `caps` is a subset of what the agent is allowed.
        if program.bytecode.is_empty() {
            warn!(%agent, "spawn rejected: empty program");
            return Err(HypervisorError::SpawnFailed("empty program".into()));
        }

        let vm = AuthorityVm::new(agent, program, caps);
        let vm_id = vm.id;
        self.vms.insert(vm_id, vm);
        self.scheduler.enqueue(vm_id, 0);
        debug!(%agent, %vm_id, "VM spawned");
        Ok(vm_id)
    }

    /// Kill a VM by id. The VM is removed from the scheduler and from the
    /// VM map. In-flight host calls are not cancelled in v0.
    pub async fn kill(&mut self, vm: VmId) -> Result<(), HypervisorError> {
        info!(%vm, "killing VM");
        match self.vms.get_mut(&vm) {
            Some(v) => {
                v.state = VmState::Faulted(crate::authority_vm::VmFault::HostCallFailed);
                // TODO(v1): cancel any in-flight host call and free isolation resources.
                self.vms.remove(&vm);
                self.scheduler.dequeue(vm);
                Ok(())
            }
            None => Err(HypervisorError::VmNotFound(vm)),
        }
    }

    /// Run the scheduler loop until `shutdown` is signaled.
    ///
    /// In v0 this is a stub: it drains the scheduler once, executing zero
    /// instructions per VM, and then exits when the shutdown signal fires.
    pub async fn run(&mut self, shutdown: watch::Receiver<bool>) -> Result<(), HypervisorError> {
        info!("hypervisor runtime entering main loop");

        let mut shutdown = shutdown;
        loop {
            if *shutdown.borrow() {
                info!("shutdown signal received; stopping runtime");
                break;
            }

            // TODO(v1): real loop body:
            //   1. pick next VM via scheduler
            //   2. give it a time slice via AuthorityVm::run
            //   3. route any host calls through host_call_handlers
            //   4. update quota counters
            //   5. reap dead VMs
            match self.scheduler.next() {
                Some(entry) => {
                    trace!(vm = %entry.vm_id, "scheduling VM (v0 stub: no-op)");
                    if let Some(vm) = self.vms.get_mut(&entry.vm_id) {
                        // v0: don't actually run; just touch the state.
                        let _ = vm.run().await;
                    }
                }
                None => {
                    // Nothing to schedule; yield to avoid busy-looping.
                    tokio::task::yield_now().await;
                }
            }
        }

        Ok(())
    }

    /// List all live VMs and their current state.
    pub fn list_vms(&self) -> Vec<VmInfo> {
        self.vms.values().map(VmInfo::from).collect()
    }

    /// Dispatch a host call issued by `vm` to the registered handler.
    ///
    /// TODO(v1): wire this into `AuthorityVm::invoke_host_call` via a channel
    /// so the VM can `await` the result without losing its time slice.
    pub fn dispatch_host_call(
        &self,
        vm: VmId,
        call: &HostCall,
    ) -> Result<HostCallResult, HypervisorError> {
        // TODO(v1): register real handlers for each HostCall variant.
        let kind: HostCallKind = match call {
            HostCall::ReadMemory => 0x01,
            HostCall::WriteMemory => 0x02,
            HostCall::QueryHal => 0x03,
            HostCall::SendMessage => 0x04,
            HostCall::LogEvent => 0x05,
        };
        match self.host_call_handlers.get(&kind) {
            Some(handler) => Ok(handler(call)),
            None => {
                warn!(%vm, ?call, "no handler registered for host call");
                Err(HypervisorError::HostCallUnregistered(format!("{:?}", call)))
            }
        }
    }
}

// ─── Standalone helpers ─────────────────────────────────────────────────────────

/// Generate a fresh `VmId`. Convenience wrapper around `Uuid::new_v4`.
pub fn new_vm_id() -> VmId {
    Uuid::new_v4()
}

// ─── SAFETY ─────────────────────────────────────────────────────────────────────
//
// `HypervisorRuntime` contains no `unsafe` operations in v0. Host-call handlers
// are plain `fn` pointers (not `FnMut` closures), so they cannot capture state
// and therefore cannot accidentally share `&mut` references across VMs. The
// `&mut self` on `spawn`/`kill`/`run` is the single mutation boundary.

// v0: stub implementation
