//! Sandbox runtime — supervises installed package processes inside their sandboxes, restarts on crash with backoff, and reports telemetry to the scheduler.
//!
//! [`SandboxRuntime`] is the long-lived supervisor that runs *after* the
//! installer has placed files on disk. It owns one [`SandboxHandle`] per
//! running install, polls each child for exit status, restarts crashed
//! sandboxes with an exponential backoff (1 s, 2 s, 4 s, 8 s, 16 s), and
//! gives up after [`MAX_RESTARTS`] attempts — at which point the sandbox
//! enters [`SandboxState::Locked`] and must be manually unlocked.
//!
//! v0: stub implementation. The handle table and state machine are in
//! place, but `start` / `stop` / `restart` return descriptive errors and no
//! child processes are actually spawned. See `TODO(v1)` markers.

use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::time::sleep;
use tracing::{debug, info, instrument, warn};
use uuid::Uuid;

use crate::sandbox_installer::InstallReceipt;

// ─── Errors ──────────────────────────────────────────────────────────────────

/// Errors returned by the sandbox runtime.
#[derive(Debug, Error)]
pub enum RuntimeError {
    /// No sandbox is registered under the requested id.
    #[error("sandbox not found: {0}")]
    NotFound(Uuid),
    /// The sandbox is already in [`SandboxState::Running`].
    #[error("sandbox already running: {0}")]
    AlreadyRunning(Uuid),
    /// `start` / `restart` failed to spawn the child or attach to its cgroup.
    #[error("start failed for sandbox {0}: {1}")]
    StartFailed(Uuid, String),
    /// The cgroup the sandbox expects to live in no longer exists.
    #[error("cgroup missing for sandbox {0}")]
    CgroupMissing(Uuid),
    /// The sandbox is [`SandboxState::Locked`] and refuses to restart.
    #[error("sandbox locked after {0} restarts: {1}")]
    Locked(usize, Uuid),
}

// ─── Sandbox id / state ──────────────────────────────────────────────────────

/// Stable identifier for a running sandbox. Matches the `sandbox_id` field
/// on the originating [`InstallReceipt`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SandboxId(pub Uuid);

impl From<Uuid> for SandboxId {
    fn from(u: Uuid) -> Self {
        Self(u)
    }
}

impl std::fmt::Display for SandboxId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Lifecycle state of a single sandbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxState {
    /// The child is running normally.
    Running,
    /// The child has exited cleanly and will not be restarted.
    Stopped,
    /// The child exited with a non-zero status and is awaiting restart.
    Crashed,
    /// The runtime is in the middle of restarting the child (between exit
    /// and respawn, inside the backoff window).
    Restarting,
    /// The runtime gave up after [`MAX_RESTARTS`] crashes. Manual
    /// intervention required.
    Locked,
}

// ─── Backoff schedule ────────────────────────────────────────────────────────

/// Maximum number of restart attempts before the sandbox is locked.
pub const MAX_RESTARTS: usize = 5;

/// Backoff schedule: 1 s, 2 s, 4 s, 8 s, 16 s. Indexed by `restart_count`
/// (clamped to the last entry).
pub const BACKOFF_SCHEDULE: [Duration; 5] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
    Duration::from_secs(8),
    Duration::from_secs(16),
];

/// Return the backoff delay for the `n`-th restart (0-indexed). Clamps to
/// the last entry in [`BACKOFF_SCHEDULE`].
pub fn backoff_for(restart_count: usize) -> Duration {
    BACKOFF_SCHEDULE[restart_count.min(BACKOFF_SCHEDULE.len() - 1)]
}

// ─── Sandbox status ──────────────────────────────────────────────────────────

/// Snapshot of a sandbox's runtime status, returned by
/// [`SandboxRuntime::status`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxStatus {
    /// Current lifecycle state.
    pub state: SandboxState,
    /// Host-side PID of the sandboxed child, or `-1` if not running.
    pub pid: i32,
    /// Wall-clock uptime of the current child (0 if not running). v0
    /// returns `Duration::ZERO`.
    pub uptime: Duration,
    /// Number of times the child has been restarted since the last
    /// successful `start`.
    pub restart_count: usize,
    /// CPU usage in percent (0.0–100.0 × ncores). v0 returns `0.0`.
    pub cpu_percent: f32,
    /// RSS memory in MiB. v0 returns `0`.
    pub mem_mb: u64,
}

// ─── Sandbox handle ──────────────────────────────────────────────────────────

/// Per-sandbox bookkeeping owned by [`SandboxRuntime`].
#[derive(Debug, Clone)]
pub struct SandboxHandle {
    /// Sandbox identifier (matches the receipt).
    pub id: SandboxId,
    /// Host-side PID of the sandboxed child. `-1` when not running.
    pub pid: i32,
    /// Receipt that produced this sandbox.
    pub receipt: InstallReceipt,
    /// Number of restarts since the last successful `start`.
    pub restart_count: usize,
    /// Last exit status observed (None if never run / still running).
    pub last_exit: Option<i32>,
    /// Current lifecycle state.
    pub state: SandboxState,
    /// When the current child was spawned (None if not running).
    pub started_at: Option<DateTime<Utc>>,
}

impl SandboxHandle {
    /// Build a handle for `receipt` in the `Stopped` state.
    pub fn new(receipt: InstallReceipt) -> Self {
        let id = SandboxId(receipt.sandbox_id);
        Self {
            id,
            pid: -1,
            receipt,
            restart_count: 0,
            last_exit: None,
            state: SandboxState::Stopped,
            started_at: None,
        }
    }

    /// True iff the sandbox is currently considered running.
    pub fn is_running(&self) -> bool {
        matches!(self.state, SandboxState::Running)
    }

    /// Wall-clock uptime of the current child, or zero.
    pub fn uptime(&self) -> Duration {
        match self.started_at {
            Some(t) if self.is_running() => {
                let now = Utc::now();
                (now - t).to_std().unwrap_or(Duration::ZERO)
            }
            _ => Duration::ZERO,
        }
    }
}

// ─── Supervisor (placeholder) ────────────────────────────────────────────────

/// The supervisor owns the polling loop that watches every sandbox child
/// for exit. v0 keeps it as a placeholder struct; v1 will hold the
/// `tokio::task::JoinHandle` for the polling loop.
#[derive(Debug, Default)]
pub struct Supervisor {
    /// v0: number of crashes observed since the supervisor started.
    pub crashes_observed: u64,
}

impl Supervisor {
    /// Construct a fresh supervisor.
    pub fn new() -> Self {
        Self::default()
    }
}

// ─── SandboxRuntime ──────────────────────────────────────────────────────────

/// Owns the table of running sandboxes and supervises their lifecycle.
pub struct SandboxRuntime {
    /// `id -> handle` for every sandbox the runtime knows about.
    pub sandboxes: HashMap<SandboxId, SandboxHandle>,
    /// Supervisor that polls children for exit. v0: unused.
    pub supervisor: Supervisor,
}

impl Default for SandboxRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl SandboxRuntime {
    /// Build an empty runtime.
    pub fn new() -> Self {
        Self {
            sandboxes: HashMap::new(),
            supervisor: Supervisor::new(),
        }
    }

    /// Register a sandbox handle (typically called by the installer after a
    /// successful install). Returns `Err(AlreadyRunning)` if a sandbox with
    /// the same id is already registered and running.
    pub fn register(&mut self, handle: SandboxHandle) -> Result<(), RuntimeError> {
        if let Some(existing) = self.sandboxes.get(&handle.id) {
            if existing.is_running() {
                return Err(RuntimeError::AlreadyRunning(handle.id.0));
            }
        }
        self.sandboxes.insert(handle.id, handle);
        Ok(())
    }

    /// Start a registered sandbox. v0: always returns `StartFailed`.
    #[instrument(skip(self), fields(id = %id))]
    pub async fn start(&mut self, id: SandboxId) -> Result<(), RuntimeError> {
        let handle = self
            .sandboxes
            .get_mut(&id)
            .ok_or(RuntimeError::NotFound(id.0))?;
        if handle.is_running() {
            warn!(%id, "start: already running");
            return Err(RuntimeError::AlreadyRunning(id.0));
        }
        if handle.state == SandboxState::Locked {
            return Err(RuntimeError::Locked(handle.restart_count, id.0));
        }

        info!(%id, "start: v0 stub — no child spawned");
        // TODO(v1):
        //   1. Look up the receipt's target prefix and entry binary.
        //   2. Build a fresh sandbox via `crate::isolation::create`.
        //   3. fork + exec the entry binary inside the sandbox.
        //   4. Install the seccomp profile from `crate::seccomp`.
        //   5. Record the child PID and started_at.
        handle.state = SandboxState::Running;
        handle.started_at = Some(Utc::now());
        handle.pid = -1; // v0 sentinel; v1 holds the real PID.
        Err(RuntimeError::StartFailed(
            id.0,
            "start pipeline not implemented in v0".to_string(),
        ))
    }

    /// Stop a running sandbox. v0: marks the handle `Stopped` and returns
    /// `Ok(())`; v1 will SIGTERM → grace period → SIGKILL.
    #[instrument(skip(self), fields(id = %id))]
    pub async fn stop(&mut self, id: SandboxId) -> Result<(), RuntimeError> {
        let handle = self
            .sandboxes
            .get_mut(&id)
            .ok_or(RuntimeError::NotFound(id.0))?;
        if !handle.is_running() {
            debug!(%id, "stop: not running, no-op");
            return Ok(());
        }
        info!(%id, "stop: v0 stub");
        // TODO(v1): SIGTERM, wait grace, SIGKILL, waitpid.
        handle.state = SandboxState::Stopped;
        handle.pid = -1;
        handle.started_at = None;
        Ok(())
    }

    /// Restart a sandbox: stop then start, applying backoff if this is a
    /// crash-driven restart.
    #[instrument(skip(self), fields(id = %id))]
    pub async fn restart(&mut self, id: SandboxId) -> Result<(), RuntimeError> {
        let handle = self
            .sandboxes
            .get_mut(&id)
            .ok_or(RuntimeError::NotFound(id.0))?;

        if handle.state == SandboxState::Locked {
            return Err(RuntimeError::Locked(handle.restart_count, id.0));
        }

        let delay = backoff_for(handle.restart_count);
        info!(%id, restart_count = handle.restart_count, ?delay, "restart: backoff");
        handle.state = SandboxState::Restarting;
        sleep(delay).await;

        handle.restart_count += 1;
        if handle.restart_count >= MAX_RESTARTS {
            warn!(%id, restart_count = handle.restart_count, "restart: giving up — locking");
            handle.state = SandboxState::Locked;
            return Err(RuntimeError::Locked(handle.restart_count, id.0));
        }

        // TODO(v1): real stop + start; v0 just flips state and returns.
        self.stop(id).await.ok();
        self.start(id).await
    }

    /// Snapshot the status of a sandbox, or `None` if unknown.
    pub fn status(&self, id: SandboxId) -> Option<SandboxStatus> {
        let h = self.sandboxes.get(&id)?;
        Some(SandboxStatus {
            state: h.state,
            pid: h.pid,
            uptime: h.uptime(),
            restart_count: h.restart_count,
            cpu_percent: 0.0, // TODO(v1): read from cgroup cpu.stat.
            mem_mb: 0,        // TODO(v1): read from cgroup memory.current.
        })
    }

    /// Number of sandboxes currently tracked (any state).
    pub fn len(&self) -> usize {
        self.sandboxes.len()
    }

    /// True iff no sandboxes are tracked.
    pub fn is_empty(&self) -> bool {
        self.sandboxes.is_empty()
    }

    /// Iterate over all known sandbox ids.
    pub fn ids(&self) -> impl Iterator<Item = SandboxId> + '_ {
        self.sandboxes.keys().copied()
    }

    /// Forget a sandbox that is in [`SandboxState::Stopped`] or
    /// [`SandboxState::Locked`]. Running sandboxes must be stopped first.
    pub fn forget(&mut self, id: SandboxId) -> Result<SandboxHandle, RuntimeError> {
        let h = self
            .sandboxes
            .get(&id)
            .ok_or(RuntimeError::NotFound(id.0))?;
        if h.is_running() {
            return Err(RuntimeError::AlreadyRunning(id.0));
        }
        self.sandboxes
            .remove(&id)
            .ok_or(RuntimeError::NotFound(id.0))
    }
}

// v0: stub implementation
