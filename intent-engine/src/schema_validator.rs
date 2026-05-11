// ============================================================================
// Intent Engine — Schema Parser
// COGNOS/OS
//
// The LLM output is untrusted input. This module is the firewall.
// Malformed, out-of-range, or logically inconsistent output is rejected here
// and NEVER passed downstream to agents or HAL.
//
// This parser must never panic. All failure paths return Result.
// ============================================================================

use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ----------------------------------------------------------------------------
// Output types: the typed schema that passes downstream
// ----------------------------------------------------------------------------

/// The fully validated, typed representation of a parsed user intent.
///
/// Every field has been range-checked, cross-validated, and derived from
/// the raw LLM JSON output. Downstream agents receive only this struct —
/// never the raw string.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntentSchema {
    /// Unique identifier for this intent instance
    pub intent_id: Uuid,

    /// The raw user input string, preserved for audit and disambiguation
    pub raw_input: String,

    /// The parsed high-level goal (e.g. "open_workspace", "install_package")
    /// Must not be empty.
    pub goal: String,

    /// Optional domain context (e.g. "robotics", "coding", "music")
    pub domain: Option<String>,

    /// Model confidence in the parsed intent ∈ [0.0, 1.0]
    pub confidence: f32,

    /// How ambiguous the intent is ∈ [0.0, 1.0]
    /// > 0.6 triggers the disambiguation protocol
    pub ambiguity_score: f32,

    /// Pre-computed risk estimate for HAL pre-screening ∈ [0.0, 1.0]
    pub risk_estimate: f32,

    /// Context keys the system needs to resolve this intent
    pub required_context: Vec<String>,

    /// Candidate actions ranked by confidence
    /// May be empty (intent was understood but no action was resolvable yet)
    pub candidate_actions: Vec<CandidateAction>,

    /// Whether the disambiguation protocol must run before proceeding
    pub disambiguation_required: bool,

    /// The single clarifying question to ask, if disambiguation is required
    /// Must be Some(_) when disambiguation_required is true
    pub disambiguation_question: Option<String>,

    /// Session state at the time this intent was received
    pub session_context: SessionContext,

    /// HAL pre-score: risk estimate before full agent processing ∈ [0.0, 1.0]
    pub hal_pre_score: f32,

    /// Whether this intent should be escalated to cloud inference
    /// True when confidence < 0.75 OR "cloud_reasoning" is in required_context
    pub escalate_to_cloud: bool,
}

/// A single candidate action the system might take to fulfill the intent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateAction {
    /// The action verb (e.g. "open_files", "install_package", "run_command")
    pub action: String,

    /// The target of the action (e.g. file path, package name, command)
    pub target: String,

    /// Model confidence that this is the correct action ∈ [0.0, 1.0]
    pub confidence: f32,

    /// How recently this target was accessed ∈ [0.0, 1.0]
    pub recency_score: f32,
}

/// Snapshot of the user's session state when the intent was received.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionContext {
    /// The domain the user was most recently active in
    pub last_active_domain: Option<String>,

    /// Files open or recently accessed in the current session
    pub last_active_files: Vec<String>,

    /// Wall-clock time when intent was received (ISO8601 or HH:MM)
    pub current_time: String,

    /// How long since the last session (human-readable, e.g. "2h")
    pub time_since_last_session: Option<String>,
}

// ----------------------------------------------------------------------------
// Raw deserialization types (untrusted LLM output)
// These are separate from the validated types to make the trust boundary clear.
// ----------------------------------------------------------------------------

/// Raw JSON structure from the LLM. All fields are unvalidated.
/// Deserialization succeeds even if values are out of range — validation
/// happens in the next step.
#[derive(Debug, Deserialize)]
struct RawIntentSchema {
    raw_input: Option<String>,
    goal: Option<String>,
    domain: Option<String>,
    confidence: Option<f32>,
    ambiguity_score: Option<f32>,
    risk_estimate: Option<f32>,
    required_context: Option<Vec<String>>,
    candidate_actions: Option<Vec<RawCandidateAction>>,
    disambiguation_required: Option<bool>,
    disambiguation_question: Option<String>,
    session_context: Option<RawSessionContext>,
    hal_pre_score: Option<f32>,
    /// LLM-provided escalation flag — intentionally ignored; derived fresh in parse_llm_output
    #[allow(dead_code)]
    escalate_to_cloud: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct RawCandidateAction {
    action: Option<String>,
    target: Option<String>,
    confidence: Option<f32>,
    recency_score: Option<f32>,
}

#[derive(Debug, Deserialize)]
struct RawSessionContext {
    last_active_domain: Option<String>,
    last_active_files: Option<Vec<String>>,
    current_time: Option<String>,
    time_since_last_session: Option<String>,
}

// ----------------------------------------------------------------------------
// Error types
// ----------------------------------------------------------------------------

/// Errors produced by the intent schema parser.
/// Each variant carries enough context for the caller to log the failure.
#[derive(Debug)]
pub enum ParseError {
    /// Input was not valid JSON
    InvalidJson(String),

    /// A required field was missing from the JSON
    MissingField(String),

    /// A float field was out of the valid [0.0, 1.0] range
    OutOfRange { field: String, value: f32 },

    /// A string field that must not be empty was empty
    EmptyField(String),

    /// Logical invariant violated (e.g. disambiguation_required=true but no question)
    InvariantViolation(String),

    /// A CandidateAction entry was malformed
    MalformedCandidateAction { index: usize, reason: String },
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::InvalidJson(msg) =>
                write!(f, "Invalid JSON: {}", msg),
            ParseError::MissingField(name) =>
                write!(f, "Missing required field: '{}'", name),
            ParseError::OutOfRange { field, value } =>
                write!(f, "Field '{}' out of range [0.0, 1.0]: {}", field, value),
            ParseError::EmptyField(name) =>
                write!(f, "Field '{}' must not be empty", name),
            ParseError::InvariantViolation(msg) =>
                write!(f, "Invariant violation: {}", msg),
            ParseError::MalformedCandidateAction { index, reason } =>
                write!(f, "CandidateAction[{}] malformed: {}", index, reason),
        }
    }
}

/// Errors produced by post-parse validation.
/// Multiple may be returned in a single call.
#[derive(Debug)]
pub enum ValidationError {
    /// Confidence below minimum acceptable level
    ConfidenceTooLow(f32),
    /// Score field is out of range even after clamping (should not happen — defensive)
    ScoreOutOfRange { field: String, value: f32 },
    /// Disambiguation invariant violated
    DisambiguationInconsistency(String),
    /// escalate_to_cloud should be true but is false
    EscalationFlagInconsistency(String),
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValidationError::ConfidenceTooLow(v) =>
                write!(f, "Confidence too low: {}", v),
            ValidationError::ScoreOutOfRange { field, value } =>
                write!(f, "Score out of range after clamping: {} = {}", field, value),
            ValidationError::DisambiguationInconsistency(msg) =>
                write!(f, "Disambiguation inconsistency: {}", msg),
            ValidationError::EscalationFlagInconsistency(msg) =>
                write!(f, "Escalation flag inconsistency: {}", msg),
        }
    }
}

// ----------------------------------------------------------------------------
// Float helpers
// ----------------------------------------------------------------------------

/// Validates that a float value is within [0.0, 1.0].
/// Returns ParseError::OutOfRange if not.
fn require_f32_in_range(field: &str, value: f32) -> Result<f32, ParseError> {
    if value < 0.0 || value > 1.0 {
        Err(ParseError::OutOfRange {
            field: field.to_string(),
            value,
        })
    } else {
        Ok(value)
    }
}

/// Clamps a float to [0.0, 1.0] defensively.
/// Used for fields where out-of-range is a clamp, not a hard rejection.
fn clamp_f32(value: f32) -> f32 {
    value.clamp(0.0, 1.0)
}

// ----------------------------------------------------------------------------
// Main parser
// ----------------------------------------------------------------------------

/// Parse raw LLM output (JSON string) into a validated IntentSchema.
///
/// The LLM output is untrusted. This function:
/// 1. Deserializes the JSON (rejects non-JSON input)
/// 2. Validates required fields are present
/// 3. Validates float fields are in [0.0, 1.0]
/// 4. Validates logical invariants
/// 5. Derives escalate_to_cloud from the spec rules
/// 6. Generates a fresh intent_id
///
/// On failure, logs the raw input (truncated to 200 chars) to stderr and
/// returns a descriptive ParseError.
///
/// This function never panics.
pub fn parse_llm_output(raw: &str) -> Result<IntentSchema, ParseError> {
    // Log raw input to stderr on any failure path
    // We define this closure early to avoid repeating the truncation logic
    let log_failure = |err: &ParseError| {
        let truncated: String = raw.chars().take(200).collect();
        eprintln!("[intent-parser] Parse failure: {} | raw_input={:?}", err, truncated);
    };

    // Step 1: JSON deserialization into raw (unvalidated) struct
    let raw_schema: RawIntentSchema = serde_json::from_str(raw).map_err(|e| {
        let err = ParseError::InvalidJson(e.to_string());
        log_failure(&err);
        err
    })?;

    // Step 2: Extract required string fields
    let raw_input = raw_schema
        .raw_input
        .ok_or_else(|| {
            let err = ParseError::MissingField("raw_input".to_string());
            log_failure(&err);
            err
        })?;

    let goal = raw_schema
        .goal
        .ok_or_else(|| {
            let err = ParseError::MissingField("goal".to_string());
            log_failure(&err);
            err
        })?;

    // goal must not be empty
    if goal.trim().is_empty() {
        let err = ParseError::EmptyField("goal".to_string());
        log_failure(&err);
        return Err(err);
    }

    // Step 3: Extract and validate required float fields
    // confidence: strict rejection on out-of-range
    let confidence_raw = raw_schema.confidence.ok_or_else(|| {
        let err = ParseError::MissingField("confidence".to_string());
        log_failure(&err);
        err
    })?;
    let confidence = require_f32_in_range("confidence", confidence_raw).map_err(|e| {
        log_failure(&e);
        e
    })?;

    // ambiguity_score, risk_estimate, hal_pre_score: clamp (not reject)
    let ambiguity_score = clamp_f32(raw_schema.ambiguity_score.unwrap_or(0.0));
    let risk_estimate   = clamp_f32(raw_schema.risk_estimate.unwrap_or(0.0));
    let hal_pre_score   = clamp_f32(raw_schema.hal_pre_score.unwrap_or(0.0));

    // Step 4: Extract list fields
    let required_context = raw_schema.required_context.unwrap_or_default();
    let candidate_actions = parse_candidate_actions(
        raw_schema.candidate_actions.unwrap_or_default(),
    ).map_err(|e| {
        log_failure(&e);
        e
    })?;

    // Step 5: Disambiguation invariant
    let disambiguation_required = raw_schema.disambiguation_required.unwrap_or(false);
    let disambiguation_question = raw_schema.disambiguation_question;

    if disambiguation_required && disambiguation_question.is_none() {
        let err = ParseError::InvariantViolation(
            "disambiguation_required=true but disambiguation_question is absent".to_string(),
        );
        log_failure(&err);
        return Err(err);
    }

    // Step 6: Session context
    let session_context = parse_session_context(raw_schema.session_context).map_err(|e| {
        log_failure(&e);
        e
    })?;

    // Step 7: Derive escalate_to_cloud per spec rules
    // true if: confidence < 0.75 OR "cloud_reasoning" in required_context
    let escalate_to_cloud = confidence < 0.75
        || required_context.iter().any(|c| c == "cloud_reasoning");

    // Step 8: Generate a fresh UUID for this intent
    let intent_id = Uuid::new_v4();

    let schema = IntentSchema {
        intent_id,
        raw_input,
        goal,
        domain: raw_schema.domain,
        confidence,
        ambiguity_score,
        risk_estimate,
        required_context,
        candidate_actions,
        disambiguation_required,
        disambiguation_question,
        session_context,
        hal_pre_score,
        escalate_to_cloud,
    };

    Ok(schema)
}

/// Parse and validate the list of candidate actions from raw LLM output.
fn parse_candidate_actions(
    raw_actions: Vec<RawCandidateAction>,
) -> Result<Vec<CandidateAction>, ParseError> {
    let mut actions = Vec::with_capacity(raw_actions.len());

    for (i, raw) in raw_actions.into_iter().enumerate() {
        let action = raw.action.ok_or_else(|| ParseError::MalformedCandidateAction {
            index: i,
            reason: "missing 'action' field".to_string(),
        })?;

        let target = raw.target.ok_or_else(|| ParseError::MalformedCandidateAction {
            index: i,
            reason: "missing 'target' field".to_string(),
        })?;

        let confidence = clamp_f32(raw.confidence.unwrap_or(0.0));
        let recency_score = clamp_f32(raw.recency_score.unwrap_or(0.0));

        actions.push(CandidateAction {
            action,
            target,
            confidence,
            recency_score,
        });
    }

    Ok(actions)
}

/// Parse session context from raw LLM output.
/// current_time is required; other fields are optional.
fn parse_session_context(
    raw: Option<RawSessionContext>,
) -> Result<SessionContext, ParseError> {
    let raw = raw.ok_or_else(|| ParseError::MissingField("session_context".to_string()))?;

    let current_time = raw
        .current_time
        .ok_or_else(|| ParseError::MissingField("session_context.current_time".to_string()))?;

    Ok(SessionContext {
        last_active_domain: raw.last_active_domain,
        last_active_files: raw.last_active_files.unwrap_or_default(),
        current_time,
        time_since_last_session: raw.time_since_last_session,
    })
}

// ----------------------------------------------------------------------------
// Post-parse validation
// ----------------------------------------------------------------------------

/// Validate all invariants of a parsed IntentSchema.
///
/// Called after parse_llm_output succeeds. Returns a list of all
/// validation errors found (not just the first one).
///
/// An empty Vec means the schema is fully valid.
pub fn validate(schema: &IntentSchema) -> Result<(), Vec<ValidationError>> {
    let mut errors: Vec<ValidationError> = Vec::new();

    // All float scores must still be in range after parsing
    for (name, value) in &[
        ("confidence",      schema.confidence),
        ("ambiguity_score", schema.ambiguity_score),
        ("risk_estimate",   schema.risk_estimate),
        ("hal_pre_score",   schema.hal_pre_score),
    ] {
        if *value < 0.0 || *value > 1.0 {
            errors.push(ValidationError::ScoreOutOfRange {
                field: name.to_string(),
                value: *value,
            });
        }
    }

    // Candidate action scores
    for (i, action) in schema.candidate_actions.iter().enumerate() {
        if action.confidence < 0.0 || action.confidence > 1.0 {
            errors.push(ValidationError::ScoreOutOfRange {
                field: format!("candidate_actions[{}].confidence", i),
                value: action.confidence,
            });
        }
        if action.recency_score < 0.0 || action.recency_score > 1.0 {
            errors.push(ValidationError::ScoreOutOfRange {
                field: format!("candidate_actions[{}].recency_score", i),
                value: action.recency_score,
            });
        }
    }

    // Disambiguation invariant (double-check after parsing)
    if schema.disambiguation_required && schema.disambiguation_question.is_none() {
        errors.push(ValidationError::DisambiguationInconsistency(
            "disambiguation_required=true but question is None after parsing".to_string(),
        ));
    }

    // Escalation flag consistency
    // If confidence < 0.75 or cloud_reasoning in required_context, flag must be true
    let should_escalate = schema.confidence < 0.75
        || schema.required_context.iter().any(|c| c == "cloud_reasoning");

    if should_escalate && !schema.escalate_to_cloud {
        errors.push(ValidationError::EscalationFlagInconsistency(
            format!(
                "escalate_to_cloud should be true (confidence={:.3}, required_context={:?})",
                schema.confidence, schema.required_context
            ),
        ));
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

// ----------------------------------------------------------------------------
// Tests
// ----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_json() -> &'static str {
        r#"{
            "raw_input": "open my robotics work",
            "goal": "open_workspace",
            "domain": "robotics",
            "confidence": 0.82,
            "ambiguity_score": 0.65,
            "risk_estimate": 0.14,
            "required_context": ["recent_project", "preferred_editor"],
            "candidate_actions": [
                {
                    "action": "open_files",
                    "target": "~/projects/robo-arm/motor.py",
                    "confidence": 0.71,
                    "recency_score": 0.9
                }
            ],
            "disambiguation_required": false,
            "session_context": {
                "last_active_domain": "robotics",
                "last_active_files": ["motor.py", "config.yaml"],
                "current_time": "14:32",
                "time_since_last_session": "2h"
            },
            "hal_pre_score": 0.14,
            "escalate_to_cloud": false
        }"#
    }

    #[test]
    fn test_valid_input_parses_successfully() {
        let result = parse_llm_output(valid_json());
        assert!(result.is_ok(), "Valid JSON should parse: {:?}", result.err());
    }

    #[test]
    fn test_invalid_json_rejected() {
        let result = parse_llm_output("this is not json {{{}");
        assert!(matches!(result, Err(ParseError::InvalidJson(_))));
    }

    #[test]
    fn test_empty_goal_rejected() {
        let json = r#"{
            "raw_input": "test",
            "goal": "",
            "confidence": 0.9,
            "ambiguity_score": 0.1,
            "risk_estimate": 0.1,
            "required_context": [],
            "candidate_actions": [],
            "disambiguation_required": false,
            "session_context": {"current_time": "10:00", "last_active_files": []},
            "hal_pre_score": 0.1,
            "escalate_to_cloud": false
        }"#;
        let result = parse_llm_output(json);
        assert!(matches!(result, Err(ParseError::EmptyField(_))));
    }

    #[test]
    fn test_confidence_out_of_range_rejected() {
        let json = r#"{
            "raw_input": "test",
            "goal": "test_goal",
            "confidence": 1.5,
            "ambiguity_score": 0.1,
            "risk_estimate": 0.1,
            "required_context": [],
            "candidate_actions": [],
            "disambiguation_required": false,
            "session_context": {"current_time": "10:00", "last_active_files": []},
            "hal_pre_score": 0.1,
            "escalate_to_cloud": false
        }"#;
        let result = parse_llm_output(json);
        assert!(
            matches!(result, Err(ParseError::OutOfRange { field, .. }) if field == "confidence")
        );
    }

    #[test]
    fn test_disambiguation_invariant_enforced() {
        let json = r#"{
            "raw_input": "open robotics",
            "goal": "open_workspace",
            "confidence": 0.82,
            "ambiguity_score": 0.7,
            "risk_estimate": 0.1,
            "required_context": [],
            "candidate_actions": [],
            "disambiguation_required": true,
            "session_context": {"current_time": "10:00", "last_active_files": []},
            "hal_pre_score": 0.1,
            "escalate_to_cloud": false
        }"#;
        let result = parse_llm_output(json);
        assert!(matches!(result, Err(ParseError::InvariantViolation(_))));
    }

    #[test]
    fn test_escalate_to_cloud_derived_correctly() {
        // Low confidence → must escalate
        let json = r#"{
            "raw_input": "do something complex",
            "goal": "complex_task",
            "confidence": 0.5,
            "ambiguity_score": 0.3,
            "risk_estimate": 0.2,
            "required_context": [],
            "candidate_actions": [],
            "disambiguation_required": false,
            "session_context": {"current_time": "10:00", "last_active_files": []},
            "hal_pre_score": 0.2,
            "escalate_to_cloud": false
        }"#;
        let schema = parse_llm_output(json).unwrap();
        assert!(schema.escalate_to_cloud, "Low confidence should force escalation");
    }

    #[test]
    fn test_cloud_reasoning_in_context_forces_escalation() {
        let json = r#"{
            "raw_input": "deep reasoning task",
            "goal": "reasoning_task",
            "confidence": 0.9,
            "ambiguity_score": 0.1,
            "risk_estimate": 0.1,
            "required_context": ["cloud_reasoning"],
            "candidate_actions": [],
            "disambiguation_required": false,
            "session_context": {"current_time": "10:00", "last_active_files": []},
            "hal_pre_score": 0.1,
            "escalate_to_cloud": false
        }"#;
        let schema = parse_llm_output(json).unwrap();
        assert!(schema.escalate_to_cloud, "cloud_reasoning context should force escalation");
    }

    #[test]
    fn test_parser_never_panics_on_garbage() {
        let long_garbage = "x".repeat(10_000);
        let garbage_inputs = vec![
            "",
            "null",
            "[]",
            "{}",
            r#"{"confidence": "not a number"}"#,
            r#"{"goal": null, "confidence": -999}"#,
            long_garbage.as_str(),
        ];
        for input in garbage_inputs {
            // Must not panic — result can be Err but not a panic
            let _ = parse_llm_output(input);
        }
    }

    #[test]
    fn test_validate_catches_inconsistency() {
        let mut schema = parse_llm_output(valid_json()).unwrap();
        // Manually break the escalation flag (parser derives it correctly,
        // but validate should catch manual tampering)
        schema.escalate_to_cloud = false;
        schema.confidence = 0.3; // below 0.75 — should escalate
        let result = validate(&schema);
        assert!(result.is_err());
    }

    #[test]
    fn test_intent_id_is_unique() {
        let s1 = parse_llm_output(valid_json()).unwrap();
        let s2 = parse_llm_output(valid_json()).unwrap();
        assert_ne!(s1.intent_id, s2.intent_id, "Each parse must produce a unique UUID");
    }
}
