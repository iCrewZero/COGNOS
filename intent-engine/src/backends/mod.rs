//! Inference backends for the intent engine.
//!
//! The intent parser talks to any [`InferenceBackend`](crate::parser::InferenceBackend).
//! Tests inject closures; production uses [`HttpLlamaBackend`], which drives a
//! local llama-server over HTTP with a GBNF grammar. Production uses
//! [`HttpVllmBackend`] against a local `vllm serve` instance.

pub mod fallback;
pub mod http_llama;
pub mod http_vllm;
pub mod mock_llama;

pub use fallback::{FallbackBackend, KeywordBackend, KEYWORD_FALLBACK_SOURCE};
pub use http_llama::HttpLlamaBackend;
pub use http_vllm::{HttpVllmBackend, VLLM_SOURCE};
pub use mock_llama::MockLlmBackend;
