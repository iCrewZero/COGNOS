//! Syscall tracker — eBPF-fed per-agent syscall profiling.
//!
//!
//! In production, a kernel eBPF program attaches to `tracepoint/syscalls`
//! and pushes events into a ring buffer that this module drains. Each
//! event is attributed to an agent (via cgroup or pid→agent map) and
//! folded into a [`SyscallProfile`]. The profile exposes a
//! `dangerous_call_rate` ∈ [0, 1] which feeds the HAL risk formula.
//!
//! "Dangerous" syscalls are those with irreversible or system-wide effects:
//! `unlink`, `execve`, `fork`/`clone` (process creation), `bind`/`connect`
//! (network), and anything touching kernel modules. The closed list is
//! deliberate: it must be human-audited.
//!
//! v0: stub implementation. The eBPF ring-buffer reader is a TODO(v1)
//! stub; the profile math is in place.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, warn};

// v0: stub implementation

/// Type alias for an agent identifier.
pub type AgentId = String;

// ─── Syscall Numbers (closed list) ──────────────────────────────────────────────

/// A closed list of syscalls that HAL tracks.
///
/// Not every Linux syscall — only those whose observation matters for
/// risk scoring. Adding a new variant is a spec change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SyscallNo {
    /// `open(2)` — file open.
    Open,
    /// `openat(2)` — file open relative to dirfd.
    OpenAt,
    /// `unlink(2)` / `unlinkat(2)` — file delete.
    Unlink,
    /// `rmdir(2)` — directory delete.
    Rmdir,
    /// `execve(2)` / `execveat(2)` — execute a program.
    Execve,
    /// `fork(2)` — create child process (legacy).
    Fork,
    /// `clone(2)` / `clone3(2)` — create child process / thread.
    Clone,
    /// `connect(2)` — initiate a TCP/UDP connection.
    Connect,
    /// `bind(2)` — bind a socket (listen for inbound).
    Bind,
    /// `listen(2)` — mark socket as passive.
    Listen,
    /// `accept(2)` / `accept4(2)` — accept inbound connection.
    Accept,
    /// `socket(2)` — create a socket.
    Socket,
    /// `mount(2)` — mount a filesystem (kernel-level).
    Mount,
    /// `chroot(2)` — change root directory.
    Chroot,
    /// `setuid(2)` / `setgid(2)` — change credentials.
    Setuid,
    /// `ptrace(2)` — process tracing (powerful).
    Ptrace,
    /// `init_module(2)` / `finit_module(2)` — load kernel module.
    InitModule,
}

impl SyscallNo {
    /// Whether this syscall is classified as "dangerous" by HAL.
    ///
    /// Dangerous == irreversible or system-wide blast radius.
    pub fn is_dangerous(&self) -> bool {
        matches!(
            self,
            Self::Unlink
                | Self::Rmdir
                | Self::Execve
                | Self::Fork
                | Self::Clone
                | Self::Bind
                | Self::Listen
                | Self::Accept
                | Self::Mount
                | Self::Chroot
                | Self::Setuid
                | Self::Ptrace
                | Self::InitModule
        )
    }
}

// ─── Syscall Profile ────────────────────────────────────────────────────────────

/// Per-agent syscall profile, accumulated over a window.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SyscallProfile {
    /// Per-syscall observation counts.
    pub counts: HashMap<SyscallNo, u64>,
    /// Total number of "dangerous" syscalls observed.
    pub dangerous_calls: u64,
    /// Total number of *all* syscalls observed.
    pub total_calls: u64,
}

impl SyscallProfile {
    /// Record a single syscall observation.
    pub fn record(&mut self, syscall: SyscallNo) {
        *self.counts.entry(syscall).or_insert(0) += 1;
        self.total_calls += 1;
        if syscall.is_dangerous() {
            self.dangerous_calls += 1;
        }
    }

    /// Rate of dangerous calls ∈ [0.0, 1.0] over the window.
    ///
    /// Returns 0.0 when no calls have been observed (not 1.0) to avoid
    /// penalizing idle agents.
    pub fn dangerous_call_rate(&self) -> f32 {
        if self.total_calls == 0 {
            return 0.0;
        }
        self.dangerous_calls as f32 / self.total_calls as f32
    }

    /// Reset the profile (typically called on window rollover).
    pub fn reset(&mut self) {
        self.counts.clear();
        self.dangerous_calls = 0;
        self.total_calls = 0;
    }
}

// ─── Syscall Tracker ────────────────────────────────────────────────────────────

/// The syscall tracker. Owns the per-agent profile table.
#[derive(Debug, Default)]
pub struct SyscallTracker {
    per_agent: HashMap<AgentId, SyscallProfile>,
}

impl SyscallTracker {
    /// Construct an empty tracker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a syscall observation for an agent.
    pub fn record(&mut self, agent: &AgentId, syscall: SyscallNo) {
        let entry = self.per_agent.entry(agent.clone()).or_default();
        entry.record(syscall);
        if syscall.is_dangerous() {
            warn!(agent = %agent, syscall = ?syscall, "dangerous syscall observed");
        }
    }

    /// Return the dangerous-call-rate for an agent (0.0 if unknown).
    pub fn dangerous_call_rate(&self, agent: &AgentId) -> f32 {
        self.per_agent
            .get(agent)
            .map(|p| p.dangerous_call_rate())
            .unwrap_or(0.0)
    }

    /// Borrow the full profile for an agent.
    pub fn profile_of(&self, agent: &AgentId) -> Option<&SyscallProfile> {
        self.per_agent.get(agent)
    }

    /// Drain pending events from the eBPF ring buffer and fold them into
    /// per-agent profiles. In v0 this is a no-op stub.
    ///
    /// TODO(v1): wire to `perf_buffer__poll` from aya/libbpf-rs.
    pub fn drain_ring_buffer(&mut self) -> Result<u64, SyscallError> {
        // TODO(v1): implement real ring-buffer drain.
        debug!("drain_ring_buffer called (v0 no-op)");
        Ok(0)
    }

    /// Reset all per-agent profiles (typically on window rollover).
    pub fn reset_all(&mut self) {
        for p in self.per_agent.values_mut() {
            p.reset();
        }
    }
}

// ─── Errors ─────────────────────────────────────────────────────────────────────

/// Errors returned by the syscall tracker.
#[derive(Debug, Error)]
pub enum SyscallError {
    /// The eBPF ring buffer could not be polled.
    #[error("eBPF ring buffer poll failed: {0}")]
    RingBufferPoll(String),
    /// The agent→pid map could not be resolved.
    #[error("agent-pid map resolution failed for pid {0}")]
    AgentResolution(u32),
}
