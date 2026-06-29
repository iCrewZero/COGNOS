//! BPE tokenizer — byte-pair-encoding tokenizer compatible with
//! GGUF/llama.cpp models. Loads vocabulary + merge rules from a
//! `tokenizer.json` file, encodes/decodes between text and token ids.
//!
//! The tokenizer is the boundary between human-readable text and the
//! integer token streams consumed by the local LLM inference engine.
//! COGNOS uses it for:
//!
//! - the intent-engine's prompt assembly,
//! - the shell assistant's translation prompts,
//! - the LSTM telemetry windowing (token-bounded context).
//!
//! # Formats
//!
//! Two loaders are provided:
//!
//! - [`Tokenizer::load`]      — HuggingFace-style `tokenizer.json`
//! - [`Tokenizer::load_gguf`] — vocabulary + merges read directly from the
//!   GGUF metadata block of a `.gguf` model file.
//!
//! # Special tokens
//!
//! The standard set `<s>`, `</s>`, `<unk>`, `<pad>`, `<bos>`, `<eos>` is
//! recognized and may be looked up via [`Tokenizer::special_token`].
//! Special tokens are *never* produced by the BPE merge loop — callers
//! must inject them explicitly.
//!
//! // v0: stub implementation

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, warn};

// ─── Types ───────────────────────────────────────────────────────────────────

/// A single BPE merge rule: "the pair `(a, b)` becomes token `new_id`".
///
/// Merges are applied in rank order — earlier entries win.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeRule {
    /// The pair of existing token ids that should be merged.
    pub pair: (u32, u32),
    /// The id of the merged token produced by this rule.
    pub new_id: u32,
}

/// The tokenizer itself.
///
/// All maps are kept in sync by [`Tokenizer::load`] / [`Tokenizer::load_gguf`].
/// Direct mutation of the fields after construction is not supported.
pub struct Tokenizer {
    /// Vocabulary ordered by token id. `vocab[id]` is the textual form.
    pub vocab: Vec<String>,
    /// Ordered merge rules (rank = index in this Vec).
    pub merges: Vec<MergeRule>,
    /// Reverse lookup: textual token → id.
    pub token_to_id: HashMap<String, u32>,
    /// Forward lookup: id → textual token.
    pub id_to_token: HashMap<u32, String>,
    /// Special tokens (e.g. `<s>`, `</s>`) keyed by name.
    pub special_tokens: HashMap<String, u32>,
}

/// Errors returned by the tokenizer.
#[derive(Debug, Error)]
pub enum TokenizerError {
    /// The requested file does not exist on disk.
    #[error("tokenizer file not found")]
    FileNotFound,
    /// The file exists but could not be parsed as the expected format.
    #[error("invalid tokenizer format")]
    InvalidFormat,
    /// [`Tokenizer::decode`] was asked to decode an id that is not in the
    /// vocabulary.
    #[error("unknown token id")]
    UnknownToken,
    /// [`Tokenizer::encode`] produced more tokens than the configured
    /// maximum context length.
    #[error("encode overflow: token count exceeds context window")]
    EncodeOverflow,
}

// ─── Special tokens ─────────────────────────────────────────────────────────

/// The canonical special-token names recognized by this tokenizer.
///
/// Models that omit one or more of these simply won't have an entry in
/// [`Tokenizer::special_tokens`] — [`Tokenizer::special_token`] will then
/// return `None`.
pub const SPECIAL_TOKEN_NAMES: &[&str] = &["<s>", "</s>", "<unk>", "<pad>", "<bos>", "<eos>"];

// ─── Loaders ────────────────────────────────────────────────────────────────

impl Tokenizer {
    /// Load a HuggingFace-style `tokenizer.json` file.
    ///
    /// The v0 stub does not actually parse the HF schema — it constructs an
    /// empty tokenizer and returns `Ok`. The real parser is TODO(v1).
    pub fn load(path: &Path) -> Result<Self, TokenizerError> {
        if !path.exists() {
            return Err(TokenizerError::FileNotFound);
        }
        debug!(path = %path.display(), "loading tokenizer.json (v0 stub)");

        // TODO(v1): parse the HF tokenizer.json schema:
        //   - `model.vocab`      → vocab + token_to_id + id_to_token
        //   - `model.merges`     → merges (parsed as "a b" → new_id)
        //   - `added_tokens`     → special_tokens
        // For v0 we return an empty-but-valid tokenizer.
        Ok(Self::empty())
    }

    /// Load vocabulary and merges directly from a `.gguf` model file.
    ///
    /// GGUF stores the tokenizer metadata in key/value pairs at the head of
    /// the file (`tokenizer.ggml.model`, `tokenizer.ggml.tokens`,
    /// `tokenizer.ggml.merges`, ...). The v0 stub does not parse the GGUF
    /// container — it just verifies the file exists.
    pub fn load_gguf(path: &Path) -> Result<Self, TokenizerError> {
        if !path.exists() {
            return Err(TokenizerError::FileNotFound);
        }
        debug!(path = %path.display(), "loading tokenizer from GGUF (v0 stub)");

        // TODO(v1): mmap the GGUF file, walk the metadata table, and
        // populate the vocab / merges / special_tokens maps. The actual
        // GGUF parser lives in `llm/llama_cpp/bindings.rs`.
        Ok(Self::empty())
    }

    /// Construct an empty tokenizer with no vocabulary.
    ///
    /// Useful as a placeholder and as the base for the v0 stub loaders.
    fn empty() -> Self {
        Self {
            vocab: Vec::new(),
            merges: Vec::new(),
            token_to_id: HashMap::new(),
            id_to_token: HashMap::new(),
            special_tokens: HashMap::new(),
        }
    }

    /// Build the special-tokens map from the current vocab.
    ///
    /// Called by the real loaders (TODO(v1)) after the vocab is populated.
    /// Kept here so the v0 stub exposes a coherent internal API.
    fn index_special_tokens(&mut self) {
        for name in SPECIAL_TOKEN_NAMES {
            if let Some(&id) = self.token_to_id.get(*name) {
                self.special_tokens.insert((*name).to_string(), id);
            }
        }
    }
}

// ─── Encode / Decode ────────────────────────────────────────────────────────

impl Tokenizer {
    /// Encode `text` into a sequence of token ids.
    ///
    /// Algorithm (standard BPE):
    ///   1. Split `text` into UTF-8 byte sequences (one "word" per Unicode
    ///      codepoint group, mirroring the GPT-2 / Llama pre-tokenizer).
    ///   2. Each byte becomes its own initial token.
    ///   3. Repeatedly apply the lowest-rank applicable merge until no
    ///      merge rule fires.
    ///   4. Special tokens are matched verbatim *before* BPE runs and
    ///      emitted as their ids without further splitting.
    ///
    /// The v0 stub returns `Ok(Vec::new())` for empty input and
    /// [`TokenizerError::UnknownToken`] otherwise — the real encoder is
    /// TODO(v1).
    pub fn encode(&self, text: &str) -> Result<Vec<u32>, TokenizerError> {
        if text.is_empty() {
            return Ok(Vec::new());
        }
        if self.vocab.is_empty() {
            // v0 stub: no vocab loaded → cannot encode.
            // TODO(v1): replace with the full BPE implementation described
            // above. Until then we surface an error so callers can detect
            // the stub rather than silently producing wrong token ids.
            warn!("encode called on empty tokenizer (v0 stub)");
            return Err(TokenizerError::UnknownToken);
        }
        // TODO(v1): real BPE merge loop.
        Ok(Vec::new())
    }

    /// Decode a sequence of token ids back into text.
    ///
    /// Special tokens are rendered as their textual form (e.g. `<s>`).
    /// Unknown ids produce [`TokenizerError::UnknownToken`].
    pub fn decode(&self, ids: &[u32]) -> Result<String, TokenizerError> {
        if self.vocab.is_empty() {
            if ids.is_empty() {
                return Ok(String::new());
            }
            warn!("decode called on empty tokenizer (v0 stub)");
            return Err(TokenizerError::UnknownToken);
        }

        let mut out = String::with_capacity(ids.len() * 4);
        for &id in ids {
            match self.id_to_token.get(&id) {
                Some(tok) => out.push_str(tok),
                None => return Err(TokenizerError::UnknownToken),
            }
        }
        Ok(out)
    }
}

// ─── Accessors ──────────────────────────────────────────────────────────────

impl Tokenizer {
    /// Return the vocabulary size.
    ///
    /// Equivalent to `vocab.len() as u32` — the highest valid token id is
    /// `vocab_size() - 1`.
    pub fn vocab_size(&self) -> u32 {
        self.vocab.len() as u32
    }

    /// Look up the id of a named special token (e.g. `<s>`, `</s>`).
    ///
    /// Returns `None` if the token is not present in this model's
    /// vocabulary.
    pub fn special_token(&self, name: &str) -> Option<u32> {
        self.special_tokens.get(name).copied()
    }
}

// ─── Tests are intentionally omitted in v0 ──────────────────────────────────
// TODO(v1): add round-trip encode/decode tests against a fixture vocab.

// v0: stub implementation
