//! Compositor hooks — subscribes to workspace switches, window focus
//! changes, and surface events. Feeds context to the cognitive preloader.
//!
//! The hooks sit between the Wayland event queue and the intent engine:
//! every workspace switch, window focus change, and surface
//! create/destroy is forwarded as a [`CompositorEvent`] on a tokio
//! channel. The intent engine's action-graph module consumes these to
//! predict the user's next intent (e.g. "user switched to the browser
//! workspace → preload browser agent context").
//!
//! Wiring target: `intent_engine::action_graph` consumes the event
//! stream to seed context prediction (see docs/SPEC.md §context preloader).
//!
//! v0: stub implementation

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::mpsc::Sender;
use tracing::{debug, info, warn};
use uuid::Uuid;

// ─── Identifiers ─────────────────────────────────────────────────────────────

/// Stable identifier for a Wayland surface / toplevel window.
pub type WindowId = Uuid;

// ─── Events ──────────────────────────────────────────────────────────────────

/// Compositor event forwarded to the cognitive preloader.
///
/// These are deliberately coarse-grained: the hooks surface *what*
/// changed, not the raw wl_* protocol arguments. The intent engine
/// decides what (if anything) to preload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CompositorEvent {
    /// The active workspace changed (the preloader typically uses this
    /// to swap in the workspace's default agent context).
    WorkspaceChanged,
    /// A window received keyboard focus.
    WindowFocused(WindowId),
    /// A new window was created.
    WindowCreated(WindowId),
    /// A window was closed.
    WindowClosed(WindowId),
    /// An output (monitor) was resized or re-arranged.
    OutputResized,
}

// ─── Errors ──────────────────────────────────────────────────────────────────

/// Errors returned by [`CompositorHooks`] operations.
#[derive(Debug, Error)]
pub enum HookError {
    /// The hook listener could not be attached to the compositor.
    #[error("install failed: {0}")]
    InstallFailed(String),
    /// The event channel was closed (consumer dropped the receiver).
    #[error("event channel closed")]
    ChannelClosed,
}

// ─── CompositorHooks ─────────────────────────────────────────────────────────

/// Subscribes to compositor events and forwards them on `tx`.
///
/// Owned by the shell; `tx` is held by the intent engine's action-graph
/// context predictor. Hooks are installed once at session start and
/// uninstalled on shutdown.
#[derive(Debug)]
pub struct CompositorHooks {
    /// Sender for the compositor-event channel. The intent engine holds
    /// the matching `Receiver`.
    pub tx: Sender<CompositorEvent>,
    /// Whether the hooks are currently installed.
    pub installed: bool,
}

impl CompositorHooks {
    /// Build a new hook set writing to `tx`.
    pub fn new(tx: Sender<CompositorEvent>) -> Self {
        Self {
            tx,
            installed: false,
        }
    }

    /// Attach the hooks to the compositor event queue.
    ///
    /// v0: returns [`HookError::InstallFailed`] — actual listener
    /// wiring lands in v1.
    pub fn install(&self) -> Result<(), HookError> {
        // TODO(v1): register a wl_registry / wlr-foreign-toplevel listener
        // that translates protocol events into CompositorEvent variants
        // and pushes them onto `tx`.
        info!("compositor-hooks: install requested (v0 stub)");
        Err(HookError::InstallFailed("v0 stub".to_string()))
    }

    /// Detach the hooks (called on session shutdown).
    pub fn uninstall(&self) {
        // TODO(v1): drop the listener handles; the event queue stops
        // calling our callbacks.
        debug!("compositor-hooks: uninstall requested (v0 stub)");
    }

    /// Forward a single event on the channel.
    ///
    /// v0: returns [`HookError::ChannelClosed`] — the v1 listener callback
    /// calls this for each translated protocol event, and the intent
    /// engine's action-graph consumer is responsible for actually
    /// predicting the next intent.
    pub fn handle_event(&self, event: CompositorEvent) -> Result<(), HookError> {
        // TODO(v1): `self.tx.try_send(event)` (or `block_on(send)` on a
        // blocking thread) — on a closed receiver, surface ChannelClosed.
        // The intent engine's action_graph consumer turns each event into
        // a context-prediction hint for the cognitive preloader.
        warn!(?event, "compositor-hooks: event dropped (v0 stub)");
        let _ = &self.tx;
        Err(HookError::ChannelClosed)
    }
}

// v0: stub implementation
