//! Re-exports from the memory/fabric sub-crate.
//!
//! Only available when the `fabric` feature is enabled.
//! Without it, this module is empty — the core memory works fine.
//!
//! Owner: iCrewZero

#[cfg(feature = "fabric")]
pub use cognos_memory_fabric::*;
