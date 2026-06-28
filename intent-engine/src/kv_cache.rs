//! KV cache for repeat intents (<3ms cache-hit target, per docs/SPEC.md).
//!
//! The implementation lives in [`crate::schema_validator`] alongside the
//! schema types it stores, avoiding a circular module dependency. This
//! module is the stable public path; downstream code should import from
//! here, not from `schema_validator` directly.

pub use crate::schema_validator::{CacheStats, IntentKvCache};
