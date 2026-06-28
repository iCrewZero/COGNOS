//! Task graph — a directed acyclic graph of tasks. Each node is a unit of
//! work assigned to one agent; edges represent data/control dependencies.
//! The graph is mutable: tasks can be added, dependencies rewired, and
//! sub-graphs replaced.
//!
//! v0: stub implementation.

use std::collections::{HashMap, HashSet, VecDeque};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

// ─── Identifiers ────────────────────────────────────────────────────────────

/// Unique identifier for a task node within the graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TaskId(pub Uuid);

/// Unique identifier for an agent that may own a [`TaskNode`]. Re-exported
/// by `runtime`, `event_bus`, and `scheduler` so callers can pull it from
/// whichever module is most convenient.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AgentId(pub String);

// ─── Node state ─────────────────────────────────────────────────────────────

/// Lifecycle state of a single [`TaskNode`] in the graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeState {
    /// Accepted but dependencies not yet satisfied.
    Pending,
    /// All dependencies satisfied; eligible for dispatch.
    Ready,
    /// Dispatched to an agent; awaiting completion.
    Running,
    /// Completed successfully; outputs available to dependents.
    Succeeded,
    /// Failed terminally; dependents will be skipped.
    Failed,
    /// Skipped because an upstream node failed or was cancelled.
    Skipped,
}

impl Default for NodeState {
    fn default() -> Self {
        Self::Pending
    }
}

// ─── Task result ────────────────────────────────────────────────────────────

/// Result payload produced by completing a [`TaskNode`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskResult {
    /// JSON-encoded output of the task (inputs to downstream nodes).
    pub output: serde_json::Value,
    /// Optional error message; set when `state == Failed`.
    pub error: Option<String>,
}

impl Default for TaskResult {
    fn default() -> Self {
        Self {
            output: serde_json::Value::Null,
            error: None,
        }
    }
}

// ─── Task node ──────────────────────────────────────────────────────────────

/// A unit of work in the task graph. Owned by one [`AgentId`] and
/// parameterised by an `intent` blob (opaque to the graph itself).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskNode {
    /// Node id. The graph mints a fresh [`Uuid`] if [`Uuid::nil`] is
    /// supplied on insertion.
    pub id: TaskId,
    /// Agent that will execute this node.
    pub agent: AgentId,
    /// Opaque intent payload (typically a serialised `runtime::Intent`).
    pub intent: serde_json::Value,
    /// Current lifecycle state.
    pub state: NodeState,
    /// Names of input slots this node consumes from its predecessors.
    pub inputs: Vec<String>,
    /// Names of output slots this node produces for its successors.
    pub outputs: Vec<String>,
    /// Number of times this node has been retried after a transient
    /// failure.
    pub retry_count: u32,
}

// ─── Edges ──────────────────────────────────────────────────────────────────

/// Kind of dependency between two [`TaskNode`]s.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EdgeKind {
    /// Data dependency. The carried [`String`] names the output slot of
    /// `from` that feeds an input slot of `to`.
    Data(String),
    /// Pure control dependency: `to` must not start until `from` finishes,
    /// but no data flows between them.
    Control,
}

/// A directed edge in the task graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edge {
    /// Source node.
    pub from: TaskId,
    /// Destination node.
    pub to: TaskId,
    /// Edge semantics.
    pub kind: EdgeKind,
}

// ─── Errors ─────────────────────────────────────────────────────────────────

/// Errors returned by graph mutation / query operations.
#[derive(Debug, Error)]
pub enum GraphError {
    /// Adding an edge would introduce a cycle.
    #[error("cycle detected when adding edge {from:?} -> {to:?}")]
    CycleDetected {
        /// Source of the offending edge.
        from: TaskId,
        /// Destination of the offending edge.
        to: TaskId,
    },
    /// One or both endpoints of an edge do not exist.
    #[error("missing node: {0:?}")]
    MissingNode(TaskId),
    /// The same edge (from, to, kind) already exists.
    #[error("duplicate edge {from:?} -> {to:?}")]
    DuplicateEdge {
        /// Source of the duplicate edge.
        from: TaskId,
        /// Destination of the duplicate edge.
        to: TaskId,
    },
}

// ─── TaskGraph ──────────────────────────────────────────────────────────────

/// A directed acyclic graph of [`TaskNode`]s connected by [`Edge`]s.
pub struct TaskGraph {
    /// id → node.
    pub nodes: HashMap<TaskId, TaskNode>,
    /// All edges, stored as a flat list. v0: a Vec is sufficient for the
    /// expected graph sizes; v1 may switch to an adjacency-list for faster
    /// successor/predecessor lookups.
    pub edges: Vec<Edge>,
}

impl TaskGraph {
    /// Construct an empty graph.
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            edges: Vec::new(),
        }
    }

    /// Add a [`TaskNode`] to the graph. If `node.id` is [`Uuid::nil`], a
    /// fresh id is minted and assigned. Returns the (possibly minted) id.
    pub fn add_task(&mut self, mut node: TaskNode) -> TaskId {
        if node.id.0 == Uuid::nil() {
            node.id = TaskId(Uuid::new_v4());
        }
        let id = node.id;
        self.nodes.insert(id, node);
        id
    }

    /// Add a directed edge `from → to` of the given [`EdgeKind`]. Returns
    /// an error if either endpoint is missing, the edge already exists, or
    /// adding it would introduce a cycle.
    pub fn add_dependency(
        &mut self,
        from: TaskId,
        to: TaskId,
        kind: EdgeKind,
    ) -> Result<(), GraphError> {
        if !self.nodes.contains_key(&from) {
            return Err(GraphError::MissingNode(from));
        }
        if !self.nodes.contains_key(&to) {
            return Err(GraphError::MissingNode(to));
        }
        let new_edge = Edge {
            from,
            to,
            kind: kind.clone(),
        };
        if self
            .edges
            .iter()
            .any(|e| e.from == new_edge.from && e.to == new_edge.to && e.kind == new_edge.kind)
        {
            return Err(GraphError::DuplicateEdge { from, to });
        }
        // Cycle check: adding `from -> to` introduces a cycle iff there is
        // already a path from `to` back to `from`.
        if self.reachable(to, from) {
            return Err(GraphError::CycleDetected { from, to });
        }
        self.edges.push(new_edge);
        Ok(())
    }

    /// Return the ids of all nodes whose dependencies are all in a terminal
    /// state (Succeeded, Failed, or Skipped) and that are themselves
    /// [`NodeState::Pending`]. A node with no predecessors is ready
    /// immediately. v0: a Failed or Skipped predecessor still counts as
    /// "satisfied" — the caller is responsible for cascade-skipping the
    /// dependent.
    pub fn ready_tasks(&self) -> Vec<TaskId> {
        self.nodes
            .values()
            .filter(|n| n.state == NodeState::Pending)
            .filter(|n| {
                let preds: Vec<&Edge> = self.edges.iter().filter(|e| e.to == n.id).collect();
                preds.iter().all(|e| {
                    matches!(
                        self.nodes.get(&e.from).map(|p| p.state),
                        Some(NodeState::Succeeded)
                            | Some(NodeState::Failed)
                            | Some(NodeState::Skipped)
                    )
                })
            })
            .map(|n| n.id)
            .collect()
    }

    /// Get an immutable reference to a node by id.
    pub fn get(&self, id: TaskId) -> Option<&TaskNode> {
        self.nodes.get(&id)
    }

    /// Get a mutable reference to a node by id.
    pub fn get_mut(&mut self, id: TaskId) -> Option<&mut TaskNode> {
        self.nodes.get_mut(&id)
    }

    /// Mark a node as [`NodeState::Running`]. No-op if the node does not
    /// exist.
    pub fn mark_running(&mut self, id: TaskId) {
        if let Some(node) = self.nodes.get_mut(&id) {
            node.state = NodeState::Running;
        }
    }

    /// Mark a node as completed with the given [`TaskResult`]. The node's
    /// state becomes [`NodeState::Succeeded`] if `result.error` is `None`,
    /// else [`NodeState::Failed`]. No-op if the node does not exist.
    pub fn mark_completed(&mut self, id: TaskId, result: TaskResult) {
        if let Some(node) = self.nodes.get_mut(&id) {
            node.state = if result.error.is_none() {
                NodeState::Succeeded
            } else {
                NodeState::Failed
            };
            // TODO(v1): stash `result` on the node so downstream consumers
            // can pull inputs by name from the graph.
            let _ = result;
        }
    }

    /// Topologically sort the graph using Kahn's algorithm. Returns an
    /// error if the graph contains a cycle (which should be impossible
    /// given [`TaskGraph::add_dependency`] rejects cycles, but is
    /// defensively checked here for callers that build the graph by other
    /// means).
    pub fn topological(&self) -> Result<Vec<TaskId>, GraphError> {
        let mut in_degree: HashMap<TaskId, usize> =
            self.nodes.keys().map(|id| (*id, 0)).collect();
        for edge in &self.edges {
            *in_degree.entry(edge.to).or_insert(0) += 1;
        }

        let mut queue: VecDeque<TaskId> = in_degree
            .iter()
            .filter(|(_, &d)| d == 0)
            .map(|(id, _)| *id)
            .collect();

        let mut order: Vec<TaskId> = Vec::with_capacity(self.nodes.len());
        while let Some(id) = queue.pop_front() {
            order.push(id);
            for edge in self.edges.iter().filter(|e| e.from == id) {
                if let Some(d) = in_degree.get_mut(&edge.to) {
                    *d -= 1;
                    if *d == 0 {
                        queue.push_back(edge.to);
                    }
                }
            }
        }

        if order.len() != self.nodes.len() {
            // TODO(v1): return the actual cycle path, not just the first
            // stale in-degree entry.
            return Err(GraphError::CycleDetected {
                from: order.last().copied().unwrap_or(TaskId(Uuid::nil())),
                to: in_degree
                    .iter()
                    .find(|(_, &d)| d > 0)
                    .map(|(id, _)| *id)
                    .unwrap_or(TaskId(Uuid::nil())),
            });
        }
        Ok(order)
    }

    /// True iff there is a directed path from `src` to `dst` along the
    /// existing edges. Used internally by the cycle check in
    /// [`TaskGraph::add_dependency`].
    fn reachable(&self, src: TaskId, dst: TaskId) -> bool {
        if src == dst {
            return true;
        }
        let mut visited: HashSet<TaskId> = HashSet::new();
        let mut stack: Vec<TaskId> = vec![src];
        while let Some(cur) = stack.pop() {
            if !visited.insert(cur) {
                continue;
            }
            for edge in self.edges.iter().filter(|e| e.from == cur) {
                if edge.to == dst {
                    return true;
                }
                stack.push(edge.to);
            }
        }
        false
    }
}

impl Default for TaskGraph {
    fn default() -> Self {
        Self::new()
    }
}


