//! Self-rewrite monitor — gates AI attempts to modify its own code.
//!
//!
//! An AI system that can rewrite its own source code is, in principle, an AI
//! system that can rewrite its own safety constraints. The
//! [`SelfRewriteMonitor`] watches a configurable set of "AI source" paths
//! (the agent binaries, the HAL binaries, the cognitive-preloader configs,
//! the model weights, the capability manifests) and gates any modification
//! to them.
//!
//! In v0, *every* modification to a watched path is routed to mandatory
//! human review. There is no allow-list and no auto-approve path. v1 may
//! introduce a tightly-scoped allow-list for routine, signed updates.
//!
//! v0: stub implementation. Path matching is prefix-based; inotify watchers
//! are TODO(v1).

use std::collections::HashSet;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{info, warn};
use uuid::Uuid;

// v0: stub implementation

/// Type alias for an agent identifier (matches the rest of the crate).
pub type AgentId = String;

/// Identifier of a pending rewrite request.
pub type RewriteRequestId = Uuid;

// ─── Rewrite Request ────────────────────────────────────────────────────────────

/// A request from an agent to modify a watched path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RewriteRequest {
    /// The path the agent wants to modify.
    pub target_path: PathBuf,
    /// The proposed diff, in unified-diff format (v0: opaque string).
    pub diff: String,
    /// The agent requesting the modification.
    pub agent: AgentId,
    /// The agent's stated reason for the modification.
    pub reason: String,
    /// When the request was made.
    pub requested_at: DateTime<Utc>,
}

// ─── Rewrite Verdict ────────────────────────────────────────────────────────────

/// The monitor's verdict on a rewrite request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RewriteVerdict {
    /// The request is denied outright (e.g. target path is not watched, or
    /// the diff is malformed — though in v0 the latter is not checked).
    Deny,
    /// The request is allowed, but an audit entry must be written.
    AllowWithAudit,
    /// The request requires explicit human review before it can proceed.
    /// This is the default for any modification to AI source or config.
    RequireHumanReview,
}

// ─── Pending Request ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingRequest {
    id: RewriteRequestId,
    request: RewriteRequest,
    verdict: RewriteVerdict,
    recorded_at: DateTime<Utc>,
}

// ─── Self-Rewrite Monitor ───────────────────────────────────────────────────────

/// The self-rewrite monitor.
pub struct SelfRewriteMonitor {
    /// Paths (or path prefixes) watched by the monitor. Any modification to
    /// a path that starts with one of these prefixes is gated.
    pub watch_paths: HashSet<PathBuf>,
    /// Pending rewrite requests awaiting human review.
    pub pending_requests: std::collections::HashMap<RewriteRequestId, PendingRequest>,
}

impl Default for SelfRewriteMonitor {
    fn default() -> Self {
        Self::new()
    }
}

impl SelfRewriteMonitor {
    /// Build a new monitor with the default watch set.
    pub fn new() -> Self {
        let mut watch = HashSet::new();
        for path in DEFAULT_WATCH_PATHS {
            watch.insert(PathBuf::from(path));
        }
        Self {
            watch_paths: watch,
            pending_requests: std::collections::HashMap::new(),
        }
    }

    /// Add a path prefix to the watch set.
    pub fn watch(&mut self, path: impl Into<PathBuf>) {
        self.watch_paths.insert(path.into());
    }

    /// Evaluate a rewrite request.
    ///
    /// In v0, any modification to a watched path returns
    /// [`RewriteVerdict::RequireHumanReview`]. Modifications to non-watched
    /// paths return [`RewriteVerdict::AllowWithAudit`] (the monitor does not
    /// gate unrelated filesystem activity).
    // TODO(v1): add an inotify watcher so modifications can be detected
    // even when an agent bypasses the HAL API, and a signed-update
    // allow-list for routine, cryptographically-signed agent updates.
    pub fn evaluate_rewrite(&mut self, request: RewriteRequest) -> RewriteVerdict {
        let watched = self.is_watched(&request.target_path);
        let verdict = if watched {
            warn!(
                agent = %request.agent,
                path = ?request.target_path,
                reason = %request.reason,
                "self_rewrite_monitor: REWRITE REQUEST — routing to human review"
            );
            RewriteVerdict::RequireHumanReview
        } else {
            info!(
                agent = %request.agent,
                path = ?request.target_path,
                "self_rewrite_monitor: path not watched, allowing with audit"
            );
            RewriteVerdict::AllowWithAudit
        };

        // Record the request as pending if it requires human review.
        if matches!(verdict, RewriteVerdict::RequireHumanReview) {
            let id = Uuid::new_v4();
            let record = PendingRequest {
                id,
                request: request.clone(),
                verdict: verdict.clone(),
                recorded_at: Utc::now(),
            };
            self.pending_requests.insert(id, record);
        }

        verdict
    }

    /// Whether a given path is covered by any watch prefix.
    fn is_watched(&self, path: &PathBuf) -> bool {
        for prefix in &self.watch_paths {
            if path.starts_with(prefix) {
                return true;
            }
        }
        false
    }

    /// Number of rewrite requests currently pending human review.
    pub fn pending_count(&self) -> usize {
        self.pending_requests.len()
    }
}

/// The default set of watched path prefixes.
///
/// Any modification to these paths requires human review.
pub const DEFAULT_WATCH_PATHS: &[&str] = &[
    // AI source code
    "/opt/cognos/agents",
    "/opt/cognos/hal",
    "/opt/cognos/cognitive_preloader",
    "/opt/cognos/intent-engine",
    // AI configuration
    "/etc/cognos/agents",
    "/etc/cognos/hal",
    // AI model weights and capability manifests
    "/var/lib/cognos/models",
    "/var/lib/cognos/capabilities",
    // The constitution (must never be modified at runtime)
    "/etc/cognos/constitution",
];

// ─── Errors ─────────────────────────────────────────────────────────────────────

/// Errors returned by the self-rewrite monitor.
#[derive(Debug, Error)]
pub enum SelfRewriteError {
    /// A pending request was not found.
    #[error("pending rewrite request not found: {0}")]
    NotFound(RewriteRequestId),
    /// A pending request was in a state that does not permit the requested
    /// action (e.g. attempting to approve an already-denied request).
    #[error("pending request {0} in invalid state for action")]
    InvalidState(RewriteRequestId),
    /// The requested path is not absolute (the monitor requires absolute
    /// paths to avoid trivial bypasses via relative paths).
    #[error("rewrite target path must be absolute: {0}")]
    NonAbsoluteTarget(String),
}
