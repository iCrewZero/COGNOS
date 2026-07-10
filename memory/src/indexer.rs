//! Indexer — consent-scoped file embedding into a local JSONL store.
//!
//! Consumes the ANFS index queue (paths of changed files), embeds file
//! content, and appends records to a plain-text store the user can
//! inspect (`cognos memory show`) and delete (`cognos memory wipe`).
//!
//! Consent boundary: files outside the allowed roots are silently
//! skipped — indexing is opt-in by scope, per the anti-Recall rules.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::embedder::Embedder;

/// Files larger than this are never read (1 MiB).
const MAX_FILE_BYTES: u64 = 1_048_576;
const PREVIEW_CHARS: usize = 160;

#[derive(Debug)]
pub enum IndexError {
    Io(std::io::Error),
    Serialize(String),
}

impl std::fmt::Display for IndexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "index io error: {}", e),
            Self::Serialize(e) => write!(f, "index serialize error: {}", e),
        }
    }
}

impl std::error::Error for IndexError {}

impl From<std::io::Error> for IndexError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// One stored embedding with full provenance (threat model: search results
/// include provenance metadata so the user can inspect why a result exists).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexRecord {
    pub path: String,
    pub embedded_at: String,
    pub embedder: String,
    pub domain: Option<String>,
    pub preview: String,
    pub embedding: Vec<f32>,
}

pub struct Indexer<E: Embedder> {
    embedder: E,
    store_path: PathBuf,
    allowed_roots: Vec<PathBuf>,
}

impl<E: Embedder> Indexer<E> {
    pub fn new(embedder: E, store_path: PathBuf, allowed_roots: Vec<PathBuf>) -> Self {
        Self {
            embedder,
            store_path,
            allowed_roots,
        }
    }

    fn in_scope(&self, path: &Path) -> bool {
        self.allowed_roots.iter().any(|root| path.starts_with(root))
    }

    /// Index a single file. Returns Ok(false) when skipped (out of scope,
    /// too large, binary, or not a regular file) — skipping is not an error.
    pub fn index_file(&self, path: &Path) -> Result<bool, IndexError> {
        if !self.in_scope(path) {
            return Ok(false);
        }
        let meta = match fs::metadata(path) {
            Ok(m) => m,
            Err(_) => return Ok(false), // vanished since queued
        };
        if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
            return Ok(false);
        }
        let bytes = fs::read(path)?;
        if bytes.iter().take(512).any(|b| *b == 0) {
            return Ok(false); // binary
        }
        let text = String::from_utf8_lossy(&bytes);

        let record = IndexRecord {
            path: path.to_string_lossy().to_string(),
            embedded_at: Utc::now().to_rfc3339(),
            embedder: self.embedder.name().to_string(),
            domain: derive_domain(&path.to_string_lossy()),
            preview: text.chars().take(PREVIEW_CHARS).collect(),
            embedding: self.embedder.embed(&text),
        };

        let json = serde_json::to_string(&record)
            .map_err(|e| IndexError::Serialize(e.to_string()))?;
        if let Some(parent) = self.store_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.store_path)?;
        writeln!(f, "{}", json)?;
        Ok(true)
    }

    /// Drain the ANFS index queue: index every queued path, then truncate
    /// the queue. Returns how many files were actually indexed.
    pub fn drain_queue(&self, queue_path: &Path) -> Result<usize, IndexError> {
        if !queue_path.exists() {
            return Ok(0);
        }
        let file = fs::File::open(queue_path)?;
        let mut indexed = 0;
        for line in BufReader::new(file).lines() {
            let line = line?;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if self.index_file(Path::new(trimmed)).unwrap_or(false) {
                indexed += 1;
            }
        }
        fs::write(queue_path, b"")?;
        Ok(indexed)
    }

    /// All stored records (`cognos memory show`).
    pub fn records(&self) -> Result<Vec<IndexRecord>, IndexError> {
        if !self.store_path.exists() {
            return Ok(Vec::new());
        }
        let file = fs::File::open(&self.store_path)?;
        let mut records = Vec::new();
        for line in BufReader::new(file).lines() {
            let line = line?;
            if let Ok(r) = serde_json::from_str::<IndexRecord>(&line) {
                records.push(r);
            }
        }
        Ok(records)
    }

    /// Delete the entire store (`cognos memory wipe`).
    pub fn wipe(&self) -> Result<(), IndexError> {
        if self.store_path.exists() {
            fs::remove_file(&self.store_path)?;
        }
        Ok(())
    }

    /// Delete records for one domain (`cognos memory wipe --scope <domain>`).
    /// Returns how many records were removed.
    pub fn wipe_domain(&self, domain: &str) -> Result<usize, IndexError> {
        let records = self.records()?;
        let kept: Vec<&IndexRecord> = records
            .iter()
            .filter(|r| r.domain.as_deref() != Some(domain))
            .collect();
        let removed = records.len() - kept.len();
        let mut out = String::new();
        for r in &kept {
            let json = serde_json::to_string(r)
                .map_err(|e| IndexError::Serialize(e.to_string()))?;
            out.push_str(&json);
            out.push('\n');
        }
        fs::write(&self.store_path, out)?;
        Ok(removed)
    }
}

/// Project domain from path structure (mirrors the ANFS tag engine rule):
/// the segment after "projects".
fn derive_domain(path: &str) -> Option<String> {
    let parts: Vec<String> = Path::new(path)
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .map(|s| s.to_lowercase())
        .collect();
    let idx = parts.iter().position(|p| p == "projects")?;
    parts.get(idx + 1).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedder::HashEmbedder;

    fn setup() -> (tempfile::TempDir, Indexer<HashEmbedder>) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = dir.path().join("index.jsonl");
        let allowed = vec![dir.path().to_path_buf()];
        (dir, IndexerBuilder(store, allowed))
    }

    #[allow(non_snake_case)]
    fn IndexerBuilder(store: PathBuf, allowed: Vec<PathBuf>) -> Indexer<HashEmbedder> {
        Indexer::new(HashEmbedder::new(), store, allowed)
    }

    #[test]
    fn indexes_text_file_in_scope() {
        let (dir, indexer) = setup();
        let f = dir.path().join("projects").join("robo-arm").join("motor.py");
        fs::create_dir_all(f.parent().expect("parent")).expect("mkdir");
        fs::write(&f, "def control_motor(): pass").expect("write");
        assert!(indexer.index_file(&f).expect("index"));
        let records = indexer.records().expect("records");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].domain.as_deref(), Some("robo-arm"));
        assert!(!records[0].preview.is_empty());
    }

    #[test]
    fn out_of_scope_file_is_skipped() {
        let (_dir, indexer) = setup();
        let outside = tempfile::tempdir().expect("tempdir2");
        let f = outside.path().join("secret.txt");
        fs::write(&f, "do not index").expect("write");
        assert!(!indexer.index_file(&f).expect("index"));
        assert!(indexer.records().expect("records").is_empty());
    }

    #[test]
    fn binary_file_is_skipped() {
        let (dir, indexer) = setup();
        let f = dir.path().join("blob.bin");
        fs::write(&f, [0u8, 159, 146, 150]).expect("write");
        assert!(!indexer.index_file(&f).expect("index"));
    }

    #[test]
    fn drain_queue_indexes_and_truncates() {
        let (dir, indexer) = setup();
        let f = dir.path().join("notes.md");
        fs::write(&f, "robotics notes").expect("write");
        let queue = dir.path().join("index_queue");
        fs::write(&queue, format!("{}\n", f.display())).expect("write queue");
        let n = indexer.drain_queue(&queue).expect("drain");
        assert_eq!(n, 1);
        assert_eq!(fs::read_to_string(&queue).expect("read"), "");
    }

    #[test]
    fn wipe_and_wipe_domain_work() {
        let (dir, indexer) = setup();
        let a = dir.path().join("projects").join("alpha").join("a.txt");
        let b = dir.path().join("projects").join("beta").join("b.txt");
        for f in [&a, &b] {
            fs::create_dir_all(f.parent().expect("parent")).expect("mkdir");
            fs::write(f, "content").expect("write");
            indexer.index_file(f).expect("index");
        }
        assert_eq!(indexer.wipe_domain("alpha").expect("wipe domain"), 1);
        assert_eq!(indexer.records().expect("records").len(), 1);
        indexer.wipe().expect("wipe");
        assert!(indexer.records().expect("records").is_empty());
    }
}
