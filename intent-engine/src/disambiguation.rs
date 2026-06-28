/// Disambiguation engine for COGNOS/OS intent resolution.
///
/// When ambiguity_score > 0.6, this module selects and asks exactly one
/// clarifying question, learns from the answer, and never repeats itself.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::hash::{Hash, Hasher};
use std::collections::hash_map::DefaultHasher;
use uuid::Uuid;

use crate::parser::{IntentSchema, CandidateAction};

// ─── Types ────────────────────────────────────────────────────────────────────

/// A resolved intent after disambiguation has selected one candidate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedIntent {
    pub intent_id: Uuid,
    pub selected_action: CandidateAction,
    pub was_disambiguated: bool,
    pub disambiguation_question: Option<String>,
    pub user_response: Option<String>,
}

/// The kind of difference between candidates that drives question selection.
#[derive(Debug, Clone, PartialEq)]
enum CandidateDifferenceKind {
    Domain,
    CompletionState,
    NameOnly,
    Recency, // No question needed — pick highest recency
}

/// A record of one disambiguation event, persisted for learning.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisambiguationRecord {
    pub context_hash: u64,
    pub goal: String,
    pub domain: Option<String>,
    pub question_asked: Option<String>,
    pub chosen_target: String,
    pub chosen_at: chrono::DateTime<chrono::Utc>,
}

/// Persisted learning data: maps context hash → chosen candidate target.
#[derive(Debug, Default, Serialize, Deserialize)]
struct DisambiguationMemory {
    records: Vec<DisambiguationRecord>,
    /// key: context_hash, value: target path/string of the learned choice
    learned_choices: HashMap<u64, String>,
}

// ─── Engine ───────────────────────────────────────────────────────────────────

/// Disambiguation engine. Owns learned patterns and drives Q&A resolution.
pub struct DisambiguationEngine {
    memory: DisambiguationMemory,
    memory_path: PathBuf,
}

impl DisambiguationEngine {
    /// Load or create the disambiguation engine from persisted state.
    pub fn load() -> Self {
        let memory_path = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join(".cognos/memory/disambiguation.json");

        let memory = if memory_path.exists() {
            std::fs::read_to_string(&memory_path)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default()
        } else {
            DisambiguationMemory::default()
        };

        Self { memory, memory_path }
    }

    /// Core entry point. Returns the question to ask, or None if we can
    /// auto-resolve (via recency or learned choice).
    ///
    /// Returns `(question, candidates_summary)` when a question is needed,
    /// or `None` when we can pick automatically.
    pub fn select_question(
        &self,
        schema: &IntentSchema,
    ) -> Option<(String, Vec<String>)> {
        if schema.candidate_actions.len() < 2 {
            return None;
        }

        // Check learned choices first — skip question entirely.
        let ctx_hash = self.context_hash(schema);
        if self.memory.learned_choices.contains_key(&ctx_hash) {
            return None;
        }

        let kind = self.classify_difference(&schema.candidate_actions);

        match kind {
            CandidateDifferenceKind::Recency => None, // auto-pick, no question
            CandidateDifferenceKind::Domain => {
                let domains: Vec<String> = schema
                    .candidate_actions
                    .iter()
                    .map(|c| self.extract_domain_label(&c.target))
                    .collect();
                let opts = domains.join(" or ");
                Some((format!("Which domain — {}?", opts), domains))
            }
            CandidateDifferenceKind::CompletionState => {
                let opts = vec![
                    "the finished one".to_string(),
                    "the one left mid-way".to_string(),
                ];
                Some((
                    "The finished version or the one you left mid-way?".to_string(),
                    opts,
                ))
            }
            CandidateDifferenceKind::NameOnly => {
                // Show short names from targets
                let names: Vec<String> = schema
                    .candidate_actions
                    .iter()
                    .take(4) // never show more than 4 options
                    .map(|c| self.short_name(&c.target))
                    .collect();
                let opts_str = names.join(" or ");
                Some((format!("{}?", opts_str), names))
            }
        }
    }

    /// Resolve a schema to a single action after the user responds to a question.
    /// Falls back to highest-confidence candidate if still ambiguous.
    pub fn resolve(
        &mut self,
        schema: IntentSchema,
        user_response: &str,
    ) -> ResolvedIntent {
        let ctx_hash = self.context_hash(&schema);

        // If we have a learned choice and the response matches it, use it.
        // Otherwise match response text against candidate targets.
        let selected = self.match_response(&schema.candidate_actions, user_response)
            .unwrap_or_else(|| {
                // Fall back to highest confidence
                schema
                    .candidate_actions
                    .iter()
                    .max_by(|a, b| a.confidence.partial_cmp(&b.confidence).unwrap_or(std::cmp::Ordering::Equal))
                    .cloned()
                    .unwrap()
            });

        // Learn this choice
        self.memory.learned_choices.insert(ctx_hash, selected.target.clone());
        self.memory.records.push(DisambiguationRecord {
            context_hash: ctx_hash,
            goal: schema.goal.clone(),
            domain: schema.domain.clone(),
            question_asked: schema.disambiguation_question.clone(),
            chosen_target: selected.target.clone(),
            chosen_at: chrono::Utc::now(),
        });

        let _ = self.persist();

        ResolvedIntent {
            intent_id: schema.intent_id,
            selected_action: selected,
            was_disambiguated: true,
            disambiguation_question: schema.disambiguation_question,
            user_response: Some(user_response.to_string()),
        }
    }

    /// Auto-resolve using a learned pattern or highest-recency pick.
    /// Returns Some(resolved) when we can skip the question, None otherwise.
    pub fn try_auto_resolve(&self, schema: &IntentSchema) -> Option<ResolvedIntent> {
        let ctx_hash = self.context_hash(schema);

        // Learned choice?
        if let Some(learned_target) = self.memory.learned_choices.get(&ctx_hash) {
            let selected = schema
                .candidate_actions
                .iter()
                .find(|c| &c.target == learned_target)
                .or_else(|| schema.candidate_actions.first())?
                .clone();

            return Some(ResolvedIntent {
                intent_id: schema.intent_id,
                selected_action: selected,
                was_disambiguated: false,
                disambiguation_question: None,
                user_response: None,
            });
        }

        // Recency auto-pick?
        let kind = self.classify_difference(&schema.candidate_actions);
        if kind == CandidateDifferenceKind::Recency {
            let selected = schema
                .candidate_actions
                .iter()
                .max_by(|a, b| a.recency_score.partial_cmp(&b.recency_score).unwrap_or(std::cmp::Ordering::Equal))?
                .clone();

            return Some(ResolvedIntent {
                intent_id: schema.intent_id,
                selected_action: selected,
                was_disambiguated: false,
                disambiguation_question: None,
                user_response: None,
            });
        }

        None
    }

    /// Return full history for user inspection.
    pub fn get_disambiguation_history(&self) -> &[DisambiguationRecord] {
        &self.memory.records
    }

    // ─── Private helpers ──────────────────────────────────────────────────────

    /// Classify the primary dimension of difference between candidates.
    fn classify_difference(&self, candidates: &[CandidateAction]) -> CandidateDifferenceKind {
        if candidates.len() < 2 {
            return CandidateDifferenceKind::NameOnly;
        }

        // Recency: large gap between top two scores → auto-pick, no question
        let mut by_recency: Vec<f32> = candidates.iter().map(|c| c.recency_score).collect();
        by_recency.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
        if by_recency[0] - by_recency.get(1).copied().unwrap_or(0.0) > 0.4 {
            return CandidateDifferenceKind::Recency;
        }

        // Domain: targets belong to detectably different domains
        let domains: Vec<String> = candidates
            .iter()
            .map(|c| self.extract_domain_label(&c.target))
            .collect();
        let unique_domains: std::collections::HashSet<&String> = domains.iter().collect();
        if unique_domains.len() > 1 {
            return CandidateDifferenceKind::Domain;
        }

        // Completion state: one target contains "unfinished" / "wip" / no extension
        let has_wip = candidates.iter().any(|c| {
            let t = c.target.to_lowercase();
            t.contains("wip") || t.contains("unfinished") || t.contains("draft")
        });
        if has_wip {
            return CandidateDifferenceKind::CompletionState;
        }

        CandidateDifferenceKind::NameOnly
    }

    /// Extract a short human-readable domain label from a file path.
    fn extract_domain_label(&self, target: &str) -> String {
        // e.g. ~/projects/school-robotics/... → "school robotics"
        //      ~/projects/pid-tuning/...      → "PID tuning"
        let path = std::path::Path::new(target);
        path.components()
            .rev()
            .skip(1)  // skip filename
            .next()
            .map(|c| {
                c.as_os_str()
                    .to_string_lossy()
                    .replace('-', " ")
                    .replace('_', " ")
            })
            .unwrap_or_else(|| target.to_string())
    }

    /// Short display name for a target (filename or last directory segment).
    fn short_name(&self, target: &str) -> String {
        let path = std::path::Path::new(target);
        path.file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| target.to_string())
    }

    /// Match user's free-text response to the best candidate.
    fn match_response(
        &self,
        candidates: &[CandidateAction],
        response: &str,
    ) -> Option<CandidateAction> {
        let response_lower = response.to_lowercase();
        candidates
            .iter()
            .max_by_key(|c| {
                let name = self.short_name(&c.target).to_lowercase();
                // Score: how many words of the name appear in the response
                name.split_whitespace()
                    .filter(|w| response_lower.contains(*w))
                    .count()
            })
            .filter(|c| {
                // Require at least one word to match
                let name = self.short_name(&c.target).to_lowercase();
                name.split_whitespace()
                    .any(|w| response_lower.contains(w))
            })
            .cloned()
    }

    /// Compute a stable hash of the disambiguation context.
    /// Same goal + domain + session context → same hash.
    fn context_hash(&self, schema: &IntentSchema) -> u64 {
        let mut hasher = DefaultHasher::new();
        schema.goal.hash(&mut hasher);
        schema.domain.hash(&mut hasher);
        schema.session_context.last_active_domain.hash(&mut hasher);
        hasher.finish()
    }

    /// Persist learned choices to disk.
    fn persist(&self) -> std::io::Result<()> {
        if let Some(parent) = self.memory_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(&self.memory)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        std::fs::write(&self.memory_path, json)
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::{IntentSchema, CandidateAction, SessionContext};

    fn make_schema(candidates: Vec<CandidateAction>) -> IntentSchema {
        IntentSchema {
            intent_id: Uuid::new_v4(),
            raw_input: "open my robotics work".into(),
            goal: "open_workspace".into(),
            domain: Some("robotics".into()),
            confidence: 0.82,
            ambiguity_score: 0.65,
            risk_estimate: 0.14,
            required_context: vec![],
            candidate_actions: candidates,
            disambiguation_required: true,
            disambiguation_question: Some("The motor driver or PID tuning?".into()),
            session_context: SessionContext {
                last_active_domain: Some("robotics".into()),
                last_active_files: vec!["motor.py".into()],
                current_time: "14:32".into(),
                time_since_last_session: Some("2h".into()),
            },
            hal_pre_score: 0.14,
            escalate_to_cloud: false,
        }
    }

    fn candidate(target: &str, confidence: f32, recency: f32) -> CandidateAction {
        CandidateAction {
            action: "open_files".into(),
            target: target.into(),
            confidence,
            recency_score: recency,
        }
    }

    #[test]
    fn auto_resolves_by_recency_when_gap_is_large() {
        let engine = DisambiguationEngine {
            memory: DisambiguationMemory::default(),
            memory_path: PathBuf::from("/tmp/test_disambig.json"),
        };
        let schema = make_schema(vec![
            candidate("~/projects/motor-driver/motor.py", 0.71, 0.9),
            candidate("~/projects/pid-tuning/pid.py", 0.45, 0.3),
        ]);
        let resolved = engine.try_auto_resolve(&schema);
        assert!(resolved.is_some());
        assert_eq!(
            resolved.unwrap().selected_action.target,
            "~/projects/motor-driver/motor.py"
        );
    }

    #[test]
    fn asks_question_when_recency_is_close() {
        let engine = DisambiguationEngine {
            memory: DisambiguationMemory::default(),
            memory_path: PathBuf::from("/tmp/test_disambig2.json"),
        };
        let schema = make_schema(vec![
            candidate("~/projects/motor-driver/motor.py", 0.71, 0.6),
            candidate("~/projects/pid-tuning/pid.py", 0.65, 0.55),
        ]);
        let q = engine.select_question(&schema);
        assert!(q.is_some());
    }

    #[test]
    fn only_one_question_asked() {
        // The spec says: never ask more than one question.
        // Our engine returns Option<question> — caller asks at most once,
        // then calls resolve() which always terminates.
        let mut engine = DisambiguationEngine {
            memory: DisambiguationMemory::default(),
            memory_path: PathBuf::from("/tmp/test_disambig3.json"),
        };
        let schema = make_schema(vec![
            candidate("~/school-robotics/arm.py", 0.6, 0.5),
            candidate("~/personal-robotics/rover.py", 0.55, 0.45),
        ]);
        let q = engine.select_question(&schema);
        assert!(q.is_some());

        // After one response, resolve always picks something — no second question
        let resolved = engine.resolve(schema, "school");
        assert!(resolved.was_disambiguated);
        assert!(resolved.selected_action.target.contains("school"));
    }

    #[test]
    fn learned_choice_skips_question_on_repeat() {
        let mut engine = DisambiguationEngine {
            memory: DisambiguationMemory::default(),
            memory_path: PathBuf::from("/tmp/test_disambig4.json"),
        };
        let schema = make_schema(vec![
            candidate("~/school-robotics/arm.py", 0.6, 0.5),
            candidate("~/personal-robotics/rover.py", 0.55, 0.45),
        ]);

        // First time: resolve and learn
        let schema_clone = schema.clone();
        engine.resolve(schema_clone, "school");

        // Second time: should auto-resolve, question = None
        let q = engine.select_question(&schema);
        assert!(q.is_none());
        let auto = engine.try_auto_resolve(&schema);
        assert!(auto.is_some());
    }
}
