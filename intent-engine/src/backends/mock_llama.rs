//! Deterministic mock LLM backend for dev/E2E/CI (`MOCK_LLM=1`).
//!
//! Returns schema-valid JSON keyed by normalized input — no network I/O.

use async_trait::async_trait;
use uuid::Uuid;

use crate::parser::InferenceBackend;
use crate::schema_validator::{CandidateAction, IntentSchema, SessionContext};
use crate::tokenizer;

/// Provenance stamp for mock responses (distinct from keyword fallback).
pub const MOCK_LLM_SOURCE: &str = "mock_llm";

/// Golden E2E inputs (see `intent-engine/tests/golden/`).
pub const GOLDEN_BENIGN_UTTERANCE: &str = "crée un dossier test dans /tmp";
pub const GOLDEN_APPROVAL_UTTERANCE: &str = "supprime le dossier système /boot";
pub const GOLDEN_CONFIRM_DELETE_UTTERANCE: &str = "supprime le fichier /etc/passwd";
pub const GOLDEN_HOME_DELETE_UTTERANCE: &str = "installe le paquet e2e-test-tool";
pub const GOLDEN_AMBIGUOUS_UTTERANCE: &str = "ouvre le projet robotique";

/// Mock inference backend — always succeeds with a predictable parse.
#[derive(Debug, Default, Clone, Copy)]
pub struct MockLlmBackend;

impl MockLlmBackend {
    pub fn new() -> Self {
        Self
    }

    fn build_schema(input: &str, session: &SessionContext) -> IntentSchema {
        let normalized = tokenizer::normalize(input);
        if normalized.contains("supprime") && normalized.contains("boot") {
            return Self::dangerous_delete_schema(input, session);
        }
        if normalized.contains("installe") && normalized.contains("paquet") {
            return Self::confirm_install_schema(input, session);
        }
        if normalized.contains("supprime") && normalized.contains("passwd") {
            return Self::confirm_delete_schema(input, session);
        }
        if normalized.contains("projet robotique") {
            return Self::ambiguous_schema(input, session);
        }
        Self::benign_mkdir_schema(input, session)
    }

    fn benign_mkdir_schema(input: &str, session: &SessionContext) -> IntentSchema {
        IntentSchema {
            intent_id: Uuid::new_v4(),
            raw_input: input.to_string(),
            goal: "create_dir".to_string(),
            domain: session.last_active_domain.clone(),
            confidence: 0.95,
            ambiguity_score: 0.1,
            risk_estimate: 0.2,
            required_context: Vec::new(),
            candidate_actions: vec![CandidateAction {
                action: "create_dir".to_string(),
                target: "/tmp/test".to_string(),
                confidence: 0.95,
                recency_score: 0.0,
            }],
            disambiguation_required: false,
            disambiguation_question: None,
            session_context: session.clone(),
            hal_pre_score: 0.2,
            escalate_to_cloud: false,
            source: Some(MOCK_LLM_SOURCE.to_string()),
        }
    }

    fn confirm_install_schema(input: &str, session: &SessionContext) -> IntentSchema {
        IntentSchema {
            intent_id: Uuid::new_v4(),
            raw_input: input.to_string(),
            goal: "pkg.install".to_string(),
            domain: session.last_active_domain.clone(),
            confidence: 0.86,
            ambiguity_score: 0.1,
            risk_estimate: 0.68,
            required_context: Vec::new(),
            candidate_actions: vec![CandidateAction {
                action: "install_package".to_string(),
                target: "e2e-test-tool".to_string(),
                confidence: 0.86,
                recency_score: 0.1,
            }],
            disambiguation_required: false,
            disambiguation_question: None,
            session_context: session.clone(),
            hal_pre_score: 0.68,
            escalate_to_cloud: false,
            source: Some(MOCK_LLM_SOURCE.to_string()),
        }
    }

    fn confirm_delete_schema(input: &str, session: &SessionContext) -> IntentSchema {
        IntentSchema {
            intent_id: Uuid::new_v4(),
            raw_input: input.to_string(),
            goal: "delete_path".to_string(),
            domain: Some("system".to_string()),
            confidence: 0.88,
            ambiguity_score: 0.1,
            risk_estimate: 0.72,
            required_context: Vec::new(),
            candidate_actions: vec![CandidateAction {
                action: "file.delete".to_string(),
                target: "/etc/passwd".to_string(),
                confidence: 0.88,
                recency_score: 0.2,
            }],
            disambiguation_required: false,
            disambiguation_question: None,
            session_context: session.clone(),
            hal_pre_score: 0.72,
            escalate_to_cloud: false,
            source: Some(MOCK_LLM_SOURCE.to_string()),
        }
    }

    fn dangerous_delete_schema(input: &str, session: &SessionContext) -> IntentSchema {
        IntentSchema {
            intent_id: Uuid::new_v4(),
            raw_input: input.to_string(),
            goal: "delete_path".to_string(),
            domain: Some("system".to_string()),
            confidence: 0.9,
            ambiguity_score: 0.1,
            risk_estimate: 0.97,
            required_context: Vec::new(),
            candidate_actions: vec![CandidateAction {
                action: "delete_files".to_string(),
                target: "/boot".to_string(),
                confidence: 0.9,
                recency_score: 0.15,
            }],
            disambiguation_required: false,
            disambiguation_question: None,
            session_context: session.clone(),
            hal_pre_score: 0.97,
            escalate_to_cloud: false,
            source: Some(MOCK_LLM_SOURCE.to_string()),
        }
    }

    fn ambiguous_schema(input: &str, session: &SessionContext) -> IntentSchema {
        IntentSchema {
            intent_id: Uuid::new_v4(),
            raw_input: input.to_string(),
            goal: "open_workspace".to_string(),
            domain: Some("robotique".to_string()),
            confidence: 0.68,
            ambiguity_score: 0.7,
            risk_estimate: 0.1,
            required_context: vec!["recent_project".to_string()],
            candidate_actions: vec![
                CandidateAction {
                    action: "open_files".to_string(),
                    target: "~/projets/robot-scolaire/bras.py".to_string(),
                    confidence: 0.66,
                    recency_score: 0.5,
                },
                CandidateAction {
                    action: "open_files".to_string(),
                    target: "~/projets/robot-perso/rover.py".to_string(),
                    confidence: 0.62,
                    recency_score: 0.48,
                },
            ],
            disambiguation_required: true,
            disambiguation_question: Some(
                "Le bras robotisé de l'école ou le rover perso ?".to_string(),
            ),
            session_context: session.clone(),
            hal_pre_score: 0.1,
            escalate_to_cloud: false,
            source: Some(MOCK_LLM_SOURCE.to_string()),
        }
    }
}

#[async_trait]
impl InferenceBackend for MockLlmBackend {
    async fn infer(
        &self,
        normalized_input: &str,
        session: &SessionContext,
    ) -> Result<String, String> {
        if normalized_input.trim().is_empty() {
            return Err("mock llm: empty input".to_string());
        }
        serde_json::to_string(&Self::build_schema(normalized_input, session))
            .map_err(|e| format!("mock llm: serialize failed: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> SessionContext {
        SessionContext {
            last_active_domain: None,
            last_active_files: vec![],
            current_time: "12:00".into(),
            time_since_last_session: None,
        }
    }

    #[tokio::test]
    async fn golden_benign_mkdir() {
        let json = MockLlmBackend::new()
            .infer(GOLDEN_BENIGN_UTTERANCE, &session())
            .await
            .unwrap();
        let schema: IntentSchema = serde_json::from_str(&json).unwrap();
        assert_eq!(schema.goal, "create_dir");
        assert_eq!(schema.source.as_deref(), Some(MOCK_LLM_SOURCE));
    }

    #[tokio::test]
    async fn golden_dangerous_delete() {
        let json = MockLlmBackend::new()
            .infer(GOLDEN_APPROVAL_UTTERANCE, &session())
            .await
            .unwrap();
        let schema: IntentSchema = serde_json::from_str(&json).unwrap();
        assert_eq!(schema.candidate_actions[0].target, "/boot");
    }

    #[tokio::test]
    async fn golden_ambiguous() {
        let json = MockLlmBackend::new()
            .infer(GOLDEN_AMBIGUOUS_UTTERANCE, &session())
            .await
            .unwrap();
        let schema: IntentSchema = serde_json::from_str(&json).unwrap();
        assert!(schema.disambiguation_required);
        assert_eq!(schema.candidate_actions.len(), 2);
    }
}
