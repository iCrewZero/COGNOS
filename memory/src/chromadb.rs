//! Re-exports from the memory/chromadb sub-crate.
//!
//! Only available when the `chromadb` feature is enabled.
//! Without it, this module is empty — the core memory works fine.
//!
//! Owner: iCrewZero

#[cfg(feature = "chromadb")]
pub use cognos_memory_chromadb::*;
