//! JSON Schema ↔ Rust LLM-emitted field contract.
//!
//! Mirrors [`grammar_schema.rs`] but for the committed JSON Schema artifact.
//! Field names are derived dynamically from [`LlmEmittedIntent`] via serde; if a
//! field is added to the struct, this test fails until the schema file is updated.

use std::collections::BTreeSet;

use cognos_intent_engine::llm_output_schema::{
    load_committed_schema, schema_object_property_names, schema_object_required_names,
    serde_candidate_field_names, serde_top_level_field_names, INJECTED_FIELDS, SCHEMA_VERSION,
};
use cognos_intent_engine::schema_validator::{
    LLM_EMITTED_CANDIDATE_FIELDS, LLM_EMITTED_TOP_LEVEL,
};

#[test]
fn committed_schema_version_matches_rust_constant() {
    let schema = load_committed_schema();
    let file_version = schema
        .get("x-cognos-schema-version")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| panic!("schema missing x-cognos-schema-version"));
    assert_eq!(file_version, SCHEMA_VERSION);
}

#[test]
fn serde_struct_covers_documented_llm_top_level_constants() {
    let serde_fields = serde_top_level_field_names();
    let documented: BTreeSet<String> = LLM_EMITTED_TOP_LEVEL
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    assert_eq!(
        serde_fields, documented,
        "LlmEmittedIntent serde fields must match LLM_EMITTED_TOP_LEVEL in schema_validator.rs"
    );
}

#[test]
fn serde_struct_covers_documented_candidate_constants() {
    let serde_fields = serde_candidate_field_names();
    let documented: BTreeSet<String> = LLM_EMITTED_CANDIDATE_FIELDS
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    assert_eq!(
        serde_fields, documented,
        "CandidateAction serde fields must match LLM_EMITTED_CANDIDATE_FIELDS"
    );
}

#[test]
fn json_schema_covers_every_llm_emitted_top_level_field() {
    let schema = load_committed_schema();
    let properties = schema_object_property_names(&schema);
    let required = schema_object_required_names(&schema);
    let serde_fields = serde_top_level_field_names();

    assert_eq!(
        properties, serde_fields,
        "schema properties must match LlmEmittedIntent fields exactly"
    );
    assert_eq!(
        required, serde_fields,
        "schema required[] must list every LlmEmittedIntent field"
    );

    for key in &serde_fields {
        assert!(
            properties.contains(key),
            "schema is missing LLM-emitted top-level field `{key}`"
        );
    }
}

#[test]
fn json_schema_covers_every_candidate_field() {
    let schema = load_committed_schema();
    let candidate = schema
        .get("$defs")
        .and_then(|d| d.get("candidate_action"))
        .expect("schema must define $defs.candidate_action");

    let properties = schema_object_property_names(candidate);
    let required = schema_object_required_names(candidate);
    let serde_fields = serde_candidate_field_names();

    assert_eq!(properties, serde_fields);
    assert_eq!(required, serde_fields);
}

#[test]
fn json_schema_excludes_injected_fields() {
    let schema = load_committed_schema();
    let properties = schema_object_property_names(&schema);

    for key in INJECTED_FIELDS {
        assert!(
            !properties.contains(*key),
            "schema must not constrain injected field `{key}`"
        );
    }

    let injected_meta = schema
        .get("x-cognos-injected-fields")
        .and_then(|v| v.as_array())
        .expect("schema must document x-cognos-injected-fields");
    let meta: BTreeSet<String> = injected_meta
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    let expected: BTreeSet<String> = INJECTED_FIELDS
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    assert_eq!(meta, expected);
}
