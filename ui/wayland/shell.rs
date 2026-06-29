//! COGNOS Wayland shell — the desktop shell surface hosting the intent
//! bar, approval popups, and widget overlays. Runs as a Wayland client
//! with layer-shell.
//!
//! The shell is the single trusted UI surface on the system: it holds
//! the layer-shell surfaces for the intent bar (top of screen), approval
//! popups (centre modal), and the always-on widget overlays (CPU/agent
//! status, system graph). All HAL approval requests route through here —
//! no other client is permitted to draw approval UI.
//!
//! v0: stub implementation

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, info, warn};
use uuid::Uuid;

// ─── Placeholder Wayland handles ─────────────────────────────────────────────

/// Opaque handle to the connected Wayland compositor.
///
/// v0: unit placeholder — v1 will alias `wayland_client::Attached<WlCompositor>`.
#[derive(Debug, Clone, Default)]
pub struct CompositorHandle;

/// Opaque handle to the bound `zwlr_layer_shell_v1` global.
///
/// v0: unit placeholder — v1 will alias the smithay-client-toolkit type.
#[derive(Debug, Clone, Default)]
pub struct LayerShellHandle;

/// Opaque handle to a single layer surface (intent bar, approval popup, …).
#[derive(Debug, Clone, Default)]
pub struct LayerSurfaceHandle {
    /// Whether the surface is currently mapped (visible).
    pub mapped: bool,
}

// ─── Widget trait ────────────────────────────────────────────────────────────

/// Toolkit-agnostic widget trait implemented by every overlay widget
/// (resource monitor, agent status panel, system graph, …).
///
/// The shell owns a boxed list of these and ticks each one per frame.
pub trait Widget: Send {
    /// Human-readable widget name (used for layer naming and debugging).
    fn name(&self) -> &str;
    /// Advance the widget by `dt` seconds (pull fresh data, run
    /// animations, …). Default is a no-op for static widgets.
    fn tick(&mut self, _dt: f32) {}
}

// ─── Gate request ────────────────────────────────────────────────────────────

/// HAL gate request surfaced to the user via the approval popup.
///
/// v0: lightweight placeholder — v1 will alias the real
/// `hal::approval_flow::GateRequest` so the shell never re-encodes the
/// HAL's decision payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateRequest {
    /// Agent requesting the action.
    pub agent_id: Uuid,
    /// Human-readable summary of the proposed action.
    pub action: String,
    /// HAL risk score in `[0.0, 1.0]`.
    pub risk: f32,
    /// Capabilities the action would exercise.
    pub capabilities: Vec<String>,
}

// ─── Intent bar & approval UI ────────────────────────────────────────────────

/// Intent bar sub-component (top-of-screen layer surface).
#[derive(Debug, Default)]
pub struct IntentBar {
    /// Layer surface handle.
    pub surface: LayerSurfaceHandle,
    /// Whether the bar is currently visible.
    pub visible: bool,
    /// Pending intent text the user is composing, if any.
    pub draft: String,
}

/// Approval popup sub-component (centre modal layer surface).
#[derive(Debug, Default)]
pub struct ApprovalUi {
    /// Layer surface handle.
    pub surface: LayerSurfaceHandle,
    /// The gate request currently awaiting user decision, if any.
    pub pending: Option<GateRequest>,
}

// ─── Errors ──────────────────────────────────────────────────────────────────

/// Errors returned by [`CognosShell`] operations.
#[derive(Debug, Error)]
pub enum ShellError {
    /// The layer-shell protocol is not supported by the compositor.
    #[error("layer-shell not supported")]
    LayerShellUnsupported,
    /// Creating a layer surface failed.
    #[error("layer surface creation failed: {0}")]
    LayerSurfaceFailed(String),
    /// The compositor connection was lost.
    #[error("compositor connection lost")]
    CompositorLost,
}

// ─── CognosShell ─────────────────────────────────────────────────────────────

/// Top-level COGNOS Wayland shell.
///
/// Owns the compositor + layer-shell handles, the intent bar, the
/// approval popup, and the set of always-on overlay widgets. The shell
/// is constructed once at session start and runs until the user logs out
/// or the compositor crashes (in which case [`WaylandSession::restart`]
/// rebuilds it).
///
/// [`WaylandSession::restart`]: super::session::WaylandSession::restart
#[derive(Debug)]
pub struct CognosShell {
    /// Connected compositor handle.
    pub compositor: CompositorHandle,
    /// Bound layer-shell global.
    pub layer_shell: LayerShellHandle,
    /// Intent bar sub-component.
    pub intent_bar: IntentBar,
    /// Approval popup sub-component.
    pub approval_ui: ApprovalUi,
    /// Registered overlay widgets, keyed by name for O(1) removal.
    pub widgets: HashMap<String, Box<dyn Widget>>,
}

impl CognosShell {
    /// Build a new shell bound to the supplied display.
    ///
    /// v0: returns [`ShellError::LayerSurfaceFailed`] — actual
    /// `zwlr_layer_shell_v1` wiring lands in v1.
    pub fn new(_display: &str) -> Result<Self, ShellError> {
        // TODO(v1): roundtrip the registry, bind wl_compositor and
        // zwlr_layer_shell_v1, allocate the intent-bar + approval layer
        // surfaces.
        Err(ShellError::LayerSurfaceFailed("v0 stub".to_string()))
    }

    /// Run the shell event loop until shutdown.
    ///
    /// v0: returns immediately — the actual dispatch loop lands in v1.
    pub fn run(&mut self) -> Result<(), ShellError> {
        info!("cognos-shell: entering run loop (v0 stub)");
        // TODO(v1): pump the Wayland event queue, dispatch intent-bar
        // text events, drive approval popup accept/deny, tick widgets.
        Ok(())
    }

    /// Show the intent bar (e.g. on user hotkey).
    pub fn show_intent_bar(&mut self) {
        debug!("cognos-shell: show intent bar");
        self.intent_bar.visible = true;
        self.intent_bar.surface.mapped = true;
    }

    /// Surface a HAL gate request as an approval popup.
    pub fn show_approval(&mut self, request: GateRequest) {
        warn!(
            agent = %request.agent_id,
            risk = request.risk,
            action = %request.action,
            "cognos-shell: surfacing HAL gate request for approval"
        );
        self.approval_ui.pending = Some(request);
        self.approval_ui.surface.mapped = true;
    }

    /// Dismiss the currently displayed approval popup.
    pub fn hide_approval(&mut self) {
        debug!("cognos-shell: hide approval popup");
        self.approval_ui.pending = None;
        self.approval_ui.surface.mapped = false;
    }

    /// Register an overlay widget under its [`Widget::name`].
    ///
    /// Replaces any existing widget with the same name.
    pub fn add_widget(&mut self, widget: Box<dyn Widget>) {
        let name = widget.name().to_string();
        debug!(widget = %name, "cognos-shell: register widget");
        self.widgets.insert(name, widget);
    }
}

// v0: stub implementation
