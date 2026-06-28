//! Agent topology — discovers the mesh topology via gossip, maintains a connectivity graph, and detects partitions.
//!
//! [`AgentTopology`] runs a gossip protocol in which each node periodically
//! shares its view of the connectivity graph with a random peer; the union
//! of all such views converges to the true mesh topology. The local node
//! also records the last time it heard from each peer in `last_seen` so
//! that stale edges can be pruned. Partition detection runs a
//! connected-components analysis over the graph and reports a [`Partition`]
//! whenever more than one component is observed.
//!
//! v0 ships the data model, the gossip/discovery entrypoints and a real
//! connected-components implementation; the network transport and the
//! failure detector that backs `last_seen` land in v1.
//!
//! v0: stub implementation

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::time::{Duration, Instant, interval};
use tracing::{debug, info, warn};

// TODO(v1): share `NodeId` with `mesh_router` and `state_sync` via a
// `mesh::types` module.

/// Cluster-unique identifier for a mesh node.
pub type NodeId = String;

/// Gossip interval — each node shares its topology view once per second.
pub const GOSSIP_INTERVAL: Duration = Duration::from_secs(1);

/// Default peer-liveness timeout; nodes silent for longer than this are
/// pruned from the graph before partition analysis runs.
pub const PEER_LIVENESS_TIMEOUT: Duration = Duration::from_secs(10);

// ─── Errors ─────────────────────────────────────────────────────────────────

/// Errors returned by the topology subsystem.
#[derive(Debug, Error)]
pub enum TopologyError {
    /// A gossip round failed (transport error, no responses, etc.).
    #[error("gossip round failed: {0}")]
    GossipFailed(String),
    /// No peers are known, so a gossip round cannot proceed.
    #[error("no peers available for gossip")]
    NoPeers,
    /// The mesh has partitioned; see [`Partition`] for the components.
    #[error("mesh partitioned into {0} components")]
    Partitioned(usize),
}

// ─── Graph ──────────────────────────────────────────────────────────────────

/// Connectivity graph of the mesh.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TopologyGraph {
    /// Set of known node ids.
    pub nodes: HashSet<NodeId>,
    /// Undirected edges, stored as `(min, max)` ordered pairs so that
    /// `(a, b)` and `(b, a)` collapse to the same entry.
    pub edges: HashSet<(NodeId, NodeId)>,
}

impl TopologyGraph {
    /// Construct an empty graph.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert an undirected edge between `a` and `b`, also ensuring both
    /// endpoints are in `nodes`.
    fn add_edge(&mut self, a: NodeId, b: NodeId) {
        let edge = if a <= b { (a, b) } else { (b, a) };
        self.nodes.insert(edge.0.clone());
        self.nodes.insert(edge.1.clone());
        self.edges.insert(edge);
    }
}

// ─── Partition ──────────────────────────────────────────────────────────────

/// A mesh partition: the graph split into two or more connected components.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Partition {
    /// The disconnected components, largest first (v0 ordering).
    pub components: Vec<HashSet<NodeId>>,
    /// Wall-clock time at which the partition was first detected.
    pub detected_at: Instant,
}

// ─── Partition detector ─────────────────────────────────────────────────────

/// Tunable state for partition detection.
#[derive(Debug, Clone)]
pub struct PartitionDetector {
    /// Liveness threshold; edges whose endpoints haven't been heard from
    /// within this window are pruned before running connected-components.
    pub liveness_timeout: Duration,
    /// Cached result of the last detection pass.
    pub last_result: Option<Partition>,
}

impl Default for PartitionDetector {
    fn default() -> Self {
        Self {
            liveness_timeout: PEER_LIVENESS_TIMEOUT,
            last_result: None,
        }
    }
}

// ─── AgentTopology ──────────────────────────────────────────────────────────

/// Mesh topology manager.
#[derive(Debug)]
pub struct AgentTopology {
    /// Identifier of the node hosting this topology manager.
    pub local_id: NodeId,
    /// Connectivity graph maintained via gossip.
    pub graph: TopologyGraph,
    /// Last time each peer was heard from.
    pub last_seen: HashMap<NodeId, Instant>,
    /// Tunable partition detector.
    pub partition_detector: PartitionDetector,
}

impl AgentTopology {
    /// Construct a new topology manager for `local_id`.
    pub fn new(local_id: NodeId) -> Self {
        let mut graph = TopologyGraph::new();
        graph.nodes.insert(local_id.clone());
        Self {
            local_id,
            graph,
            last_seen: HashMap::new(),
            partition_detector: PartitionDetector::default(),
        }
    }

    /// Drive one round of topology discovery: gossip the local view to a
    /// random peer, merge the response, and recompute partitions.
    // TODO(v1): pick a random live peer, serialize the local graph, send
    // it over the mesh transport, and merge the remote graph on reply.
    pub async fn discover(&mut self) -> Result<TopologyGraph, TopologyError> {
        self.gossip().await?;
        if let Some(partition) = self.detect_partition() {
            warn!(components = partition.components.len(), "discover: mesh partitioned");
            self.partition_detector.last_result = Some(partition);
        }
        Ok(self.graph.clone())
    }

    /// Exchange the local graph with a random peer. v0 is a no-op that
    /// merely records the gossip round.
    // TODO(v1): wire up the actual peer exchange over the mesh transport.
    pub async fn gossip(&self) -> Result<(), TopologyError> {
        if self.graph.nodes.len() <= 1 {
            debug!("gossip: no peers known, skipping round");
            return Err(TopologyError::NoPeers);
        }
        info!(peers = self.graph.nodes.len(), "gossip: round complete (stub)");
        // v0: stub implementation — real peer exchange lands in v1.
        Ok(())
    }

    /// Background gossip loop ticking at [`GOSSIP_INTERVAL`]. v0 spins
    /// forever calling [`gossip`](Self::gossip); v1 will be spawned on a
    /// supplied runtime handle and honour a shutdown signal.
    // TODO(v1): take a `tokio::runtime::Handle` + shutdown `CancellationToken`
    // and `select!` between them and the ticker.
    pub async fn gossip_loop(&self) {
        let mut ticker = interval(GOSSIP_INTERVAL);
        loop {
            ticker.tick().await;
            if let Err(err) = self.gossip().await {
                debug!(?err, "gossip_loop: round failed");
            }
        }
    }

    /// Record `node` as a directly-connected neighbor.
    pub fn add_neighbor(&mut self, node: NodeId) {
        if node == self.local_id {
            warn!(node = %node, "add_neighbor: refusing to add self as neighbor");
            return;
        }
        self.graph.add_edge(self.local_id.clone(), node.clone());
        self.last_seen.insert(node, Instant::now());
        debug!(peers = self.graph.nodes.len(), "add_neighbor: edge recorded");
    }

    /// Remove `node` from the neighbor set and prune edges to it.
    pub fn remove_neighbor(&mut self, node: NodeId) {
        self.last_seen.remove(&node);
        self.graph.edges.retain(|(a, b)| *a != node && *b != node);
        self.graph.nodes.remove(&node);
        debug!(node = %node, "remove_neighbor: pruned");
    }

    /// Run connected-components over the graph; return a [`Partition`] if
    /// more than one component is found, else `None`.
    // TODO(v1): incorporate `last_seen` liveness pruning before running
    // the analysis so silent nodes don't skew the result.
    pub fn detect_partition(&self) -> Option<Partition> {
        let components = self.connected_components();
        if components.len() > 1 {
            Some(Partition {
                components,
                detected_at: Instant::now(),
            })
        } else {
            None
        }
    }

    /// Compute connected components via DFS over the adjacency list
    /// derived from `self.graph`.
    fn connected_components(&self) -> Vec<HashSet<NodeId>> {
        let mut adj: HashMap<&NodeId, Vec<&NodeId>> = HashMap::new();
        for node in &self.graph.nodes {
            adj.entry(node).or_default();
        }
        for (a, b) in &self.graph.edges {
            adj.entry(a).or_default().push(b);
            adj.entry(b).or_default().push(a);
        }
        let mut visited: HashSet<&NodeId> = HashSet::new();
        let mut components = Vec::new();
        for start in self.graph.nodes.iter() {
            if visited.contains(start) {
                continue;
            }
            let mut component: HashSet<NodeId> = HashSet::new();
            let mut stack = vec![start];
            while let Some(node) = stack.pop() {
                if !visited.insert(node) {
                    continue;
                }
                component.insert(node.clone());
                if let Some(neighbors) = adj.get(node) {
                    for n in neighbors {
                        if !visited.contains(*n) {
                            stack.push(*n);
                        }
                    }
                }
            }
            components.push(component);
        }
        // Largest first for deterministic reporting.
        components.sort_by(|a, b| b.len().cmp(&a.len()));
        components
    }
}

impl Default for AgentTopology {
    fn default() -> Self {
        // TODO(v1): default to a randomly-minted NodeId.
        Self::new(String::new())
    }
}

// TODO(v1): feed discovered neighbor sets into `mesh_router::MeshRouter`
// so the routing table stays consistent with the live topology.

// v0: stub implementation
