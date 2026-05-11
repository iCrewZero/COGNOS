use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateAction {
    pub id: String,
    pub target: String,
    pub domain: Option<String>,
    pub recency_score: f32,
    pub completion_state: Option<String>,
    pub confidence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntentSchema {
    pub intent_id: String,
    pub goal: String,
    pub domain: Option<String>,
    pub context_hash: Option<String>,
    pub ambiguity_score: f32,
    pub candidate_actions: Vec<CandidateAction>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedIntent {
    pub intent_id: String,
    pub selected_candidate: CandidateAction,
    pub asked_question: bool,
    pub uncertainty_logged: bool,
    pub notification: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisambiguationRecord {
    pub goal: String,
    pub domain: Option<String>,
    pub context_hash: String,
    pub chosen_candidate_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PersistedStore {
    records: Vec<DisambiguationRecord>,
}

pub struct DisambiguationEngine {
    memory_path: PathBuf,
    history: Vec<DisambiguationRecord>,
}

impl DisambiguationEngine {
    pub fn new() -> Self {
        let memory_path = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".cognos/memory/disambiguation.json");
        let history = Self::load_history(&memory_path);
        Self { memory_path, history }
    }

    pub fn build_question(&self, schema: &IntentSchema) -> Option<String> {
        if schema.ambiguity_score <= 0.6 || schema.candidate_actions.len() < 2 {
            return None;
        }

        if self.differs_by_domain(&schema.candidate_actions) {
            let options = self.collect_options(schema, |c| c.domain.clone());
            return Some(format!("Do you mean {}?", options.join(" or ")));
        }

        if self.differs_only_by_recency(&schema.candidate_actions) {
            return None;
        }

        if self.differs_by_completion(&schema.candidate_actions) {
            let options = self.collect_options(schema, |c| c.completion_state.clone());
            return Some(format!("Should I open the {}?", options.join(" or the ")));
        }

        let options: Vec<String> = schema
            .candidate_actions
            .iter()
            .map(|c| c.target.clone())
            .take(4)
            .collect();
        Some(format!("Pick one: {}.", options.join(", ")))
    }

    pub fn resolve(&mut self, schema: IntentSchema, user_response: &str) -> ResolvedIntent {
        if let Some(chosen) = self.apply_learned_choice(&schema) {
            return ResolvedIntent {
                intent_id: schema.intent_id,
                selected_candidate: chosen.clone(),
                asked_question: false,
                uncertainty_logged: false,
                notification: Some(format!("Opened {} (your usual choice)", chosen.target)),
            };
        }

        let response_lower = user_response.to_lowercase();
        let mut matches: Vec<CandidateAction> = schema
            .candidate_actions
            .iter()
            .filter(|c| {
                response_lower.contains(&c.target.to_lowercase())
                    || c.domain.as_ref().is_some_and(|d| response_lower.contains(&d.to_lowercase()))
                    || c.completion_state
                        .as_ref()
                        .is_some_and(|s| response_lower.contains(&s.to_lowercase()))
            })
            .cloned()
            .collect();

        if matches.is_empty() {
            matches.push(self.pick_highest_confidence(&schema));
        }

        let selected = matches
            .into_iter()
            .max_by(|a, b| a.confidence.total_cmp(&b.confidence))
            .unwrap_or_else(|| self.pick_highest_confidence(&schema));

        self.persist_choice(&schema, &selected.id);

        ResolvedIntent {
            intent_id: schema.intent_id,
            selected_candidate: selected,
            asked_question: true,
            uncertainty_logged: schema.ambiguity_score > 0.6,
            notification: None,
        }
    }

    pub fn get_disambiguation_history(&self) -> Vec<DisambiguationRecord> {
        self.history.clone()
    }

    fn load_history(path: &PathBuf) -> Vec<DisambiguationRecord> {
        if let Ok(raw) = fs::read_to_string(path) {
            if let Ok(store) = serde_json::from_str::<PersistedStore>(&raw) {
                return store.records;
            }
        }
        Vec::new()
    }

    fn persist_choice(&mut self, schema: &IntentSchema, chosen_candidate_id: &str) {
        let record = DisambiguationRecord {
            goal: schema.goal.clone(),
            domain: schema.domain.clone(),
            context_hash: schema.context_hash.clone().unwrap_or_else(|| self.compute_context_hash(schema)),
            chosen_candidate_id: chosen_candidate_id.to_string(),
        };
        self.history.push(record);
        if let Some(parent) = self.memory_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let payload = PersistedStore {
            records: self.history.clone(),
        };
        let _ = fs::write(
            &self.memory_path,
            serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{\"records\":[]}".to_string()),
        );
    }

    fn apply_learned_choice(&self, schema: &IntentSchema) -> Option<CandidateAction> {
        let context_hash = schema
            .context_hash
            .clone()
            .unwrap_or_else(|| self.compute_context_hash(schema));
        let found = self.history.iter().rev().find(|r| {
            r.goal == schema.goal && r.domain == schema.domain && r.context_hash == context_hash
        })?;
        schema
            .candidate_actions
            .iter()
            .find(|c| c.id == found.chosen_candidate_id)
            .cloned()
    }

    fn pick_highest_confidence(&self, schema: &IntentSchema) -> CandidateAction {
        schema
            .candidate_actions
            .iter()
            .max_by(|a, b| a.confidence.total_cmp(&b.confidence))
            .cloned()
            .or_else(|| schema.candidate_actions.first().cloned())
            .expect("candidate_actions must be non-empty")
    }

    fn compute_context_hash(&self, schema: &IntentSchema) -> String {
        let mut hasher = Sha256::new();
        hasher.update(schema.goal.as_bytes());
        if let Some(d) = &schema.domain {
            hasher.update(d.as_bytes());
        }
        for c in &schema.candidate_actions {
            hasher.update(c.target.as_bytes());
        }
        format!("{:x}", hasher.finalize())
    }

    fn differs_by_domain(&self, c: &[CandidateAction]) -> bool {
        unique_count(c.iter().filter_map(|x| x.domain.clone())) > 1
    }

    fn differs_by_completion(&self, c: &[CandidateAction]) -> bool {
        unique_count(c.iter().filter_map(|x| x.completion_state.clone())) > 1
    }

    fn differs_only_by_recency(&self, c: &[CandidateAction]) -> bool {
        let same_domain = unique_count(c.iter().map(|x| x.domain.clone().unwrap_or_default())) <= 1;
        let same_completion = unique_count(c.iter().map(|x| x.completion_state.clone().unwrap_or_default())) <= 1;
        same_domain && same_completion
    }

    fn collect_options<F>(&self, schema: &IntentSchema, f: F) -> Vec<String>
    where
        F: Fn(&CandidateAction) -> Option<String>,
    {
        schema
            .candidate_actions
            .iter()
            .filter_map(f)
            .take(3)
            .collect()
    }
}

fn unique_count<I>(iter: I) -> usize
where
    I: Iterator<Item = String>,
{
    iter.fold(HashMap::<String, bool>::new(), |mut acc, v| {
        acc.insert(v, true);
        acc
    })
    .len()
}
