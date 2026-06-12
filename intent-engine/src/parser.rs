//! Intent parser — the single entry point for raw user input.
//!
//! Pipeline: raw input → tokenizer normalization → KV cache lookup →
//! LLM inference (injected seam) → schema parsing/validation → cache insert.
//!
//! The schema types themselves live in [`crate::schema_validator`]; this
//! module re-exports them so downstream modules (disambiguation, action
//! graph) depend on a single stable path.

pub use crate::schema_validator::{
    parse_llm_output, validate, CandidateAction, IntentSchema, ParseError,
    SessionContext, ValidationError,
};

use crate::kv_cache::{CacheStats, IntentKvCache};
use crate::tokenizer;

/// Error type for the full parse pipeline.
#[derive(Debug)]
pub enum IntentError {
    /// Input was empty after normalization.
    EmptyInput,
    /// The injected inference backend failed.
    Inference(String),
    /// LLM output failed schema parsing/validation.
    Schema(ParseError),
}

impl std::fmt::Display for IntentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyInput => write!(f, "intent input was empty"),
            Self::Inference(e) => write!(f, "inference backend error: {}", e),
            Self::Schema(e) => write!(f, "schema error: {}", e),
        }
    }
}

impl std::error::Error for IntentError {}

/// Inference backend seam. Phase 3 wires llama.cpp here; tests inject closures.
///
/// The backend receives normalized input and session context, and must
/// return raw JSON conforming to the intent schema in docs/SPEC.md.
/// Its output is never trusted: it always passes through
/// [`parse_llm_output`] before anything downstream sees it.
pub trait InferenceBackend {
    fn infer(
        &self,
        normalized_input: &str,
        session: &SessionContext,
    ) -> Result<String, String>;
}

impl<F> InferenceBackend for F
where
    F: Fn(&str, &SessionContext) -> Result<String, String>,
{
    fn infer(
        &self,
        normalized_input: &str,
        session: &SessionContext,
    ) -> Result<String, String> {
        self(normalized_input, session)
    }
}

/// The intent parser: owns the KV cache and drives the parse pipeline.
pub struct IntentParser {
    cache: IntentKvCache,
}

impl Default for IntentParser {
    fn default() -> Self {
        Self::new()
    }
}

impl IntentParser {
    pub fn new() -> Self {
        Self {
            cache: IntentKvCache::new(),
        }
    }

    /// Parse raw user input into a validated [`IntentSchema`].
    ///
    /// Cache hits skip inference entirely (<3ms target, per spec). Cache
    /// misses run the injected backend, then parse + validate its output.
    pub fn parse(
        &mut self,
        raw_input: &str,
        session: &SessionContext,
        backend: &dyn InferenceBackend,
    ) -> Result<IntentSchema, IntentError> {
        let normalized = tokenizer::normalize(raw_input);
        if normalized.is_empty() {
            return Err(IntentError::EmptyInput);
        }

        let key = IntentKvCache::make_key(&normalized, session);
        if let Some(mut hit) = self.cache.get(key) {
            // Re-stamp the cached schema with the live raw input so the
            // audit trail records what the user actually typed.
            hit.raw_input = raw_input.to_string();
            return Ok(hit);
        }

        let llm_json = backend
            .infer(&normalized, session)
            .map_err(IntentError::Inference)?;

        let schema = parse_llm_output(&llm_json).map_err(IntentError::Schema)?;
        self.cache.insert(key, schema.clone());
        Ok(schema)
    }

    /// Cache statistics, surfaced by `cognos memory audit`.
    pub fn cache_stats(&self) -> CacheStats {
        self.cache.stats()
    }

    /// Invalidate cached intents for a domain
    /// (e.g. after `cognos memory wipe --scope <domain>`).
    pub fn invalidate_domain(&mut self, domain: &str) {
        self.cache.invalidate_domain(domain);
    }

    /// Wipe the entire intent cache (e.g. after `cognos memory wipe`).
    pub fn invalidate_all(&mut self) {
        self.cache.invalidate_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> SessionContext {
        SessionContext {
            last_active_domain: Some("robotics".into()),
            last_active_files: vec![],
            current_time: "10:00".into(),
            time_since_last_session: None,
        }
    }

    fn llm_json(goal: &str, confidence: f32) -> String {
        format!(
            r#"{{
            "intent_id": "550e8400-e29b-41d4-a716-446655440000",
            "raw_input": "test",
            "goal": "{}",
            "confidence": {},
            "ambiguity_score": 0.3,
            "risk_estimate": 0.1,
            "hal_pre_score": 0.1,
            "required_context": [],
            "candidate_actions": [],
            "disambiguation_required": false,
            "session_context": {{"last_active_files": [], "current_time": "10:00"}},
            "escalate_to_cloud": false
        }}"#,
            goal, confidence
        )
    }

    #[test]
    fn parses_via_backend_on_cache_miss() {
        let mut parser = IntentParser::new();
        let backend =
            |_: &str, _: &SessionContext| Ok(llm_json("open_workspace", 0.9));
        let schema = parser
            .parse("Open my robotics work", &session(), &backend)
            .expect("parse should succeed");
        assert_eq!(schema.goal, "open_workspace");
    }

    #[test]
    fn second_parse_hits_cache() {
        use std::cell::Cell;
        let calls = Cell::new(0u32);
        let backend = |_: &str, _: &SessionContext| {
            calls.set(calls.get() + 1);
            Ok(llm_json("open_workspace", 0.9))
        };
        let mut parser = IntentParser::new();
        let s = session();
        parser.parse("open robotics", &s, &backend).expect("first");
        parser.parse("open robotics", &s, &backend).expect("second");
        assert_eq!(calls.get(), 1, "second parse must be served from cache");
    }

    #[test]
    fn empty_input_rejected() {
        let mut parser = IntentParser::new();
        let backend = |_: &str, _: &SessionContext| Ok(String::new());
        let err = parser.parse("  !!!  ", &session(), &backend);
        assert!(matches!(err, Err(IntentError::EmptyInput)));
    }

    #[test]
    fn schema_error_propagates() {
        let mut parser = IntentParser::new();
        let backend = |_: &str, _: &SessionContext| Ok("not json".to_string());
        let err = parser.parse("open thing", &session(), &backend);
        assert!(matches!(err, Err(IntentError::Schema(_))));
    }

    #[test]
    fn backend_failure_propagates() {
        let mut parser = IntentParser::new();
        let backend =
            |_: &str, _: &SessionContext| Err("model not loaded".to_string());
        let err = parser.parse("open thing", &session(), &backend);
        assert!(matches!(err, Err(IntentError::Inference(_))));
    }
}
