//! Relationship graph — "what files were open together last time".
//!
//! Third signal in the ambiguity resolution protocol (docs/SPEC.md):
//! session context → temporal signals → **relationship graph** → one question.
//!
//! The graph counts pairwise co-open events. It stores only paths and
//! counts — workflow signals, never content. Fully inspectable and
//! deletable via `cognos memory` commands.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct RelationshipGraph {
    /// edges[file_a][file_b] = number of sessions both were open together.
    edges: HashMap<String, HashMap<String, u32>>,
}

impl RelationshipGraph {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one session's co-open set. Every unordered pair is counted once.
    pub fn record_co_open(&mut self, files: &[String]) {
        for (i, a) in files.iter().enumerate() {
            for b in files.iter().skip(i + 1) {
                if a == b {
                    continue;
                }
                *self
                    .edges
                    .entry(a.clone())
                    .or_default()
                    .entry(b.clone())
                    .or_insert(0) += 1;
                *self
                    .edges
                    .entry(b.clone())
                    .or_default()
                    .entry(a.clone())
                    .or_insert(0) += 1;
            }
        }
    }

    /// Top-k files most often co-opened with `path`, strongest first.
    /// Deterministic: ties break alphabetically for reproducible audits.
    pub fn related(&self, path: &str, k: usize) -> Vec<(String, u32)> {
        let mut pairs: Vec<(String, u32)> = self
            .edges
            .get(path)
            .map(|m| m.iter().map(|(p, c)| (p.clone(), *c)).collect())
            .unwrap_or_default();
        pairs.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        pairs.truncate(k);
        pairs
    }

    /// Remove every edge touching paths under `prefix`
    /// (supports `cognos memory wipe --scope`).
    pub fn forget_prefix(&mut self, prefix: &str) {
        self.edges.retain(|k, _| !k.starts_with(prefix));
        for neighbors in self.edges.values_mut() {
            neighbors.retain(|k, _| !k.starts_with(prefix));
        }
    }

    /// Number of files with at least one relationship.
    pub fn len(&self) -> usize {
        self.edges.len()
    }

    pub fn is_empty(&self) -> bool {
        self.edges.is_empty()
    }

    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        std::fs::write(path, json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &str) -> String {
        v.to_string()
    }

    #[test]
    fn co_open_builds_symmetric_edges() {
        let mut g = RelationshipGraph::new();
        g.record_co_open(&[s("~/p/motor.py"), s("~/p/config.yaml")]);
        assert_eq!(g.related("~/p/motor.py", 5), vec![(s("~/p/config.yaml"), 1)]);
        assert_eq!(g.related("~/p/config.yaml", 5), vec![(s("~/p/motor.py"), 1)]);
    }

    #[test]
    fn repeated_sessions_strengthen_edges() {
        let mut g = RelationshipGraph::new();
        for _ in 0..3 {
            g.record_co_open(&[s("a"), s("b")]);
        }
        g.record_co_open(&[s("a"), s("c")]);
        let related = g.related("a", 5);
        assert_eq!(related[0], (s("b"), 3));
        assert_eq!(related[1], (s("c"), 1));
    }

    #[test]
    fn forget_prefix_removes_both_directions() {
        let mut g = RelationshipGraph::new();
        g.record_co_open(&[s("~/work/a"), s("~/personal/b")]);
        g.forget_prefix("~/personal");
        assert!(g.related("~/work/a", 5).is_empty());
        assert!(g.related("~/personal/b", 5).is_empty());
    }

    #[test]
    fn roundtrips_to_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("rel.json");
        let mut g = RelationshipGraph::new();
        g.record_co_open(&[s("a"), s("b")]);
        g.save(&path).expect("save");
        let loaded = RelationshipGraph::load(&path);
        assert_eq!(loaded.related("a", 1), vec![(s("b"), 1)]);
    }
}
