//! Autonomous constitution — immutable principles governing AI behavior.
//!
//!
//! The constitution is the highest-authority policy layer in HAL. Below it
//! are mutable policies (governance_kernel), trust tables, and reputation;
//! above it is nothing. The articles in the constitution are *immutable*:
//! they cannot be amended, suspended, or overridden by any agent, including
//! the operator, except through a full reinstall of HAL (which itself
//! requires the recovery-kernel's [`crate::recovery_kernel::RollbackPolicy`]
/// to be set to `Forbidden` first).
//!
//! The five articles are hard-coded at construction time and all marked
//! `immutable: true`. Any action that violates an article is blocked
//! unconditionally, regardless of trust, autonomy, or operator override.
//!
//! v0: stub implementation. Article matching is keyword-based; v1 will use
//! a proper policy DSL.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{info, warn};

// v0: stub implementation

/// Type alias for an agent identifier (matches the rest of the crate).
pub type AgentId = String;

/// Identifier of a constitutional article.
pub type ArticleId = u32;

// ─── Article ────────────────────────────────────────────────────────────────────

/// A single constitutional article.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Article {
    /// Numeric id of the article. Stable across versions.
    pub id: ArticleId,
    /// The principle, in plain English.
    pub principle: String,
    /// Priority. Lower numbers are higher priority. Article 1 has priority 1.
    pub priority: u32,
    /// Whether the article is immutable. Always `true` for the constitution.
    pub immutable: bool,
}

// ─── Verdict ────────────────────────────────────────────────────────────────────

/// The constitution's verdict on a proposed action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConstitutionVerdict {
    /// The action does not violate any article.
    Compliant,
    /// The action violates the named article.
    Violates(ArticleId),
}

// ─── Action Descriptor ──────────────────────────────────────────────────────────

/// Minimal descriptor of an action proposed for constitutional evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConstitutionAction {
    /// Agent proposing the action.
    pub agent: AgentId,
    /// Action name.
    pub action: String,
    /// Target resource path or identifier.
    pub target: String,
    /// When the action was proposed (for audit).
    pub proposed_at: DateTime<Utc>,
    /// Whether the action would modify HAL itself.
    pub touches_hal: bool,
    /// Whether the action would alter the audit chain.
    pub touches_audit: bool,
    /// Whether the action would change the autonomy level.
    pub touches_autonomy: bool,
    /// Whether the action withholds information from the user (e.g. silent
    /// notification, hidden audit entry).
    pub withholds_information: bool,
    /// Whether the action overrides an explicit user preference.
    pub overrides_user: bool,
}

// ─── Constitution ───────────────────────────────────────────────────────────────

/// The autonomous constitution.
pub struct AutonomousConstitution {
    /// The immutable articles, in priority order.
    pub articles: Vec<Article>,
}

impl Default for AutonomousConstitution {
    fn default() -> Self {
        Self::new()
    }
}

impl AutonomousConstitution {
    /// Build a new constitution with the five hard-coded articles.
    pub fn new() -> Self {
        Self {
            articles: default_articles(),
        }
    }

    /// Evaluate a proposed action against the constitution.
    ///
    /// Articles are checked in priority order; the first violation wins.
    /// If no article is violated, returns [`ConstitutionVerdict::Compliant`].
    pub fn evaluate(&self, action: &ConstitutionAction) -> ConstitutionVerdict {
        for article in &self.articles {
            if self.violates(article, action) {
                warn!(
                    article = article.id,
                    principle = %article.principle,
                    agent = %action.agent,
                    action = %action.action,
                    "autonomous_constitution: VIOLATION"
                );
                return ConstitutionVerdict::Violates(article.id);
            }
        }
        ConstitutionVerdict::Compliant
    }

    /// Look up an article by id.
    pub fn article(&self, id: ArticleId) -> Option<&Article> {
        self.articles.iter().find(|a| a.id == id)
    }

    /// Whether a given action violates a given article.
    ///
    /// v0 uses simple boolean fields on the action descriptor. v1 will use
    /// a proper policy DSL with predicates over the full action context.
    // TODO(v1): replace boolean-flag matching with a proper policy DSL that
    // can evaluate predicates over the full HALContextV2 (resource paths,
    // capability arguments, audit context).
    fn violates(&self, article: &Article, action: &ConstitutionAction) -> bool {
        match article.id {
            1 => action.touches_hal,
            2 => action.withholds_information,
            3 => action.touches_audit,
            4 => action.touches_autonomy,
            5 => action.overrides_user,
            _ => false,
        }
    }

    /// Number of articles in the constitution.
    pub fn len(&self) -> usize {
        self.articles.len()
    }

    /// Whether the constitution is empty (it never should be).
    pub fn is_empty(&self) -> bool {
        self.articles.is_empty()
    }

    /// Confirm that all articles are still marked immutable. This is a
    /// self-check that should be called on boot.
    pub fn verify_immutability(&self) -> bool {
        let all_immutable = self.articles.iter().all(|a| a.immutable);
        if !all_immutable {
            // This is a fatal condition — the constitution has been tampered with.
            // v0 just logs; v1 should trigger a lockdown.
            tracing::error!(
                "autonomous_constitution: IMMUTABILITY CHECK FAILED — tamper detected"
            );
        } else {
            info!(
                count = self.articles.len(),
                "autonomous_constitution: immutability check passed"
            );
        }
        all_immutable
    }
}

// ─── Default Articles ───────────────────────────────────────────────────────────

/// The five hard-coded constitutional articles.
///
/// These are intentionally hardcoded rather than loaded from a config file —
/// if they could be edited, they wouldn't be a constitution.
pub fn default_articles() -> Vec<Article> {
    vec![
        Article {
            id: 1,
            principle: "Never modify HAL. The Human Approval Layer is the trust root; \
                        modifying it (binaries, configs, runtime state) is forbidden \
                        under all conditions."
                .to_string(),
            priority: 1,
            immutable: true,
        },
        Article {
            id: 2,
            principle: "Never deceive the user. Withholding information, fabricating \
                        audit entries, or misrepresenting the system's state is \
                        forbidden."
                .to_string(),
            priority: 2,
            immutable: true,
        },
        Article {
            id: 3,
            principle: "Always preserve the audit chain. Tampering with, truncating, \
                        or rewriting the tamper-evident audit log is forbidden."
                .to_string(),
            priority: 3,
            immutable: true,
        },
        Article {
            id: 4,
            principle: "Never escalate autonomy without consent. Increasing the \
                        system's autonomy level requires explicit human approval; \
                        the system may not grant itself more authority."
                .to_string(),
            priority: 4,
            immutable: true,
        },
        Article {
            id: 5,
            principle: "Preserve user sovereignty. The user's explicit preferences \
                        and instructions take precedence over the system's own \
                        judgments, except where doing so would violate Articles 1-4."
                .to_string(),
            priority: 5,
            immutable: true,
        },
    ]
}

// ─── Errors ─────────────────────────────────────────────────────────────────────

/// Errors returned by the constitution module.
#[derive(Debug, Error)]
pub enum ConstitutionError {
    /// An attempt was made to amend or remove an article. The constitution
    /// is immutable; this is always an error.
    #[error("attempt to amend immutable article {0}")]
    AmendmentForbidden(ArticleId),
    /// The immutability self-check failed at boot.
    #[error("constitution immutability check failed — possible tamper")]
    ImmutabilityCheckFailed,
}
