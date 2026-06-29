//! Raw FFI bindings to llama.cpp. Declares the C functions and types used by
//! the high-level engine. Bindings are minimal — only what COGNOS needs.
//!
//! This module is the *only* place in the crate that is allowed to call into
//! `libllama`. Everything above it works with safe Rust types. Each extern
//! function has a thin safe wrapper that performs the required pointer
//! validation and converts C return codes into [`BindingError`].

use std::ffi::CString;
use std::os::raw::{c_char, c_float, c_int, c_void};
use std::ptr::NonNull;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, error, warn};

// ─── Token Type ────────────────────────────────────────────────────────────

/// A llama.cpp token id. Re-exported by other modules under the same name.
pub type Token = i32;

// ─── Opaque C Types ────────────────────────────────────────────────────────

/// Opaque handle to a loaded llama.cpp model (`struct llama_model`).
#[repr(C)]
pub struct LlamaModel {
    _opaque: [u8; 0],
}

/// Opaque handle to a llama.cpp decoding context (`struct llama_context`).
#[repr(C)]
pub struct LlamaContext {
    _opaque: [u8; 0],
}

// ─── C Parameter Structs ───────────────────────────────────────────────────

/// Mirrors `struct llama_model_param` — parameters passed when loading a
/// GGUF file from disk.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct LlamaModelParams {
    /// Number of layers to offload to the GPU (`-1` = all).
    pub n_gpu_layers: c_int,
    /// Split mode for tensor splitting across multiple GPUs.
    pub split_mode: c_int,
    /// Main GPU to use when `split_mode == 1`.
    pub main_gpu: c_int,
    /// Fraction of each layer to offload per GPU (size = `n_gpu`).
    pub tensor_split: *const c_float,
    /// Whether to use mmap for loading weights.
    pub use_mmap: c_int,
    /// Whether to keep the model in system RAM (locked) for fast swap-in.
    pub use_mlock: c_int,
    /// Pointer to a `rope_scaling` overrides struct, or NULL.
    pub rope_scaling: *const c_void,
    /// Whether to load only the vocabulary (skip weights).
    pub vocab_only: c_int,
    /// Pointer to a `kv_overrides` struct, or NULL.
    pub kv_overrides: *const c_void,
}

impl Default for LlamaModelParams {
    fn default() -> Self {
        Self {
            n_gpu_layers: 0,
            split_mode: 0,
            main_gpu: 0,
            tensor_split: std::ptr::null(),
            use_mmap: 1,
            use_mlock: 0,
            rope_scaling: std::ptr::null(),
            vocab_only: 0,
            kv_overrides: std::ptr::null(),
        }
    }
}

/// Mirrors `struct llama_context_param` — parameters passed when creating a
/// decoding context bound to an already-loaded model.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct LlamaContextParams {
    /// Context window size in tokens.
    pub n_ctx: c_int,
    /// Maximum batch size for a single `llama_decode` call.
    pub n_batch: c_int,
    /// Number of threads used for prompt processing.
    pub n_threads: c_int,
    /// Number of threads used for generation.
    pub n_threads_batch: c_int,
    /// RoPE frequency base, or `0` for the model default.
    pub rope_freq_base: c_float,
    /// RoPE frequency scale, or `0.0` for the model default.
    pub rope_freq_scale: c_float,
    /// Number of KV cache slots (`-1` = derived from `n_ctx`).
    pub n_kv_slots: c_int,
    /// Whether the KV cache uses `f16` precision.
    pub f16_kv: c_int,
    /// Whether logits are materialized for every token (`0` = last only).
    pub logits_all: c_int,
    /// Embedding mode (`0` = generation, `1` = embeddings).
    pub embedding: c_int,
}

impl Default for LlamaContextParams {
    fn default() -> Self {
        Self {
            n_ctx: 4096,
            n_batch: 512,
            n_threads: 8,
            n_threads_batch: 8,
            rope_freq_base: 0.0,
            rope_freq_scale: 0.0,
            n_kv_slots: -1,
            f16_kv: 1,
            logits_all: 0,
            embedding: 0,
        }
    }
}

/// Mirrors `struct llama_batch` — a batch of tokens to submit to
/// `llama_decode`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct LlamaBatch {
    /// Number of tokens in this batch.
    pub n_tokens: c_int,
    /// Pointer to the token ids (`i32` each).
    pub token: *const Token,
    /// Pointer to per-token embedding input, or NULL.
    pub embd: *const c_float,
    /// Pointer to per-token position ids.
    pub pos: *const c_int,
    /// Number of sequences for each token (`n_seq_id` per token).
    pub n_seq_id: *const c_int,
    /// Sequence id matrix (`[n_tokens][n_seq_id[i]]`).
    pub seq_id: *const *const c_int,
    /// Pointer to per-token logits flags (`0` = don't compute, `1` = compute).
    pub logits: *const c_int,
}

// ─── Errors ────────────────────────────────────────────────────────────────

/// Errors returned by the FFI wrappers in this module.
#[derive(Debug, Error, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub enum BindingError {
    /// The supplied path could not be converted to a valid C string (e.g. it
    /// contained an interior NUL byte).
    #[error("invalid path: contains NUL byte")]
    InvalidPath,

    /// The C call returned a null pointer or negative return code.
    #[error("ffi call `{0}` failed")]
    CallFailed(String),

    /// The supplied text could not be tokenized (e.g. it was empty or the
    /// model vocabulary rejected it).
    #[error("tokenization failed")]
    TokenizeFailed,

    /// The supplied buffer was too small to hold the result.
    #[error("buffer too small")]
    BufferTooSmall,
}

// ─── Extern Declarations ───────────────────────────────────────────────────

extern "C" {
    /// Load a GGUF model from `path` using `params`. Returns a heap-allocated
    /// opaque model handle, or NULL on failure.
    fn llama_load_model_from_file(
        path: *const c_char,
        params: LlamaModelParams,
    ) -> *mut LlamaModel;

    /// Free a model previously returned by [`llama_load_model_from_file`].
    /// Passing NULL is a no-op.
    fn llama_free_model(model: *mut LlamaModel);

    /// Create a new decoding context bound to `model` using `params`.
    /// Returns NULL on failure.
    fn llama_new_context_with_model(
        model: *mut LlamaModel,
        params: LlamaContextParams,
    ) -> *mut LlamaContext;

    /// Free a context previously returned by
    /// [`llama_new_context_with_model`]. Passing NULL is a no-op.
    fn llama_free(ctx: *mut LlamaContext);

    /// Tokenize `text` into `tokens_buf`. Returns the number of tokens
    /// written, or a negative value on error. If `tokens_buf` is NULL, returns
    /// the number of tokens that *would* be written (sizing probe).
    fn llama_tokenize(
        model: *const LlamaModel,
        text: *const c_char,
        text_len: c_int,
        tokens_buf: *mut Token,
        n_max_tokens: c_int,
        add_special: c_int,
        parse_special: c_int,
    ) -> c_int;

    /// Submit `batch` to the context for decoding. Returns `0` on success or a
    /// negative value on error.
    fn llama_decode(ctx: *mut LlamaContext, batch: LlamaBatch) -> c_int;

    /// Returns a pointer to the logits buffer for the `i`-th token of the last
    /// `llama_decode` call. The buffer remains valid until the next decode.
    fn llama_get_logits_ith(ctx: *mut LlamaContext, i: c_int) -> *mut c_float;

    /// Convert a single token id to its UTF-8 piece, writing into `buf`.
    /// Returns the number of bytes written, or a negative value on error.
    fn llama_token_to_piece(
        model: *const LlamaModel,
        token: Token,
        buf: *mut c_char,
        length: c_int,
        lstrip: c_int,
        special: c_int,
    ) -> c_int;
}

// ─── Safe Wrappers ─────────────────────────────────────────────────────────

/// Load a model from `path` using `params`.
///
/// On success returns a [`NonNull`] owning the model; the caller is
/// responsible for freeing it via [`free_model`].
///
/// [`free_model`]: free_model
pub fn load_model(
    path: &str,
    params: LlamaModelParams,
) -> Result<NonNull<LlamaModel>, BindingError> {
    let c_path = CString::new(path).map_err(|_| {
        error!(path, "model path contains interior NUL byte");
        BindingError::InvalidPath
    })?;

    debug!(path, n_gpu_layers = params.n_gpu_layers, "loading model via FFI");

    // TODO(v1): link libllama and perform the actual call. For v0 we return
    // an error so callers fail fast rather than dereferencing a dangling
    // pointer.
    let _ = (c_path, params); // suppress unused warnings in v0 stub
    warn!("load_model() not implemented in v0 stub — libllama not linked");
    Err(BindingError::CallFailed(
        "load_model() not implemented in v0 stub".into(),
    ))
}

/// Free a model handle returned by [`load_model`].
///
/// # Safety
/// `model` must point to a valid `LlamaModel` previously returned by
/// [`load_model`] and must not already have been freed.
pub unsafe fn free_model(model: NonNull<LlamaModel>) {
    // SAFETY: the caller guarantees `model` is a valid, non-null handle
    // obtained from `llama_load_model_from_file` via `load_model`, and that
    // it has not been freed yet. `llama_free_model` tolerates the raw pointer
    // form and performs its own null check internally.
    unsafe {
        llama_free_model(model.as_ptr());
    }
}

/// Create a decoding context bound to `model`.
///
/// # Safety
/// `model` must be a valid, live `LlamaModel` handle.
pub unsafe fn new_context(
    model: NonNull<LlamaModel>,
    params: LlamaContextParams,
) -> Result<NonNull<LlamaContext>, BindingError> {
    // SAFETY: caller guarantees `model` is a valid live handle.
    let raw = unsafe { llama_new_context_with_model(model.as_ptr(), params) };
    NonNull::new(raw).ok_or_else(|| {
        error!("llama_new_context_with_model returned NULL");
        BindingError::CallFailed("llama_new_context_with_model".into())
    })
}

/// Free a context handle returned by [`new_context`].
///
/// # Safety
/// `ctx` must point to a valid `LlamaContext` previously returned by
/// [`new_context`] and must not already have been freed.
pub unsafe fn free_context(ctx: NonNull<LlamaContext>) {
    // SAFETY: caller guarantees `ctx` is a valid, non-null handle obtained
    // from `llama_new_context_with_model` via `new_context`, and that it has
    // not been freed yet.
    unsafe {
        llama_free(ctx.as_ptr());
    }
}

/// Tokenize `text` against `model`, returning a fresh `Vec<Token>`.
///
/// This is a safe wrapper that performs the two-pass sizing probe + real
/// tokenization dance required by `llama_tokenize`.
///
/// # Safety
/// `model` must be a valid, live `LlamaModel` handle.
pub unsafe fn safe_tokenize(
    model: NonNull<LlamaModel>,
    text: &str,
) -> Result<Vec<Token>, BindingError> {
    if text.is_empty() {
        debug!("safe_tokenize() called with empty text");
        return Err(BindingError::TokenizeFailed);
    }

    let c_text = CString::new(text).map_err(|_| {
        error!("text contains interior NUL byte");
        BindingError::TokenizeFailed
    })?;

    // SAFETY: caller guarantees `model` is valid; `c_text` is a valid C string
    // owned by us. First call passes NULL buffer to probe size.
    let n_required = unsafe {
        llama_tokenize(
            model.as_ptr(),
            c_text.as_ptr(),
            text.len() as c_int,
            std::ptr::null_mut(),
            0,
            0,
            0,
        )
    };
    if n_required < 0 {
        error!(n_required, "llama_tokenize sizing probe failed");
        return Err(BindingError::TokenizeFailed);
    }

    let mut buf = vec![0i32; n_required as usize + 1];
    // SAFETY: same model validity; `buf` has at least `n_required + 1` slots
    // so the call will not overflow.
    let n_written = unsafe {
        llama_tokenize(
            model.as_ptr(),
            c_text.as_ptr(),
            text.len() as c_int,
            buf.as_mut_ptr(),
            buf.len() as c_int,
            0,
            0,
        )
    };
    if n_written < 0 {
        error!(n_written, "llama_tokenize real call failed");
        return Err(BindingError::TokenizeFailed);
    }

    buf.truncate(n_written as usize);
    debug!(tokens = buf.len(), "tokenized text");
    Ok(buf)
}

/// Submit a batch of tokens for decoding.
///
/// # Safety
/// `ctx` must be a valid, live `LlamaContext` handle, and every pointer
/// inside `batch` must remain valid for the duration of the call.
pub unsafe fn decode(ctx: NonNull<LlamaContext>, batch: LlamaBatch) -> Result<(), BindingError> {
    // SAFETY: caller guarantees `ctx` is valid and that `batch`'s internal
    // pointers are live for the call.
    let rc = unsafe { llama_decode(ctx.as_ptr(), batch) };
    if rc != 0 {
        error!(rc, "llama_decode failed");
        return Err(BindingError::CallFailed("llama_decode".into()));
    }
    Ok(())
}

/// Borrow the logits for the `i`-th token of the most recent decode.
///
/// The returned slice has length equal to the model's vocabulary size. The
/// memory is owned by the context and remains valid only until the next
/// `llama_decode` call.
///
/// # Safety
/// `ctx` must be valid, `i` must be in range for the last batch, and the
/// caller must not hold the slice across a subsequent decode.
pub unsafe fn get_logits_ith(
    ctx: NonNull<LlamaContext>,
    i: c_int,
    vocab_size: usize,
) -> Result<&'static [f32], BindingError> {
    // SAFETY: caller guarantees `ctx` is valid and `i` is in range.
    let ptr = unsafe { llama_get_logits_ith(ctx.as_ptr(), i) };
    if ptr.is_null() {
        error!(i, "llama_get_logits_ith returned NULL");
        return Err(BindingError::CallFailed("llama_get_logits_ith".into()));
    }
    // SAFETY: caller guarantees the buffer is valid for `vocab_size` f32s and
    // will not be mutated for the lifetime of the returned reference (no
    // intervening decode). We extend the lifetime to `'static` to satisfy the
    // signature; the safety contract is documented above.
    Ok(unsafe { std::slice::from_raw_parts(ptr, vocab_size) })
}

/// Convert a single token id to its UTF-8 piece.
///
/// # Safety
/// `model` must be a valid, live `LlamaModel` handle.
pub unsafe fn token_to_piece(
    model: NonNull<LlamaModel>,
    token: Token,
) -> Result<String, BindingError> {
    let mut buf = vec![0u8; 16];
    // SAFETY: caller guarantees `model` is valid; `buf` is owned and has
    // `length` bytes available for writing.
    let n = unsafe {
        llama_token_to_piece(
            model.as_ptr(),
            token,
            buf.as_mut_ptr() as *mut c_char,
            buf.len() as c_int,
            0,
            0,
        )
    };
    if n < 0 {
        // Buffer too small — retry with a larger one. The negative return
        // value gives the required size on newer llama.cpp builds.
        let need = (-n) as usize + 1;
        buf.resize(need, 0);
        // SAFETY: as above, but now `buf.len()` is large enough.
        let n2 = unsafe {
            llama_token_to_piece(
                model.as_ptr(),
                token,
                buf.as_mut_ptr() as *mut c_char,
                buf.len() as c_int,
                0,
                0,
            )
        };
        if n2 < 0 {
            error!(token, n2, "llama_token_to_piece failed on retry");
            return Err(BindingError::CallFailed("llama_token_to_piece".into()));
        }
        buf.truncate(n2 as usize);
    } else {
        buf.truncate(n as usize);
    }

    String::from_utf8(buf).map_err(|_| BindingError::CallFailed("non-utf8 token piece".into()))
}

// v0: stub implementation
