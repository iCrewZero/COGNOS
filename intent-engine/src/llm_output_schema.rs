//! Versioned JSON Schema contract for **LLM-emitted** [`IntentSchema`] fields.
//!
//! Full [`IntentSchema`] also carries system-injected metadata
//! (`raw_input`, `intent_id`, `session_context`, `source`) that must never
//! appear in constrained LLM output (GBNF or JSON Schema).
//!
//! Committed artifact: [`SCHEMA_REL_PATH`] (consumed by vLLM/XGrammar POC and
//! CI coverage tests).

use std::collections::BTreeSet;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::schema_validator::CandidateAction;

/// Semantic version of the committed JSON Schema file (bump when the contract changes).
pub const SCHEMA_VERSION: &str = "2.0.0";

/// Path to the versioned schema, relative to the `intent-engine` crate root.
pub const SCHEMA_REL_PATH: &str = "schema/intent-llm-output.schema.json";

/// Fields stamped by the parser/runtime — excluded from LLM output constraints.
pub const INJECTED_FIELDS: &[&str] = &[
    "raw_input",
    "intent_id",
    "session_context",
    "source",
];

/// LLM-emitted subset of [`IntentSchema`](crate::IntentSchema).
///
/// Field names are the single source of truth checked by
/// `tests/json_schema_coverage.rs` against the committed JSON Schema.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmEmittedIntent {
    pub goal: String,
    pub domain: Option<String>,
    pub confidence: f32,
    pub ambiguity_score: f32,
    pub risk_estimate: f32,
    pub required_context: Vec<String>,
    pub candidate_actions: Vec<CandidateAction>,
    pub disambiguation_required: bool,
    pub disambiguation_question: Option<String>,
    pub hal_pre_score: f32,
    pub escalate_to_cloud: bool,
}

impl Default for LlmEmittedIntent {
    fn default() -> Self {
        Self {
            goal: String::new(),
            domain: None,
            confidence: 0.0,
            ambiguity_score: 0.0,
            risk_estimate: 0.0,
            required_context: Vec::new(),
            candidate_actions: Vec::new(),
            disambiguation_required: false,
            disambiguation_question: None,
            hal_pre_score: 0.0,
            escalate_to_cloud: false,
        }
    }
}

/// Absolute path to the committed schema file on disk.
pub fn schema_file_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(SCHEMA_REL_PATH)
}

/// Load the committed, versioned JSON Schema artifact.
pub fn load_committed_schema() -> serde_json::Value {
    let raw = std::fs::read_to_string(schema_file_path())
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", SCHEMA_REL_PATH));
    serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("invalid JSON in {}: {e}", SCHEMA_REL_PATH))
}

/// Top-level property names derived from [`LlmEmittedIntent`] via serde serialization.
pub fn serde_top_level_field_names() -> BTreeSet<String> {
    serde_object_field_names(&LlmEmittedIntent::default())
}

/// Nested `candidate_actions[]` property names derived from [`CandidateAction`] via serde.
pub fn serde_candidate_field_names() -> BTreeSet<String> {
    serde_object_field_names(&CandidateAction {
        action: String::new(),
        target: String::new(),
        confidence: 0.0,
        recency_score: 0.0,
    })
}

/// Property keys on a JSON Schema `object` node (`properties` map).
pub fn schema_object_property_names(node: &serde_json::Value) -> BTreeSet<String> {
    node.get("properties")
        .and_then(|p| p.as_object())
        .map(|obj| obj.keys().cloned().collect())
        .unwrap_or_default()
}

/// Required field names on a JSON Schema `object` node.
pub fn schema_object_required_names(node: &serde_json::Value) -> BTreeSet<String> {
    node.get("required")
        .and_then(|r| r.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn serde_object_field_names<T: Serialize>(value: &T) -> BTreeSet<String> {
    let json = serde_json::to_value(value).expect("serde value");
    json.as_object()
        .expect("struct serializes to JSON object")
        .keys()
        .cloned()
        .collect()
}
