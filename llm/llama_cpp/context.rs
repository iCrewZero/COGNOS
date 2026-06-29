//! Per-session LLM context — manages KV cache slots, context window, and
//! token accounting so multiple agents can share a model without re-warming.
//!
//! A [`LlmContext`] is the per-conversation companion to a loaded model. It
//! owns the token list that has been fed to the model so far, tracks how full
//! the underlying KV cache is, and provides [`shift`] for sliding-window
//! eviction when the conversation grows past the model's context window.
//!
//! [`shift`]: LlmContext::shift

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, info, warn};

use crate::bindings::Token;

// ─── Errors ────────────────────────────────────────────────────────────────

/// Errors returned by [`LlmContext`] operations.
#[derive(Debug, Error, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub enum ContextError {
    /// The context window is full and no more tokens can be appended without
    /// a [`LlmContext::shift`] or [`LlmContext::reset`].
    #[error("context window is full")]
    Full,

    /// The caller supplied tokens that the context could not accept (e.g. an
    /// empty slice or tokens that would overflow the window by themselves).
    #[error("invalid token batch")]
    InvalidTokens,
}

// ─── KV Cache State ────────────────────────────────────────────────────────

/// Snapshot of the underlying llama.cpp KV cache for a context.
///
/// `slots_total` is fixed when the context is created; `slots_used` grows as
/// tokens are appended. `head` is the index of the oldest resident token and
/// is advanced by [`LlmContext::shift`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct KvCacheState {
    /// Number of KV cache slots currently occupied.
    pub slots_used: u32,
    /// Total number of KV cache slots this context was created with.
    pub slots_total: u32,
    /// Index of the oldest resident token; advances on `shift`.
    pub head: u32,
    /// Monotonic counter incremented every time the cache is shifted, so
    /// callers can detect that prior token ids are no longer valid.
    pub last_shift: u64,
}

impl Default for KvCacheState {
    fn default() -> Self {
        Self {
            slots_used: 0,
            slots_total: 4096,
            head: 0,
            last_shift: 0,
        }
    }
}

impl KvCacheState {
    /// Returns the number of free slots remaining.
    pub fn free(&self) -> u32 {
        self.slots_total.saturating_sub(self.slots_used)
    }

    /// Returns the fraction of slots currently in use (`0.0..=1.0`).
    pub fn usage(&self) -> f32 {
        if self.slots_total == 0 {
            return 0.0;
        }
        self.slots_used as f32 / self.slots_total as f32
    }
}

// ─── Llm Context ───────────────────────────────────────────────────────────

/// Identifier for a context — typically a session or conversation id.
pub type ContextId = String;

/// Identifier for the model this context is bound to.
pub type ModelIdRef = String;

/// A per-conversation LLM context.
///
/// Holds the running token list, the KV cache accounting, and the maximum
/// context window size. Multiple [`LlmContext`] instances may share the same
/// loaded model — they simply occupy different KV cache slots.
pub struct LlmContext {
    /// Unique id for this context (usually a session id).
    pub id: ContextId,
    /// The model this context is bound to.
    pub model_id: ModelIdRef,
    /// KV cache accounting.
    pub kv_cache: KvCacheState,
    /// Tokens that have been committed to the context, in order.
    pub tokens: Vec<Token>,
    /// Hard cap on the number of resident tokens.
    pub max_tokens: u32,
}

impl LlmContext {
    /// Construct a fresh context bound to `model_id` with the given capacity.
    pub fn new(id: impl Into<ContextId>, model_id: impl Into<ModelIdRef>, max_tokens: u32) -> Self {
        let id = id.into();
        let model_id = model_id.into();
        info!(context.id = %id, model.id = %model_id, max_tokens, "new LlmContext");
        Self {
            id,
            model_id,
            kv_cache: KvCacheState {
                slots_total: max_tokens,
                ..KvCacheState::default()
            },
            tokens: Vec::new(),
            max_tokens,
        }
    }

    /// Append a batch of tokens to the context.
    ///
    /// Returns [`ContextError::Full`] if the batch would exceed `max_tokens`
    /// without an intervening [`shift`](Self::shift) or
    /// [`reset`](Self::reset), and [`ContextError::InvalidTokens`] if the
    /// batch is empty.
    pub fn append(&mut self, tokens: &[Token]) -> Result<(), ContextError> {
        if tokens.is_empty() {
            warn!(context.id = %self.id, "append() called with empty token slice");
            return Err(ContextError::InvalidTokens);
        }

        let needed = self.tokens.len() + tokens.len();
        if needed > self.max_tokens as usize {
            warn!(
                context.id = %self.id,
                needed, max = self.max_tokens, "append() would overflow context"
            );
            return Err(ContextError::Full);
        }

        debug!(
            context.id = %self.id,
            appending = tokens.len(),
            total = needed,
            "appending tokens"
        );
        self.tokens.extend_from_slice(tokens);
        // TODO(v1): forward the appended batch to llama.cpp via llama_decode.
        self.kv_cache.slots_used = self.tokens.len() as u32;
        Ok(())
    }

    /// Drop the oldest `n` tokens and shift the KV cache window forward.
    ///
    /// This is the sliding-window eviction strategy used when a context grows
    /// past its window: the model loses its memory of the dropped tokens but
    /// keeps the recent ones, avoiding a full re-warm.
    pub fn shift(&mut self, n: usize) {
        if n == 0 {
            return;
        }
        let drop_n = n.min(self.tokens.len());
        debug!(
            context.id = %self.id,
            drop_n, before = self.tokens.len(),
            "shifting context window"
        );
        self.tokens.drain(0..drop_n);
        self.kv_cache.head = self.kv_cache.head.saturating_add(drop_n as u32);
        self.kv_cache.slots_used = self.tokens.len() as u32;
        self.kv_cache.last_shift = self.kv_cache.last_shift.wrapping_add(1);
        // TODO(v1): call llama_kv_cache_seq_rm / seq_shift to mirror this on
        // the C side so the next decode does not re-process the dropped range.
    }

    /// Reset the context to an empty state, dropping all tokens and freeing
    /// every KV cache slot.
    pub fn reset(&mut self) {
        info!(context.id = %self.id, "resetting LlmContext");
        self.tokens.clear();
        self.kv_cache.slots_used = 0;
        self.kv_cache.head = 0;
        self.kv_cache.last_shift = self.kv_cache.last_shift.wrapping_add(1);
        // TODO(v1): call llama_kv_cache_seq_rm for this context's seq_id.
    }

    /// Returns the fraction of the context window currently in use
    /// (`0.0..=1.0`).
    pub fn usage(&self) -> f32 {
        if self.max_tokens == 0 {
            return 0.0;
        }
        self.tokens.len() as f32 / self.max_tokens as f32
    }

    /// Number of tokens currently resident.
    pub fn len(&self) -> usize {
        self.tokens.len()
    }

    /// `true` if no tokens are currently resident.
    pub fn is_empty(&self) -> bool {
        self.tokens.is_empty()
    }
}

impl Default for LlmContext {
    fn default() -> Self {
        Self::new("default", "default", 4096)
    }
}

// v0: stub implementation
