//! Cross-constraint consistency: GBNF (`intent.gbnf`) ↔ JSON Schema.
//!
//! Both constrain the same LLM-emitted field set. Enum literals and nested
//! candidate fields must agree so llama.cpp and vLLM/XGrammar stay aligned.

use std::collections::BTreeSet;

use cognos_intent_engine::llm_output_schema::{
    load_committed_schema, schema_object_property_names, serde_candidate_field_names,
    serde_top_level_field_names,
};
use cognos_intent_engine::schema_validator::{
    LLM_EMITTED_CANDIDATE_FIELDS, LLM_EMITTED_TOP_LEVEL, INJECTED_FIELDS,
};
use cognos_intent_engine::HttpLlamaBackend;

fn grammar() -> String {
    HttpLlamaBackend::grammar().to_string()
}

fn grammar_has_key(grammar: &str, key: &str) -> bool {
    grammar.contains(&format!("\\\"{key}\\\""))
}

fn grammar_rule_literals(grammar: &str, rule_name: &str) -> BTreeSet<String> {
    let needle = format!("{rule_name} ::=");
    let line = grammar
        .lines()
        .find(|l| l.trim_start().starts_with(&needle))
        .unwrap_or_else(|| panic!("grammar missing rule `{rule_name}`"));
    let mut out = BTreeSet::new();
    for segment in line.split('|') {
        let trimmed = segment.trim();
        if trimmed == "null" {
            out.insert("null".to_string());
            continue;
        }
        // GBNF string literals appear as \"value\" inside the rule line.
        if let Some(start) = trimmed.find("\\\"") {
            let rest = &trimmed[start + 2..];
            if let Some(end) = rest.find("\\\"") {
                out.insert(rest[..end].to_string());
            }
        }
    }
    out
}

fn json_string_enum(schema: &serde_json::Value, pointer: &str) -> BTreeSet<String> {
    schema
        .pointer(pointer)
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn grammar_and_json_schema_share_top_level_field_set() {
    let grammar = grammar();
    let schema = load_committed_schema();
    let json_fields = schema_object_property_names(&schema);
    let serde_fields = serde_top_level_field_names();

    let grammar_fields: BTreeSet<String> = LLM_EMITTED_TOP_LEVEL
        .iter()
        .filter(|key| grammar_has_key(&grammar, key))
        .map(|k| (*k).to_string())
        .collect();

    assert_eq!(
        grammar_fields, json_fields,
        "GBNF and JSON Schema top-level LLM field sets differ"
    );
    assert_eq!(
        grammar_fields, serde_fields,
        "GBNF and LlmEmittedIntent serde field sets differ"
    );
}

#[test]
fn grammar_and_json_schema_share_candidate_field_set() {
    let grammar = grammar();
    let schema = load_committed_schema();
    let candidate = schema
        .get("$defs")
        .and_then(|d| d.get("candidate_action"))
        .expect("$defs.candidate_action");
    let json_fields = schema_object_property_names(candidate);
    let serde_fields = serde_candidate_field_names();

    let grammar_fields: BTreeSet<String> = LLM_EMITTED_CANDIDATE_FIELDS
        .iter()
        .filter(|key| grammar_has_key(&grammar, key))
        .map(|k| (*k).to_string())
        .collect();

    assert_eq!(grammar_fields, json_fields);
    assert_eq!(grammar_fields, serde_fields);
}

#[test]
fn grammar_and_json_schema_exclude_injected_fields() {
    let grammar = grammar();
    let schema = load_committed_schema();
    let json_fields = schema_object_property_names(&schema);

    for key in INJECTED_FIELDS {
        assert!(
            !grammar_has_key(&grammar, key),
            "GBNF must not constrain injected field `{key}`"
        );
        assert!(
            !json_fields.contains(*key),
            "JSON Schema must not constrain injected field `{key}`"
        );
    }
}

#[test]
fn grammar_and_json_schema_goal_enums_match() {
    let grammar = grammar();
    let schema = load_committed_schema();
    let gbnf = grammar_rule_literals(&grammar, "goal-string");
    let json = json_string_enum(&schema, "/properties/goal/enum");
    assert_eq!(gbnf, json, "goal enum mismatch between GBNF and JSON Schema");
}

#[test]
fn grammar_and_json_schema_action_enums_match() {
    let grammar = grammar();
    let schema = load_committed_schema();
    let gbnf = grammar_rule_literals(&grammar, "action-string");
    let json = json_string_enum(
        &schema,
        "/$defs/candidate_action/properties/action/enum",
    );
    assert_eq!(
        gbnf, json,
        "candidate action enum mismatch between GBNF and JSON Schema"
    );
}

#[test]
fn grammar_and_json_schema_domain_enums_match() {
    let grammar = grammar();
    let schema = load_committed_schema();
    let mut gbnf = grammar_rule_literals(&grammar, "domain-value");
    // GBNF uses bare `null`; JSON Schema uses JSON null in the enum array.
    gbnf.remove("null");
    let mut json = json_string_enum(&schema, "/properties/domain/enum");
    json.remove("null");
    assert_eq!(
        gbnf, json,
        "domain enum mismatch between GBNF and JSON Schema (excluding null)"
    );
}
