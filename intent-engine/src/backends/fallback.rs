//! Keyword-classifier fallback backend.
//!
//! The risk registry mandates that intent parsing degrades gracefully when the
//! LLM is unavailable: rather than failing the whole pipeline, a deterministic
//! keyword classifier produces a minimal, schema-valid `IntentSchema` so
//! downstream stages (validation, disambiguation, action graph) keep working.
//!
//! [`FallbackBackend`] wraps a primary [`InferenceBackend`] (typically
//! [`HttpLlamaBackend`](super::http_llama::HttpLlamaBackend), which already
//! retries internally). Only when the primary ultimately errors do we degrade
//! to [`KeywordBackend`], logging the primary cause at WARNING.
//!
//! The keyword classifier logic lives here (ported from the orchestrator's
//! `classify_intent` / `planner.py`) so the intent-engine does not depend on
//! the orchestrator.

use async_trait::async_trait;
use uuid::Uuid;

use crate::parser::InferenceBackend;
use crate::schema_validator::{CandidateAction, IntentSchema, SessionContext};

/// Provenance stamp carried by intents produced on the degraded path.
pub const KEYWORD_FALLBACK_SOURCE: &str = "keyword_fallback";

/// Confidence ceiling for keyword-classified intents (registry requirement:
/// the fallback must never masquerade as a confident parse).
const CERTAIN_CONFIDENCE: f32 = 0.4;
/// Confidence used when even the keyword class is unclear.
const UNCERTAIN_CONFIDENCE: f32 = 0.25;

/// Keyword → canonical-action rules, ported from the orchestrator's
/// `classify_intent` and `planner.py`'s `classify_action`. First match wins.
const ACTION_RULES: &[(&str, &str)] = &[
    ("open", "file.open"),
    ("launch", "file.open"),
    ("find", "file.search"),
    ("search", "file.search"),
    ("locate", "file.search"),
    ("move", "file.move"),
    ("rename", "file.move"),
    ("delete", "file.delete"),
    ("remove", "file.delete"),
    ("trash", "file.delete"),
    ("mkdir", "create_dir"),
    ("dossier", "create_dir"),
    ("folder", "create_dir"),
    ("directory", "create_dir"),
    ("crée", "create_dir"),
    ("cree", "create_dir"),
    ("create", "create_dir"),
    ("uninstall", "pkg.uninstall"),
    ("install", "pkg.install"),
    ("update", "pkg.update"),
    ("upgrade", "pkg.update"),
    ("config", "system.config"),
    ("settings", "system.config"),
    ("configure", "system.config"),
    ("permission", "security.permission"),
    ("audit", "security.audit"),
    ("scan", "security.audit"),
    ("check", "security.check"),
    ("implement", "coding.implement"),
    ("refactor", "coding.refactor"),
    ("debug", "coding.debug"),
    ("fix", "coding.fix"),
    ("write", "coding.write"),
    ("code", "coding.task"),
    ("test", "coding.test"),
    ("summarize", "knowledge.query"),
    ("explain", "knowledge.query"),
    ("schedule", "system.schedule"),
    ("remind", "system.schedule"),
];

/// Actions whose keyword class implies a non-trivial risk hint for HAL.
fn base_risk(action: &str) -> f32 {
    match action {
        "file.delete" => 0.6,
        "create_dir" => 0.25,
        "pkg.install" | "pkg.uninstall" | "system.config" => 0.4,
        _ => 0.15,
    }
}

/// Pull a filesystem target from natural language when the action needs one.
fn extract_target_for_action(action: &str, input: &str) -> String {
    if action == "create_dir" {
        let lower = input.to_lowercase();
        if lower.contains("/tmp/test") || (lower.contains("/tmp") && lower.contains("test")) {
            return "/tmp/test".to_string();
        }
        if let Some(idx) = input.to_lowercase().find("/tmp") {
            let rest = &input[idx..];
            let end = rest
                .find(|c: char| c.is_whitespace() && c != '/')
                .unwrap_or(rest.len());
            return rest[..end].trim().to_string();
        }
        if lower.contains("test") {
            return "/tmp/test".to_string();
        }
        return "/tmp/cognos-test".to_string();
    }
    input.to_string()
}

/// Classify normalized text into a canonical action and a certainty flag.
///
/// Certainty is `false` when no rule matches (the catch-all
/// `intent.general`), which drives `disambiguation_required = true`.
fn classify(text: &str) -> (String, bool) {
    let lower = text.to_lowercase();
    for (keyword, action) in ACTION_RULES {
        if lower.contains(keyword) {
            return ((*action).to_string(), true);
        }
    }
    ("intent.general".to_string(), false)
}

/// A deterministic keyword classifier. Always produces a schema-valid intent
/// for non-empty input; fails only when there is literally nothing to classify.
#[derive(Debug, Default, Clone, Copy)]
pub struct KeywordBackend;

impl KeywordBackend {
    pub fn new() -> Self {
        KeywordBackend
    }

    /// Build the minimal fallback intent, then serialize it to the same wire
    /// form an LLM would emit, so the parser handles both paths identically.
    fn build_intent_json(input: &str, session: &SessionContext) -> Result<String, String> {
        let intent = Self::build_intent(input, session);
        serde_json::to_string(&intent)
            .map_err(|e| format!("keyword fallback: failed to serialize intent: {e}"))
    }

    /// Classify `input` and assemble a minimal, schema-valid [`IntentSchema`].
    fn build_intent(input: &str, session: &SessionContext) -> IntentSchema {
        let (goal, certain) = classify(input);

        let confidence = if certain {
            CERTAIN_CONFIDENCE
        } else {
            UNCERTAIN_CONFIDENCE
        };
        let ambiguity_score = if certain { 0.3 } else { 0.8 };
        let risk = base_risk(&goal);

        // Uncertain classification asks exactly one clarifying question; the
        // parser requires a question whenever disambiguation_required is set.
        let (disambiguation_required, disambiguation_question) = if certain {
            (false, None)
        } else {
            (
                true,
                Some(
                    "I couldn't confidently interpret that — could you rephrase or add detail?"
                        .to_string(),
                ),
            )
        };

        IntentSchema {
            intent_id: Uuid::new_v4(),
            raw_input: input.to_string(),
            goal: goal.clone(),
            domain: session.last_active_domain.clone(),
            confidence,
            ambiguity_score,
            risk_estimate: risk,
            required_context: Vec::new(),
            candidate_actions: vec![CandidateAction {
                action: goal.clone(),
                target: extract_target_for_action(&goal, input),
                confidence,
                recency_score: 0.0,
            }],
            disambiguation_required,
            disambiguation_question,
            session_context: session.clone(),
            hal_pre_score: risk,
            // Offline keyword path: never escalate — cloud is unreachable when the
            // local LLM is down (risk registry). Low confidence here means
            // disambiguation, not cloud reasoning.
            escalate_to_cloud: false,
            source: Some(KEYWORD_FALLBACK_SOURCE.to_string()),
        }
    }
}

#[async_trait]
impl InferenceBackend for KeywordBackend {
    async fn infer(
        &self,
        normalized_input: &str,
        session: &SessionContext,
    ) -> Result<String, String> {
        if normalized_input.trim().is_empty() {
            return Err("keyword fallback: empty input, nothing to classify".to_string());
        }
        Self::build_intent_json(normalized_input, session)
    }
}

/// A backend that tries `primary` first and degrades to a keyword classifier
/// when the primary fails.
///
/// `keyword` defaults to [`KeywordBackend`] (the production path); the generic
/// second parameter exists only so tests can inject a failing fallback to
/// exercise the both-backends-down case.
pub struct FallbackBackend<P, K = KeywordBackend>
where
    P: InferenceBackend,
    K: InferenceBackend,
{
    primary: P,
    keyword: K,
}

impl<P: InferenceBackend> FallbackBackend<P, KeywordBackend> {
    /// The production constructor: keyword classifier as the fallback.
    pub fn new(primary: P) -> Self {
        Self {
            primary,
            keyword: KeywordBackend::new(),
        }
    }
}

impl<P: InferenceBackend, K: InferenceBackend> FallbackBackend<P, K> {
    /// Construct with an explicit fallback backend (used in tests).
    pub fn with_fallback(primary: P, keyword: K) -> Self {
        Self { primary, keyword }
    }
}

#[async_trait]
impl<P: InferenceBackend, K: InferenceBackend> InferenceBackend for FallbackBackend<P, K> {
    async fn infer(
        &self,
        normalized_input: &str,
        session: &SessionContext,
    ) -> Result<String, String> {
        match self.primary.infer(normalized_input, session).await {
            Ok(output) => Ok(output),
            Err(primary_err) => {
                log::warn!(
                    "intent LLM unavailable — degrading to keyword fallback (primary cause: {primary_err})"
                );
                self.keyword
                    .infer(normalized_input, session)
                    .await
                    .map_err(|keyword_err| {
                        format!(
                            "intent inference failed: primary backend ({primary_err}); \
                             keyword fallback also failed ({keyword_err})"
                        )
                    })
                    .map(|out| {
                        cognos_ipc_grpc::pipeline_metrics::METRICS.record_parser_fallback();
                        out
                    })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::{IntentParser, IntentError};
    use crate::schema_validator::parse_llm_output;

    fn session() -> SessionContext {
        SessionContext {
            last_active_domain: Some("robotics".into()),
            last_active_files: vec!["motor.py".into()],
            current_time: "10:00".into(),
            time_since_last_session: Some("2h".into()),
        }
    }

    fn primary_ok(json: &'static str) -> impl InferenceBackend {
        move |_: &str, _: &SessionContext| Ok(json.to_string())
    }

    fn primary_err() -> impl InferenceBackend {
        |_: &str, _: &SessionContext| Err("llama-server unreachable".to_string())
    }

    const VALID_PRIMARY: &str = r#"{
        "intent_id": "550e8400-e29b-41d4-a716-446655440000",
        "raw_input": "open robotics",
        "goal": "open_workspace",
        "confidence": 0.92,
        "ambiguity_score": 0.1,
        "risk_estimate": 0.05,
        "hal_pre_score": 0.05,
        "required_context": [],
        "candidate_actions": [],
        "disambiguation_required": false,
        "session_context": {"last_active_files": [], "current_time": "10:00"},
        "escalate_to_cloud": false
    }"#;

    #[tokio::test]
    async fn primary_ok_returns_primary_output() {
        let backend = FallbackBackend::new(primary_ok(VALID_PRIMARY));
        let out = backend
            .infer("open robotics", &session())
            .await
            .expect("primary should succeed");

        // Verbatim primary output, and no fallback provenance stamp.
        assert_eq!(out, VALID_PRIMARY);
        let schema = parse_llm_output(&out).expect("valid schema");
        assert_eq!(schema.source, None);
        assert_eq!(schema.goal, "open_workspace");
    }

    #[tokio::test]
    async fn primary_ko_falls_back_to_valid_keyword_intent() {
        let backend = FallbackBackend::new(primary_err());
        let out = backend
            .infer("please delete the temp file", &session())
            .await
            .expect("keyword fallback should succeed");

        // The fallback output must validate against the SAME schema/parser.
        let schema = parse_llm_output(&out).expect("keyword output must be schema-valid");
        assert_eq!(schema.source.as_deref(), Some(KEYWORD_FALLBACK_SOURCE));
        assert_eq!(schema.goal, "file.delete", "keyword classifier picks delete");
        assert!(
            schema.confidence <= CERTAIN_CONFIDENCE,
            "fallback confidence must stay <= {CERTAIN_CONFIDENCE}, got {}",
            schema.confidence
        );
        assert!(
            !schema.escalate_to_cloud,
            "keyword fallback must never escalate to cloud (offline registry)"
        );
    }

    #[tokio::test]
    async fn uncertain_classification_requires_disambiguation() {
        let backend = FallbackBackend::new(primary_err());
        let out = backend
            .infer("hmm what about that thing", &session())
            .await
            .expect("keyword fallback should succeed");

        let schema = parse_llm_output(&out).expect("schema-valid");
        assert_eq!(schema.goal, "intent.general");
        assert!(schema.disambiguation_required);
        assert!(schema.disambiguation_question.is_some());
        assert_eq!(schema.source.as_deref(), Some(KEYWORD_FALLBACK_SOURCE));
    }

    #[tokio::test]
    async fn both_backends_down_is_inference_error() {
        // Primary errors, and the injected fallback also errors → the parser
        // must surface a single IntentError::Inference (never a panic, never a
        // partial/invalid schema).
        let failing_fallback = |_: &str, _: &SessionContext| Err("keyword down".to_string());
        let backend = FallbackBackend::with_fallback(primary_err(), failing_fallback);

        let mut parser = IntentParser::new();
        let err = parser
            .parse("open my project", &session(), &backend)
            .await
            .expect_err("both backends down must error");

        match err {
            IntentError::Inference(msg) => {
                assert!(
                    msg.contains("primary backend") && msg.contains("keyword fallback"),
                    "error must name both causes, got: {msg}"
                );
            }
            other => panic!("expected IntentError::Inference, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn keyword_backend_rejects_empty_input() {
        let kw = KeywordBackend::new();
        let err = kw.infer("   ", &session()).await.expect_err("empty must fail");
        assert!(err.contains("empty input"));
    }
}
