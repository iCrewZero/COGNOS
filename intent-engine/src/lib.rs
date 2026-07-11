//! COGNOS Intent Engine — the single entry point for user commands.
//!
//! Pipeline (see docs/SPEC.md, "SYSTEM ARCHITECTURE"):
//! tokenizer → inference → schema parser → disambiguation → action graph.
//!
//! Output is a typed [`parser::IntentSchema`], resolved by the
//! [`disambiguation::DisambiguationEngine`] (one clarifying question,
//! maximum), then converted into an [`action_graph::ActionGraph`] that is
//! handed to HAL for risk scoring. No module bypasses this crate, and HAL
//! scores every action node before anything executes.

pub mod action_graph;
pub mod backends;
pub mod config;
pub mod disambiguation;
pub mod kv_cache;
pub mod llm_output_schema;
pub mod parser;
pub mod prompt;
pub mod schema_validator;
pub mod tokenizer;
pub mod unsupported_goals;

pub use action_graph::{ActionGraph, ActionNode, GraphError};
pub use backends::{FallbackBackend, HttpLlamaBackend, HttpVllmBackend, KeywordBackend, KEYWORD_FALLBACK_SOURCE, VLLM_SOURCE};
pub use config::{InferenceBackendKind, IntentConfig};
pub use prompt::{build_prompt, estimate_tokens, MAX_PROMPT_TOKENS};
pub use disambiguation::{DisambiguationEngine, DisambiguationRecord, ResolvedIntent};
pub use kv_cache::{CacheStats, IntentKvCache};
pub use parser::{
    parse_llm_output, parse_llm_output_with_context, parse_llm_output_with_input, validate,
    CandidateAction, InferenceBackend, IntentError, IntentParser, IntentSchema, ParseError,
    ParseResult, SessionContext, ValidationError,
};
pub use schema_validator::{
    INJECTED_FIELDS, LLM_EMITTED_CANDIDATE_FIELDS, LLM_EMITTED_TOP_LEVEL, InjectedIntentFields,
};
pub use unsupported_goals::{
    non_executable_reason, NON_EXECUTABLE_NETWORK_GOALS_V1, NON_EXECUTABLE_V1_MESSAGE,
};
pub use llm_output_schema::{
    load_committed_schema, LlmEmittedIntent, SCHEMA_REL_PATH, SCHEMA_VERSION,
};
