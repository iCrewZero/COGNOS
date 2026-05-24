/// Intent Schema Validator for COGNOS/OS.
///
/// The schema is the contract between the LLM and the rest of the system.
/// Malformed or unexpected output is rejected here — never passed downstream.
/// The parser must never panic. All errors return descriptive variants.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ─── Schema types ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateAction {
    pub action: String,
    pub target: String,
    pub confidence: f32,
    pub recency_score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionContext {
    pub last_active_domain: Option<String>,
    pub last_active_files: Vec<String>,
    pub current_time: String,
    pub time_since_last_session: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone)]
pub enum ValidationError {
    ConfidenceBelowZero,
    DisambiguationWithoutQuestion,
    CloudEscalationInconsistent,
}

/// Parse raw LLM JSON output into a typed IntentSchema.
/// Rejects malformed output with a descriptive error.
pub fn parse_llm_output(raw: &str) -> Result<IntentSchema, ParseError> {
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

    // escalate_to_cloud: auto-derive if not explicit
    let explicit_escalate = v["escalate_to_cloud"].as_bool();
    let required_context: Vec<String> = v["required_context"]
        .as_array()
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
        .unwrap_or_default();

    let escalate_to_cloud = explicit_escalate.unwrap_or(
        confidence < 0.75 || required_context.contains(&"cloud_reasoning".to_string())
    );

    let session_ctx = &v["session_context"];
    let session_context = SessionContext {
        last_active_domain: session_ctx["last_active_domain"].as_str().map(str::to_string),
        last_active_files: session_ctx["last_active_files"]
            .as_array()
            .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
            .unwrap_or_default(),
        current_time: session_ctx["current_time"].as_str()
            .unwrap_or("00:00").to_string(),
        time_since_last_session: session_ctx["time_since_last_session"]
            .as_str().map(str::to_string),
    };

    let schema = IntentSchema {
        intent_id: v["intent_id"].as_str()
            .and_then(|s| Uuid::parse_str(s).ok())
            .unwrap_or_else(Uuid::new_v4),
        raw_input: v["raw_input"].as_str().unwrap_or("").to_string(),
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
pub fn validate(schema: &IntentSchema) -> Result<(), Vec<ValidationError>> {
    let mut errors = Vec::new();
    if schema.confidence < 0.0 {
        errors.push(ValidationError::ConfidenceBelowZero);
    }
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
            }},
            "escalate_to_cloud": false
        }}"#, goal, confidence)
    }

    #[test]
    fn valid_json_parses() {
        let result = parse_llm_output(&minimal_json("open_workspace", 0.85));
        assert!(result.is_ok(), "{:?}", result);
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
