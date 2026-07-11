//! Grammar ↔ schema field contract.
//!
//! The GBNF grammar constrains only **LLM-emitted** reasoning fields. System
//! metadata (`raw_input`, `intent_id`, `session_context`, `source`) is injected
//! in [`parse_llm_output_with_context`] and must never appear in the grammar.

use cognos_intent_engine::schema_validator::{
    INJECTED_FIELDS, LLM_EMITTED_CANDIDATE_FIELDS, LLM_EMITTED_TOP_LEVEL,
    SessionContext, parse_llm_output_with_context,
};
use cognos_intent_engine::HttpLlamaBackend;

fn grammar() -> String {
    HttpLlamaBackend::grammar().to_string()
}

fn grammar_has_key(grammar: &str, key: &str) -> bool {
    grammar.contains(&format!("\\\"{key}\\\""))
}

#[test]
fn grammar_contains_every_llm_emitted_field() {
    let grammar = grammar();

    for key in LLM_EMITTED_TOP_LEVEL {
        assert!(
            grammar_has_key(&grammar, key),
            "grammar is missing LLM-emitted top-level field `{key}`"
        );
    }
    for key in LLM_EMITTED_CANDIDATE_FIELDS {
        assert!(
            grammar_has_key(&grammar, key),
            "grammar is missing LLM-emitted candidate field `{key}`"
        );
    }
}

#[test]
fn grammar_excludes_injected_fields() {
    let grammar = grammar();

    for key in INJECTED_FIELDS {
        assert!(
            !grammar_has_key(&grammar, key),
            "grammar must not constrain injected field `{key}`"
        );
    }
}

#[test]
fn grammar_has_a_root_rule() {
    let grammar = grammar();
    assert!(
        grammar.lines().any(|l| l.trim_start().starts_with("root ::=")),
        "GBNF grammar must define a `root` rule"
    );
}

#[test]
fn parse_injects_known_fields_from_context() {
    let llm_only = r#"{
        "goal": "create_dir",
        "domain": "system",
        "confidence": 0.9,
        "ambiguity_score": 0.1,
        "risk_estimate": 0.0,
        "required_context": [],
        "candidate_actions": [],
        "disambiguation_required": false,
        "disambiguation_question": null,
        "hal_pre_score": 0.0,
        "escalate_to_cloud": false
    }"#;

    let session = SessionContext {
        last_active_domain: Some("system".into()),
        last_active_files: vec!["/tmp/foo".into()],
        current_time: "14:32".into(),
        time_since_last_session: Some("1h".into()),
    };

    let schema = parse_llm_output_with_context(
        llm_only,
        "crée un dossier test dans /tmp",
        &session,
    )
    .expect("LLM-only JSON parses with injection");

    assert_eq!(schema.raw_input, "crée un dossier test dans /tmp");
    assert_eq!(schema.session_context, session);
    assert_ne!(schema.intent_id, uuid::Uuid::nil());
    assert!(schema.source.is_none());
}
