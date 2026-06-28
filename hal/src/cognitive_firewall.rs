//! Cognitive firewall — filters cognitive content to prevent the system
//!
//!
//! from surfacing emotional, intimate, or creepy predictions to the user.
//!
//! The HAL's UX principle is "stay invisible when in doubt". The cognitive
//! firewall is the last content-level filter on the cognitive preloader's
//! output: even if the restraint model and the autonomy controller both
//! approve a prediction, the firewall may still block it because of *what*
//! it predicts, not *whether* it should be acted on.
//!
//! The firewall classifies predictions along four axes:
//!   - Emotional content (sentiment, intimacy markers)
//!   - Private domain (health, finance, personal, relationships)
//!   - Unrequested domain (a domain the user has not opted into)
//!   - Time inappropriateness (late-night, in a sensitive window)
//!
//! When in doubt, the firewall returns [`FirewallVerdict::Block`]. It may
//! also return [`FirewallVerdict::Anonymize`] when the prediction is safe
//! to surface with sensitive fields stripped.
//!
//! v0: stub implementation. The intimacy classifier and domain filter are
//! stub heuristics; v1 will swap in a real classifier.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::restraint_model::ContextPrediction;

// v0: stub implementation

// ─── Block Reasons ──────────────────────────────────────────────────────────────

/// Reasons the firewall may block a prediction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlockReason {
    /// The prediction contains emotional or intimate content.
    EmotionalContent,
    /// The prediction touches a private domain (health, finance, etc.).
    PrivateDomain,
    /// The prediction is in a domain the user has not opted into.
    UnrequestedDomain,
    /// The prediction is being surfaced at a time-inappropriate moment.
    TimeInappropriate,
}

impl BlockReason {
    /// Human-readable description shown in the audit log.
    pub fn description(&self) -> &'static str {
        match self {
            Self::EmotionalContent => "prediction contains emotional/intimate content",
            Self::PrivateDomain => "prediction touches a private domain",
            Self::UnrequestedDomain => "prediction is in an unrequested domain",
            Self::TimeInappropriate => "prediction surfaced at a time-inappropriate moment",
        }
    }
}

// ─── Firewall Verdict ───────────────────────────────────────────────────────────

/// The firewall's verdict on a prediction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FirewallVerdict {
    /// Prediction may be surfaced as-is.
    Pass,
    /// Prediction must be blocked. The reason is included for audit.
    Block(BlockReason),
    /// Prediction may be surfaced with sensitive fields stripped.
    Anonymize,
}

// ─── Intimacy Classifier ────────────────────────────────────────────────────────

/// Stub intimacy classifier. v0 uses substring matching against a small
/// seed lexicon; v1 will swap in a real model.
#[derive(Debug, Default, Clone)]
pub struct IntimacyClassifier {
    /// Lexicon of substrings that mark emotional/intimate content.
    lexicon: Vec<String>,
}

impl IntimacyClassifier {
    /// Construct with the v0 default lexicon.
    pub fn new() -> Self {
        Self {
            lexicon: DEFAULT_INTIMACY_LEXICON
                .iter()
                .map(|s| s.to_string())
                .collect(),
        }
    }

    /// Classify a prediction's text fields. Returns true if any field
    /// matches the intimacy lexicon (case-insensitive).
    ///
    // TODO(v1): swap substring matching for a trained intimacy classifier
    // (small fine-tuned transformer) so we catch paraphrases and misspellings.
    pub fn is_intimate(&self, prediction: &ContextPrediction) -> bool {
        let haystacks = [
            prediction.predicted_action.to_lowercase(),
            prediction.domain.to_lowercase(),
        ];
        let path_lower: Vec<String> = prediction
            .file_paths
            .iter()
            .map(|p| p.to_lowercase())
            .collect();

        for needle in &self.lexicon {
            let needle = needle.to_lowercase();
            for h in &haystacks {
                if h.contains(&needle) {
                    return true;
                }
            }
            for p in &path_lower {
                if p.contains(&needle) {
                    return true;
                }
            }
        }
        false
    }
}

/// Default intimacy lexicon. v0 placeholder — v1 will load from a config file.
const DEFAULT_INTIMACY_LEXICON: &[&str] = &[
    "diary",
    "journal",
    "love",
    "relationship",
    "therapy",
    "medical",
    "health",
    "password",
    "credential",
    "secret",
    "private",
    "personal",
    "finance",
    "bank",
    "kdbx",
    "emotion",
    "feeling",
];

// ─── Domain Filter ──────────────────────────────────────────────────────────────

/// Domain allowlist filter. A domain must be both non-private AND opted-in
/// to pass the firewall.
#[derive(Debug, Default, Clone)]
pub struct DomainFilter {
    /// Domains the user has explicitly opted into.
    opted_in: HashSet<String>,
    /// Domains always treated as private (never surfaced).
    always_private: HashSet<String>,
}

impl DomainFilter {
    /// Construct with v0 defaults.
    pub fn new() -> Self {
        let always_private: HashSet<String> = DEFAULT_PRIVATE_DOMAINS
            .iter()
            .map(|s| s.to_string())
            .collect();
        Self {
            opted_in: HashSet::new(),
            always_private,
        }
    }

    /// Mark a domain as opted-in by the user.
    pub fn opt_in(&mut self, domain: &str) {
        self.opted_in.insert(domain.to_string());
    }

    /// Withdraw opt-in for a domain.
    pub fn opt_out(&mut self, domain: &str) {
        self.opted_in.remove(domain);
    }

    /// Whether the domain is in the always-private set.
    pub fn is_private(&self, domain: &str) -> bool {
        self.always_private.contains(domain)
    }

    /// Whether the domain has been opted into by the user.
    pub fn is_opted_in(&self, domain: &str) -> bool {
        self.opted_in.contains(domain)
    }
}

/// Domains always treated as private.
const DEFAULT_PRIVATE_DOMAINS: &[&str] = &[
    "personal",
    "finance",
    "health",
    "private",
    "relationships",
    "therapy",
];

// ─── Cognitive Firewall ─────────────────────────────────────────────────────────

/// The cognitive firewall. Holds the intimacy classifier and domain filter.
#[derive(Debug, Default, Clone)]
pub struct CognitiveFirewall {
    intimacy_classifier: IntimacyClassifier,
    domain_filter: DomainFilter,
}

impl CognitiveFirewall {
    /// Construct a firewall with v0 defaults.
    pub fn new() -> Self {
        Self {
            intimacy_classifier: IntimacyClassifier::new(),
            domain_filter: DomainFilter::new(),
        }
    }

    /// Filter a prediction. Returns the verdict.
    ///
    /// Implements the "stay invisible when in doubt" principle: when in
    /// doubt between Block and Anonymize, we Block.
    pub fn filter(&self, prediction: &ContextPrediction) -> FirewallVerdict {
        // 1. Emotional content → always block.
        if self.intimacy_classifier.is_intimate(prediction) {
            warn!(
                action = %prediction.predicted_action,
                "firewall blocked: emotional content"
            );
            return FirewallVerdict::Block(BlockReason::EmotionalContent);
        }

        // 2. Private domain → always block.
        if self.domain_filter.is_private(&prediction.domain) {
            warn!(
                domain = %prediction.domain,
                "firewall blocked: private domain"
            );
            return FirewallVerdict::Block(BlockReason::PrivateDomain);
        }

        // 3. Unrequested domain → block (user must opt in).
        if !self.domain_filter.is_opted_in(&prediction.domain) {
            debug!(
                domain = %prediction.domain,
                "firewall blocked: unrequested domain"
            );
            return FirewallVerdict::Block(BlockReason::UnrequestedDomain);
        }

        // 4. Late-night in a sensitive window → block.
        //    v0: treat 22:00–06:00 as time-inappropriate for any non-coding
        //    domain.
        let hour = prediction.time_of_day;
        let late_night = hour >= 22 || hour <= 6;
        if late_night && prediction.domain != "coding" && prediction.domain != "work" {
            debug!(
                hour,
                domain = %prediction.domain,
                "firewall blocked: time-inappropriate"
            );
            return FirewallVerdict::Block(BlockReason::TimeInappropriate);
        }

        // All checks passed — surface the prediction.
        FirewallVerdict::Pass
    }

    /// Borrow the domain filter (for opt-in management).
    pub fn domain_filter(&self) -> &DomainFilter {
        &self.domain_filter
    }

    /// Mutably borrow the domain filter.
    pub fn domain_filter_mut(&mut self) -> &mut DomainFilter {
        &mut self.domain_filter
    }
}
