//! Mesh router — routes messages peer-to-peer between COGNOS agents across the mesh. Each node maintains a routing table; messages are forwarded toward the destination by shortest path.
//!
//! The mesh router is the data-plane of the COGNOS agent mesh: it accepts
//! locally-originated [`MeshMessage`]s, looks up the next hop toward the
//! destination in [`MeshRouter::routing_table`], and hands the message off
//! to a directly-connected neighbor in [`MeshRouter::neighbors`]. At each
//! hop the TTL is decremented; messages reaching TTL 0 are dropped with
//! [`RouteError::MaxHopsExceeded`]. Loop detection uses the per-message
//! `trace` vector — if the local node already appears in the trace the
//! message is discarded with [`RouteError::LoopDetected`].
//!
//! v0 ships the data model and the routing/forwarding entrypoints as
//! stubs; the real transport (QUIC/WebSocket), shortest-path recomputation,
//! and link-state propagation land in v1.
//!
//! v0: stub implementation

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, warn};

// TODO(v1): promote `NodeId` into a shared `mesh::types` module so that
// mesh_router, agent_topology and state_sync all share one canonical alias.

/// Cluster-unique identifier for a mesh node (agent or relay).
pub type NodeId = String;

/// Default TTL stamped on locally-originated messages.
pub const DEFAULT_TTL: u32 = 32;

// ─── Errors ─────────────────────────────────────────────────────────────────

/// Errors returned by [`MeshRouter::route`] / [`MeshRouter::forward`].
#[derive(Debug, Error)]
pub enum RouteError {
    /// No entry for the destination in the local routing table.
    #[error("no route to destination")]
    NoRoute,
    /// TTL reached 0 before the message could be delivered.
    #[error("max hops exceeded (ttl exhausted)")]
    MaxHopsExceeded,
    /// The chosen next hop is not in the local neighbor set.
    #[error("unknown neighbor: {0}")]
    UnknownNeighbor(NodeId),
    /// This node already appears in the message trace — the message is
    /// looping and has been dropped.
    #[error("loop detected: node {0} already in trace")]
    LoopDetected(NodeId),
}

// ─── Messages ───────────────────────────────────────────────────────────────

/// A message routed peer-to-peer across the mesh.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeshMessage {
    /// Originator of the message.
    pub source: NodeId,
    /// Final destination node.
    pub destination: NodeId,
    /// Opaque application payload.
    pub payload: Vec<u8>,
    /// Time-to-live in hops; decremented at every forward.
    pub ttl: u32,
    /// Ordered list of nodes the message has already traversed; used for
    /// loop detection.
    pub trace: Vec<NodeId>,
}

// ─── Routing table ──────────────────────────────────────────────────────────

/// One row of the mesh routing table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteEntry {
    /// Next hop to use when forwarding messages destined for `dest`.
    pub next_hop: NodeId,
    /// Estimated distance in hops to the destination.
    pub hop_count: u32,
    /// Wall-clock time this entry was last refreshed.
    pub last_updated: DateTime<Utc>,
    /// Link-quality metric (lower is better); v0 leaves this at 0.0.
    pub metric: f64,
}

// ─── Router ─────────────────────────────────────────────────────────────────

/// Peer-to-peer mesh message router.
#[derive(Debug)]
pub struct MeshRouter {
    /// Identifier of the node hosting this router.
    pub node_id: NodeId,
    /// Directly-connected peers; the router only ever hands messages off
    /// to nodes in this set.
    pub neighbors: HashSet<NodeId>,
    /// Destination → [`RouteEntry`] lookup table.
    pub routing_table: HashMap<NodeId, RouteEntry>,
}

impl MeshRouter {
    /// Construct a new router bound to `node_id` with no neighbors and an
    /// empty routing table.
    pub fn new(node_id: NodeId) -> Self {
        Self {
            node_id,
            neighbors: HashSet::new(),
            routing_table: HashMap::new(),
        }
    }

    /// Route a locally-originated [`MeshMessage`] toward its destination.
    ///
    /// Looks up the next hop, verifies it is a known neighbor, appends
    /// `self.node_id` to the trace and forwards the message. The TTL is
    /// **not** decremented here — that happens in [`forward`](Self::forward)
    /// once the message is received by the next hop.
    // TODO(v1): real shortest-path recomputation, link-state propagation,
    // and an actual QUIC/WebSocket transport to the next hop.
    pub async fn route(&self, msg: MeshMessage) -> Result<(), RouteError> {
        if msg.ttl == 0 {
            warn!(destination = %msg.destination, "route: ttl exhausted at origin");
            return Err(RouteError::MaxHopsExceeded);
        }
        let entry = self
            .routing_table
            .get(&msg.destination)
            .ok_or(RouteError::NoRoute)?;
        if !self.neighbors.contains(&entry.next_hop) {
            warn!(next_hop = %entry.next_hop, "route: next hop not in neighbor set");
            return Err(RouteError::UnknownNeighbor(entry.next_hop.clone()));
        }
        debug!(
            destination = %msg.destination,
            next_hop = %entry.next_hop,
            hop_count = entry.hop_count,
            "route: forwarding message",
        );
        // v0: stub implementation — actual send to `entry.next_hop` lands
        // in v1 alongside the transport layer.
        Ok(())
    }

    /// Forward a [`MeshMessage`] received from a peer one hop closer to
    /// its destination.
    ///
    /// Decrements the TTL (dropping the message at 0), runs loop detection
    /// against the trace, then either hands the message off to the next
    /// hop or, if `self.node_id == msg.destination`, delivers it locally.
    // TODO(v1): emit a `MeshMessageDelivered` event on local delivery and
    // surface a per-link send error type instead of collapsing to
    // [`RouteError`].
    pub async fn forward(&self, msg: MeshMessage) -> Result<(), RouteError> {
        if msg.trace.contains(&self.node_id) {
            warn!(node = %self.node_id, "forward: loop detected, dropping");
            return Err(RouteError::LoopDetected(self.node_id.clone()));
        }
        if msg.ttl == 0 {
            warn!(destination = %msg.destination, "forward: ttl exhausted, dropping");
            return Err(RouteError::MaxHopsExceeded);
        }
        if msg.destination == self.node_id {
            debug!(source = %msg.source, "forward: delivered locally");
            return Ok(());
        }
        let entry = self
            .routing_table
            .get(&msg.destination)
            .ok_or(RouteError::NoRoute)?;
        if !self.neighbors.contains(&entry.next_hop) {
            return Err(RouteError::UnknownNeighbor(entry.next_hop.clone()));
        }
        debug!(
            destination = %msg.destination,
            next_hop = %entry.next_hop,
            ttl = msg.ttl.saturating_sub(1),
            "forward: forwarding one hop",
        );
        // v0: stub implementation — TTL decrement, trace append and the
        // actual send to the next hop happen in v1 once the transport is
        // wired up.
        Ok(())
    }

    /// Insert or replace a routing-table entry for `dest`.
    pub fn update_route(&mut self, dest: NodeId, entry: RouteEntry) {
        debug!(dest = %dest, next_hop = %entry.next_hop, "update_route: installing entry");
        self.routing_table.insert(dest, entry);
    }

    /// Remove the routing-table entry for `dest`, if any.
    pub fn remove_route(&mut self, dest: NodeId) {
        if self.routing_table.remove(&dest).is_some() {
            debug!(dest = %dest, "remove_route: entry removed");
        }
    }
}

impl Default for MeshRouter {
    fn default() -> Self {
        // TODO(v1): default to a randomly-minted NodeId once `mesh::types`
        // exports a constructor.
        Self::new(String::new())
    }
}

// TODO(v1): integrate with `agent_topology::AgentTopology` so that
// discovered neighbor sets feed `MeshRouter::neighbors` and link-state
// updates feed `update_route`.

// v0: stub implementation
