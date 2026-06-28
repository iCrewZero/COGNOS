//! Agent status widget — shows each AI agent's state, current task, trust
//! score, capability grants, and last action. Click-through to audit trail.
//!
//! The panel is fed by the agent registry (one [`AgentState`] per running
//! agent). Clicking an agent row raises an [`Action`] that the shell
//! interprets — typically opening the HAL audit trail filtered to that
//! agent, or surfacing the agent's pending HAL gate request.
//!
//! v0: stub implementation

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

// Re-use the toolkit-agnostic widget tree defined alongside the resource
// monitor widget. v1 will promote this into a shared `ui::widget` module.
use super::resource_monitor::{WidgetKind, WidgetNode, WidgetTree};

// ─── Identifiers & shared types ──────────────────────────────────────────────

/// Stable identifier for an agent (same type used across HAL / IPC).
pub type AgentId = Uuid;

/// A capability grant issued to an agent by the HAL capability lattice.
///
/// v0: opaque string tag (e.g. `"fs.read:/home"`, `"net.tcp:443"`).
/// v1 will alias `hal::capability_lattice::Capability`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Capability(pub String);

/// Discrete lifecycle state of an agent, mirrored from the registry.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentStatus {
    /// Idle — no active intent, awaiting input.
    #[default]
    Idle,
    /// Thinking — running inference / planning.
    Thinking,
    /// Awaiting HAL — a proposed action is pending HAL approval.
    AwaitingHal,
    /// Executing — running an approved action.
    Executing,
    /// Error — last action failed or agent crashed.
    Error,
    /// Stopped — terminated by the user or scheduler.
    Stopped,
}

// ─── AgentState ──────────────────────────────────────────────────────────────

/// Per-agent snapshot rendered as a single row in the panel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentState {
    /// Stable agent identifier.
    pub id: AgentId,
    /// Human-readable display name (e.g. `"file_agent"`).
    pub name: String,
    /// Lifecycle state.
    pub status: AgentStatus,
    /// Short description of the current task, if any.
    pub current_task: Option<String>,
    /// HAL trust score in `[0.0, 1.0]`.
    pub trust_score: f32,
    /// Capabilities currently granted to this agent.
    pub capabilities: Vec<Capability>,
    /// Human-readable summary of the last action attempted.
    pub last_action: Option<String>,
    /// When the last action was observed.
    pub last_action_time: Option<DateTime<Utc>>,
}

// ─── Click actions ───────────────────────────────────────────────────────────

/// Action emitted by [`AgentStatusPanel::handle_click`] for the shell to
/// interpret. Lets the widget stay decoupled from the audit-trail UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Action {
    /// Open the HAL audit trail filtered to the selected agent.
    OpenAuditTrail(AgentId),
    /// Surface the agent's pending HAL gate request, if any.
    ShowPendingGate(AgentId),
    /// No action (e.g. clicked the already-selected row).
    None,
}

// ─── AgentStatusPanel ────────────────────────────────────────────────────────

/// Top-level per-agent status panel.
///
/// Holds the latest set of agent states plus the currently selected row.
/// The shell calls [`AgentStatusPanel::update`] whenever the registry
/// publishes a new snapshot, then [`AgentStatusPanel::render`] each frame.
#[derive(Debug, Clone, Default)]
pub struct AgentStatusPanel {
    /// Latest per-agent states, in registry order.
    pub agents: Vec<AgentState>,
    /// Currently highlighted agent row, if any.
    pub selected: Option<AgentId>,
}

impl AgentStatusPanel {
    /// Build an empty panel.
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the entire agent set (called by the registry poller).
    pub fn update(&mut self, agents: Vec<AgentState>) {
        // Preserve selection if the agent is still present.
        if let Some(sel) = self.selected {
            if !agents.iter().any(|a| a.id == sel) {
                self.selected = None;
            }
        }
        self.agents = agents;
    }

    /// Render the panel as a toolkit-agnostic widget tree.
    ///
    /// v0: returns an empty tree — the row layout lands in v1.
    pub fn render(&self) -> WidgetTree {
        // TODO(v1): emit a vertical Box of per-agent rows; each row shows
        // name, status badge, trust bar, capability count, and last-action
        // summary. The selected row gets a highlight class.
        let _ = WidgetKind::default();
        let _ = WidgetNode::default();
        WidgetTree::default()
    }

    /// Handle a click on a row identified by agent id.
    ///
    /// Returns the [`Action`] the shell should perform; updates the
    /// internal selection state.
    ///
    /// v0: always returns [`Action::None`] — click-through wiring is v1.
    pub fn handle_click(&mut self, agent_id: AgentId) -> Action {
        // TODO(v1): set `self.selected`, look up the agent, and decide
        // between OpenAuditTrail / ShowPendingGate based on whether the
        // agent currently holds a pending HAL gate request.
        self.selected = Some(agent_id);
        Action::None
    }
}

// ─── Errors ──────────────────────────────────────────────────────────────────

/// Errors returned by the agent status panel (reserved for v1).
#[derive(Debug, Error)]
pub enum AgentStatusError {
    /// The supplied agent id is not present in the panel.
    #[error("unknown agent: {0}")]
    UnknownAgent(AgentId),
}

// v0: stub implementation
