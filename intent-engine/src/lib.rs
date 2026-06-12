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
pub mod disambiguation;
pub mod kv_cache;
pub mod parser;
pub mod schema_validator;
pub mod tokenizer;

pub use action_graph::{ActionGraph, ActionNode, GraphError};
pub use disambiguation::{DisambiguationEngine, DisambiguationRecord, ResolvedIntent};
pub use kv_cache::{CacheStats, IntentKvCache};
pub use parser::{
    parse_llm_output, validate, CandidateAction, InferenceBackend, IntentError,
    IntentParser, IntentSchema, ParseError, SessionContext, ValidationError,
};
