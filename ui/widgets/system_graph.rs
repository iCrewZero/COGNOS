//! System graph widget — renders the live agent/IPC/capability graph as a
//! force-directed visualization. Highlights active edges and capability
//! flows.
//!
//! The graph mirrors the runtime topology: each agent, service, ANFS
//! mount, memory store, kernel object, and user session is a node; each
//! IPC channel, capability grant, or open file is an edge. The widget
//! runs a small force simulation locally so the layout is independent of
//! the backend renderer (GTK4 canvas, custom GPU compositor, …).
//!
//! v0: stub implementation

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use super::resource_monitor::WidgetTree;

// ─── Geometry ────────────────────────────────────────────────────────────────

/// 2D position or velocity in graph-space pixels.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Vec2 {
    /// X component.
    pub x: f32,
    /// Y component.
    pub y: f32,
}

impl Vec2 {
    /// Build a new vector.
    pub fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    /// Element-wise add.
    pub fn add(self, other: Self) -> Self {
        Self::new(self.x + other.x, self.y + other.y)
    }

    /// Scalar multiply.
    pub fn scale(self, s: f32) -> Self {
        Self::new(self.x * s, self.y * s)
    }
}

/// Visible region of the graph in graph-space pixels.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Viewport {
    /// Top-left corner of the visible region.
    pub origin: Vec2,
    /// Width / height of the visible region.
    pub size: Vec2,
    /// Current zoom factor (1.0 = identity).
    pub zoom: f32,
}

// ─── Nodes & edges ───────────────────────────────────────────────────────────

/// Topological kind of a graph node — drives colour and iconography.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeKind {
    /// An AI agent (file_agent, coding_agent, …).
    #[default]
    Agent,
    /// A long-running service (memory, scheduler, …).
    Service,
    /// An ANFS filesystem mount.
    Filesystem,
    /// A cognitive memory store.
    MemoryStore,
    /// A kernel / HAL object.
    Kernel,
    /// A user session.
    User,
}

/// A single node in the live system graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphNode {
    /// Stable node identifier (UUID).
    pub id: Uuid,
    /// Display label (agent name, service name, …).
    pub label: String,
    /// Topological kind.
    pub kind: NodeKind,
    /// Current position in graph-space pixels.
    pub position: Vec2,
    /// Current velocity in graph-space pixels/sec (force simulation).
    pub velocity: Vec2,
}

/// A directed edge between two nodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphEdge {
    /// Source node id.
    pub from: Uuid,
    /// Destination node id.
    pub to: Uuid,
    /// Display label (capability name, IPC method, …).
    pub label: String,
    /// Activity level in `[0.0, 1.0]` — drives edge brightness / width.
    pub activity: f32,
}

// ─── Force layout ────────────────────────────────────────────────────────────

/// Parameters for the force-directed layout simulation.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct ForceLayout {
    /// Repulsion constant applied between every pair of nodes.
    pub repulsion: f32,
    /// Spring constant applied along every edge.
    pub spring: f32,
    /// Velocity damping factor applied each tick (0..1).
    pub damping: f32,
}

impl ForceLayout {
    /// Sensible defaults for a desktop-sized canvas.
    pub fn defaults() -> Self {
        Self {
            repulsion: 8000.0,
            spring: 0.05,
            damping: 0.85,
        }
    }
}

// ─── SystemGraph ─────────────────────────────────────────────────────────────

/// Top-level live system graph widget.
///
/// Holds the current node/edge sets plus the force-layout state. The
/// shell calls [`SystemGraph::tick`] each frame to advance the
/// simulation, then [`SystemGraph::render`] to produce a [`WidgetTree`].
#[derive(Debug, Clone)]
pub struct SystemGraph {
    /// Current set of nodes.
    pub nodes: Vec<GraphNode>,
    /// Current set of edges.
    pub edges: Vec<GraphEdge>,
    /// Force-layout parameters.
    pub layout: ForceLayout,
    /// Visible region of the graph.
    pub viewport: Viewport,
}

impl SystemGraph {
    /// Build an empty graph with default layout and viewport.
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            edges: Vec::new(),
            layout: ForceLayout::defaults(),
            viewport: Viewport::default(),
        }
    }

    /// Advance the force simulation by `dt` seconds.
    ///
    /// v0: no-op — the actual repulsion / spring / damping integration
    /// lands in v1.
    pub fn tick(&mut self, _dt: f32) {
        // TODO(v1): for every pair of nodes, apply a repulsive force
        // proportional to `layout.repulsion / dist^2`; for every edge,
        // apply a spring force proportional to `layout.spring * dist`;
        // integrate velocity with `layout.damping`; clamp to viewport.
    }

    /// Render the graph as a toolkit-agnostic widget tree.
    ///
    /// v0: returns an empty tree — the canvas / node rendering lands in v1.
    pub fn render(&self) -> WidgetTree {
        // TODO(v1): emit a Canvas node with one sub-node per graph node
        // (positioned by `position`, coloured by `kind`) and one line
        // sub-node per edge (width / brightness by `activity`).
        WidgetTree::default()
    }
}

impl Default for SystemGraph {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Errors ──────────────────────────────────────────────────────────────────

/// Errors returned by the system graph (reserved for v1).
#[derive(Debug, Error)]
pub enum SystemGraphError {
    /// An edge referenced a node id that is not in `nodes`.
    #[error("dangling edge: {0}")]
    DanglingEdge(Uuid),
}

// v0: stub implementation
