/// Intent Schema Validator for COGNOS/OS.
///
/// The schema is the contract between the LLM and the rest of the system.
/// Malformed or unexpected output is rejected here — never passed downstream.
/// The parser must never panic. All errors return descriptive variants.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Provenance stamp for keyword-classifier output. Must match
/// [`crate::backends::fallback::KEYWORD_FALLBACK_SOURCE`].
const KEYWORD_FALLBACK_SOURCE: &str = "keyword_fallback";

// ─── Schema types ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CandidateAction {
    pub action: String,
    pub target: String,
    pub confidence: f32,
    pub recency_score: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionContext {
    pub last_active_domain: Option<String>,
    pub last_active_files: Vec<String>,
    pub current_time: String,
    pub time_since_last_session: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IntentSchema {
    pub intent_id: Uuid,
    pub raw_input: String,
    pub goal: String,
    pub domain: Option<String>,
    /// 0.0–1.0
    pub confidence: f32,
    /// 0.0–1.0
    pub ambiguity_score: f32,
    /// 0.0–1.0
    pub risk_estimate: f32,
    pub required_context: Vec<String>,
    pub candidate_actions: Vec<CandidateAction>,
    pub disambiguation_required: bool,
    pub disambiguation_question: Option<String>,
    pub session_context: SessionContext,
    pub hal_pre_score: f32,
    pub escalate_to_cloud: bool,
    /// Provenance of this intent. Absent/`None` on the normal LLM path;
    /// `Some("keyword_fallback")` when produced by the degraded keyword
    /// classifier. System-set metadata — NOT part of the LLM output contract,
    /// so it is intentionally absent from the GBNF grammar.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

// ─── Parse errors ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum ParseError {
    InvalidJson(String),
    MissingField(String),
    EmptyGoal,
    FloatOutOfRange { field: String, value: f32 },
    DisambiguationMissingQuestion,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidJson(e)            => write!(f, "Invalid JSON: {}", e),
            Self::MissingField(name)        => write!(f, "Missing required field: '{}'", name),
            Self::EmptyGoal                 => write!(f, "goal field must not be empty"),
            Self::FloatOutOfRange{field,value} => write!(f, "Field '{}' value {} out of [0.0,1.0]", field, value),
            Self::DisambiguationMissingQuestion => write!(f, "disambiguation_required=true but no question provided"),
        }
    }
}

/// Validation errors for IntentSchema fields.
/// Owner: iCrewZero — added thiserror derive so errors are useful
/// when returned from validate(). Removed dead ConfidenceBelowZero
/// variant (unreachable: parse_f32 already enforces 0.0..=1.0).
#[derive(Debug, Clone, thiserror::Error)]
pub enum ValidationError {
    #[error("disambiguation required but no question provided")]
    DisambiguationWithoutQuestion,
    #[error("cloud escalation parameters are inconsistent")]
    CloudEscalationInconsistent,
}

/// Caller-known values stamped after inference (never LLM-emitted).
pub struct InjectedIntentFields<'a> {
    pub user_input: &'a str,
    pub session: &'a SessionContext,
}

/// Top-level schema fields the GBNF grammar constrains the model to emit.
pub const LLM_EMITTED_TOP_LEVEL: &[&str] = &[
    "goal",
    "domain",
    "confidence",
    "ambiguity_score",
    "risk_estimate",
    "required_context",
    "candidate_actions",
    "disambiguation_required",
    "disambiguation_question",
    "hal_pre_score",
    "escalate_to_cloud",
];

/// Nested fields inside `candidate_actions` objects (LLM-emitted).
pub const LLM_EMITTED_CANDIDATE_FIELDS: &[&str] =
    &["action", "target", "confidence", "recency_score"];

/// Fields always set by the parser/runtime — must not appear in the GBNF grammar.
pub const INJECTED_FIELDS: &[&str] = &[
    "raw_input",
    "intent_id",
    "session_context",
    "source",
];

/// Parse raw LLM JSON output into a typed IntentSchema.
/// Rejects malformed output with a descriptive error.
///
/// When no injection context is given, `raw_input`, `intent_id`, and
/// `session_context` are read from the JSON blob (golden fixtures / legacy tests).
pub fn parse_llm_output(raw: &str) -> Result<IntentSchema, ParseError> {
    parse_llm_output_with_injection(raw, None)
}

/// Inject only `raw_input`; `intent_id` and `session_context` fall back to JSON.
pub fn parse_llm_output_with_input(
    raw: &str,
    user_input: Option<&str>,
) -> Result<IntentSchema, ParseError> {
    match user_input {
        Some(text) => {
            let default_session = SessionContext {
                last_active_domain: None,
                last_active_files: vec![],
                current_time: "00:00".into(),
                time_since_last_session: None,
            };
            parse_llm_output_with_injection(
                raw,
                Some(InjectedIntentFields {
                    user_input: text,
                    session: &default_session,
                }),
            )
        }
        None => parse_llm_output_with_injection(raw, None),
    }
}

/// Production path: stamp `raw_input`, `intent_id`, and `session_context` from the caller.
pub fn parse_llm_output_with_context(
    raw: &str,
    user_input: &str,
    session: &SessionContext,
) -> Result<IntentSchema, ParseError> {
    parse_llm_output_with_injection(
        raw,
        Some(InjectedIntentFields {
            user_input,
            session,
        }),
    )
}

fn parse_llm_output_with_injection(
    raw: &str,
    injected: Option<InjectedIntentFields<'_>>,
) -> Result<IntentSchema, ParseError> {
    let v: serde_json::Value = serde_json::from_str(raw)
        .map_err(|e| {
            let snippet = raw.chars().take(200).collect::<String>();
            eprintln!("[intent_parser] Parse failure on: {}", snippet);
            ParseError::InvalidJson(e.to_string())
        })?;

    // goal — required, non-empty
    let goal = v["goal"].as_str()
        .ok_or_else(|| ParseError::MissingField("goal".into()))?
        .to_string();
    if goal.trim().is_empty() {
        return Err(ParseError::EmptyGoal);
    }

    // f32 fields — all must be in [0.0, 1.0]
    let confidence = parse_f32(&v, "confidence")?;
    let ambiguity_score = parse_f32(&v, "ambiguity_score")?;
    let risk_estimate = parse_f32(&v, "risk_estimate")?;
    let hal_pre_score = parse_f32(&v, "hal_pre_score")?;

    // candidate_actions — must be present as array
    let candidates_val = v["candidate_actions"]
        .as_array()
        .ok_or_else(|| ParseError::MissingField("candidate_actions".into()))?;

    let candidate_actions: Vec<CandidateAction> = candidates_val.iter()
        .filter_map(|ca| {
            Some(CandidateAction {
                action: ca["action"].as_str()?.to_string(),
                target: ca["target"].as_str()?.to_string(),
                confidence: ca["confidence"].as_f64()?.clamp(0.0, 1.0) as f32,
                recency_score: ca["recency_score"].as_f64()?.clamp(0.0, 1.0) as f32,
            })
        })
        .collect();

    // disambiguation_required + question consistency
    let disambiguation_required = v["disambiguation_required"].as_bool().unwrap_or(false);
    let disambiguation_question = v["disambiguation_question"].as_str().map(str::to_string);
    if disambiguation_required && disambiguation_question.is_none() {
        return Err(ParseError::DisambiguationMissingQuestion);
    }

    // escalate_to_cloud: auto-derive if not explicit. The confidence<0.75 rule
    // applies to the LLM path only; keyword fallback (offline registry) must
    // never escalate — cloud egress is blocked when the machine is offline.
    let explicit_escalate = v["escalate_to_cloud"].as_bool();
    let source = v["source"].as_str().map(str::to_string);
    let required_context: Vec<String> = v["required_context"]
        .as_array()
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
        .unwrap_or_default();

    let escalate_to_cloud = match explicit_escalate {
        Some(flag) => flag,
        None if source.as_deref() == Some(KEYWORD_FALLBACK_SOURCE) => false,
        None => {
            confidence < 0.75
                || required_context
                    .iter()
                    .any(|c| c == "cloud_reasoning")
        }
    };

    let session_context = if let Some(c) = &injected {
        c.session.clone()
    } else {
        let session_ctx = &v["session_context"];
        SessionContext {
            last_active_domain: session_ctx["last_active_domain"].as_str().map(str::to_string),
            last_active_files: session_ctx["last_active_files"]
                .as_array()
                .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
                .unwrap_or_default(),
            current_time: session_ctx["current_time"].as_str()
                .unwrap_or("00:00").to_string(),
            time_since_last_session: session_ctx["time_since_last_session"]
                .as_str().map(str::to_string),
        }
    };

    let intent_id = if injected.is_some() {
        Uuid::new_v4()
    } else {
        v["intent_id"].as_str()
            .and_then(|s| Uuid::parse_str(s).ok())
            .unwrap_or_else(Uuid::new_v4)
    };

    let raw_input = if let Some(c) = &injected {
        c.user_input.to_string()
    } else {
        v["raw_input"].as_str().unwrap_or("").to_string()
    };

    let schema = IntentSchema {
        intent_id,
        raw_input,
        goal,
        domain: v["domain"].as_str().map(str::to_string),
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
        source,
    };

    validate(&schema).map_err(|errs| {
        ParseError::InvalidJson(errs.iter().map(|e| format!("{:?}", e)).collect::<Vec<_>>().join(", "))
    })?;

    Ok(schema)
}

fn parse_f32(v: &serde_json::Value, field: &str) -> Result<f32, ParseError> {
    let val = v[field].as_f64()
        .ok_or_else(|| ParseError::MissingField(field.into()))? as f32;
    if !(0.0..=1.0).contains(&val) {
        return Err(ParseError::FloatOutOfRange { field: field.into(), value: val });
    }
    Ok(val)
}

/// Validate all invariants after parsing.
/// Owner: iCrewZero — removed dead confidence<0 check (parse_f32 already
/// enforces 0.0..=1.0, so it can never be negative here).
pub fn validate(schema: &IntentSchema) -> Result<(), Vec<ValidationError>> {
    let mut errors = Vec::new();
    if schema.disambiguation_required && schema.disambiguation_question.is_none() {
        errors.push(ValidationError::DisambiguationWithoutQuestion);
    }
    if errors.is_empty() { Ok(()) } else { Err(errors) }
}

// ─── KV Cache ─────────────────────────────────────────────────────────────────

use std::collections::HashMap;
use std::time::Instant;

const MAX_ENTRIES: usize = 100;
const MAX_AGE_SECS: u64 = 4 * 3600; // 4 hours
const MIN_CONFIDENCE_TO_CACHE: f32 = 0.80;

#[derive(Debug)]
struct CacheEntry {
    schema: IntentSchema,
    hit_count: u32,
    created_at: Instant,
    last_hit_at: Instant,
}

#[derive(Debug, Default)]
pub struct CacheStats {
    pub total_entries: usize,
    pub hit_rate: f32,
    pub oldest_entry_age_secs: u64,
    pub eviction_count: u64,
}

/// In-memory LRU cache for repeat intents.
/// Cache key is a hash of normalized input + domain + time-of-day bucket + day-of-week.
pub struct IntentKvCache {
    entries: HashMap<u64, CacheEntry>,
    hits: u64,
    misses: u64,
    evictions: u64,
}

impl IntentKvCache {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            hits: 0,
            misses: 0,
            evictions: 0,
        }
    }

    /// Compute a stable cache key from intent input and light context.
    pub fn make_key(raw_input: &str, session: &SessionContext) -> u64 {
        use std::hash::{Hash, Hasher};
        use std::collections::hash_map::DefaultHasher;
        use chrono::{Datelike, Timelike};

        let normalized = raw_input.trim().to_lowercase()
            .replace(|c: char| c.is_ascii_punctuation(), "");

        let hour: u8 = chrono::Local::now().hour() as u8;
        let hour_bucket = match hour {
            0..=5  => 0u8,
            6..=11 => 1,
            12..=17 => 2,
            _ => 3,
        };
        let day = chrono::Local::now().weekday() as u8;

        let mut h = DefaultHasher::new();
        normalized.hash(&mut h);
        session.last_active_domain.hash(&mut h);
        hour_bucket.hash(&mut h);
        day.hash(&mut h);
        h.finish()
    }

    /// Look up a cached IntentSchema. Returns None if not found or stale.
    pub fn get(&mut self, key: u64) -> Option<IntentSchema> {
        let entry = self.entries.get_mut(&key)?;
        if entry.created_at.elapsed().as_secs() > MAX_AGE_SECS {
            self.entries.remove(&key);
            self.misses += 1;
            return None;
        }
        entry.hit_count += 1;
        entry.last_hit_at = Instant::now();
        self.hits += 1;
        // Clone schema for return (lock held for minimum time)
        Some(entry.schema.clone())
    }

    /// Insert a schema. Only caches high-confidence results.
    pub fn insert(&mut self, key: u64, schema: IntentSchema) {
        if schema.confidence < MIN_CONFIDENCE_TO_CACHE {
            return;
        }
        if self.entries.len() >= MAX_ENTRIES {
            self.evict_lru();
        }
        self.entries.insert(key, CacheEntry {
            schema,
            hit_count: 0,
            created_at: Instant::now(),
            last_hit_at: Instant::now(),
        });
    }

    /// Invalidate all entries for a given domain.
    pub fn invalidate_domain(&mut self, domain: &str) {
        self.entries.retain(|_, e| {
            e.schema.domain.as_deref() != Some(domain)
        });
    }

    /// Wipe the entire cache.
    pub fn invalidate_all(&mut self) {
        self.entries.clear();
    }

    pub fn stats(&self) -> CacheStats {
        let total = self.hits + self.misses;
        let hit_rate = if total > 0 { self.hits as f32 / total as f32 } else { 0.0 };
        let oldest = self.entries.values()
            .map(|e| e.created_at.elapsed().as_secs())
            .max()
            .unwrap_or(0);
        CacheStats {
            total_entries: self.entries.len(),
            hit_rate,
            oldest_entry_age_secs: oldest,
            eviction_count: self.evictions,
        }
    }

    fn evict_lru(&mut self) {
        // Find the entry with the oldest last_hit_at
        if let Some(key) = self.entries.iter()
            .min_by_key(|(_, e)| e.last_hit_at)
            .map(|(k, _)| *k)
        {
            self.entries.remove(&key);
            self.evictions += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_json(goal: &str, confidence: f32) -> String {
        format!(r#"{{
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
            "session_context": {{
                "last_active_files": [],
                "current_time": "10:00"
            }}
        }}"#, goal, confidence)
    }

    #[test]
    fn valid_json_parses() {
        let result = parse_llm_output(&minimal_json("open_workspace", 0.85));
        assert!(result.is_ok(), "{:?}", result);
    }

    #[test]
    fn context_injects_session_and_intent_id() {
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
            last_active_files: vec!["/tmp".into()],
            current_time: "12:00".into(),
            time_since_last_session: Some("30m".into()),
        };
        let schema = parse_llm_output_with_context(llm_only, "mkdir /tmp/test", &session).unwrap();
        assert_eq!(schema.raw_input, "mkdir /tmp/test");
        assert_eq!(schema.session_context, session);
        assert_ne!(schema.intent_id, Uuid::nil());
    }

    #[test]
    fn user_input_stamped_as_raw_input() {
        let mut v: serde_json::Value = serde_json::from_str(&minimal_json("create_dir", 0.9)).unwrap();
        v.as_object_mut().unwrap().remove("raw_input");
        let json = v.to_string();
        let schema = parse_llm_output_with_input(&json, Some("crée un dossier test dans /tmp"))
            .expect("parses without LLM raw_input");
        assert_eq!(schema.raw_input, "crée un dossier test dans /tmp");
    }

    #[test]
    fn empty_goal_rejected() {
        let result = parse_llm_output(&minimal_json("", 0.85));
        assert_eq!(result, Err(ParseError::EmptyGoal));
    }

    #[test]
    fn out_of_range_confidence_rejected() {
        let json = minimal_json("test", 1.5);
        let result = parse_llm_output(&json);
        assert!(matches!(result, Err(ParseError::FloatOutOfRange { .. })));
    }

    #[test]
    fn disambiguation_without_question_rejected() {
        let json = r#"{
            "intent_id": "550e8400-e29b-41d4-a716-446655440000",
            "raw_input": "open robotics",
            "goal": "open_workspace",
            "confidence": 0.8,
            "ambiguity_score": 0.7,
            "risk_estimate": 0.1,
            "hal_pre_score": 0.1,
            "required_context": [],
            "candidate_actions": [],
            "disambiguation_required": true,
            "session_context": {"last_active_files": [], "current_time": "10:00"},
            "escalate_to_cloud": false
        }"#;
        let result = parse_llm_output(json);
        assert_eq!(result, Err(ParseError::DisambiguationMissingQuestion));
    }

    #[test]
    fn low_confidence_triggers_cloud_escalation() {
        let result = parse_llm_output(&minimal_json("open_workspace", 0.60)).unwrap();
        assert!(result.escalate_to_cloud);
    }

    #[test]
    fn keyword_fallback_source_never_auto_escalates() {
        // Low confidence but keyword provenance — offline path must not escalate
        // even when escalate_to_cloud is omitted from the JSON blob.
        let json = r#"{
            "intent_id": "550e8400-e29b-41d4-a716-446655440000",
            "raw_input": "delete temp",
            "goal": "file.delete",
            "confidence": 0.25,
            "ambiguity_score": 0.3,
            "risk_estimate": 0.6,
            "hal_pre_score": 0.6,
            "required_context": [],
            "candidate_actions": [],
            "disambiguation_required": false,
            "session_context": {"last_active_files": [], "current_time": "10:00"},
            "source": "keyword_fallback"
        }"#;
        let result = parse_llm_output(json).unwrap();
        assert!(!result.escalate_to_cloud);
        assert_eq!(result.source.as_deref(), Some("keyword_fallback"));
    }

    #[test]
    fn cache_hit_returns_schema() {
        let mut cache = IntentKvCache::new();
        let schema = parse_llm_output(&minimal_json("open_workspace", 0.90)).unwrap();
        let session = schema.session_context.clone();
        let key = IntentKvCache::make_key("open workspace", &session);
        cache.insert(key, schema.clone());
        let hit = cache.get(key);
        assert!(hit.is_some());
    }

    #[test]
    fn low_confidence_not_cached() {
        let mut cache = IntentKvCache::new();
        let schema = parse_llm_output(&minimal_json("open_workspace", 0.70)).unwrap();
        let session = schema.session_context.clone();
        let key = IntentKvCache::make_key("open workspace", &session);
        cache.insert(key, schema);
        assert!(cache.get(key).is_none());
    }
}
