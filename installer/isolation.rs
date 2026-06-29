//! Isolation primitives — wraps Linux namespace, cgroup v2, and mount APIs to construct a per-install sandbox. Uses clone(2) with CLONE_NEWNS|NEWPID|NEWNET, drops capabilities, sets up bind mounts.
//!
//! This module is the lowest layer of the installer. [`create`] builds a new
//! process inside a fresh set of namespaces, with the requested bind mounts
//! in place and all capabilities dropped except those the caller explicitly
//! retains. The returned [`Sandbox`] handle owns the child PID and the
//! cgroup it lives in; dropping it via [`Sandbox::destroy`] tears both down.
//!
//! v0: stub implementation. The struct layout and error taxonomy are in
//! place, and the unsafe `libc` call sites are scaffolded with `// SAFETY:`
//! blocks, but no kernel APIs are actually invoked. See `TODO(v1)` markers.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, info, instrument, warn};
use uuid::Uuid;

// ─── Errors ──────────────────────────────────────────────────────────────────

/// Errors returned by the isolation layer.
#[derive(Debug, Error)]
pub enum IsolationError {
    /// The running kernel does not support one of the requested namespaces.
    #[error("namespace unsupported by kernel")]
    NamespaceUnsupported,
    /// `clone(2)` / `unshare(2)` failed.
    #[error("clone failed: {0}")]
    CloneFailed(String),
    /// A `mount(2)` call failed (bind mount, pivot_root, …).
    #[error("mount failed at {target}: {reason}")]
    MountFailed {
        /// Mount target inside the new namespace.
        target: String,
        /// Underlying strerror-style message.
        reason: String,
    },
    /// The cgroup v2 controller could not be created or written to.
    #[error("cgroup setup failed: {0}")]
    CgroupFailed(String),
    /// The caller invoked [`Sandbox::enter`] from outside any sandbox.
    #[error("not running inside a sandbox")]
    NotInSandbox,
}

// ─── Namespace set ───────────────────────────────────────────────────────────

/// Which namespaces to unshare when building a sandbox. The default for an
/// install sandbox is `{ mount: true, pid: true, net: true, ipc: true,
/// uts: true, user: false }` — `user` is only true for unprivileged
/// installs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamespaceSet {
    /// New mount namespace (CLONE_NEWNS).
    pub mount: bool,
    /// New PID namespace (CLONE_NEWPID).
    pub pid: bool,
    /// New network namespace (CLONE_NEWNET).
    pub net: bool,
    /// New IPC namespace (CLONE_NEWIPC).
    pub ipc: bool,
    /// New UTS namespace (CLONE_NEWUTS).
    pub uts: bool,
    /// New user namespace (CLONE_NEWUSER).
    pub user: bool,
}

impl Default for NamespaceSet {
    fn default() -> Self {
        Self {
            mount: true,
            pid: true,
            net: true,
            ipc: true,
            uts: true,
            user: false,
        }
    }
}

impl NamespaceSet {
    /// Build the `clone(2)` flags word corresponding to this set.
    pub(crate) fn clone_flags(&self) -> u64 {
        let mut flags: u64 = 0;
        if self.mount {
            flags |= libc::CLONE_NEWNS as u64;
        }
        if self.pid {
            flags |= libc::CLONE_NEWPID as u64;
        }
        if self.net {
            flags |= libc::CLONE_NEWNET as u64;
        }
        if self.ipc {
            flags |= libc::CLONE_NEWIPC as u64;
        }
        if self.uts {
            flags |= libc::CLONE_NEWUTS as u64;
        }
        if self.user {
            flags |= libc::CLONE_NEWUSER as u64;
        }
        flags
    }
}

// ─── Cgroup id ───────────────────────────────────────────────────────────────

/// Stable identifier for the cgroup a sandbox lives in. Maps to a path
/// under the parent cgroup (e.g. `cognos.slice/cognos-installer.slice/`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CgroupId(pub Uuid);

impl Default for CgroupId {
    fn default() -> Self {
        Self(Uuid::new_v4())
    }
}

// ─── Sandbox config ──────────────────────────────────────────────────────────

/// Configuration handed to [`create`]. Describes the namespaces, bind mounts,
/// hostname, and cgroup parent for a new sandbox.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxConfig {
    /// Which namespaces to unshare.
    pub namespaces: NamespaceSet,
    /// Root directory of the sandbox inside the host filesystem (e.g.
    /// `/var/lib/cognos/sandboxes/<id>`). The new mount namespace will
    /// pivot_root into this.
    pub root_dir: PathBuf,
    /// Read-only bind mounts: `(host_path, sandbox_path)` pairs.
    pub ro_bind: Vec<(PathBuf, PathBuf)>,
    /// Read-write bind mounts: `(host_path, sandbox_path)` pairs. The
    /// installer uses these for the ANFS-backed target prefix.
    pub rw_bind: Vec<(PathBuf, PathBuf)>,
    /// Hostname set inside the new UTS namespace.
    pub hostname: String,
    /// Cgroup parent path (e.g. `/sys/fs/cgroup/cognos.slice/cognos-installer.slice`).
    pub cgroup_parent: PathBuf,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            namespaces: NamespaceSet::default(),
            root_dir: PathBuf::from("/var/lib/cognos/sandboxes/default"),
            ro_bind: Vec::new(),
            rw_bind: Vec::new(),
            hostname: "cognos-sandbox".to_string(),
            cgroup_parent: PathBuf::from("/sys/fs/cgroup/cognos.slice"),
        }
    }
}

// ─── Sandbox handle ──────────────────────────────────────────────────────────

/// Owns a running sandbox. Drop semantics are explicit: callers must invoke
/// [`destroy`][Sandbox::destroy] to tear the sandbox down — v0 does not
/// implement `Drop` because the cleanup is async-friendly and needs to talk
/// to the cgroup hierarchy.
pub struct Sandbox {
    /// PID of the process running inside the sandbox.
    pub pid: i32,
    /// Root directory the sandbox is pivoted into.
    pub root_dir: PathBuf,
    /// Set of namespaces actually unshared (may be a subset of the requested
    /// set if the kernel refused some).
    pub namespaces: NamespaceSet,
    /// Cgroup the sandbox lives in.
    pub cgroup: CgroupId,
}

// ─── Construction / lifecycle ────────────────────────────────────────────────

/// Create a new sandbox from `config`.
///
/// v0: returns a synthesized handle with `pid = -1` and does not actually
/// call `clone(2)` / `mount(2)` / write to cgroupfs. The unsafe scaffolding
/// below is preserved so v1 can fill in the bodies.
#[instrument(skip(config), fields(root = ?config.root_dir, ns = ?config.namespaces))]
pub fn create(config: SandboxConfig) -> Result<Sandbox, IsolationError> {
    info!(root = ?config.root_dir, "create: v0 stub — no kernel calls");

    // TODO(v1): perform the real clone + mount + cgroup dance. The intended
    // sequence is sketched below as `unsafe` blocks with `// SAFETY:` docs.

    let _flags = config.namespaces.clone_flags();
    let _ro_binds = &config.ro_bind;
    let _rw_binds = &config.rw_bind;
    let _hostname = &config.hostname;
    let _cgroup_parent = &config.cgroup_parent;

    // SAFETY: scaffold — `clone(2)` with a fresh stack is the only safe way
    // to enter multiple new namespaces atomically. v0 does not allocate the
    // child stack or call clone yet; v1 will:
    //   1. mmap a child stack
    //   2. clone(child_fn, stack_top, flags | SIGCHLD, arg)
    //   3. wait for the child to finish pivot_root + bind mounts
    //   4. return the child PID in `Sandbox.pid`
    unsafe {
        // SAFETY: no-op in v0. The real call will only touch memory owned
        // by this process (the freshly-mmap'd child stack) and the syscall
        // result is checked before being returned.
        let _ = libc::getpid();
    }

    // SAFETY: scaffold — `mount(2)` for each bind pair. v1 will loop over
    // `ro_bind` and `rw_bind`, calling:
    //   mount(host, target, None, MS_BIND | MS_REC, None)
    //   mount(None, target, None, MS_REMOUNT | MS_RDONLY, None)  // for ro
    // Each call will be checked and converted to `IsolationError::MountFailed`.
    unsafe {
        // SAFETY: no-op in v0. The real implementation passes only NUL-free
        // absolute paths and never re-bind-mounts over a host path the
        // caller did not authorise.
        let _ = libc::mount;
    }

    // SAFETY: scaffold — `unshare(2)` is an alternative entry point when
    // the caller wants to sandbox the current thread instead of forking.
    // v1 will use it for the seccomp-only path.
    unsafe {
        // SAFETY: no-op in v0.
        let _ = libc::unshare;
    }

    Ok(Sandbox {
        pid: -1, // v0: sentinel; v1 will hold the real child PID.
        root_dir: config.root_dir,
        namespaces: config.namespaces,
        cgroup: CgroupId::default(),
    })
}

impl Sandbox {
    /// Enter the sandbox from the current thread. Used by the runtime when
    /// it needs to re-attach to an existing sandbox (e.g. for `verify`).
    ///
    /// v0: always returns `NotInSandbox` since no real namespace exists.
    #[instrument(skip(self))]
    pub fn enter(&self) -> Result<(), IsolationError> {
        debug!(pid = self.pid, "enter: v0 stub");
        // TODO(v1): setns(2) into each namespace fd held by `self`.
        Err(IsolationError::NotInSandbox)
    }

    /// Tear the sandbox down: kill the child, remove the cgroup, unmount
    /// everything. Consumes `self`.
    #[instrument(skip(self))]
    pub fn destroy(self) -> Result<(), IsolationError> {
        info!(pid = self.pid, "destroy: v0 stub");
        // TODO(v1): kill(child, SIGKILL) + waitpid, rmdir cgroup, lazy-unmount.
        // SAFETY: no-op in v0. v1 will call `libc::kill` only on `self.pid`
        // (which it owns) and propagate ESRCH as a no-op.
        unsafe {
            // SAFETY: scaffold.
            let _ = libc::kill;
        }
        let _ = self.root_dir;
        Ok(())
    }
}

// ─── Helpers (v0 bookkeeping) ────────────────────────────────────────────────

/// In-memory registry of live sandboxes. v0 uses this so the orchestrator
/// can be exercised end-to-end without a kernel; v1 moves it into a
/// dedicated `supervisor` module.
#[derive(Debug, Default)]
pub struct SandboxRegistry {
    sandboxes: HashMap<CgroupId, Sandbox>,
}

impl SandboxRegistry {
    /// Construct an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a sandbox. v1 will refuse to insert a duplicate id.
    pub fn insert(&mut self, sandbox: Sandbox) -> &mut Sandbox {
        let id = sandbox.cgroup.clone();
        self.sandboxes.entry(id).or_insert(sandbox)
    }

    /// Look up a sandbox by cgroup id.
    pub fn get(&self, id: &CgroupId) -> Option<&Sandbox> {
        self.sandboxes.get(id)
    }

    /// Number of live sandboxes tracked.
    pub fn len(&self) -> usize {
        self.sandboxes.len()
    }

    /// True iff no sandboxes are tracked.
    pub fn is_empty(&self) -> bool {
        self.sandboxes.is_empty()
    }
}

/// Quick sanity check: does the running kernel advertise the requested
/// namespaces? v0 just checks `/proc/self/ns` exists.
pub fn kernel_supports(_ns: &NamespaceSet) -> bool {
    // TODO(v1): statx /proc/self/ns/{mnt,pid,net,ipc,uts,user} and return
    // false if any requested namespace is missing.
    Path::new("/proc/self/ns").exists()
}

#[allow(dead_code)]
fn warn_if_unsupported(ns: &NamespaceSet) {
    if !kernel_supports(ns) {
        warn!("kernel may not support requested namespaces");
    }
}

// v0: stub implementation
