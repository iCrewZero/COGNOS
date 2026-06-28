//! Recursion limiter — prevents AI self-recursion and infinite meta-reasoning.
//!
//!
//! A sufficiently capable agent can ask itself to evaluate its own evaluation,
//! recursively, forever — burning CPU and producing no useful output. Worse,
//! such loops can be *strategically* useful for the agent (to delay a verdict,
//! to confuse the audit trail, or to exhaust operator attention). The
//! [`RecursionLimiter`] puts a hard, per-agent cap on how deeply an agent may
//! re-enter HAL evaluation, plus a windowed rate limit to catch tight loops
//! that stay below the depth cap.
//!
//! The depth model is a stack: [`RecursionLimiter::enter`] returns a
//! [`RecursionGuard`] that decrements the counter on drop. This makes it
//! impossible to forget to pop the stack — the borrow checker enforces it.
//!
//! v0: stub implementation. Loop detection (recognizing *which* agent-pair
//! loops are occurring, not just *that* one is) is TODO(v1).

use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, warn};

// v0: stub implementation

/// Type alias for an agent identifier (matches the rest of the crate).
pub type AgentId = String;

/// Default maximum recursion depth: 8.
pub const DEFAULT_MAX_DEPTH: u32 = 8;

/// Default sliding window for rate limiting: 1 second.
pub const DEFAULT_WINDOW_SECS: u64 = 1;

/// Default maximum number of entries permitted within the window.
pub const DEFAULT_WINDOW_MAX_ENTRIES: u32 = 64;

// ─── Errors ─────────────────────────────────────────────────────────────────────

/// Errors returned by the recursion limiter.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum RecursionError {
    /// The agent has exceeded the maximum permitted recursion depth.
    #[error("max recursion depth exceeded for agent {agent}: depth {depth} >= max {max}")]
    MaxDepthExceeded {
        /// Agent that exceeded the depth.
        agent: AgentId,
        /// Current depth at the point of refusal.
        depth: u32,
        /// Configured maximum.
        max: u32,
    },
    /// The agent has entered HAL too many times within the rate-limit window.
    #[error("rate limited for agent {agent}: {count} entries in {window_secs}s")]
    RateLimited {
        /// Agent that was rate limited.
        agent: AgentId,
        /// Number of entries observed in the window.
        count: u32,
        /// Window length in seconds.
        window_secs: u64,
    },
    /// A loop was detected (same agent re-entering with identical arguments).
    /// v0 never emits this; v1 will.
    #[error("recursion loop detected for agent {agent}")]
    LoopDetected {
        /// Agent that triggered loop detection.
        agent: AgentId,
    },
}

// ─── Recursion Guard ────────────────────────────────────────────────────────────

/// RAII guard returned by [`RecursionLimiter::enter`].
///
/// Holding this guard means the agent is currently inside HAL evaluation at
/// one level of recursion. Dropping it decrements the agent's depth counter.
pub struct RecursionGuard<'a> {
    /// Reference to the limiter that issued the guard.
    limiter: &'a mut RecursionLimiter,
    /// Agent this guard is for.
    agent: AgentId,
    /// Whether the guard has already been released.
    released: bool,
}

impl<'a> RecursionGuard<'a> {
    /// Explicitly release the guard early. Equivalent to dropping it.
    pub fn release(mut self) {
        self.released = true;
        // `self` will still be dropped; the Drop impl checks `released`.
    }
}

impl<'a> Drop for RecursionGuard<'a> {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        // Clone the agent id out so we don't hold an immutable borrow of
        // `self.agent` across the mutable borrow of `self.limiter.depth`.
        // TODO(v1): also drop the corresponding recent_entries timestamp
        // so a long-running guard does not artificially inflate the
        // rate-limit window. v0 leaves recent_entries untouched on drop.
        let agent = self.agent.clone();
        if let Some(entry) = self.limiter.depth.get_mut(&agent) {
            if *entry > 0 {
                *entry -= 1;
                debug!(agent = %agent, depth = *entry, "recursion_limiter: exit");
            } else {
                warn!(agent = %agent, "recursion_limiter: depth underflow on drop");
            }
        }
    }
}

// ─── Recursion Limiter ──────────────────────────────────────────────────────────

/// Per-agent recursion depth and rate limiter.
///
/// The `depth` map holds the current per-agent recursion depth (matches
/// the spec's `HashMap<AgentId, u32>`). The `recent_entries` map holds the
/// per-agent entry timestamps used for windowed rate limiting.
pub struct RecursionLimiter {
    /// Per-agent current recursion depth (number of active guards).
    depth: HashMap<AgentId, u32>,
    /// Maximum permitted recursion depth.
    max_depth: u32,
    /// Sliding window length for rate limiting.
    window: Duration,
    /// Per-agent entry timestamps, for windowed rate limiting.
    recent_entries: HashMap<AgentId, Vec<DateTime<Utc>>>,
    /// Maximum number of entries permitted within the window.
    window_max_entries: u32,
}

impl RecursionLimiter {
    /// Build a new limiter with default parameters.
    pub fn new() -> Self {
        Self {
            depth: HashMap::new(),
            max_depth: DEFAULT_MAX_DEPTH,
            window: Duration::from_secs(DEFAULT_WINDOW_SECS),
            recent_entries: HashMap::new(),
            window_max_entries: DEFAULT_WINDOW_MAX_ENTRIES,
        }
    }

    /// Build a limiter with a custom max depth and window.
    pub fn with_config(max_depth: u32, window: Duration) -> Self {
        Self {
            depth: HashMap::new(),
            max_depth,
            window,
            recent_entries: HashMap::new(),
            window_max_entries: DEFAULT_WINDOW_MAX_ENTRIES,
        }
    }

    /// Attempt to enter HAL evaluation for `agent`.
    ///
    /// Returns a guard on success; the guard must be dropped before the agent
    /// may enter again. Returns [`RecursionError::MaxDepthExceeded`] if the
    /// agent is already at the depth cap, or [`RecursionError::RateLimited`]
    /// if the agent has entered too many times within the sliding window.
    pub fn enter(&mut self, agent: &AgentId) -> Result<RecursionGuard<'_>, RecursionError> {
        let now = Utc::now();

        // Depth check (immutable read of `self.depth`).
        let current_depth = self.depth.get(agent).copied().unwrap_or(0);
        if current_depth >= self.max_depth {
            warn!(
                agent = %agent,
                depth = current_depth,
                max = self.max_depth,
                "recursion_limiter: depth exceeded"
            );
            return Err(RecursionError::MaxDepthExceeded {
                agent: agent.clone(),
                depth: current_depth,
                max: self.max_depth,
            });
        }

        // Rate-limit check (mutable borrow on `self.recent_entries` only).
        // Scoped so the borrow is released before we touch `self.depth`.
        let window_secs = self.window.as_secs();
        {
            let entries = self.recent_entries.entry(agent.clone()).or_default();
            entries
                .retain(|t| now.signed_duration_since(*t).num_seconds() < window_secs as i64);
            if entries.len() as u32 >= self.window_max_entries {
                let count = entries.len() as u32;
                warn!(
                    agent = %agent,
                    count,
                    window_secs,
                    "recursion_limiter: rate limited"
                );
                return Err(RecursionError::RateLimited {
                    agent: agent.clone(),
                    count,
                    window_secs,
                });
            }
            entries.push(now);
        }

        // Increment depth (mutable borrow on `self.depth` only).
        *self.depth.entry(agent.clone()).or_insert(0) += 1;
        let new_depth = current_depth + 1;
        debug!(agent = %agent, depth = new_depth, "recursion_limiter: enter");
        Ok(RecursionGuard {
            limiter: self,
            agent: agent.clone(),
            released: false,
        })
    }

    /// Current recursion depth for `agent` (0 if unknown).
    pub fn current_depth(&self, agent: &AgentId) -> u32 {
        self.depth.get(agent).copied().unwrap_or(0)
    }

    /// Number of agents currently tracked by the limiter.
    pub fn tracked_agents(&self) -> usize {
        self.depth.len()
    }
}

impl Default for RecursionLimiter {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Configuration ──────────────────────────────────────────────────────────────

/// Serializable configuration for a [`RecursionLimiter`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecursionLimiterConfig {
    /// Maximum permitted recursion depth.
    pub max_depth: u32,
    /// Sliding window length for rate limiting, in seconds.
    pub window_secs: u64,
    /// Maximum number of entries permitted within the window.
    pub window_max_entries: u32,
}

impl Default for RecursionLimiterConfig {
    fn default() -> Self {
        Self {
            max_depth: DEFAULT_MAX_DEPTH,
            window_secs: DEFAULT_WINDOW_SECS,
            window_max_entries: DEFAULT_WINDOW_MAX_ENTRIES,
        }
    }
}
