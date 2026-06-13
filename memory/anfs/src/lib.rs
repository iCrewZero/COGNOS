//! COGNOS ANFS — the AI-Native File System overlay.
//!
//! - [`fuse_overlay`] (Unix only) — transparent FUSE passthrough with
//!   snapshot-before-AI-edit, delete intercept, and metadata batching
//! - [`relationship`] — co-open relationship graph, the third signal in the
//!   ambiguity resolution protocol (docs/SPEC.md)
//! - [`tag_engine`] — semantic tags from workflow signals only; never reads
//!   file content (anti-overreach rule)

#[cfg(unix)]
pub mod fuse_overlay;
pub mod relationship;
pub mod tag_engine;

#[cfg(unix)]
pub use fuse_overlay::{AnfsFilesystem, FileAnfsMeta};
pub use relationship::RelationshipGraph;
pub use tag_engine::{derive_domain, derive_tags};
