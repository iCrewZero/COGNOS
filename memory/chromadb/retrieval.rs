//! Retrieval — re-ranks ChromaDB results using recency, importance, relationship-graph proximity, and current-session context.
//!
//! Plain vector similarity is a poor ranker for episodic memory: a stale,
//! high-importance fact should outrank a fresh low-importance one even if its
//! cosine score is slightly lower. This module fuses four signals into a
//! single [`ScoreBreakdown`]:
//!
//!  1. **semantic** — cosine similarity from ChromaDB,
//!  2. **recency** — exponential decay since `last_accessed`,
//!  3. **importance** — the stored importance score (clamped), and
//!  4. **graph_proximity** — hop count in the [`RelationshipGraph`].
//!
//! `session_relevance` is a per-call boost applied when a memory mentions an
//! entity already in the active session context.
//!
//! v0: stub implementation — `retrieve` returns the unmodified top-k from the
//! store and `rerank` leaves scores at `Default::default()`. Weight tuning
//! and the actual fusion land in v1.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, instrument};

use crate::context_store::{ContextStore, Memory, RecallFilter};

// ─── Errors ──────────────────────────────────────────────────────────────────

/// Errors returned by [`Retriever`] operations.
#[derive(Debug, Error)]
pub enum RetrievalError {
    /// The underlying store returned an error.
    #[error("store error: {0}")]
    Store(String),
    /// The relationship graph was unavailable or corrupted.
    #[error("graph error: {0}")]
    Graph(String),
}

// ─── RelationshipGraph ───────────────────────────────────────────────────────

/// Stable identifier for an entity node in the relationship graph.
pub type EntityId = String;

/// A simple adjacency-list graph linking memories to entities and entities
/// to each other. v0 is empty; v1 wires this to `memory/anfs`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RelationshipGraph {
    /// `entity -> { neighbours }`.
    edges: HashMap<EntityId, Vec<EntityId>>,
    /// `memory_id -> { entities it mentions }`.
    mentions: HashMap<String, Vec<EntityId>>,
}

impl RelationshipGraph {
    /// Construct an empty graph.
    pub fn new() -> Self {
        Self::default()
    }

    /// Approximate hop distance between any two entities. Returns `0.0` if
    /// either endpoint is unknown; otherwise `1.0 / (1 + hops)`.
    pub fn proximity(&self, _a: &EntityId, _b: &EntityId) -> f32 {
        // TODO(v1): BFS with a hop cap (e.g. 3). For now treat everything as
        //           maximally distant.
        0.0
    }
}

// ─── Scoring ─────────────────────────────────────────────────────────────────

/// Per-signal breakdown of a memory's final retrieval score.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct ScoreBreakdown {
    /// Cosine similarity in [-1.0, 1.0]; usually [0.0, 1.0] for embeddings.
    pub semantic: f32,
    /// Exponential decay since `last_accessed`, in [0.0, 1.0].
    pub recency: f32,
    /// Stored importance score, in [0.0, 1.0].
    pub importance: f32,
    /// `1.0 / (1 + hops)` in the [`RelationshipGraph`], in [0.0, 1.0].
    pub graph_proximity: f32,
    /// Per-call boost for memories mentioning session entities, in [0.0, 1.0].
    pub session_relevance: f32,
}

impl ScoreBreakdown {
    /// Weighted sum of all five signals. Weights are configurable in v1.
    pub fn total(&self) -> f32 {
        // TODO(v1): make these weights configurable per agent / per task.
        const W_SEM: f32 = 0.40;
        const W_REC: f32 = 0.20;
        const W_IMP: f32 = 0.20;
        const W_GRH: f32 = 0.10;
        const W_SES: f32 = 0.10;
        W_SEM * self.semantic
            + W_REC * self.recency
            + W_IMP * self.importance
            + W_GRH * self.graph_proximity
            + W_SES * self.session_relevance
    }
}

/// A [`Memory`] paired with its computed score and breakdown.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoredMemory {
    /// The underlying memory record.
    pub memory: Memory,
    /// Final fused score (sum of weighted breakdown).
    pub score: f32,
    /// Per-signal components, for transparency and debugging.
    pub score_breakdown: ScoreBreakdown,
}

// ─── SessionContext ──────────────────────────────────────────────────────────

/// Lightweight view of what the agent is currently doing, used to boost
/// session-relevant memories.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionContext {
    /// Entities currently in scope (e.g. file paths, agent names, topics).
    pub active_entities: Vec<EntityId>,
    /// Free-text summary of the last user turn, for cheap lexical overlap.
    pub recent_turn_summary: String,
}

// ─── Retriever ───────────────────────────────────────────────────────────────

/// Top-level retrieval entrypoint used by agents.
pub struct Retriever {
    store: ContextStore,
    graph: RelationshipGraph,
}

impl Retriever {
    /// Construct a new retriever.
    pub fn new(store: ContextStore, graph: RelationshipGraph) -> Self {
        Self { store, graph }
    }

    /// Borrow the underlying store.
    pub fn store(&self) -> &ContextStore {
        &self.store
    }

    /// Borrow the relationship graph.
    pub fn graph(&self) -> &RelationshipGraph {
        &self.graph
    }

    /// Retrieve up to `n` memories for `query`, scored and ranked.
    ///
    /// v0: calls [`ContextStore::recall`] with an empty filter and re-ranks
    /// with [`Retriever::rerank`] using the default [`SessionContext`].
    #[instrument(skip(self, query))]
    pub async fn retrieve(
        &self,
        query: impl AsRef<str>,
        n: usize,
    ) -> Result<Vec<ScoredMemory>, RetrievalError> {
        let query = query.as_ref();
        let memories = self
            .store
            .recall(query, n, RecallFilter::default())
            .await
            .map_err(|e| RetrievalError::Store(e.to_string()))?;
        let context = SessionContext::default();
        Ok(self.rerank(memories, context))
    }

    /// Re-rank a batch of memories against the given session context.
    ///
    /// Sort order: `score` descending, then `memory.id` ascending for
    /// deterministic audits.
    pub fn rerank(&self, results: Vec<Memory>, context: SessionContext) -> Vec<ScoredMemory> {
        let mut scored: Vec<ScoredMemory> = results
            .into_iter()
            .map(|memory| {
                let breakdown = self.score(&memory, &context);
                ScoredMemory {
                    score: breakdown.total(),
                    score_breakdown: breakdown,
                    memory,
                }
            })
            .collect();
        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.memory.id.cmp(&b.memory.id))
        });
        debug!(count = scored.len(), "rerank: complete");
        scored
    }

    // ─── Internals ───────────────────────────────────────────────────────────

    /// Compute the per-signal score breakdown for `memory` against `context`.
    fn score(&self, memory: &Memory, _context: &SessionContext) -> ScoreBreakdown {
        // TODO(v1): real fusion —
        //   semantic           : cosine(query, memory.embedding)
        //   recency            : exp(-Δt / τ), τ ~ 24h
        //   importance         : memory.importance
        //   graph_proximity    : min over (session_entity, mentioned_entity)
        //   session_relevance  : lexical overlap with context.recent_turn_summary
        ScoreBreakdown {
            semantic: 0.0,
            recency: 0.0,
            importance: memory.importance,
            graph_proximity: 0.0,
            session_relevance: 0.0,
        }
    }
}
