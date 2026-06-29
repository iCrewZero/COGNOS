//! Semantic search over the local index with provenance metadata.
//!
//! Threat-model mitigation for poisoned embeddings (docs/SPEC.md): every
//! result carries provenance (path, when it was embedded, by which
//! embedder, and a content preview) so the user can always inspect why a
//! result was returned.

use crate::embedder::Embedder;
use crate::indexer::IndexRecord;

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub path: String,
    pub score: f32,
    /// Provenance: content preview at index time.
    pub preview: String,
    /// Provenance: when this record was embedded.
    pub embedded_at: String,
    /// Provenance: which embedder produced the vector.
    pub embedder: String,
    pub domain: Option<String>,
}

/// Cosine similarity. Zero vectors score 0.0.
/// Owner: iCrewZero — added dimension mismatch guard so silently wrong
/// scores aren't produced when the embedder dimension changes.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    // If dimensions don't match, something went wrong in the indexer
    // or embedder. Return 0 rather than producing a silently wrong score.
    if a.len() != b.len() {
        tracing::warn!(
            "cosine dimension mismatch: a.len()={} b.len()={} — returning 0.0",
            a.len(), b.len(),
        );
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let na = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
    }
}

/// Top-k records most similar to `query`. Deterministic ordering:
/// score descending, then path ascending for reproducible audits.
pub fn search(
    records: &[IndexRecord],
    embedder: &dyn Embedder,
    query: &str,
    k: usize,
) -> Vec<SearchResult> {
    let qv = embedder.embed(query);
    let mut results: Vec<SearchResult> = records
        .iter()
        .map(|r| SearchResult {
            path: r.path.clone(),
            score: cosine(&qv, &r.embedding),
            preview: r.preview.clone(),
            embedded_at: r.embedded_at.clone(),
            embedder: r.embedder.clone(),
            domain: r.domain.clone(),
        })
        .collect();
    results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.path.cmp(&b.path))
    });
    results.truncate(k);
    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedder::{Embedder, HashEmbedder};

    fn record(path: &str, text: &str, e: &HashEmbedder) -> IndexRecord {
        IndexRecord {
            path: path.into(),
            embedded_at: "2026-01-01T00:00:00Z".into(),
            embedder: e.name().into(),
            domain: None,
            preview: text.chars().take(40).collect(),
            embedding: e.embed(text),
        }
    }

    #[test]
    fn relevant_file_ranks_first() {
        let e = HashEmbedder::new();
        let records = vec![
            record("~/projects/robo/motor.py", "motor driver control loop pwm", &e),
            record("~/recipes/bread.md", "banana bread recipe cinnamon sugar", &e),
        ];
        let results = search(&records, &e, "motor control", 2);
        assert_eq!(results[0].path, "~/projects/robo/motor.py");
        assert!(results[0].score > results[1].score);
    }

    #[test]
    fn results_carry_provenance() {
        let e = HashEmbedder::new();
        let records = vec![record("~/a.txt", "hello world", &e)];
        let results = search(&records, &e, "hello", 1);
        assert_eq!(results[0].embedder, "hash-v0");
        assert!(!results[0].embedded_at.is_empty());
        assert!(!results[0].preview.is_empty());
    }

    #[test]
    fn k_limits_results() {
        let e = HashEmbedder::new();
        let records: Vec<IndexRecord> = (0..10)
            .map(|i| record(&format!("~/f{}.txt", i), "same content", &e))
            .collect();
        assert_eq!(search(&records, &e, "content", 3).len(), 3);
    }

    #[test]
    fn cosine_handles_zero_vectors() {
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 0.0]), 0.0);
    }
}
