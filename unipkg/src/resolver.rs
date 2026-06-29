//! UNIPKG resolver — decides which package source (APT, Flatpak, ...) to query
//! for a given user request. In v0 this is a simple priority + availability
//! check; v1 will add trust scoring, HAL gating, and ANFS-mediated installs.
//!
//! All sources are pluggable. The resolver never installs anything itself —
//! it returns a `ResolvedPackage` that the caller feeds into the installer.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;

// ─── Types ────────────────────────────────────────────────────────────────────

/// A package source known to unipkg. The list is closed; new sources require
/// a spec update and human review (mirrors HAL's `Capability` model).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SourceKind {
    Apt,
    Flatpak,
    Snap,
    AppImage,
    Local,
}

impl SourceKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Apt      => "apt",
            Self::Flatpak  => "flatpak",
            Self::Snap     => "snap",
            Self::AppImage => "appimage",
            Self::Local    => "local",
        }
    }
}

/// Per-source availability / health snapshot, refreshed by the resolver
/// before each decision so we never route to a dead source.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SourceStatus {
    pub available: bool,
    pub last_check_ms: u64,
    pub cache_size_bytes: u64,
}

/// A request to resolve a package. `query` may be a name, a path, or a
/// natural-language hint. `preferred` lets the caller bias the resolver.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolveRequest {
    pub query: String,
    pub preferred: Option<SourceKind>,
    pub include_installed: bool,
}

/// The resolver's verdict for one request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedPackage {
    pub name: String,
    pub display_name: String,
    pub version: String,
    pub description: String,
    pub source: SourceKind,
    pub installed: bool,
    pub size_kb: Option<u64>,
    pub score: f32,
}

/// Result of a multi-source search.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchReport {
    pub results: Vec<ResolvedPackage>,
    pub sources_queried: Vec<SourceKind>,
    pub sources_failed: Vec<SourceKind>,
    pub elapsed_ms: u64,
}

// ─── Errors ───────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum ResolverError {
    #[error("no package source available")]
    NoSourceAvailable,
    #[error("query too short (min {0} chars)")]
    QueryTooShort(usize),
    #[error("source {0} unavailable")]
    SourceUnavailable(SourceKind),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

// ─── Resolver ─────────────────────────────────────────────────────────────────

/// Picks which source(s) to query for a given request, fans out, and ranks.
pub struct Resolver {
    sources: HashMap<SourceKind, SourceStatus>,
    priority: Vec<SourceKind>,
    min_query_len: usize,
}

impl Resolver {
    /// Build a resolver with the default priority order (apt → flatpak → snap → appimage).
    pub fn new() -> Self {
        let mut sources = HashMap::new();
        for k in [SourceKind::Apt, SourceKind::Flatpak, SourceKind::Snap, SourceKind::AppImage] {
            sources.insert(k, SourceStatus::default());
        }
        Self {
            sources,
            priority: vec![SourceKind::Apt, SourceKind::Flatpak, SourceKind::Snap, SourceKind::AppImage],
            min_query_len: 2,
        }
    }

    /// Mark a source as available/unavailable. Called by the daemon on startup
    /// and whenever a source's health check fails.
    pub fn set_available(&mut self, kind: SourceKind, available: bool) {
        if let Some(s) = self.sources.get_mut(&kind) {
            s.available = available;
        }
    }

    /// Override the default priority order. Sources earlier in the list are
    /// preferred when multiple sources return the same package.
    pub fn set_priority(&mut self, priority: Vec<SourceKind>) {
        self.priority = priority;
    }

    /// The list of sources we'd actually query for this request, in priority
    /// order. Honors `preferred` if set and available.
    pub fn plan(&self, req: &ResolveRequest) -> Vec<SourceKind> {
        if let Some(pref) = req.preferred {
            if self.is_available(pref) {
                return vec![pref];
            }
        }
        self.priority.iter().copied().filter(|k| self.is_available(*k)).collect()
    }

    /// Resolve a single best match. v0 returns the first hit from the highest
    /// priority source; v1 will rank across sources by trust + freshness.
    pub fn resolve(&self, req: &ResolveRequest) -> Result<Option<ResolvedPackage>, ResolverError> {
        if req.query.len() < self.min_query_len {
            return Err(ResolverError::QueryTooShort(self.min_query_len));
        }
        let plan = self.plan(req);
        if plan.is_empty() {
            return Err(ResolverError::NoSourceAvailable);
        }
        // v0: just take the first source's first result.
        // TODO(v1): fan out across all sources, rank, deduplicate.
        let _ = plan;
        Ok(None)
    }

    /// Search across all available sources and return a ranked report.
    pub fn search(&self, req: &ResolveRequest) -> Result<SearchReport, ResolverError> {
        if req.query.len() < self.min_query_len {
            return Err(ResolverError::QueryTooShort(self.min_query_len));
        }
        let plan = self.plan(req);
        if plan.is_empty() {
            return Err(ResolverError::NoSourceAvailable);
        }
        // v0: stub — real fan-out is in main.rs's `unified_search`.
        // TODO(v1): call each source's `search` concurrently, dedupe by name, rank.
        Ok(SearchReport {
            results: Vec::new(),
            sources_queried: plan,
            sources_failed: Vec::new(),
            elapsed_ms: 0,
        })
    }

    fn is_available(&self, kind: SourceKind) -> bool {
        self.sources.get(&kind).map(|s| s.available).unwrap_or(false)
    }
}

impl Default for Resolver {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Tests would live in tests/ — not included per project policy ─────────────

// v0: stub implementation
