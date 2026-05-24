/// HAL Restraint Model — gating for the cognitive context preloader.
///
/// THIS FILE IS HUMAN-WRITTEN ONLY. Zero AI authorship.
///
/// Ensures predictions only surface when:
///   - Confidence score > 0.85
///   - The predicted action is low-intimacy
///   - The action is in a domain the user has accepted predictions for
///   - The time and context match established patterns
///
/// When in doubt, stay invisible. An OS that does nothing unexpected
/// is more trustworthy than one that is occasionally brilliant.

use serde::{Deserialize, Serialize};

const CONFIDENCE_THRESHOLD: f32 = 0.85;

/// Domains that are ALWAYS held back — no exceptions.
const HIGH_INTIMACY_DOMAINS: &[&str] = &["personal", "finance", "health", "private"];

/// File path substrings that trigger hold-back regardless of domain.
const SENSITIVE_PATH_SUBSTRINGS: &[&str] = &[
    "diary", "journal", "private", "personal", ".kdbx",
    "password", "credential", "therapy", "medical", "secret",
];

/// Domains where late-night preloading is acceptable.
const NIGHT_OK_DOMAINS: &[&str] = &["coding", "work"];

/// Number of positive interactions needed before a domain unlocks.
pub const ACCEPTANCE_THRESHOLD: u32 = 3;

// ─── Types ────────────────────────────────────────────────────────────────────

/// A prediction from the LSTM about what the user will do next.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextPrediction {
    pub predicted_action: String,
    pub confidence: f32,
    pub domain: String,
    pub file_paths: Vec<String>,
    pub trigger_signal: String,
    /// Hour of day, 0–23.
    pub time_of_day: u8,
}

/// The restraint model's decision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PreloadDecision {
    Preload,
    HoldBack { reason: String },
}

impl PreloadDecision {
    pub fn is_preload(&self) -> bool {
        matches!(self, Self::Preload)
    }
}

// ─── Domain acceptance state ──────────────────────────────────────────────────

/// Per-domain acceptance counter. Determines whether predictions for a domain
/// have earned the right to surface.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct DomainAcceptance {
    /// Map from domain name → count of positive interactions.
    positive_counts: std::collections::HashMap<String, u32>,
    /// Domains the user has explicitly disabled.
    locked: std::collections::HashSet<String>,
}

impl DomainAcceptance {
    pub fn is_unlocked(&self, domain: &str) -> bool {
        if self.locked.contains(domain) {
            return false;
        }
        let count = self.positive_counts.get(domain).copied().unwrap_or(0);
        count >= ACCEPTANCE_THRESHOLD
    }

    pub fn record_acceptance(&mut self, domain: &str) {
        *self.positive_counts.entry(domain.to_string()).or_insert(0) += 1;
    }

    pub fn lock(&mut self, domain: &str) {
        self.locked.insert(domain.to_string());
    }

    pub fn unlock(&mut self, domain: &str) {
        self.locked.remove(domain);
    }

    pub fn acceptance_count(&self, domain: &str) -> u32 {
        self.positive_counts.get(domain).copied().unwrap_or(0)
    }
}

// ─── Restraint model ─────────────────────────────────────────────────────────

/// The restraint model. Stateless decision logic; state is in DomainAcceptance.
pub struct RestraintModel;

impl RestraintModel {
    /// Evaluate whether a prediction should be acted upon.
    ///
    /// All conditions are evaluated; the FIRST failing check causes HoldBack.
    /// Order matches priority: confidence, domain, path, time, action type, acceptance.
    pub fn should_preload(
        prediction: &ContextPrediction,
        acceptance: &DomainAcceptance,
    ) -> PreloadDecision {

        // 1. Confidence threshold — the model must be highly certain
        if prediction.confidence < CONFIDENCE_THRESHOLD {
            return PreloadDecision::HoldBack {
                reason: format!(
                    "Confidence {:.2} below threshold {:.2}",
                    prediction.confidence, CONFIDENCE_THRESHOLD
                ),
            };
        }

        // 2. High-intimacy domains are always held back
        if HIGH_INTIMACY_DOMAINS.contains(&prediction.domain.as_str()) {
            return PreloadDecision::HoldBack {
                reason: format!(
                    "Domain '{}' is always held back (high-intimacy)",
                    prediction.domain
                ),
            };
        }

        // 3. Sensitive file path patterns
        for path in &prediction.file_paths {
            let path_lower = path.to_lowercase();
            for pattern in SENSITIVE_PATH_SUBSTRINGS {
                if path_lower.contains(pattern) {
                    return PreloadDecision::HoldBack {
                        reason: format!(
                            "File path contains sensitive pattern '{}' — held back",
                            pattern
                        ),
                    };
                }
            }
        }

        // 4. Late-night non-work preloads
        let hour = prediction.time_of_day;
        let is_late_night = hour >= 22 || hour <= 6;
        if is_late_night && !NIGHT_OK_DOMAINS.contains(&prediction.domain.as_str()) {
            return PreloadDecision::HoldBack {
                reason: format!(
                    "Late night ({:02}:00) preload in domain '{}' — held back",
                    hour, prediction.domain
                ),
            };
        }

        // 5. Content-reading actions are not preloaded (opening apps only)
        let action_lower = prediction.predicted_action.to_lowercase();
        if action_lower.contains("read") || action_lower.contains("extract")
            || action_lower.contains("parse")
        {
            return PreloadDecision::HoldBack {
                reason: "Predicted action reads file content — only app-open preloading is allowed".to_string(),
            };
        }

        // 6. Domain must be unlocked by 3 prior positive interactions
        if !acceptance.is_unlocked(&prediction.domain) {
            let count = acceptance.acceptance_count(&prediction.domain);
            return PreloadDecision::HoldBack {
                reason: format!(
                    "Domain '{}' not yet unlocked ({}/{} positive interactions)",
                    prediction.domain, count, ACCEPTANCE_THRESHOLD
                ),
            };
        }

        // All checks passed — preload is approved
        PreloadDecision::Preload
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn unlocked_acceptance(domain: &str) -> DomainAcceptance {
        let mut a = DomainAcceptance::default();
        for _ in 0..ACCEPTANCE_THRESHOLD {
            a.record_acceptance(domain);
        }
        a
    }

    fn base_pred() -> ContextPrediction {
        ContextPrediction {
            predicted_action: "open_workspace".to_string(),
            confidence: 0.92,
            domain: "coding".to_string(),
            file_paths: vec!["~/projects/motor.py".to_string()],
            trigger_signal: "9am pattern".to_string(),
            time_of_day: 9,
        }
    }

    #[test]
    fn valid_preload_passes() {
        let acc = unlocked_acceptance("coding");
        let result = RestraintModel::should_preload(&base_pred(), &acc);
        assert_eq!(result, PreloadDecision::Preload);
    }

    #[test]
    fn low_confidence_holds_back() {
        let acc = unlocked_acceptance("coding");
        let mut pred = base_pred();
        pred.confidence = 0.70;
        let result = RestraintModel::should_preload(&pred, &acc);
        assert!(matches!(result, PreloadDecision::HoldBack { .. }));
    }

    #[test]
    fn personal_domain_always_held_back() {
        let acc = unlocked_acceptance("personal");
        let mut pred = base_pred();
        pred.domain = "personal".to_string();
        pred.confidence = 0.99;
        let result = RestraintModel::should_preload(&pred, &acc);
        assert!(matches!(result, PreloadDecision::HoldBack { .. }));
    }

    #[test]
    fn sensitive_path_holds_back() {
        let acc = unlocked_acceptance("coding");
        let mut pred = base_pred();
        pred.file_paths = vec!["~/diary/2025.md".to_string()];
        let result = RestraintModel::should_preload(&pred, &acc);
        assert!(matches!(result, PreloadDecision::HoldBack { .. }));
    }

    #[test]
    fn late_night_personal_holds_back() {
        let acc = unlocked_acceptance("gaming");
        let mut pred = base_pred();
        pred.domain = "gaming".to_string();
        pred.time_of_day = 23;
        let result = RestraintModel::should_preload(&pred, &acc);
        assert!(matches!(result, PreloadDecision::HoldBack { .. }));
    }

    #[test]
    fn late_night_coding_is_ok() {
        let acc = unlocked_acceptance("coding");
        let mut pred = base_pred();
        pred.time_of_day = 23;
        let result = RestraintModel::should_preload(&pred, &acc);
        assert_eq!(result, PreloadDecision::Preload);
    }

    #[test]
    fn read_action_held_back() {
        let acc = unlocked_acceptance("coding");
        let mut pred = base_pred();
        pred.predicted_action = "read_file_content".to_string();
        let result = RestraintModel::should_preload(&pred, &acc);
        assert!(matches!(result, PreloadDecision::HoldBack { .. }));
    }

    #[test]
    fn new_domain_held_back() {
        let acc = DomainAcceptance::default(); // no acceptances
        let result = RestraintModel::should_preload(&base_pred(), &acc);
        assert!(matches!(result, PreloadDecision::HoldBack { .. }));
    }

    #[test]
    fn locked_domain_held_back() {
        let mut acc = unlocked_acceptance("coding");
        acc.lock("coding");
        let result = RestraintModel::should_preload(&base_pred(), &acc);
        assert!(matches!(result, PreloadDecision::HoldBack { .. }));
    }

    #[test]
    fn credentials_path_held_back() {
        let acc = unlocked_acceptance("coding");
        let mut pred = base_pred();
        pred.file_paths = vec!["~/credentials.env".to_string()];
        let result = RestraintModel::should_preload(&pred, &acc);
        assert!(matches!(result, PreloadDecision::HoldBack { .. }));
    }
}
