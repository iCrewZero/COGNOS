//! Wayland session — manages compositor connection, registry bind, and
//! session teardown. Restarts shell on compositor crash.
//!
//! The session is the outermost lifecycle boundary for the UI: it opens
//! the `wayland-0` socket, performs the registry handshake, binds the
//! globals the shell needs (`wl_compositor`, `wl_shm`, `zwlr_layer_shell_v1`,
//! `wl_output`, …), and tears everything down on shutdown. If the
//! compositor crashes mid-session, [`WaylandSession::restart`] re-runs
//! the handshake and lets the shell re-attach.
//!
//! v0: stub implementation

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{info, warn};

// ─── Placeholder Wayland handles ─────────────────────────────────────────────

/// Opaque handle to an open `wl_display` connection.
///
/// v0: unit placeholder — v1 will alias `wayland_client::Display`.
#[derive(Debug, Clone, Default)]
pub struct DisplayHandle;

/// Opaque handle to a bound `wl_registry`.
///
/// v0: unit placeholder.
#[derive(Debug, Clone, Default)]
pub struct RegistryHandle;

/// Identifier for a bound Wayland global.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GlobalName(pub u32);

/// Snapshot of all globals currently bound by this session.
///
/// Keyed by the registry-assigned name; value is the interface string
/// (e.g. `"wl_compositor"`, `"zwlr_layer_shell_v1"`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BoundObjects {
    /// Map of global name → interface name.
    pub globals: HashMap<GlobalName, String>,
}

// ─── Errors ──────────────────────────────────────────────────────────────────

/// Errors returned by [`WaylandSession`] operations.
#[derive(Debug, Error)]
pub enum SessionError {
    /// No `WAYLAND_DISPLAY` / `XDG_RUNTIME_DIR` was found.
    #[error("no wayland display available")]
    NoDisplay,
    /// The compositor process crashed or dropped the connection.
    #[error("compositor crash")]
    CompositorCrash,
    /// The registry global disappeared before we could bind it.
    #[error("registry lost")]
    RegistryLost,
    /// Binding a required global failed.
    #[error("bind failed for {0}")]
    BindFailed(String),
}

// ─── WaylandSession ──────────────────────────────────────────────────────────

/// Top-level Wayland session lifecycle manager.
///
/// Owns the display connection, the bound registry, and the set of
/// globals currently bound. The shell borrows the session to obtain
/// handles; on compositor crash, the session tears down and rebuilds
/// itself in place.
#[derive(Debug)]
pub struct WaylandSession {
    /// Open display connection.
    pub display: DisplayHandle,
    /// Bound registry handle.
    pub registry: RegistryHandle,
    /// Snapshot of currently bound globals.
    pub bound_objects: BoundObjects,
    /// Number of times [`WaylandSession::restart`] has fired.
    pub restart_count: u32,
}

impl WaylandSession {
    /// Open the default display and perform the initial registry bind.
    ///
    /// v0: returns [`SessionError::NoDisplay`] — actual `wl_display`
    /// wiring lands in v1.
    pub fn connect() -> Result<Self, SessionError> {
        // TODO(v1): read $WAYLAND_DISPLAY / $XDG_RUNTIME_DIR, open
        // `wayland_client::Display::connect_to_env`, roundtrip the
        // registry, bind wl_compositor / wl_shm / zwlr_layer_shell_v1.
        info!("cognos-session: connect requested (v0 stub)");
        Err(SessionError::NoDisplay)
    }

    /// Dispatch one batch of pending Wayland events.
    ///
    /// v0: returns `Ok(())` immediately.
    pub fn dispatch(&mut self) -> Result<(), SessionError> {
        // TODO(v1): `display.flush()` + `event_queue.dispatch()`,
        // surface any compositor disconnection as `CompositorCrash`.
        Ok(())
    }

    /// Tear down the current connection and re-perform the handshake.
    ///
    /// Bounded by [`Self::restart_count`] so a flapping compositor does
    /// not spin forever.
    ///
    /// v0: returns [`SessionError::CompositorCrash`] — actual re-handshake
    /// lands in v1.
    pub fn restart(&mut self) -> Result<(), SessionError> {
        const MAX_RESTARTS: u32 = 5;
        self.restart_count += 1;
        warn!(
            restart_count = self.restart_count,
            max = MAX_RESTARTS,
            "cognos-session: compositor restart requested (v0 stub)"
        );
        if self.restart_count > MAX_RESTARTS {
            return Err(SessionError::CompositorCrash);
        }
        // TODO(v1): drop display + registry, re-call connect(), rebind
        // all required globals, signal the shell to re-attach.
        Err(SessionError::CompositorCrash)
    }

    /// Gracefully tear down the session.
    ///
    /// v0: no-op.
    pub fn shutdown(&mut self) {
        info!("cognos-session: shutdown requested (v0 stub)");
        // TODO(v1): drop display + registry, flush any pending events,
        // release bound globals in reverse bind order.
    }
}

// v0: stub implementation
