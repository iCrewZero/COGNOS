//! ChromaDB client — talks to a local ChromaDB server over HTTP to store and query semantic memories (embeddings + metadata).
//!
//! COGNOS treats ChromaDB as a pluggable vector backend that runs in its own
//! process (see `services/cognos-memory.service`). This module is the thin
//! REST client the rest of the memory layer uses; it owns no business logic
//! beyond translating Rust types into the Chroma v2 HTTP API.
//!
//! v0: stub implementation — every call returns a descriptive `Err` or an
//! empty result. Wiring of `reqwest` against a running server is scheduled
//! for v1, once the embedding pipeline stabilises.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, instrument};

// ─── Errors ──────────────────────────────────────────────────────────────────

/// Errors surfaced by [`ChromaClient`] when talking to the ChromaDB server.
#[derive(Debug, Error)]
pub enum ClientError {
    /// The HTTP call failed at the transport level (DNS, TCP, TLS, ...).
    #[error("transport error talking to chromadb: {0}")]
    Transport(String),
    /// The server returned a non-2xx status code.
    #[error("chromadb returned status {status}: {body}")]
    Status { status: u16, body: String },
    /// The response body could not be deserialised.
    #[error("decode error: {0}")]
    Decode(String),
    /// The caller supplied inconsistent vector lengths or empty batches.
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    /// The configured base URL was malformed.
    #[error("invalid base url: {0}")]
    InvalidBaseUrl(String),
}

// ─── Data types ──────────────────────────────────────────────────────────────

/// A ChromaDB collection — a namespaced bucket of embeddings + metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Collection {
    /// Server-assigned collection identifier.
    pub id: String,
    /// Human-readable collection name (unique per tenant).
    pub name: String,
}

/// Per-record metadata stored alongside each embedding.
///
/// ChromaDB requires metadata to be a flat `String -> value` map. We keep the
/// typed surface in Rust and serialise to/from a `serde_json::Map` on the wire.
pub type Metadata = serde_json::Map<String, serde_json::Value>;

/// Result of a similarity query against a collection.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QueryResult {
    /// Outer dimension = 1 (single query). Inner list = top-k ids.
    pub ids: Vec<Vec<String>>,
    /// Top-k embeddings, parallel to `ids`.
    pub embeddings: Vec<Vec<Vec<f32>>>,
    /// Top-k metadata, parallel to `ids`.
    pub metadatas: Vec<Vec<Option<Metadata>>>,
    /// Top-k distances, parallel to `ids` (lower = more similar in Chroma).
    pub distances: Vec<Vec<f32>>,
}

// ─── ChromaClient ────────────────────────────────────────────────────────────

/// REST client for a local ChromaDB server.
///
/// All calls are async and respect the configured `timeout`. The client is
/// cheap to clone internally (it shares a connection pool via `reqwest`).
pub struct ChromaClient {
    /// Base URL, e.g. `http://127.0.0.1:8000/api/v2`.
    base_url: String,
    /// Shared HTTP client with connection pooling.
    http: reqwest::Client,
    /// Per-request timeout.
    timeout: Duration,
}

impl ChromaClient {
    /// Construct a new client and verify the base URL is well-formed.
    ///
    /// v0 does *not* ping the server — that happens lazily on the first call.
    pub async fn new(base_url: impl Into<String>) -> Result<Self, ClientError> {
        let base_url = base_url.into();
        // Validate the URL parses. We don't canonicalise; v1 will normalise
        // trailing slashes and inject `/api/v2` if missing.
        let _ = reqwest::Url::parse(&base_url)
            .map_err(|e| ClientError::InvalidBaseUrl(format!("{e}")))?;

        let http = reqwest::Client::builder()
            .pool_idle_timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| ClientError::Transport(e.to_string()))?;

        Ok(Self {
            base_url,
            http,
            timeout: Duration::from_secs(10),
        })
    }

    /// Override the default 10s timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    // ─── Collection operations ───────────────────────────────────────────────

    /// List every collection visible to this client.
    #[instrument(skip(self), fields(base = %self.base_url))]
    pub async fn list_collections(&self) -> Result<Vec<Collection>, ClientError> {
        debug!(url = %self.base_url, "list_collections: stub");
        // TODO(v1): GET /api/v2/tenants/{tenant}/databases/{db}/collections
        Ok(Vec::new())
    }

    /// Create a new collection. Returns the server-assigned record.
    #[instrument(skip(self), fields(name = %name))]
    pub async fn create_collection(
        &self,
        name: impl Into<String>,
    ) -> Result<Collection, ClientError> {
        let name = name.into();
        debug!(%name, "create_collection: stub");
        // TODO(v1): POST /api/v2/tenants/{tenant}/databases/{db}/collections
        //           with { "name": name, "get_or_create": true }
        Err(ClientError::InvalidRequest(format!(
            "create_collection({name}) not implemented in v0 stub"
        )))
    }

    // ─── Record operations ───────────────────────────────────────────────────

    /// Insert or upsert embeddings into `collection`.
    ///
    /// All four slices must be the same length; each index describes one record.
    /// `embeddings[i]` may be empty only if the caller expects ChromaDB to embed
    /// server-side (v1 feature).
    #[instrument(skip(self, embeddings, metadatas, ids))]
    pub async fn add(
        &self,
        collection: &Collection,
        embeddings: &[Vec<f32>],
        metadatas: &[Option<Metadata>],
        ids: &[String],
    ) -> Result<(), ClientError> {
        if embeddings.len() != ids.len() || metadatas.len() != ids.len() {
            return Err(ClientError::InvalidRequest(format!(
                "length mismatch: embeddings={} metadatas={} ids={}",
                embeddings.len(),
                metadatas.len(),
                ids.len()
            )));
        }
        debug!(collection = %collection.name, count = ids.len(), "add: stub");
        // TODO(v1): POST /api/v2/tenants/{t}/databases/{d}/collections/{id}/add
        Ok(())
    }

    /// Run a similarity query against `collection`.
    ///
    /// `where_filter` is an opaque ChromaDB `where` clause (JSON object).
    /// Pass `None` for no filtering.
    #[instrument(skip(self, query_embedding, where_filter))]
    pub async fn query(
        &self,
        collection: &Collection,
        query_embedding: &[f32],
        n_results: usize,
        where_filter: Option<Metadata>,
    ) -> Result<QueryResult, ClientError> {
        if query_embedding.is_empty() {
            return Err(ClientError::InvalidRequest(
                "query_embedding must be non-empty".to_string(),
            ));
        }
        debug!(
            collection = %collection.name,
            dim = query_embedding.len(),
            n_results,
            has_filter = where_filter.is_some(),
            "query: stub"
        );
        // TODO(v1): POST /api/v2/.../collections/{id}/query
        //           body: { query_embeddings, n_results, where, include: [...] }
        Ok(QueryResult::default())
    }
}

// ─── Defaults / tests-friendly constructors ─────────────────────────────────

impl Default for ChromaClient {
    fn default() -> Self {
        // A best-effort stub URL. Real callers should use [`ChromaClient::new`].
        Self {
            base_url: "http://127.0.0.1:8000/api/v2".to_string(),
            http: reqwest::Client::new(),
            timeout: Duration::from_secs(10),
        }
    }
}
