//! Memory browser widget — lets the user inspect, search, edit, and forget
//! memories stored in the cognitive context store. Shows provenance and
//! importance.
//!
//! The browser is the human-facing surface for the memory subsystem
//! (`memory/` crate). It exposes the same operations the agents use
//! internally — but gated by an explicit `memory.write` / `memory.forget`
//! capability so that editing or forgetting a memory is always a
//! deliberate, audited action.
//!
//! v0: stub implementation

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use super::resource_monitor::WidgetTree;

// ─── Identifiers & shared types ──────────────────────────────────────────────

/// Stable identifier for a memory record.
pub type MemoryId = Uuid;

/// Stable identifier for the agent that authored / owns a memory.
pub type AgentId = Uuid;

// ─── Memory record ───────────────────────────────────────────────────────────

/// A single memory record rendered as a row in the browser.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memory {
    /// Stable memory identifier.
    pub id: MemoryId,
    /// Free-text content of the memory.
    pub content: String,
    /// Agent that authored the memory.
    pub agent: AgentId,
    /// Importance score in `[0.0, 1.0]` (drives retention / eviction).
    pub importance: f32,
    /// Free-form tags attached by the agent or user.
    pub tags: Vec<String>,
    /// Where the memory originated (file path, IPC envelope id, …).
    pub provenance: String,
    /// When the memory was created.
    pub created_at: DateTime<Utc>,
    /// When the memory was last accessed or reinforced.
    pub updated_at: DateTime<Utc>,
}

// ─── Filter ──────────────────────────────────────────────────────────────────

/// Filter applied on top of the search query.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryFilter {
    /// Restrict to memories authored by this agent, if set.
    pub agent: Option<AgentId>,
    /// Restrict to memories carrying any of these tags.
    pub tags: Vec<String>,
    /// Drop memories with `importance < min_importance`.
    pub min_importance: f32,
    /// Inclusive `[start, end]` date range on `created_at`.
    pub date_range: Option<(DateTime<Utc>, DateTime<Utc>)>,
}

// ─── Browser actions ─────────────────────────────────────────────────────────

/// Operations the user can perform on the visible memory set.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BrowserAction {
    /// Replace the content of a memory with the supplied string.
    Edit(MemoryId, String),
    /// Permanently forget a memory (requires `memory.forget` capability).
    Forget(MemoryId),
    /// Attach a tag to a memory.
    Tag(MemoryId, String),
    /// Remove a tag from a memory.
    Untag(MemoryId, String),
    /// Export the listed memories to a JSON-LD bundle.
    Export(Vec<MemoryId>),
}

// ─── Errors ──────────────────────────────────────────────────────────────────

/// Errors returned by [`MemoryBrowser::handle_action`].
#[derive(Debug, Error)]
pub enum BrowserError {
    /// The supplied memory id is not present in the browser.
    #[error("unknown memory: {0}")]
    UnknownMemory(MemoryId),
    /// The caller lacks the capability required for the action.
    #[error("capability denied: {0}")]
    CapabilityDenied(String),
    /// The backing memory store rejected the write.
    #[error("store error: {0}")]
    Store(String),
    /// Export target was unwritable.
    #[error("export failed: {0}")]
    Export(String),
}

// ─── MemoryBrowser ───────────────────────────────────────────────────────────

/// Top-level memory browser widget.
///
/// Holds the visible memory set, the current search query, the active
/// filter, and the selected row. The shell pushes new query results via
/// [`MemoryBrowser::search`] / [`MemoryBrowser::apply_filter`], then
/// calls [`MemoryBrowser::render`] each frame.
#[derive(Debug, Clone, Default)]
pub struct MemoryBrowser {
    /// Memories currently matching the query + filter.
    pub memories: Vec<Memory>,
    /// Active free-text search query (empty = match all).
    pub query: String,
    /// Active structured filter.
    pub filter: MemoryFilter,
    /// Currently highlighted memory row, if any.
    pub selected: Option<MemoryId>,
}

impl MemoryBrowser {
    /// Build an empty browser.
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the free-text search query.
    ///
    /// v0: stores the query only — actual vector / tag search lands in v1.
    pub fn search(&mut self, query: String) {
        // TODO(v1): proxy to `memory::query::search` with the current
        // filter applied, then populate `self.memories`.
        self.query = query;
    }

    /// Replace the structured filter.
    ///
    /// v0: stores the filter only.
    pub fn apply_filter(&mut self, filter: MemoryFilter) {
        // TODO(v1): re-run the search with the new filter.
        self.filter = filter;
    }

    /// Render the browser as a toolkit-agnostic widget tree.
    ///
    /// v0: returns an empty tree — the list layout lands in v1.
    pub fn render(&self) -> WidgetTree {
        // TODO(v1): emit a search box, tag filter chips, a list of memory
        // rows (content preview, importance bar, provenance, tags), and
        // a detail pane for the selected memory.
        WidgetTree::default()
    }

    /// Execute a user-initiated [`BrowserAction`].
    ///
    /// v0: always returns [`BrowserError::Store`] — the memory store
    /// plumbing lands in v1.
    pub fn handle_action(&mut self, action: BrowserAction) -> Result<(), BrowserError> {
        // TODO(v1): enforce the relevant capability (`memory.write` for
        // Edit/Tag/Untag, `memory.forget` for Forget, `memory.export`
        // for Export), then forward to the memory crate.
        match action {
            BrowserAction::Edit(id, _) => {
                if !self.memories.iter().any(|m| m.id == id) {
                    return Err(BrowserError::UnknownMemory(id));
                }
                Err(BrowserError::Store("edit: v0 stub".to_string()))
            }
            BrowserAction::Forget(id) => {
                if !self.memories.iter().any(|m| m.id == id) {
                    return Err(BrowserError::UnknownMemory(id));
                }
                Err(BrowserError::Store("forget: v0 stub".to_string()))
            }
            BrowserAction::Tag(id, _) => {
                if !self.memories.iter().any(|m| m.id == id) {
                    return Err(BrowserError::UnknownMemory(id));
                }
                Err(BrowserError::Store("tag: v0 stub".to_string()))
            }
            BrowserAction::Untag(id, _) => {
                if !self.memories.iter().any(|m| m.id == id) {
                    return Err(BrowserError::UnknownMemory(id));
                }
                Err(BrowserError::Store("untag: v0 stub".to_string()))
            }
            BrowserAction::Export(_) => Err(BrowserError::Export("v0 stub".to_string())),
        }
    }
}

// v0: stub implementation
