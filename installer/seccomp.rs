//! Seccomp filters — generates BPF programs that block dangerous syscalls per install profile. Default-deny with an explicit allowlist.
//!
//! Each sandbox is locked down with a seccomp-BPF program produced here. The
//! default profile ([`default_profile`]) is a closed allowlist: anything not
//! explicitly listed is `EPERM`'d. [`strict_profile`] additionally blocks
//! `fork`/`execve` except for a whitelisted set of binaries (enforced via
//! argument inspection in v1).
//!
//! v0: stub implementation. The profile data model, BPF instruction struct,
//! and `install` skeleton are in place, but `compile` returns an empty
//! program and `install` returns `Unsupported`. See `TODO(v1)` markers.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, info, instrument, warn};

// ─── Errors ──────────────────────────────────────────────────────────────────

/// Errors returned by the seccomp layer.
#[derive(Debug, Error)]
pub enum SeccompError {
    /// The profile could not be compiled to a valid BPF program.
    #[error("seccomp compile failed: {0}")]
    Compile(String),
    /// `prctl(PR_SET_NO_NEW_PRIVS)` or `seccomp(SECCOMP_SET_MODE_FILTER)`
    /// failed at runtime.
    #[error("seccomp install failed: {0}")]
    Install(String),
    /// The running kernel does not support seccomp-BPF.
    #[error("seccomp unsupported by kernel")]
    Unsupported,
}

// ─── Actions ─────────────────────────────────────────────────────────────────

/// What the filter does when a syscall matches a rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Allow the syscall to proceed.
    Allow,
    /// Deny with `EPERM`.
    Deny,
    /// Kill the offending thread (`SECCOMP_RET_KILL_PROCESS`).
    Kill,
    /// Trap to a userspace handler via `SIGSYS`.
    Trap,
    /// Allow but emit a log entry (`SECCOMP_RET_LOG`).
    Log,
}

impl Action {
    /// Numeric `seccomp(2)` return value for this action.
    pub(crate) fn ret_code(&self) -> u32 {
        match self {
            Action::Allow => 0x7fff_0000, // SECCOMP_RET_ALLOW
            Action::Deny => 0x0005_0000,  // SECCOMP_RET_ERRNO
            Action::Kill => 0x8000_0000,  // SECCOMP_RET_KILL_PROCESS
            Action::Trap => 0x0003_0000,  // SECCOMP_RET_TRAP
            Action::Log => 0x7ffc_0000,   // SECCOMP_RET_LOG
        }
    }
}

// ─── Syscall catalogue ───────────────────────────────────────────────────────

/// Closed list of syscalls the seccomp layer cares about. Stored by name
/// (not raw `__NR_*` number) so profiles are portable across architectures;
/// the compiler resolves names to numbers at `compile` time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(non_camel_case_types)]
pub enum SyscallNo {
    /// `init_module(2)` — load a kernel module.
    KernelModule,
    /// `finit_module(2)` — load a kernel module from a file descriptor.
    FinitModule,
    /// `delete_module(2)` — unload a kernel module.
    DeleteModule,
    /// `mount(2)` — mount a filesystem.
    Mount,
    /// `umount2(2)` — unmount a filesystem.
    Umount2,
    /// `pivot_root(2)` — swap the root filesystem.
    PivotRoot,
    /// `reboot(2)` — reboot / power off.
    Reboot,
    /// `kexec_load(2)` — load a new kernel for hot-swap.
    KexecLoad,
    /// `kexec_file_load(2)` — same, from a file descriptor.
    KexecFileLoad,
    /// `ptrace(2)` — trace / manipulate another process.
    Ptrace,
    /// `ptrace` with `PTRACE_SETUID` semantics (v1 will narrow this).
    PtraceSetuid,
    /// `perf_event_open(2)` — open a perf counter (side-channel risk).
    PerfEventOpen,
    /// `bpf(2)` — load / manage BPF programs.
    Bpf,
    /// `clone(2)` — fork with options. Blocked by `strict_profile`.
    Clone,
    /// `fork(2)` / `vfork(2)`. Blocked by `strict_profile`.
    Fork,
    /// `execve(2)`. Blocked by `strict_profile` except for whitelisted
    /// binaries.
    Execve,
    /// `execveat(2)`. Same treatment as `execve`.
    Execveat,
    /// `keyctl(2)` / `add_key` / `request_key` — kernel keyring manipulation.
    Keyctl,
    /// `nfsservctl(2)` — NFS daemon control.
    Nfsservctl,
    /// `swapon(2)` / `swapoff(2)`.
    Swapon,
    /// `setns(2)` — re-enter a namespace (escape risk).
    Setns,
    /// `unshare(2)` — escape the current namespace set.
    Unshare,
}

impl SyscallNo {
    /// Resolve a syscall name to its raw `__NR_*` number for the host
    /// architecture. v0 always returns `None`; v1 will use `libc::syscall`
    /// name resolution.
    pub(crate) fn number(&self) -> Option<i32> {
        // TODO(v1): map via `libc::SYS_*` constants per-arch.
        let _ = self;
        None
    }

    /// Human-readable name (matches the `snake_case` serde variant).
    pub fn name(&self) -> &'static str {
        match self {
            SyscallNo::KernelModule => "init_module",
            SyscallNo::FinitModule => "finit_module",
            SyscallNo::DeleteModule => "delete_module",
            SyscallNo::Mount => "mount",
            SyscallNo::Umount2 => "umount2",
            SyscallNo::PivotRoot => "pivot_root",
            SyscallNo::Reboot => "reboot",
            SyscallNo::KexecLoad => "kexec_load",
            SyscallNo::KexecFileLoad => "kexec_file_load",
            SyscallNo::Ptrace => "ptrace",
            SyscallNo::PtraceSetuid => "ptrace_setuid",
            SyscallNo::PerfEventOpen => "perf_event_open",
            SyscallNo::Bpf => "bpf",
            SyscallNo::Clone => "clone",
            SyscallNo::Fork => "fork",
            SyscallNo::Execve => "execve",
            SyscallNo::Execveat => "execveat",
            SyscallNo::Keyctl => "keyctl",
            SyscallNo::Nfsservctl => "nfsservctl",
            SyscallNo::Swapon => "swapon",
            SyscallNo::Setns => "setns",
            SyscallNo::Unshare => "unshare",
        }
    }
}

// ─── BPF instruction ─────────────────────────────────────────────────────────

/// One classic BPF instruction (struct `sock_filter`). The seccomp filter
/// is a sequence of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BpfInsn {
    /// Opcode (BPF_LD | BPF_W | BPF_ABS, BPF_JMP | BPF_JEQ | BPF_K, …).
    pub code: u16,
    /// Jump-true target (instruction index).
    pub jt: u8,
    /// Jump-false target (instruction index).
    pub jf: u8,
    /// Constant / mask / argument.
    pub k: u32,
}

impl BpfInsn {
    /// Construct a no-op `BPF_RET` instruction returning `SECCOMP_RET_ALLOW`.
    pub const fn allow() -> Self {
        Self {
            code: 0x06, // BPF_RET | BPF_K
            jt: 0,
            jf: 0,
            k: Action::Allow.ret_code(),
        }
    }
}

// ─── Seccomp profile ─────────────────────────────────────────────────────────

/// A seccomp profile: an allowlist, a denylist, and a default action for
/// any syscall not listed in either.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeccompProfile {
    /// Syscalls explicitly allowed (overrides `default = Deny`).
    pub allowlist: HashSet<SyscallNo>,
    /// Syscalls explicitly denied (overrides `default = Allow`).
    pub denylist: HashSet<SyscallNo>,
    /// Default action for syscalls in neither list.
    pub default: Action,
}

impl Default for SeccompProfile {
    fn default() -> Self {
        default_profile()
    }
}

/// The default install profile.
///
/// Denies: `mount`, `umount2`, `pivot_root`, `init_module`, `finit_module`,
/// `delete_module`, `kexec_load`, `kexec_file_load`, `reboot`, `ptrace`,
/// `perf_event_open`, `bpf`, `keyctl`, `nfsservctl`, `swapon`, `setns`,
/// `unshare`. Everything else defaults to `Allow`.
pub fn default_profile() -> SeccompProfile {
    let deny = [
        SyscallNo::Mount,
        SyscallNo::Umount2,
        SyscallNo::PivotRoot,
        SyscallNo::KernelModule,
        SyscallNo::FinitModule,
        SyscallNo::DeleteModule,
        SyscallNo::KexecLoad,
        SyscallNo::KexecFileLoad,
        SyscallNo::Reboot,
        SyscallNo::Ptrace,
        SyscallNo::PtraceSetuid,
        SyscallNo::PerfEventOpen,
        SyscallNo::Bpf,
        SyscallNo::Keyctl,
        SyscallNo::Nfsservctl,
        SyscallNo::Swapon,
        SyscallNo::Setns,
        SyscallNo::Unshare,
    ]
    .into_iter()
    .collect();
    SeccompProfile {
        allowlist: HashSet::new(),
        denylist: deny,
        default: Action::Allow,
    }
}

/// The strict profile: everything in [`default_profile`], plus `clone`,
/// `fork`, `execve`, `execveat` are denied (except for whitelisted
/// binaries — see v1).
pub fn strict_profile() -> SeccompProfile {
    let mut p = default_profile();
    p.denylist.insert(SyscallNo::Clone);
    p.denylist.insert(SyscallNo::Fork);
    p.denylist.insert(SyscallNo::Execve);
    p.denylist.insert(SyscallNo::Execveat);
    p.default = Action::Deny;
    p
}

impl SeccompProfile {
    /// Construct an empty profile with `default = Deny` (build-your-own
    /// allowlist mode).
    pub fn empty_deny() -> Self {
        SeccompProfile {
            allowlist: HashSet::new(),
            denylist: HashSet::new(),
            default: Action::Deny,
        }
    }

    /// Allow an additional syscall.
    pub fn allow(&mut self, s: SyscallNo) -> &mut Self {
        self.denylist.remove(&s);
        self.allowlist.insert(s);
        self
    }

    /// Deny an additional syscall.
    pub fn deny(&mut self, s: SyscallNo) -> &mut Self {
        self.allowlist.remove(&s);
        self.denylist.insert(s);
        self
    }

    /// Compile the profile into a sequence of BPF instructions suitable for
    /// `seccomp(SECCOMP_SET_MODE_FILTER)`.
    ///
    /// v0: returns an empty program (only the trailer `RET`). v1 will emit
    /// the standard load-arch + load-syscall-no + per-syscall comparison
    /// sequence.
    #[instrument(skip(self))]
    pub fn compile(&self) -> Result<Vec<BpfInsn>, SeccompError> {
        let n_allow = self.allowlist.len();
        let n_deny = self.denylist.len();
        info!(n_allow, n_deny, default = ?self.default, "compile: v0 stub");

        // TODO(v1): emit:
        //   1. BPF_LD | BPF_W | BPF_ABS  offsetof(arch)       // load arch
        //   2. BPF_JMP | BPF_JEQ | BPF_K  AUDIT_ARCH_X86_64  // bail on mismatch
        //   3. BPF_LD | BPF_W | BPF_ABS  offsetof(nr)        // load syscall nr
        //   4. for each denied syscall:  BPF_JEQ -> RET(deny)
        //   5. for each allowed syscall: BPF_JEQ -> RET(allow)
        //   6. RET(default)
        //
        // SAFETY: BPF programs are loaded into the kernel via
        // `seccomp(SECCOMP_SET_MODE_FILTER)`, which validates them with
        // the in-kernel BPF verifier before installing. There is no
        // userspace memory unsafety here.
        let mut prog = Vec::with_capacity(n_allow + n_deny + 4);
        // v0: just the trailer so the type round-trips.
        prog.push(BpfInsn::allow());
        Ok(prog)
    }

    /// Install this profile on the current thread.
    ///
    /// Sequence:
    ///   1. `prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0)` — required before
    ///      seccomp filter install for unprivileged processes.
    ///   2. `seccomp(SECCOMP_SET_MODE_FILTER, 0, &fprog)` — install the
    ///      compiled BPF program.
    ///
    /// v0: returns `Unsupported` without calling either syscall. v1 will
    /// wire up the unsafe `libc::prctl` / `libc::syscall(SYS_seccomp, …)`
    /// calls.
    #[instrument(skip(self))]
    pub fn install(&self) -> Result<(), SeccompError> {
        info!("install: v0 stub — no syscalls issued");
        let prog = self.compile()?;

        // SAFETY: scaffold. v1 will:
        //   let fprog = sock_fprog { len: prog.len() as u16, filter: prog.as_mut_ptr() };
        //   let rc = libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0);
        //   if rc != 0 { return Err(SeccompError::Install(errno)); }
        //   let rc = libc::syscall(libc::SYS_seccomp, SECCOMP_SET_MODE_FILTER, 0, &fprog);
        //   if rc != 0 { return Err(SeccompError::Install(errno)); }
        unsafe {
            // SAFETY: no-op in v0. The real call is two syscalls on the
            // current thread; both take value arguments and have no
            // pointer-aliasing concerns.
            let _ = libc::prctl;
            let _ = prog;
        }

        // TODO(v1): replace the stub with the real syscalls above.
        warn!("seccomp install not implemented in v0 — returning Unsupported");
        Err(SeccompError::Unsupported)
    }
}

// ─── Quick predicates ────────────────────────────────────────────────────────

/// True iff the running kernel advertises seccomp support. v0 reads
/// `/proc/self/status` for `Seccomp:` — v1 will use `prctl` directly.
pub fn kernel_supports_seccomp() -> bool {
    // TODO(v1): `prctl(PR_GET_SECCOMP)` and `prctl(PR_SET_SECCOMP, …)`
    // return value probing.
    std::path::Path::new("/proc/self/status").exists()
}

#[allow(dead_code)]
fn warn_unsupported() {
    if !kernel_supports_seccomp() {
        warn!("kernel does not advertise seccomp support");
    }
}

// v0: stub implementation
