//! Index management — maintains per-agent collection namespaces and rebuilds indexes on corruption.
//!
//! Each agent in COGNOS gets its own ChromaDB collection so that:
//!   1. recall is scoped (an agent never accidentally pulls another agent's
//!      memories via a stray filter),
//!   2. re-indexing one agent doesn't stall the rest, and
//!   3. integrity can be verified per-namespace.
//!
//! v0: stub implementation — collection lookup is an in-memory `HashMap` and
//! rebuilds are no-ops. v1 will hit the live ChromaDB server via
//! [`crate::client::ChromaClient`] and persist the schema to disk.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::Mutex;
use tracing::{info, instrument, warn};

use crate::client::ChromaClient;

// ─── Errors ──────────────────────────────────────────────────────────────────

/// Errors from index management operations.
#[derive(Debug, Error)]
pub enum IndexError {
    /// The agent has no collection yet and creation failed.
    #[error("no collection for agent {0}")]
    NoCollection(String),
    /// The underlying ChromaDB call failed.
    #[error("chromadb client error: {0}")]
    Client(String),
    /// An integrity check failed and the index needs a rebuild.
    #[error("integrity failure for agent {0}: {1}")]
    IntegrityFailure(String, String),
    /// The schema could not be loaded or persisted.
    #[error("schema io error: {0}")]
    SchemaIo(String),
}

// ─── Types ───────────────────────────────────────────────────────────────────

/// Stable identifier for an agent (UUID v4 string in v0).
pub type AgentId = String;

/// ChromaDB collection name, derived from the agent id.
pub type CollectionName = String;

/// On-disk schema mapping each agent to its ChromaDB collection.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IndexSchema {
    /// `agent_id -> collection_name`.
    pub collections: HashMap<AgentId, CollectionName>,
    /// Schema version — bump on incompatible changes.
    pub version: u32,
}

impl IndexSchema {
    /// Current schema version. v0 == 0.
    pub const CURRENT_VERSION: u32 = 0;
}

/// A single integrity report for one agent's collection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrityReport {
    /// The agent this report describes.
    pub agent: AgentId,
    /// The collection name that was checked.
    pub collection: CollectionName,
    /// `true` if the collection passes all checks.
    pub ok: bool,
    /// Human-readable diagnostics; empty when `ok`.
    pub findings: Vec<String>,
}

// ─── IndexManager ────────────────────────────────────────────────────────────

/// Owns the [`IndexSchema`] and the [`ChromaClient`] used to materialise it.
///
/// All mutators take `&self` and synchronise internally via a [`Mutex`], so
/// the manager is safe to share (v1 will wrap it in an `Arc`).
pub struct IndexManager {
    client: ChromaClient,
    /// Schema mutations are guarded by this mutex so `&self` methods can stay
    /// non-mutating. v1 will swap this for an `RwLock`.
    inner: Mutex<IndexManagerInner>,
}

/// Private mutable state shared between [`IndexManager`] methods.
struct IndexManagerInner {
    schema: IndexSchema,
}

impl IndexManager {
    /// Construct a new manager with an empty schema.
    pub fn new(client: ChromaClient) -> Self {
        Self {
            client,
            inner: Mutex::new(IndexManagerInner {
                schema: IndexSchema {
                    collections: HashMap::new(),
                    version: IndexSchema::CURRENT_VERSION,
                },
            }),
        }
    }

    /// Load a schema from JSON bytes (called by the persistence layer).
    pub fn with_schema(client: ChromaClient, schema: IndexSchema) -> Self {
        Self {
            client,
            inner: Mutex::new(IndexManagerInner { schema }),
        }
    }

    /// Return a snapshot of the current schema.
    pub async fn schema(&self) -> IndexSchema {
        self.inner.lock().await.schema.clone()
    }

    /// Borrow the underlying client (used by retrieval / context_store).
    pub fn client(&self) -> &ChromaClient {
        &self.client
    }

    /// Ensure `agent` has a collection. Creates one if missing.
    ///
    /// The collection name is derived from the agent id with a `cognos:agent:`
    /// prefix so multiple COGNOS tenants on the same ChromaDB server don't
    /// collide.
    #[instrument(skip(self), fields(agent = %agent))]
    pub async fn ensure_collection(
        &self,
        agent: impl Into<AgentId>,
    ) -> Result<CollectionName, IndexError> {
        let agent = agent.into();
        let mut inner = self.inner.lock().await;
        if let Some(name) = inner.schema.collections.get(&agent) {
            return Ok(name.clone());
        }

        let name = format!("cognos:agent:{agent}");
        info!(%agent, %name, "ensure_collection: creating (stub)");

        // TODO(v1): call self.client.create_collection(&name).await and
        //           surface ClientError as IndexError::Client. In v0 we
        //           optimistically record the mapping without server contact.
        inner.schema.collections.insert(agent, name.clone());
        Ok(name)
    }

    /// Rebuild an agent's collection from scratch.
    ///
    /// v0 just clears the schema entry; v1 will drop the ChromaDB collection,
    /// re-create it, and re-embed every persisted memory from the source log.
    #[instrument(skip(self), fields(agent = %agent))]
    pub async fn rebuild(&self, agent: impl Into<AgentId>) -> Result<(), IndexError> {
        let agent = agent.into();
        let mut inner = self.inner.lock().await;
        let removed = inner.schema.collections.remove(&agent);
        if removed.is_none() {
            warn!(%agent, "rebuild: no prior collection");
            return Err(IndexError::NoCollection(agent));
        }
        info!(%agent, "rebuild: stub completed");
        // TODO(v1): drop + recreate ChromaDB collection, re-embed from log.
        Ok(())
    }

    /// Run an integrity check against every collection in the schema.
    ///
    /// v0 returns an empty `Vec` (everything is "OK" by absence of checks).
    #[instrument(skip(self))]
    pub async fn integrity_check(&self) -> Result<Vec<IntegrityReport>, IndexError> {
        let inner = self.inner.lock().await;
        let mut reports = Vec::with_capacity(inner.schema.collections.len());
        for (agent, collection) in &inner.schema.collections {
            // TODO(v1): hit ChromaDB /collections/{id}/count, compare against
            //           the source log, verify embedding dims, spot-check
            //           cosine distances against a known-good query.
            reports.push(IntegrityReport {
                agent: agent.clone(),
                collection: collection.clone(),
                ok: true,
                findings: Vec::new(),
            });
        }
        Ok(reports)
    }
}

// TODO(v1): wrap IndexManager in `Arc<IndexManager>` and expose a `Clone`
//           implementation; swap `Mutex` for `RwLock` to allow concurrent
//           reads of the schema during recall.

// Keep `Arc` import alive for the v1 migration note above.
#[allow(dead_code)]
type _ArcPlaceholder<T> = Arc<T>;
