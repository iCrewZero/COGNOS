//! Context store — turns raw memories into retrievable cognitive context for agents, with TTL, importance scoring, and provenance.
//!
//! The store sits above [`crate::client::ChromaClient`] and exposes a small
//! verb set — [`remember`](ContextStore::remember),
//! [`recall`](ContextStore::recall),
//! [`forget`](ContextStore::forget),
//! [`consolidate`](ContextStore::consolidate) — that mirrors the way agents
//! talk about memory in plain English.
//!
//! v0: stub implementation — `remember`/`recall`/`forget` round-trip data
//! through the ChromaDB client stubs and `consolidate` is a no-op. Importance
//! decay and TTL eviction land in v1 along with the consolidation worker.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::Mutex;
use tracing::{debug, info, instrument};
use uuid::Uuid;

use crate::client::{ChromaClient, Collection, Metadata};

// ─── Errors ──────────────────────────────────────────────────────────────────

/// Errors returned by [`ContextStore`] operations.
#[derive(Debug, Error)]
pub enum StoreError {
    /// The embedder pipeline was unavailable.
    #[error("embedder unavailable: {0}")]
    Embedder(String),
    /// The underlying ChromaDB call failed.
    #[error("chromadb client error: {0}")]
    Client(String),
    /// The caller referenced a memory id that does not exist.
    #[error("memory not found: {0}")]
    NotFound(String),
    /// The TTL had already expired by the time the write was attempted.
    #[error("ttl expired before write")]
    TtlExpired,
    /// Importance score was outside [0.0, 1.0].
    #[error("invalid importance {0}: must be in [0.0, 1.0]")]
    InvalidImportance(f32),
}

// ─── Handles & types ─────────────────────────────────────────────────────────

/// Opaque handle to the embedder process. v0 is `()`; v1 holds an
/// `mpsc::Sender<EmbedRequest>` plus the embedder version string.
#[derive(Debug, Clone, Default)]
pub struct EmbedderHandle {
    /// Embedder version / name, used for provenance.
    pub name: String,
}

/// Stable identifier for a stored memory (UUID v4 string).
pub type MemoryId = String;

/// Stable identifier for an agent.
pub type AgentId = String;

/// Free-form tag attached to a memory for later filtering.
pub type Tag = String;

/// Time-to-live for a memory. `None` means "remember forever".
pub type Ttl = Option<Duration>;

/// A stored memory record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memory {
    /// Server-assigned id (UUID v4).
    pub id: MemoryId,
    /// Raw textual content stored alongside the embedding.
    pub content: String,
    /// The embedding vector produced by [`EmbedderHandle`].
    pub embedding: Vec<f32>,
    /// Importance score in [0.0, 1.0]. Used by [`crate::retrieval::Retriever`].
    pub importance: f32,
    /// When the memory was first written.
    pub created: DateTime<Utc>,
    /// When the memory was last read back (updated on recall).
    pub last_accessed: DateTime<Utc>,
    /// Which agent / pipeline produced this memory.
    pub provenance: String,
    /// Free-form tags. Mirrored into ChromaDB metadata for filtering.
    pub tags: Vec<Tag>,
}

/// Input to [`ContextStore::remember`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryContext {
    /// Raw content to embed and store.
    pub content: String,
    /// Agent that produced or owns this memory.
    pub source_agent: AgentId,
    /// Importance score in [0.0, 1.0].
    pub importance: f32,
    /// Free-form tags.
    pub tags: Vec<Tag>,
    /// Optional TTL; `None` means remember forever.
    pub ttl: Ttl,
}

impl MemoryContext {
    /// Validate the importance score before sending to the store.
    pub fn validate(&self) -> Result<(), StoreError> {
        if !(0.0..=1.0).contains(&self.importance) {
            return Err(StoreError::InvalidImportance(self.importance));
        }
        Ok(())
    }
}

/// Optional filter passed to [`ContextStore::recall`]. `None` returns all
/// matches within `n`; otherwise all non-`None` fields must match.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RecallFilter {
    /// Restrict to memories from this agent.
    pub source_agent: Option<AgentId>,
    /// Restrict to memories carrying all of these tags.
    pub tags: Vec<Tag>,
    /// Restrict to memories with importance >= this floor.
    pub min_importance: Option<f32>,
}

// ─── ContextStore ────────────────────────────────────────────────────────────

/// High-level cognitive context storage.
///
/// Internally owns a [`ChromaClient`] (or shares one) and an
/// [`EmbedderHandle`] used to vectorise incoming [`MemoryContext`] payloads.
pub struct ContextStore {
    client: ChromaClient,
    embedder: EmbedderHandle,
    /// v0 in-memory index of memory id -> metadata, used by `forget` and
    /// `consolidate` without hitting ChromaDB. v1 will read this from the
    /// server.
    index: Arc<Mutex<HashMap<MemoryId, Memory>>>,
}

impl ContextStore {
    /// Construct a new store.
    pub fn new(client: ChromaClient, embedder: EmbedderHandle) -> Self {
        Self {
            client,
            embedder,
            index: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Borrow the underlying ChromaDB client.
    pub fn client(&self) -> &ChromaClient {
        &self.client
    }

    // ─── Verbs ───────────────────────────────────────────────────────────────

    /// Persist a memory. Returns its new [`MemoryId`].
    #[instrument(skip(self, ctx), fields(agent = %ctx.source_agent))]
    pub async fn remember(&self, ctx: MemoryContext) -> Result<MemoryId, StoreError> {
        ctx.validate()?;
        let now = Utc::now();
        let id = Uuid::new_v4().to_string();
        let embedding = self.embed(&ctx.content).await?;

        let memory = Memory {
            id: id.clone(),
            content: ctx.content,
            embedding,
            importance: ctx.importance,
            created: now,
            last_accessed: now,
            provenance: ctx.source_agent,
            tags: ctx.tags,
        };

        let collection = Collection {
            id: format!("stub-{}", memory.provenance),
            name: format!("cognos:agent:{}", memory.provenance),
        };

        // TODO(v1): convert tags -> Metadata and call self.client.add(...).
        let _ = &collection;
        debug!(%id, "remember: stub insert");
        self.index.lock().await.insert(id.clone(), memory);
        Ok(id)
    }

    /// Retrieve up to `n` memories matching `query`, subject to `filter`.
    #[instrument(skip(self, query, filter))]
    pub async fn recall(
        &self,
        query: &str,
        n: usize,
        filter: RecallFilter,
    ) -> Result<Vec<Memory>, StoreError> {
        let qv = self.embed(query).await?;
        let collection = Collection {
            id: "stub-collection".to_string(),
            name: "cognos:agent:global".to_string(),
        };

        // TODO(v1): build a Metadata `where` clause from `filter` and call
        //           self.client.query(...). Map ClientError -> StoreError.
        let _ = (&collection, &filter);
        debug!(dim = qv.len(), n, "recall: stub");

        let mut hits: Vec<Memory> = self
            .index
            .lock()
            .await
            .values()
            .cloned()
            .collect();
        // Deterministic ordering so audits are reproducible: id ascending.
        hits.sort_by(|a, b| a.id.cmp(&b.id));
        hits.truncate(n);
        Ok(hits)
    }

    /// Delete a memory by id.
    #[instrument(skip(self))]
    pub async fn forget(&self, id: impl Into<MemoryId>) -> Result<(), StoreError> {
        let id = id.into();
        let removed = self.index.lock().await.remove(&id);
        if removed.is_none() {
            return Err(StoreError::NotFound(id));
        }
        // TODO(v1): call self.client.delete(&collection, &[id.clone()]).
        debug!(%id, "forget: stub delete");
        Ok(())
    }

    /// Consolidate the store: merge near-duplicates and decay old, low-importance
    /// memories past their TTL.
    ///
    /// v0 is a no-op; v1 will run this as a background task on a cron.
    pub async fn consolidate(&self) {
        info!("consolidate: stub no-op");
        // TODO(v1): for each pair of memories with cosine >= 0.95, merge into
        //           a single record (keep the higher-importance one, union the
        //           tags). Evict memories whose TTL has expired or whose
        //           decayed importance has dropped below a floor.
    }

    // ─── Internals ───────────────────────────────────────────────────────────

    /// Call the embedder. v0 returns an empty vector — v1 will dispatch over
    /// the embedder process via the [`EmbedderHandle`].
    async fn embed(&self, _text: &str) -> Result<Vec<f32>, StoreError> {
        // TODO(v1): send EmbedRequest over the embedder channel, await reply.
        Ok(Vec::new())
    }

    /// Helper used by tests / future persistence layer.
    #[allow(dead_code)]
    fn metadata_for(memory: &Memory) -> Metadata {
        let mut m = Metadata::new();
        m.insert("importance".to_string(), memory.importance.into());
        m.insert("provenance".to_string(), memory.provenance.clone().into());
        m.insert(
            "created".to_string(),
            memory.created.to_rfc3339().into(),
        );
        let tags: Vec<serde_json::Value> = memory
            .tags
            .iter()
            .map(|t| serde_json::Value::String(t.clone()))
            .collect();
        m.insert("tags".to_string(), serde_json::Value::Array(tags));
        m
    }
}
