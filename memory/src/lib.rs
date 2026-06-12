//! COGNOS local memory — consent-scoped, inspectable, deletable.
//!
//! Anti-Recall rules (docs/SPEC.md): memory is opt-in, scoped, and
//! deletable. The indexer only touches paths inside explicitly allowed
//! roots, the store is a plain JSONL file the user can read, and wipe
//! operations are first-class.
//!
//! - [`embedder`] — [`embedder::Embedder`] trait + deterministic v0 fallback
//!   (model-backed embedder lands in Phase 3 behind the same trait)
//! - [`indexer`] — consumes the ANFS index queue, embeds, stores
//! - [`query`] — cosine top-k search with provenance metadata

pub mod embedder;
pub mod indexer;
pub mod query;

pub use embedder::{Embedder, HashEmbedder, EMBED_DIM};
pub use indexer::{IndexError, IndexRecord, Indexer};
pub use query::{cosine, search, SearchResult};
