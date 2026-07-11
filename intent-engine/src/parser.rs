//! Intent parser — the single entry point for raw user input.
//!
//! Pipeline: raw input → tokenizer normalization → KV cache lookup →
//! LLM inference (injected seam) → schema parsing/validation → cache insert.
//!
//! The schema types themselves live in [`crate::schema_validator`]; this
//! module re-exports them so downstream modules (disambiguation, action
//! graph) depend on a single stable path.

pub use crate::schema_validator::{
    parse_llm_output, parse_llm_output_with_context, parse_llm_output_with_input, validate,
    CandidateAction, IntentSchema, ParseError, SessionContext, ValidationError,
};

use crate::kv_cache::{CacheStats, IntentKvCache};
use crate::tokenizer;
use async_trait::async_trait;

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

/// Inference backend seam. Phase 3 wires llama-server here (see
/// [`crate::backends::http_llama`]); tests inject closures.
///
/// The backend receives normalized input and session context, and must
/// return raw JSON conforming to the intent schema in docs/SPEC.md.
/// Its output is never trusted: it always passes through
/// [`parse_llm_output`] before anything downstream sees it.
///
/// The trait is `async` (real backends do network I/O). It is kept
/// dyn-compatible via [`async_trait`] and requires `Send + Sync` so a backend
/// can be driven from a multi-threaded server handler (the intent-engine binary
/// serves `DispatchIntent` on a tonic runtime, whose futures must be `Send`).
#[async_trait]
pub trait InferenceBackend: Send + Sync {
    async fn infer(
        &self,
        normalized_input: &str,
        session: &SessionContext,
    ) -> Result<String, String>;
}

/// Adapter: any synchronous closure is a valid (trivially-async) backend.
///
/// This keeps test fixtures ergonomic — `|input, ctx| Ok(json)` — while the
/// production path (`HttpLlamaBackend`) implements the trait natively. The
/// closure must be `Send + Sync` (use `Arc<Atomic*>` for shared test counters
/// rather than `Cell`, which is `!Sync`).
#[async_trait]
impl<F> InferenceBackend for F
where
    F: Fn(&str, &SessionContext) -> Result<String, String> + Send + Sync,
{
    async fn infer(
        &self,
        normalized_input: &str,
        session: &SessionContext,
    ) -> Result<String, String> {
        self(normalized_input, session)
    }
}

/// Result of a successful parse, including cache provenance.
#[derive(Debug, Clone)]
pub struct ParseResult {
    pub schema: IntentSchema,
    pub cache_hit: bool,
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

    /// Deterministic intent for empty/whitespace-only input — no LLM call.
    fn await_input_for_empty(raw_input: &str, session: &SessionContext) -> IntentSchema {
        IntentSchema {
            intent_id: uuid::Uuid::new_v4(),
            raw_input: raw_input.to_string(),
            goal: "await_input".into(),
            domain: None,
            confidence: 0.1,
            ambiguity_score: 0.5,
            risk_estimate: 0.0,
            required_context: vec!["user_clarification".into()],
            candidate_actions: vec![],
            disambiguation_required: false,
            disambiguation_question: None,
            session_context: session.clone(),
            hal_pre_score: 0.0,
            escalate_to_cloud: true,
            source: None,
        }
    }

    /// Parse raw user input into a validated [`IntentSchema`].
    ///
    /// Cache hits skip inference entirely (<3ms target, per spec). Cache
    /// misses run the injected backend, then parse + validate its output.
    ///
    /// Async because real backends perform network I/O against llama-server.
    pub async fn parse(
        &mut self,
        raw_input: &str,
        session: &SessionContext,
        backend: &dyn InferenceBackend,
    ) -> Result<ParseResult, IntentError> {
        let normalized = tokenizer::normalize(raw_input);
        if normalized.is_empty() {
            return Ok(ParseResult {
                schema: Self::await_input_for_empty(raw_input, session),
                cache_hit: false,
            });
        }

        let key = IntentKvCache::make_key(&normalized, session);
        if let Some(mut hit) = self.cache.get(key) {
            cognos_ipc_grpc::pipeline_metrics::METRICS.record_parser_cache_hit();
            hit.raw_input = raw_input.to_string();
            return Ok(ParseResult {
                schema: hit,
                cache_hit: true,
            });
        }
        cognos_ipc_grpc::pipeline_metrics::METRICS.record_parser_cache_miss();

        let infer_started = std::time::Instant::now();
        let llm_json = backend
            .infer(&normalized, session)
            .await
            .map_err(IntentError::Inference)?;
        let _infer_ms = infer_started.elapsed().as_millis() as u64;

        let schema =
            parse_llm_output_with_context(&llm_json, raw_input, session).map_err(IntentError::Schema)?;
        self.cache.insert(key, schema.clone());
        Ok(ParseResult {
            schema,
            cache_hit: false,
        })
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

    #[tokio::test]
    async fn parses_via_backend_on_cache_miss() {
        let mut parser = IntentParser::new();
        let backend =
            |_: &str, _: &SessionContext| Ok(llm_json("open_workspace", 0.9));
        let schema = parser
            .parse("Open my robotics work", &session(), &backend)
            .await
            .expect("parse should succeed")
            .schema;
        assert_eq!(schema.goal, "open_workspace");
    }

    #[tokio::test]
    async fn second_parse_hits_cache() {
        use std::sync::atomic::{AtomicU32, Ordering};
        let calls = AtomicU32::new(0);
        let backend = |_: &str, _: &SessionContext| {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(llm_json("open_workspace", 0.9))
        };
        let mut parser = IntentParser::new();
        let s = session();
        parser.parse("open robotics", &s, &backend).await.expect("first");
        parser.parse("open robotics", &s, &backend).await.expect("second");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "second parse must be served from cache"
        );
    }

    #[tokio::test]
    async fn empty_input_short_circuits_without_backend() {
        use std::sync::atomic::{AtomicU32, Ordering};
        let calls = AtomicU32::new(0);
        let backend = |_: &str, _: &SessionContext| {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(String::new())
        };
        let mut parser = IntentParser::new();
        let session = SessionContext {
            last_active_domain: None,
            last_active_files: vec![],
            current_time: "10:00".into(),
            time_since_last_session: None,
        };
        let result = parser
            .parse("   \t  ", &session, &backend)
            .await
            .expect("empty input returns await_input");
        assert_eq!(result.schema.goal, "await_input");
        assert_eq!(result.schema.raw_input, "   \t  ");
        assert!(result.schema.candidate_actions.is_empty());
        assert!(result.schema.escalate_to_cloud);
        assert_eq!(calls.load(Ordering::SeqCst), 0, "backend must not run");
    }

    #[tokio::test]
    async fn whitespace_only_punctuation_short_circuits() {
        let mut parser = IntentParser::new();
        let backend = |_: &str, _: &SessionContext| {
            panic!("backend must not be called for normalized-empty input")
        };
        let result = parser
            .parse("  !!!  ", &session(), &backend)
            .await
            .expect("punctuation-only input short-circuits");
        assert_eq!(result.schema.goal, "await_input");
    }

    #[tokio::test]
    async fn schema_error_propagates() {
        let mut parser = IntentParser::new();
        let backend = |_: &str, _: &SessionContext| Ok("not json".to_string());
        let err = parser.parse("open thing", &session(), &backend).await;
        assert!(matches!(err, Err(IntentError::Schema(_))));
    }

    #[tokio::test]
    async fn backend_failure_propagates() {
        let mut parser = IntentParser::new();
        let backend =
            |_: &str, _: &SessionContext| Err("model not loaded".to_string());
        let err = parser.parse("open thing", &session(), &backend).await;
        assert!(matches!(err, Err(IntentError::Inference(_))));
    }
}
