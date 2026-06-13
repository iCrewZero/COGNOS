//! Embedding seam for the memory layer.
//!
//! Threat-model requirement (docs/SPEC.md): the embedding model is
//! separate from the instruction model and runs isolated. This module
//! defines the trait boundary; the model-backed implementation (Phase 3)
//! plugs in behind it. v0 ships a deterministic feature-hashing embedder
//! so indexing and search are functional and testable without a model.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// Embedding dimensionality for the v0 store.
pub const EMBED_DIM: usize = 256;

pub trait Embedder: Send + Sync {
    /// Embed text into a unit-length vector of [`EMBED_DIM`] dimensions.
    fn embed(&self, text: &str) -> Vec<f32>;
    /// Identifier persisted with each record for provenance.
    fn name(&self) -> &str;
}

/// Deterministic bag-of-words feature-hashing embedder (v0 fallback).
/// No model, no network, no state — same text always embeds identically.
#[derive(Debug, Default)]
pub struct HashEmbedder;

impl HashEmbedder {
    pub fn new() -> Self {
        Self
    }
}

impl Embedder for HashEmbedder {
    fn embed(&self, text: &str) -> Vec<f32> {
        let mut v = vec![0.0f32; EMBED_DIM];
        let tokens = text
            .to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|t| t.len() > 1)
            .map(str::to_string)
            .collect::<Vec<_>>();
        for token in tokens {
            let mut h = DefaultHasher::new();
            token.hash(&mut h);
            let digest = h.finish();
            let idx = (digest % EMBED_DIM as u64) as usize;
            let sign = if (digest >> 63) == 0 { 1.0 } else { -1.0 };
            v[idx] += sign;
        }
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for x in &mut v {
                *x /= norm;
            }
        }
        v
    }

    fn name(&self) -> &str {
        "hash-v0"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedding_is_deterministic() {
        let e = HashEmbedder::new();
        assert_eq!(e.embed("motor control loop"), e.embed("motor control loop"));
    }

    #[test]
    fn embedding_is_unit_length() {
        let e = HashEmbedder::new();
        let v = e.embed("pid tuning experiments for the robot arm");
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5);
    }

    #[test]
    fn empty_text_embeds_to_zero_vector() {
        let e = HashEmbedder::new();
        let v = e.embed("");
        assert!(v.iter().all(|x| *x == 0.0));
    }

    #[test]
    fn similar_texts_are_closer_than_dissimilar() {
        let e = HashEmbedder::new();
        let a = e.embed("motor driver control code");
        let b = e.embed("motor control firmware");
        let c = e.embed("banana bread recipe with cinnamon");
        let sim_ab = crate::query::cosine(&a, &b);
        let sim_ac = crate::query::cosine(&a, &c);
        assert!(sim_ab > sim_ac);
    }
}
