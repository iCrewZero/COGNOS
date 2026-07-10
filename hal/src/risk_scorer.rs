/// HAL Risk Scorer — full v1 formal model for COGNOS/OS.
///
///
/// Implements the formal risk model:
///   R(A) = w1·Irreversibility(A) + w2·Scope(A) + w3·TrustContext(A)
///          + w4·TimeAnomaly(A)   + w5·VibeCodeFlag(A)
///          - w6·UserHistory(A)   - w7·PatternMatch(A)
///
/// All weights sum to 1.0. All component scores ∈ [0.0, 1.0].
/// Result is clamped to [0.0, 1.0] and hard floors applied last.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ─── Component enums ──────────────────────────────────────────────────────────

/// How reversible the action is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum IrreversibilityLevel {
    /// Open app, read file — fully reversible.
    FullyReversible,
    /// Config change, moved file — reversible with effort.
    ReversibleWithEffort,
    /// Package install, permission grant — hard to reverse.
    HardToReverse,
    /// Delete, format, credential change — irreversible.
    Irreversible,
}

impl IrreversibilityLevel {
    fn score(&self) -> f32 {
        match self {
            Self::FullyReversible      => 0.0,
            Self::ReversibleWithEffort => 0.3,
            Self::HardToReverse        => 0.7,
            Self::Irreversible         => 1.0,
        }
    }
}

/// How wide the blast radius of the action is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ScopeLevel {
    /// Single file in user home.
    SingleFileUserHome,
    /// Multiple files, single directory.
    MultipleFileSingleDir,
    /// System-wide, multiple users.
    SystemWide,
    /// Kernel-level, hardware-level.
    KernelLevel,
}

impl ScopeLevel {
    fn score(&self) -> f32 {
        match self {
            Self::SingleFileUserHome    => 0.0,
            Self::MultipleFileSingleDir => 0.3,
            Self::SystemWide            => 0.7,
            Self::KernelLevel           => 1.0,
        }
    }
}

/// How trustworthy the source of the action is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TrustContextLevel {
    /// Known app, established behavior, signed source.
    KnownTrusted,
    /// Known app, minor behavioral anomaly.
    KnownAnomalous,
    /// New app, unverified behavior.
    NewApp,
    /// Unknown binary, behavioral red flag, unsigned.
    Unknown,
}

impl TrustContextLevel {
    fn score(&self) -> f32 {
        match self {
            Self::KnownTrusted   => 0.0,
            Self::KnownAnomalous => 0.4,
            Self::NewApp         => 0.7,
            Self::Unknown        => 1.0,
        }
    }
}

/// Whether the action occurs at an unusual time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TimeAnomalyLevel {
    /// Action within established time patterns.
    Normal,
    /// Action outside normal hours but not unprecedented.
    UnusualHour,
    /// Unusual time AND unusual scope combination.
    UnusualTimeAndScope,
}

impl TimeAnomalyLevel {
    fn score(&self) -> f32 {
        match self {
            Self::Normal               => 0.0,
            Self::UnusualHour          => 0.5,
            Self::UnusualTimeAndScope  => 1.0,
        }
    }
}

/// Whether AI-generated code is involved.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum VibeFlagLevel {
    /// No AI-generated code involved.
    None,
    /// AI-generated code, not yet human-reviewed.
    AiGeneratedUnreviewed,
    /// AI-generated code touching kernel or HAL-adjacent paths.
    AiGeneratedKernelAdjacent,
}

impl VibeFlagLevel {
    fn score(&self) -> f32 {
        match self {
            Self::None                    => 0.0,
            Self::AiGeneratedUnreviewed   => 0.8,
            Self::AiGeneratedKernelAdjacent => 1.0,
        }
    }
}

/// How many times the user has done this exact action before.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum UserHistoryLevel {
    /// Never done before.
    Never,
    /// Done fewer than 5 times.
    Rare,
    /// Done more than 20 times with consistent context.
    Frequent,
    /// Done more than 100 times in identical context (routine).
    Routine,
}

impl UserHistoryLevel {
    fn score(&self) -> f32 {
        match self {
            Self::Never    => 0.0,
            Self::Rare     => 0.3,
            Self::Frequent => 0.7,
            Self::Routine  => 1.0,
        }
    }
}

/// How closely this matches a learned behavioral pattern.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PatternMatchLevel {
    /// No matching learned pattern.
    NoMatch,
    /// Partial pattern match.
    PartialMatch,
    /// Exact pattern match with high confidence.
    ExactMatch,
}

impl PatternMatchLevel {
    fn score(&self) -> f32 {
        match self {
            Self::NoMatch      => 0.0,
            Self::PartialMatch => 0.5,
            Self::ExactMatch   => 1.0,
        }
    }
}

// ─── Input / output types ─────────────────────────────────────────────────────

/// An action proposed by an agent, ready to be risk-scored.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposedAction {
    /// The action name, e.g. "delete_file", "install_package".
    pub action_type: String,
    /// The target resource path or identifier.
    pub target: String,
    /// The agent proposing this action.
    pub agent: String,
    pub irreversibility: IrreversibilityLevel,
    pub scope: ScopeLevel,
    pub trust_context: TrustContextLevel,
    pub time_anomaly: TimeAnomalyLevel,
    pub vibe_flag: VibeFlagLevel,
    pub user_history: UserHistoryLevel,
    pub pattern_match: PatternMatchLevel,
    /// Whether this action targets kernel-adjacent paths.
    pub is_kernel_adjacent: bool,
    /// Whether the action is a delete operation.
    pub is_delete: bool,
}

/// System context at the time of scoring.
#[derive(Debug, Clone)]
pub struct SystemContext {
    pub current_time: DateTime<Utc>,
    pub session_id: String,
}

impl Default for SystemContext {
    fn default() -> Self {
        Self {
            current_time: Utc::now(),
            session_id: String::new(),
        }
    }
}

/// The output of the risk scorer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskScore {
    /// Final score ∈ [0.0, 1.0].
    pub score: f32,
    /// The HAL level this score maps to.
    pub level: RiskLevel,
    /// Plain-English explanation shown to the user in confirmation dialogs.
    pub explanation: String,
    /// Scores of each component, for auditing.
    pub components: ComponentScores,
}

/// Individual component scores for transparency and audit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentScores {
    pub irreversibility: f32,
    pub scope: f32,
    pub trust_context: f32,
    pub time_anomaly: f32,
    pub vibe_flag: f32,
    pub user_history: f32,
    pub pattern_match: f32,
    pub hard_floor_applied: bool,
    pub hard_floor_reason: Option<String>,
}

/// The four HAL response levels.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RiskLevel {
    /// [0.0, 0.3) — execute, write to audit log.
    Silent,
    /// [0.3, 0.6) — toast notification, 5s undo window.
    Notify,
    /// [0.6, 0.8) — dialog with plain-English explanation.
    Confirm,
    /// [0.8, 1.0] — full breakdown, explicit approve/deny, mandatory audit.
    Block,
}

impl RiskLevel {
    fn from_score(score: f32) -> Self {
        if score < 0.3 {
            Self::Silent
        } else if score < 0.6 {
            Self::Notify
        } else if score < 0.8 {
            Self::Confirm
        } else {
            Self::Block
        }
    }
}

// ─── Weights ──────────────────────────────────────────────────────────────────

/// Formal model weights. Must sum to 1.0.
/// Justification:
///   w1 (irreversibility) = 0.25 — the single most important factor
///   w2 (scope)           = 0.20 — blast radius matters almost as much
///   w3 (trust_context)   = 0.20 — unverified source is a major risk signal
///   w4 (time_anomaly)    = 0.10 — useful signal but not primary
///   w5 (vibe_flag)       = 0.10 — AI code needs elevated scrutiny
///   w6 (user_history)    = 0.10 — established routines reduce risk
///   w7 (pattern_match)   = 0.05 — weakest signal, easily gamed without floors
const W1_IRREVERSIBILITY: f32 = 0.25;
const W2_SCOPE:           f32 = 0.20;
const W3_TRUST_CONTEXT:   f32 = 0.20;
const W4_TIME_ANOMALY:    f32 = 0.10;
const W5_VIBE_FLAG:       f32 = 0.10;
const W6_USER_HISTORY:    f32 = 0.10;
const W7_PATTERN_MATCH:   f32 = 0.05;

// Compile-time assertion: weights sum to 1.0 within floating-point tolerance.
const _: () = {
    let sum = W1_IRREVERSIBILITY + W2_SCOPE + W3_TRUST_CONTEXT
        + W4_TIME_ANOMALY + W5_VIBE_FLAG + W6_USER_HISTORY + W7_PATTERN_MATCH;
    // f32 const arithmetic: check within 0.001
    assert!((sum - 1.0_f32).abs() < 0.001_f32);
};

// ─── Scorer ───────────────────────────────────────────────────────────────────

/// Compute the risk score R(A) for a proposed action.
///
/// This is the entry point for all HAL scoring decisions.
/// Hard floor rules are applied after the formula and cannot be overridden.
pub fn score_action(action: &ProposedAction, _context: &SystemContext) -> RiskScore {
    // Component raw scores
    let s_irreversibility = action.irreversibility.score();
    let s_scope           = action.scope.score();
    let s_trust_context   = action.trust_context.score();
    let s_time_anomaly    = action.time_anomaly.score();
    let s_vibe_flag       = action.vibe_flag.score();
    let s_user_history    = action.user_history.score();
    let s_pattern_match   = action.pattern_match.score();

    // Apply the formal formula
    let raw_score = W1_IRREVERSIBILITY * s_irreversibility
        + W2_SCOPE           * s_scope
        + W3_TRUST_CONTEXT   * s_trust_context
        + W4_TIME_ANOMALY    * s_time_anomaly
        + W5_VIBE_FLAG       * s_vibe_flag
        - W6_USER_HISTORY    * s_user_history
        - W7_PATTERN_MATCH   * s_pattern_match;

    // Clamp to [0.0, 1.0] before applying hard floors
    let mut score = raw_score.clamp(0.0, 1.0);

    // Hard floors — these cannot be overridden by any weighting or history.
    // Security justification: these actions have catastrophic failure modes
    // that no amount of established trust can fully mitigate.
    let mut floor_applied = false;
    let mut floor_reason: Option<String> = None;

    if action.is_delete {
        if score < 0.5 {
            score = 0.5;
            floor_applied = true;
            floor_reason = Some("Hard floor: delete actions always ≥ 0.5 regardless of history".to_string());
        }
    }

    if action.is_kernel_adjacent || action.scope == ScopeLevel::KernelLevel {
        if score < 0.7 {
            score = 0.7;
            floor_applied = true;
            floor_reason = Some("Hard floor: kernel-adjacent actions always ≥ 0.7".to_string());
        }
    }

    if matches!(action.vibe_flag, VibeFlagLevel::AiGeneratedUnreviewed | VibeFlagLevel::AiGeneratedKernelAdjacent) {
        if score < 0.8 {
            score = 0.8;
            floor_applied = true;
            floor_reason = Some("Hard floor: AI-generated unreviewed code always ≥ 0.8".to_string());
        }
    }

    let level = RiskLevel::from_score(score);
    let explanation = describe_score_internal(
        action, score, &level,
        s_irreversibility, s_scope, s_trust_context,
        s_time_anomaly, s_vibe_flag, s_user_history, s_pattern_match,
        floor_applied, floor_reason.as_deref(),
    );

    RiskScore {
        score,
        level,
        explanation,
        components: ComponentScores {
            irreversibility:     s_irreversibility,
            scope:               s_scope,
            trust_context:       s_trust_context,
            time_anomaly:        s_time_anomaly,
            vibe_flag:           s_vibe_flag,
            user_history:        s_user_history,
            pattern_match:       s_pattern_match,
            hard_floor_applied:  floor_applied,
            hard_floor_reason:   floor_reason,
        },
    }
}

/// Generate a plain-English explanation of the score for the user dialog.
///
/// This explanation is what appears in HAL's confirmation and block dialogs.
/// It must be concise, honest, and actionable.
pub fn describe_score(score: &RiskScore) -> String {
    score.explanation.clone()
}

fn describe_score_internal(
    action: &ProposedAction,
    score: f32,
    level: &RiskLevel,
    s_irrev: f32, s_scope: f32, s_trust: f32,
    s_time: f32, s_vibe: f32, s_hist: f32, _s_pat: f32,
    floor_applied: bool, floor_reason: Option<&str>,
) -> String {
    let mut parts: Vec<String> = Vec::new();

    // Lead with the action and level
    let level_label = match level {
        RiskLevel::Silent  => "auto-approved",
        RiskLevel::Notify  => "notification sent",
        RiskLevel::Confirm => "requires confirmation",
        RiskLevel::Block   => "blocked — explicit approval required",
    };
    parts.push(format!(
        "{} on '{}': {} (score {:.2})",
        action.action_type, action.target, level_label, score
    ));

    // Explain significant positive contributors (risk-raising)
    if s_irrev >= 0.7 {
        parts.push("This action is difficult or impossible to reverse.".to_string());
    } else if s_irrev >= 0.3 {
        parts.push("Reversible, but not trivially so.".to_string());
    }

    if s_scope >= 0.7 {
        parts.push("Affects system-wide or multiple-user scope.".to_string());
    }

    if s_trust >= 0.7 {
        parts.push("Source is new or unverified.".to_string());
    }

    if s_vibe >= 0.8 {
        parts.push("This action involves AI-generated code that has not been human-reviewed.".to_string());
    }

    if s_time >= 0.5 {
        parts.push("This action is occurring outside your normal usage patterns.".to_string());
    }

    // Explain significant negative contributors (trust-reducing risk)
    if s_hist >= 0.7 {
        parts.push(format!(
            "You have done this {} times before in similar context.",
            if s_hist >= 1.0 { "over 100" } else { "over 20" }
        ));
    }

    if floor_applied {
        if let Some(reason) = floor_reason {
            parts.push(format!("Note: {}", reason));
        }
    }

    parts.join(" ")
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> SystemContext {
        SystemContext::default()
    }

    fn base_action() -> ProposedAction {
        ProposedAction {
            action_type: "open_file".to_string(),
            target: "~/motor.py".to_string(),
            agent: "file".to_string(),
            irreversibility: IrreversibilityLevel::FullyReversible,
            scope: ScopeLevel::SingleFileUserHome,
            trust_context: TrustContextLevel::KnownTrusted,
            time_anomaly: TimeAnomalyLevel::Normal,
            vibe_flag: VibeFlagLevel::None,
            user_history: UserHistoryLevel::Routine,
            pattern_match: PatternMatchLevel::ExactMatch,
            is_kernel_adjacent: false,
            is_delete: false,
        }
    }

    #[test]
    fn routine_open_file_is_silent() {
        let score = score_action(&base_action(), &ctx());
        assert_eq!(score.level, RiskLevel::Silent, "score={}", score.score);
    }

    #[test]
    fn delete_hard_floor_respected() {
        let mut action = base_action();
        action.action_type = "delete_file".to_string();
        action.irreversibility = IrreversibilityLevel::Irreversible;
        action.is_delete = true;
        // Give maximum history to try to bring score below floor
        action.user_history = UserHistoryLevel::Routine;
        action.pattern_match = PatternMatchLevel::ExactMatch;

        let score = score_action(&action, &ctx());
        assert!(score.score >= 0.5, "delete floor violated: score={}", score.score);
        assert!(score.level != RiskLevel::Silent, "delete must not be Silent");
        assert!(score.components.hard_floor_applied);
    }

    #[test]
    fn kernel_floor_respected() {
        let mut action = base_action();
        action.scope = ScopeLevel::KernelLevel;
        action.is_kernel_adjacent = true;
        action.user_history = UserHistoryLevel::Routine;

        let score = score_action(&action, &ctx());
        assert!(score.score >= 0.7, "kernel floor violated: score={}", score.score);
    }

    #[test]
    fn ai_unreviewed_floor_respected() {
        let mut action = base_action();
        action.vibe_flag = VibeFlagLevel::AiGeneratedUnreviewed;
        action.user_history = UserHistoryLevel::Routine;
        action.pattern_match = PatternMatchLevel::ExactMatch;

        let score = score_action(&action, &ctx());
        assert!(score.score >= 0.8, "AI code floor violated: score={}", score.score);
        assert_eq!(score.level, RiskLevel::Block);
    }

    #[test]
    fn unknown_binary_scores_high() {
        let mut action = base_action();
        action.trust_context = TrustContextLevel::Unknown;
        action.vibe_flag = VibeFlagLevel::AiGeneratedKernelAdjacent;
        action.scope = ScopeLevel::KernelLevel;
        action.is_kernel_adjacent = true;

        let score = score_action(&action, &ctx());
        assert_eq!(score.level, RiskLevel::Block);
        assert!(score.score >= 0.8);
    }

    #[test]
    fn score_always_in_range() {
        // Worst possible action
        let worst = ProposedAction {
            action_type: "format_disk".to_string(),
            target: "/dev/sda".to_string(),
            agent: "unknown".to_string(),
            irreversibility: IrreversibilityLevel::Irreversible,
            scope: ScopeLevel::KernelLevel,
            trust_context: TrustContextLevel::Unknown,
            time_anomaly: TimeAnomalyLevel::UnusualTimeAndScope,
            vibe_flag: VibeFlagLevel::AiGeneratedKernelAdjacent,
            user_history: UserHistoryLevel::Never,
            pattern_match: PatternMatchLevel::NoMatch,
            is_kernel_adjacent: true,
            is_delete: true,
        };
        let score = score_action(&worst, &ctx());
        assert!((0.0..=1.0).contains(&score.score));

        // Best possible action
        let best = base_action();
        let score = score_action(&best, &ctx());
        assert!((0.0..=1.0).contains(&score.score));
    }

    #[test]
    fn explanation_is_non_empty() {
        let score = score_action(&base_action(), &ctx());
        assert!(!score.explanation.is_empty());
        assert!(score.explanation.contains("open_file") || score.explanation.contains("motor.py"));
    }

    #[test]
    fn weights_sum_to_one() {
        let sum = W1_IRREVERSIBILITY + W2_SCOPE + W3_TRUST_CONTEXT
            + W4_TIME_ANOMALY + W5_VIBE_FLAG + W6_USER_HISTORY + W7_PATTERN_MATCH;
        assert!((sum - 1.0).abs() < 0.001, "Weights sum to {}", sum);
    }

    #[test]
    fn higher_history_reduces_score() {
        let mut low_hist = base_action();
        low_hist.user_history = UserHistoryLevel::Never;

        let mut high_hist = base_action();
        high_hist.user_history = UserHistoryLevel::Routine;

        // Must not involve delete (floor would obscure the comparison)
        let score_low  = score_action(&low_hist,  &ctx()).score;
        let score_high = score_action(&high_hist, &ctx()).score;
        assert!(score_high <= score_low, "Higher history should reduce or equal score");
    }
}
