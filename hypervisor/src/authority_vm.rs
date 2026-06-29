//! Authority VM — a minimal virtual machine that executes AI agent logic at reduced privilege. Each agent runs in its own VM instance with a capability-bounded syscall interface. The VM is interpretive (no JIT) to keep the TCB small.
//!
//! The Authority VM is the smallest unit of agent execution in COGNOS. It is
//! intentionally *interpretive* (no JIT, no self-modifying code) so that the
//! trusted computing base for agent isolation stays small and auditable. Each
//! VM has its own bytecode program, private linear memory, stack, and a
//! `HashSet<Capability>` that bounds which host calls (syscalls) it may issue.
//!
//! Host calls are dispatched synchronously through [`AuthorityVm::invoke_host_call`]
//! and routed by the [`HypervisorRuntime`](crate::runtime::HypervisorRuntime) to the
//! appropriate handler. The VM never sees raw host pointers — host calls operate
//! on VM-relative addresses and capability tokens.
//!
//! v0: stub implementation. The interpreter loop, decoder, and capability checks
//! are stubbed; only the public surface and ISA documentation are present.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, trace, warn};
use uuid::Uuid;

// TODO(v1): the interpreter loop will likely need `tokio::sync::Mutex` once
// host-call channels are introduced; left out of v0 to avoid an unused import.

// v0: stub implementation

// ─── Identifiers ────────────────────────────────────────────────────────────────

/// Stable identifier for a VM instance.
pub type VmId = Uuid;
// TODO(v1): unify VmId/AgentId/Capability with HAL/permissions modules.

/// Stable identifier for the agent that owns this VM.
pub type AgentId = Uuid;

/// A capability token held by a VM. Capabilities gate which host calls
/// the VM is allowed to issue.
///
/// TODO(v1): replace the stringly-typed stub with the shared
/// `hal::permissions::Capability` enum once the cross-crate dependency is wired up.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Capability(pub String);

// ─── Opcodes ────────────────────────────────────────────────────────────────────
//
// The Authority VM uses a small fixed-width RISC-like ISA. Every instruction is
// 4 bytes: a 1-byte opcode followed by a 3-byte operand (register index,
// constant index, jump target, or immediate — interpreted per-opcode).
//
// Opcode   Mnemonic        Operand meaning                  Notes
// ------   --------        ----------------                  ----
// 0x00     NOP             ignored                           No-op.
// 0x01     HALT            ignored                           Stop execution; VM exits Halted.
// 0x02     LOAD_CONST      const_idx -> reg                  Load constant[idx] into reg.
// 0x03     LOAD_MEM        addr -> reg                       Load word from VM memory[addr].
// 0x04     STORE_MEM       reg -> addr                       Store reg into VM memory[addr].
// 0x05     ADD             reg_a, reg_b -> reg_c             reg_c = reg_a + reg_b.
// 0x06     SUB             reg_a, reg_b -> reg_c             reg_c = reg_a - reg_b.
// 0x07     MUL             reg_a, reg_b -> reg_c             reg_c = reg_a * reg_b.
// 0x08     DIV             reg_a, reg_b -> reg_c             reg_c = reg_a / reg_b (traps on 0).
// 0x09     JMP             target                            Unconditional jump.
// 0x0A     JZ              reg, target                       Jump if reg == 0.
// 0x0B     JNZ             reg, target                       Jump if reg != 0.
// 0x0C     CALL            target                            Push PC, jump.
// 0x0D     RET             ignored                           Pop PC.
// 0x0E     PUSH            reg                               Push reg onto stack.
// 0x0F     POP             reg                               Pop stack into reg.
// 0x10     SYSCALL         host_call_idx                     Issue a host call (capability-checked).
// 0x11     YIELD           ignored                           Yield the time slice; VM exits Yielded.
// 0xFF     FAULT           reason                            Force a VM fault (debugging).

/// Opcode enumeration. TODO(v1): use this in a real `decode()` function.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum Opcode {
    Nop = 0x00,
    Halt = 0x01,
    LoadConst = 0x02,
    LoadMem = 0x03,
    StoreMem = 0x04,
    Add = 0x05,
    Sub = 0x06,
    Mul = 0x07,
    Div = 0x08,
    Jmp = 0x09,
    Jz = 0x0A,
    Jnz = 0x0B,
    Call = 0x0C,
    Ret = 0x0D,
    Push = 0x0E,
    Pop = 0x0F,
    Syscall = 0x10,
    Yield = 0x11,
    Fault = 0xFF,
}

// ─── Program & Memory ───────────────────────────────────────────────────────────

/// A compiled VM program: bytecode plus its constant pool and entry point.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmProgram {
    /// Raw bytecode — a flat `Vec<u8>` of 4-byte instructions.
    pub bytecode: Vec<u8>,
    /// Byte offset of the entry point within `bytecode`.
    pub entry: u32,
    /// Constant pool indexed by `LOAD_CONST` operands.
    pub constants: Vec<u64>,
}

impl Default for VmProgram {
    fn default() -> Self {
        // TODO(v1): reject empty programs at load time.
        Self {
            bytecode: Vec::new(),
            entry: 0,
            constants: Vec::new(),
        }
    }
}

/// The VM's private linear memory: a flat byte vector plus a stack region
/// growing down from the top and a heap region growing up from the bottom.
///
/// Memory is never shared with the host or with other VMs. All addresses
/// in the bytecode are interpreted relative to this buffer; the runtime
/// translates host-call arguments out of this buffer with bounds checks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmMemory {
    /// Backing storage.
    pub bytes: Vec<u8>,
    /// Current stack pointer (grows down from `bytes.len()`).
    pub stack_ptr: usize,
    /// Current heap bump pointer (grows up from 0).
    pub heap_ptr: usize,
}

impl Default for VmMemory {
    fn default() -> Self {
        // TODO(v1): make initial size configurable via ResourceQuota.
        Self {
            bytes: vec![0u8; 64 * 1024],
            stack_ptr: 64 * 1024,
            heap_ptr: 0,
        }
    }
}

// ─── VM State & Faults ──────────────────────────────────────────────────────────

/// The runtime state of an `AuthorityVm`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum VmState {
    /// VM has halted normally (executed `HALT`) or has not yet been started.
    #[default]
    Halted,
    /// VM is currently executing instructions.
    Running,
    /// VM has issued a host call and is waiting for the host to respond.
    AwaitingHostCall,
    /// VM has crashed with a fault; it cannot resume.
    Faulted(VmFault),
}

/// A non-recoverable VM fault.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Error)]
pub enum VmFault {
    /// The decoder encountered an unknown opcode or a malformed instruction.
    #[error("illegal instruction")]
    IllegalInstruction,
    /// A memory access exceeded the VM's allocated memory.
    #[error("out of memory")]
    OutOfMemory,
    /// The VM attempted a host call for which it lacks the capability.
    #[error("capability violation")]
    CapabilityViolation,
    /// The stack pointer crossed the heap pointer or went out of bounds.
    #[error("stack overflow")]
    StackOverflow,
    /// A host call returned an error; the VM was forcibly faulted.
    #[error("host call failed")]
    HostCallFailed,
}

/// The reason a VM stopped running, returned by [`AuthorityVm::run`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum VmExit {
    /// VM executed `HALT`.
    Halted,
    /// VM executed `YIELD` or exhausted its time slice.
    Yielded,
    /// VM faulted and cannot resume.
    Faulted(VmFault),
    /// VM was killed externally by the runtime.
    Killed,
}

/// The result of a single [`AuthorityVm::step`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum StepResult {
    /// One instruction was executed successfully.
    #[default]
    Continued,
    /// VM halted after this step.
    Halted,
    /// VM yielded after this step.
    Yielded,
    /// VM issued a host call and is now awaiting a response.
    AwaitingHostCall,
    /// VM faulted during this step.
    Faulted(VmFault),
}

// ─── Host Calls ─────────────────────────────────────────────────────────────────

/// A syscall issued by the VM to the host. Each variant is gated by one or
/// more [`Capability`]s; the runtime checks the VM's capability set before
/// dispatching.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HostCall {
    /// Read a range of bytes from the agent's memory namespace.
    ReadMemory,
    /// Write a range of bytes to the agent's memory namespace.
    WriteMemory,
    /// Query the HAL for a decision on a proposed action.
    QueryHal,
    /// Send a message to another agent (capability-gated to recipient set).
    SendMessage,
    /// Emit a structured event into the audit log.
    LogEvent,
}

/// The result returned by the host in response to a [`HostCall`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum HostCallResult {
    /// Call succeeded; payload is opaque to the VM.
    #[default]
    Ok,
    /// Call succeeded with a small inline payload.
    Bytes(Vec<u8>),
    /// Call succeeded with a scalar.
    Scalar(u64),
    /// Call was rejected by the host (e.g. HAL denied it).
    Denied,
    /// Call failed with a host-side error.
    Error(String),
}

// ─── Errors ─────────────────────────────────────────────────────────────────────

/// Errors returned by VM control-plane operations (as opposed to in-VM faults,
/// which are represented by [`VmFault`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Error)]
pub enum VmError {
    /// The provided program was malformed or unsupported.
    #[error("invalid program: {0}")]
    InvalidProgram(String),
    /// The VM lacks a capability required for the requested operation.
    #[error("capability denied: {0}")]
    CapabilityDenied(String),
    /// A host-side error occurred while servicing the VM.
    #[error("host error: {0}")]
    HostError(String),
}

// ─── AuthorityVm ────────────────────────────────────────────────────────────────

/// The Authority VM.
///
/// One `AuthorityVm` exists per running agent. The VM is single-threaded by
/// construction: only one [`AuthorityVm::step`] call may be in flight at a
/// time. The outer [`HypervisorRuntime`](crate::runtime::HypervisorRuntime)
/// is responsible for multiplexing many VMs across tokio tasks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorityVm {
    /// Stable VM identifier.
    pub id: VmId,
    /// Owning agent identifier.
    pub agent: AgentId,
    /// The program loaded into this VM.
    pub program: VmProgram,
    /// The VM's private linear memory.
    pub memory: VmMemory,
    /// The capability set bounding which host calls this VM may issue.
    pub capabilities: HashSet<Capability>,
    /// Current execution state.
    pub state: VmState,
    /// Number of instructions executed since the VM was last reset.
    ///
    /// Used by the scheduler for fair time-slicing and by the quota system
    /// to enforce `max_cpu_seconds` (via instructions-per-second estimate).
    pub instruction_count: u64,
}

impl AuthorityVm {
    /// Construct a new VM with the given agent, program, and capabilities.
    ///
    /// The VM starts in [`VmState::Halted`]; call [`Self::run`] to begin
    /// execution.
    // TODO(v1): validate program structure (entry in range, opcode table).
    pub fn new(agent: AgentId, program: VmProgram, capabilities: HashSet<Capability>) -> Self {
        Self {
            id: Uuid::new_v4(),
            agent,
            program,
            memory: VmMemory::default(),
            capabilities,
            state: VmState::Halted,
            instruction_count: 0,
        }
    }

    /// Run the VM until it exits (halt, yield, fault, or kill).
    ///
    /// This is the main entry point for execution. In v0 it is a stub: it
    /// transitions the VM to `Running`, executes zero instructions, and
    /// immediately returns [`VmExit::Halted`].
    pub async fn run(&mut self) -> Result<VmExit, VmError> {
        debug!(vm = %self.id, agent = %self.agent, "AuthorityVm::run entered");
        self.state = VmState::Running;

        // TODO(v1): implement the interpreter loop:
        //   loop {
        //       match self.step().await? {
        //           StepResult::Continued => continue,
        //           StepResult::Halted => return Ok(VmExit::Halted),
        //           StepResult::Yielded => return Ok(VmExit::Yielded),
        //           StepResult::AwaitingHostCall => return Ok(VmExit::Yielded),
        //           StepResult::Faulted(f) => return Ok(VmExit::Faulted(f)),
        //       }
        //   }
        trace!(vm = %self.id, "v0 stub: no instructions executed");

        self.state = VmState::Halted;
        Ok(VmExit::Halted)
    }

    /// Execute exactly one instruction.
    ///
    /// Returns [`StepResult::Continued`] if the VM may continue, or a
    /// terminal/await variant describing why execution should stop.
    pub async fn step(&mut self) -> Result<StepResult, VmError> {
        // TODO(v1): decode 4 bytes at self.program.entry + self.instruction_count * 4,
        // dispatch on Opcode, update registers/memory, and bump instruction_count.
        self.instruction_count = self.instruction_count.saturating_add(1);
        trace!(vm = %self.id, pc = self.instruction_count, "step (v0 stub)");
        Ok(StepResult::Continued)
    }

    /// Invoke a host call on behalf of the VM.
    ///
    /// The runtime checks `self.capabilities` against the call's required
    /// capability; if the check fails, the VM is transitioned to
    /// [`VmState::Faulted`] with [`VmFault::CapabilityViolation`] and a
    /// [`VmError::CapabilityDenied`] is returned.
    ///
    /// In v0 this is a stub: it always returns [`HostCallResult::Ok`] without
    /// actually dispatching to a host handler.
    pub fn invoke_host_call(&mut self, call: HostCall) -> Result<HostCallResult, VmError> {
        debug!(vm = %self.id, ?call, "host call requested");

        // TODO(v1): map HostCall -> required Capability and check the set.
        // For now we accept all calls so the surface is exercisable.
        if !self.has_capability_for(&call) {
            warn!(vm = %self.id, ?call, "capability denied for host call");
            self.state = VmState::Faulted(VmFault::CapabilityViolation);
            return Err(VmError::CapabilityDenied(format!("{:?}", call)));
        }

        self.state = VmState::AwaitingHostCall;
        // TODO(v1): route to the registered host handler via the runtime.
        self.state = VmState::Running;
        Ok(HostCallResult::Ok)
    }

    /// Best-effort capability check for a host call.
    ///
    /// TODO(v1): replace with a real capability lattice lookup.
    fn has_capability_for(&self, _call: &HostCall) -> bool {
        // v0: stub — always allow. The real implementation will consult
        // the shared HAL capability lattice.
        true
    }
}

impl Default for AuthorityVm {
    fn default() -> Self {
        Self::new(Uuid::nil(), VmProgram::default(), HashSet::new())
    }
}

// ─── SAFETY ─────────────────────────────────────────────────────────────────────
//
// `AuthorityVm` contains no raw pointers and performs no `unsafe` operations in
// v0. The `*mut u8` returned by `VmIsolation::allocate` in `isolation.rs` is
// the only unsafe boundary; it is never exposed to bytecode — host-call
// arguments are always copied out of VM memory with bounds checks.

// v0: stub implementation
