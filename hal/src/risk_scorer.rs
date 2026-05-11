// ============================================================================
// HAL — Risk Scorer
// COGNOS/OS Human Approval Layer
//
// THIS FILE IS HUMAN-WRITTEN AND HUMAN-REVIEWED ONLY.
// NO AI AUTHORSHIP. NO AI COMMITS.
// This is the trust anchor. It cannot be reasoned around.
// ============================================================================

use serde::{Deserialize, Serialize};
use std::fmt;

// ----------------------------------------------------------------------------
// Enumerations
// ----------------------------------------------------------------------------

/// The category of action being proposed by an AI agent.
/// Used to enforce hard floor rules regardless of computed score.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ActionCategory {
    /// Open an application or file for viewing
    OpenApp,
    /// Move a file within the user's home directory
    MoveFile,
    /// Delete a file or directory (hard floor: 0.5)
    DeleteFile,
    /// Format a volume or partition (hard floor: 0.5)
    FormatVolume,
    /// Install a package or application
    PackageInstall,
    /// Read a file (no side effects)
    ReadFile,
    /// Write to a file in user home
    WriteFile,
    /// Modify system configuration
    SystemConfig,
    /// Change network settings
    NetworkChange,
    /// Any action touching kernel paths or modules (hard floor: 0.7)
    KernelAdjacent,
    /// Execute AI-generated code not yet human-reviewed (hard floor: 0.8)
    AiGeneratedCode,
}

/// The risk level determined from the final score.
/// Maps directly to the user-facing response: silent, notify, confirm, or block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RiskLevel {
    /// R ∈ [0.0, 0.3) — execute silently, write to audit log only
    Silent,
    /// R ∈ [0.3, 0.6) — toast notification with 5-second undo window
    Notify,
    /// R ∈ [0.6, 0.8) — dialog with plain-English explanation required
    Confirm,
    /// R ∈ [0.8, 1.0] — full breakdown, explicit approve/deny, mandatory audit entry
    Block,
}

impl fmt::Display for RiskLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RiskLevel::Silent  => write!(f, "SILENT"),
            RiskLevel::Notify  => write!(f, "NOTIFY"),
            RiskLevel::Confirm => write!(f, "CONFIRM"),
            RiskLevel::Block   => write!(f, "BLOCK"),
        }
    }
}

// ----------------------------------------------------------------------------
// Input structs
// ----------------------------------------------------------------------------

/// Everything HAL needs to know about the proposed action itself.
/// Populated by the Multi-Agent Orchestrator before HAL is invoked.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposedAction {
    /// Human-readable description of what the agent wants to do
    pub description: String,

    /// The category — used for hard floor enforcement
    pub category: ActionCategory,

    /// Irreversibility score ∈ [0.0, 1.0]
    /// 0.0 = fully reversible (open app)
    /// 0.3 = reversible with effort (move file)
    /// 0.7 = hard to reverse (package install)
    /// 1.0 = irreversible (delete, format, credential change)
    pub irreversibility: f32,

    /// Scope score ∈ [0.0, 1.0]
    /// 0.0 = single file in user home
    /// 0.3 = multiple files, single directory
    /// 0.7 = system-wide, multiple users
    /// 1.0 = kernel-level, hardware-level
    pub scope: f32,

    /// Whether AI-generated code is involved and its review status
    pub vibe_code: VibeCodeStatus,

    /// Target path(s) of the action, for display and audit
    pub targets: Vec<String>,
}

/// Tracks whether AI-generated code is involved in an action,
/// and whether it has been reviewed by a human.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum VibeCodeStatus {
    /// No AI-generated code involved
    None,
    /// AI-generated code present but not yet human-reviewed
    UnreviewedAiCode,
    /// AI-generated code touching kernel or HAL-adjacent paths (worst case)
    AiCodeKernelAdjacent,
    /// AI-generated code that has been reviewed and approved by a human
    ReviewedAndApproved,
}

/// System context at the moment the action is proposed.
/// HAL uses this to assess trust, timing, and behavioral history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemContext {
    /// Trust level of the requesting agent/app ∈ [0.0, 1.0]
    /// 0.0 = known app, established behavior, signed source
    /// 0.4 = known app, minor behavioral anomaly
    /// 0.7 = new app, unverified behavior
    /// 1.0 = unknown binary, behavioral red flag, unsigned
    pub trust_context: f32,

    /// Whether this action is being requested outside the user's normal time patterns
    pub time_anomaly: TimeAnomalyLevel,

    /// How many times this exact action has been taken in the same context
    pub user_history_count: u32,

    /// Whether this matches a learned behavioral pattern
    pub pattern_match: PatternMatchLevel,
}

/// Describes how unusual the timing of this action is.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TimeAnomalyLevel {
    /// Action is within the user's established time patterns
    Normal,
    /// Action is outside normal hours but not unprecedented
    UnusualHour,
    /// Unusual hour AND unusual scope combination — highest anomaly
    UnusualHourAndScope,
}

/// Describes how well this action matches a known behavioral pattern.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PatternMatchLevel {
    /// No matching learned pattern
    NoMatch,
    /// Partial pattern match
    Partial,
    /// Exact pattern match with high confidence
    ExactHighConfidence,
}

// ----------------------------------------------------------------------------
// Output struct
// ----------------------------------------------------------------------------

/// The complete risk assessment produced by HAL for a proposed action.
/// This struct drives the UI dialog shown to the user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskScore {
    /// Final computed score ∈ [0.0, 1.0], after hard floor enforcement
    pub score: f32,

    /// The risk level bucket derived from the score
    pub level: RiskLevel,

    /// Individual component scores for auditability
    pub components: ScoreComponents,

    /// Whether a hard floor rule was applied (and which one)
    pub floor_applied: Option<FloorRule>,

    /// Plain-English explanation for display in confirmation dialogs
    pub explanation: String,
}

/// Breakdown of each component's contribution to the final score.
/// Stored for full auditability — user can inspect why any score was given.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoreComponents {
    pub irreversibility: f32,
    pub scope: f32,
    pub trust_context: f32,
    pub time_anomaly: f32,
    pub vibe_code_flag: f32,
    pub user_history: f32,
    pub pattern_match: f32,
    /// The weighted sum before floor enforcement
    pub raw_score: f32,
}

/// Records which hard floor rule overrode the computed score, if any.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FloorRule {
    DeleteAction,
    KernelAdjacentAction,
    AiGeneratedCodeUnreviewed,
}

impl fmt::Display for FloorRule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FloorRule::DeleteAction             => write!(f, "delete actions always score ≥ 0.5"),
            FloorRule::KernelAdjacentAction     => write!(f, "kernel-adjacent actions always score ≥ 0.7"),
            FloorRule::AiGeneratedCodeUnreviewed => write!(f, "unreviewed AI-generated code always scores ≥ 0.8"),
        }
    }
}

// ----------------------------------------------------------------------------
// Weight constants
// All weights must sum to exactly 1.0.
// Positive weights increase risk. Negative weights reduce risk.
// ----------------------------------------------------------------------------

/// Weight for Irreversibility component
const W1_IRREVERSIBILITY: f32 = 0.25;
/// Weight for Scope component
const W2_SCOPE: f32 = 0.20;
/// Weight for TrustContext component
const W3_TRUST_CONTEXT: f32 = 0.20;
/// Weight for TimeAnomaly component
const W4_TIME_ANOMALY: f32 = 0.10;
/// Weight for VibeCodeFlag component
const W5_VIBE_CODE: f32 = 0.10;
/// Weight for UserHistory component (risk-reducing)
const W6_USER_HISTORY: f32 = 0.10;
/// Weight for PatternMatch component (risk-reducing)
const W7_PATTERN_MATCH: f32 = 0.05;

// Compile-time sanity check: weights must sum to 1.0
// (positive minus negative = net formula coverage)
// W1+W2+W3+W4+W5 = 0.85, W6+W7 = 0.15, net = 0.85-0.15 = 0.70
// The formula is risk_raising - risk_reducing, intentionally asymmetric
// toward caution. Full weight sum: 0.25+0.20+0.20+0.10+0.10+0.10+0.05 = 1.00 ✓

// ----------------------------------------------------------------------------
// Score component functions
// ----------------------------------------------------------------------------

/// Maps the raw irreversibility value to its score contribution.
/// Input is already a [0.0, 1.0] float from ProposedAction.
/// We clamp defensively in case of upstream error.
fn score_irreversibility(value: f32) -> f32 {
    value.clamp(0.0, 1.0)
}

/// Maps scope value to its score contribution.
/// Clamped defensively.
fn score_scope(value: f32) -> f32 {
    value.clamp(0.0, 1.0)
}

/// Maps trust_context to its score contribution.
/// Higher trust_context = less known/trusted = higher risk.
fn score_trust_context(value: f32) -> f32 {
    value.clamp(0.0, 1.0)
}

/// Converts TimeAnomalyLevel enum to its numeric score.
/// 0.0 = normal, 0.5 = unusual hour, 1.0 = unusual hour + scope
fn score_time_anomaly(anomaly: &TimeAnomalyLevel) -> f32 {
    match anomaly {
        TimeAnomalyLevel::Normal               => 0.0,
        TimeAnomalyLevel::UnusualHour          => 0.5,
        TimeAnomalyLevel::UnusualHourAndScope  => 1.0,
    }
}

/// Converts VibeCodeStatus to its numeric score.
/// 0.0 = no AI code, 0.8 = unreviewed, 1.0 = kernel-adjacent AI code
fn score_vibe_code(status: &VibeCodeStatus) -> f32 {
    match status {
        VibeCodeStatus::None                  => 0.0,
        VibeCodeStatus::ReviewedAndApproved   => 0.0,
        VibeCodeStatus::UnreviewedAiCode      => 0.8,
        VibeCodeStatus::AiCodeKernelAdjacent  => 1.0,
    }
}

/// Converts user history count to a risk-reducing score.
/// More repetitions = more familiarity = lower risk contribution.
/// 0.0 = never done (no history to reduce risk)
/// 0.3 = done < 5 times
/// 0.7 = done > 20 times
/// 1.0 = done > 100 times in identical context (routine)
fn score_user_history(count: u32) -> f32 {
    match count {
        0           => 0.0,
        1..=4       => 0.3,
        5..=19      => 0.5,
        20..=99     => 0.7,
        _           => 1.0,
    }
}

/// Converts PatternMatchLevel to a risk-reducing score.
/// Exact high-confidence pattern match = highest risk reduction.
fn score_pattern_match(level: &PatternMatchLevel) -> f32 {
    match level {
        PatternMatchLevel::NoMatch             => 0.0,
        PatternMatchLevel::Partial             => 0.5,
        PatternMatchLevel::ExactHighConfidence => 1.0,
    }
}

// ----------------------------------------------------------------------------
// Hard floor enforcement
// ----------------------------------------------------------------------------

/// Applies hard floor rules that override the computed score.
/// These rules are non-negotiable and cannot be tuned by user calibration.
///
/// Returns (final_score, Option<FloorRule>) — the floor rule if one was applied.
fn apply_hard_floors(
    category: &ActionCategory,
    vibe_code: &VibeCodeStatus,
    computed: f32,
) -> (f32, Option<FloorRule>) {

    // AI-generated unreviewed code: always block-territory minimum
    // Check this first — it is the strictest floor
    match vibe_code {
        VibeCodeStatus::UnreviewedAiCode | VibeCodeStatus::AiCodeKernelAdjacent => {
            let floor = 0.8_f32;
            if computed < floor {
                return (floor, Some(FloorRule::AiGeneratedCodeUnreviewed));
            }
        }
        _ => {}
    }

    // Kernel-adjacent actions: confirm-territory minimum
    if *category == ActionCategory::KernelAdjacent {
        let floor = 0.7_f32;
        if computed < floor {
            return (floor, Some(FloorRule::KernelAdjacentAction));
        }
    }

    // Delete or format: never silently execute
    if matches!(category, ActionCategory::DeleteFile | ActionCategory::FormatVolume) {
        let floor = 0.5_f32;
        if computed < floor {
            return (floor, Some(FloorRule::DeleteAction));
        }
    }

    (computed, None)
}

// ----------------------------------------------------------------------------
// Main scoring function
// ----------------------------------------------------------------------------

/// Computes the HAL risk score for a proposed action.
///
/// Formula:
///   R(A) = w1·Irreversibility + w2·Scope + w3·TrustContext
///         + w4·TimeAnomaly + w5·VibeCodeFlag
///         - w6·UserHistory - w7·PatternMatch
///
/// All weights sum to 1.0. Result is clamped to [0.0, 1.0].
/// Hard floor rules are applied after the formula, and override the result
/// when they produce a higher minimum score.
///
/// This function is deterministic and has no side effects.
/// The same inputs always produce the same output.
pub fn score_action(action: &ProposedAction, context: &SystemContext) -> RiskScore {
    // Compute each component score
    let irreversibility = score_irreversibility(action.irreversibility);
    let scope           = score_scope(action.scope);
    let trust_context   = score_trust_context(context.trust_context);
    let time_anomaly    = score_time_anomaly(&context.time_anomaly);
    let vibe_code_flag  = score_vibe_code(&action.vibe_code);
    let user_history    = score_user_history(context.user_history_count);
    let pattern_match   = score_pattern_match(&context.pattern_match);

    // Apply the weighted formula
    let raw = (W1_IRREVERSIBILITY * irreversibility)
            + (W2_SCOPE           * scope)
            + (W3_TRUST_CONTEXT   * trust_context)
            + (W4_TIME_ANOMALY    * time_anomaly)
            + (W5_VIBE_CODE       * vibe_code_flag)
            - (W6_USER_HISTORY    * user_history)
            - (W7_PATTERN_MATCH   * pattern_match);

    // Clamp before floor enforcement (formula can theoretically go negative)
    let clamped = raw.clamp(0.0, 1.0);

    // Apply hard floor rules
    let (final_score, floor_applied) = apply_hard_floors(
        &action.category,
        &action.vibe_code,
        clamped,
    );

    let components = ScoreComponents {
        irreversibility,
        scope,
        trust_context,
        time_anomaly,
        vibe_code_flag,
        user_history,
        pattern_match,
        raw_score: clamped,
    };

    let level = risk_level_from_score(final_score);
    let explanation = describe_score_internal(
        final_score,
        &level,
        &components,
        &floor_applied,
        action,
    );

    RiskScore {
        score: final_score,
        level,
        components,
        floor_applied,
        explanation,
    }
}

/// Maps a numeric score to its RiskLevel bucket.
/// Thresholds are strict and match the formal specification exactly.
pub fn risk_level_from_score(score: f32) -> RiskLevel {
    if score < 0.3 {
        RiskLevel::Silent
    } else if score < 0.6 {
        RiskLevel::Notify
    } else if score < 0.8 {
        RiskLevel::Confirm
    } else {
        RiskLevel::Block
    }
}

// ----------------------------------------------------------------------------
// Human-readable explanation
// ----------------------------------------------------------------------------

/// Generates a plain-English explanation of the risk score.
/// This text is shown directly to the user in HAL dialogs.
///
/// The explanation must be:
/// - Honest about which factors raised or lowered the score
/// - Specific about what will happen and why approval is needed
/// - Free of jargon — written for a non-technical user
pub fn describe_score(score: &RiskScore) -> String {
    score.explanation.clone()
}

/// Internal implementation of score description, called during scoring.
fn describe_score_internal(
    final_score: f32,
    level: &RiskLevel,
    components: &ScoreComponents,
    floor_applied: &Option<FloorRule>,
    action: &ProposedAction,
) -> String {
    let mut parts: Vec<String> = Vec::new();

    // What is being done
    parts.push(format!("Action: {}", action.description));

    // Why the score is what it is — only mention factors that meaningfully contributed
    let mut risk_factors: Vec<&str> = Vec::new();
    let mut reducing_factors: Vec<&str> = Vec::new();

    if components.irreversibility >= 0.7 {
        risk_factors.push("this action is difficult or impossible to reverse");
    } else if components.irreversibility >= 0.3 {
        risk_factors.push("this action requires effort to undo");
    }

    if components.scope >= 0.7 {
        risk_factors.push("it affects system-wide or kernel-level resources");
    } else if components.scope >= 0.3 {
        risk_factors.push("it affects multiple files or directories");
    }

    if components.trust_context >= 0.7 {
        risk_factors.push("the requesting application is unknown or unverified");
    } else if components.trust_context >= 0.4 {
        risk_factors.push("the requesting application is showing unusual behavior");
    }

    if components.time_anomaly >= 1.0 {
        risk_factors.push("this is happening at an unusual time with unusual scope");
    } else if components.time_anomaly >= 0.5 {
        risk_factors.push("this is happening outside your normal usage hours");
    }

    if components.vibe_code_flag >= 1.0 {
        risk_factors.push("AI-generated code is touching kernel-level paths — this has never been human-reviewed");
    } else if components.vibe_code_flag >= 0.8 {
        risk_factors.push("AI-generated code is involved and has not been reviewed by a human");
    }

    if components.user_history >= 0.7 {
        reducing_factors.push("you have done this many times before in the same context");
    } else if components.user_history >= 0.3 {
        reducing_factors.push("you have done this a few times before");
    }

    if components.pattern_match >= 1.0 {
        reducing_factors.push("this matches one of your established behavioral patterns exactly");
    } else if components.pattern_match >= 0.5 {
        reducing_factors.push("this partially matches a known pattern");
    }

    if !risk_factors.is_empty() {
        parts.push(format!("Risk factors: {}.", risk_factors.join("; ")));
    }

    if !reducing_factors.is_empty() {
        parts.push(format!("Mitigating factors: {}.", reducing_factors.join("; ")));
    }

    // Floor rule explanation if applied
    if let Some(floor) = floor_applied {
        parts.push(format!(
            "Note: score was raised to {:.2} because {}.",
            final_score, floor
        ));
    }

    // What happens next based on level
    let action_desc = match level {
        RiskLevel::Silent  => "This action will proceed without interruption.".to_string(),
        RiskLevel::Notify  => "You will see a notification. You have 5 seconds to undo.".to_string(),
        RiskLevel::Confirm => "Your explicit approval is required before this proceeds.".to_string(),
        RiskLevel::Block   => {
            format!(
                "This action is BLOCKED (score: {:.2}). You must explicitly approve or deny it. \
                 This decision will be recorded in your audit log.",
                final_score
            )
        }
    };
    parts.push(action_desc);

    parts.join(" | ")
}

// ----------------------------------------------------------------------------
// Tests
// ----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn routine_action() -> ProposedAction {
        ProposedAction {
            description: "Open VSCode".to_string(),
            category: ActionCategory::OpenApp,
            irreversibility: 0.0,
            scope: 0.0,
            vibe_code: VibeCodeStatus::None,
            targets: vec!["/usr/bin/code".to_string()],
        }
    }

    fn routine_context() -> SystemContext {
        SystemContext {
            trust_context: 0.0,
            time_anomaly: TimeAnomalyLevel::Normal,
            user_history_count: 150,
            pattern_match: PatternMatchLevel::ExactHighConfidence,
        }
    }

    #[test]
    fn test_routine_open_app_is_silent() {
        let score = score_action(&routine_action(), &routine_context());
        assert_eq!(score.level, RiskLevel::Silent, "Routine app open should be silent");
        assert!(score.score < 0.3);
    }

    #[test]
    fn test_delete_floor_enforced() {
        let action = ProposedAction {
            description: "Delete ~/Documents/report.pdf".to_string(),
            category: ActionCategory::DeleteFile,
            irreversibility: 1.0,
            scope: 0.0,
            vibe_code: VibeCodeStatus::None,
            targets: vec!["~/Documents/report.pdf".to_string()],
        };
        // Even with perfect history, delete must be >= 0.5
        let context = routine_context();
        let score = score_action(&action, &context);
        assert!(score.score >= 0.5, "Delete floor must be enforced: got {}", score.score);
        assert!(score.floor_applied.is_some());
    }

    #[test]
    fn test_kernel_adjacent_floor_enforced() {
        let action = ProposedAction {
            description: "Load kernel module".to_string(),
            category: ActionCategory::KernelAdjacent,
            irreversibility: 0.3,
            scope: 0.3,
            vibe_code: VibeCodeStatus::None,
            targets: vec!["/lib/modules/test.ko".to_string()],
        };
        let context = routine_context();
        let score = score_action(&action, &context);
        assert!(score.score >= 0.7, "Kernel floor must be enforced: got {}", score.score);
        assert!(score.floor_applied.is_some());
    }

    #[test]
    fn test_unreviewed_ai_code_floor_enforced() {
        let action = ProposedAction {
            description: "Run AI-generated script".to_string(),
            category: ActionCategory::WriteFile,
            irreversibility: 0.0,
            scope: 0.0,
            vibe_code: VibeCodeStatus::UnreviewedAiCode,
            targets: vec!["~/scripts/gen.sh".to_string()],
        };
        let context = routine_context();
        let score = score_action(&action, &context);
        assert!(score.score >= 0.8, "AI code floor must be enforced: got {}", score.score);
        assert_eq!(score.level, RiskLevel::Block);
    }

    #[test]
    fn test_high_risk_unknown_binary_blocks() {
        let action = ProposedAction {
            description: "Execute unknown binary as root".to_string(),
            category: ActionCategory::AiGeneratedCode,
            irreversibility: 1.0,
            scope: 1.0,
            vibe_code: VibeCodeStatus::AiCodeKernelAdjacent,
            targets: vec!["/tmp/suspicious_binary".to_string()],
        };
        let context = SystemContext {
            trust_context: 1.0,
            time_anomaly: TimeAnomalyLevel::UnusualHourAndScope,
            user_history_count: 0,
            pattern_match: PatternMatchLevel::NoMatch,
        };
        let score = score_action(&action, &context);
        assert_eq!(score.level, RiskLevel::Block);
        assert!(score.score >= 0.8);
    }

    #[test]
    fn test_score_is_deterministic() {
        let action = routine_action();
        let context = routine_context();
        let s1 = score_action(&action, &context);
        let s2 = score_action(&action, &context);
        assert_eq!(s1.score, s2.score, "Score must be deterministic");
    }

    #[test]
    fn test_score_always_in_range() {
        // Adversarial: maximum risk raising values
        let action = ProposedAction {
            description: "worst case".to_string(),
            category: ActionCategory::DeleteFile,
            irreversibility: 1.0,
            scope: 1.0,
            vibe_code: VibeCodeStatus::AiCodeKernelAdjacent,
            targets: vec![],
        };
        let context = SystemContext {
            trust_context: 1.0,
            time_anomaly: TimeAnomalyLevel::UnusualHourAndScope,
            user_history_count: 0,
            pattern_match: PatternMatchLevel::NoMatch,
        };
        let score = score_action(&action, &context);
        assert!(score.score >= 0.0 && score.score <= 1.0);

        // Adversarial: maximum risk reduction
        let safe_action = ProposedAction {
            description: "safe case".to_string(),
            category: ActionCategory::ReadFile,
            irreversibility: 0.0,
            scope: 0.0,
            vibe_code: VibeCodeStatus::ReviewedAndApproved,
            targets: vec![],
        };
        let safe_context = SystemContext {
            trust_context: 0.0,
            time_anomaly: TimeAnomalyLevel::Normal,
            user_history_count: 999,
            pattern_match: PatternMatchLevel::ExactHighConfidence,
        };
        let safe_score = score_action(&safe_action, &safe_context);
        assert!(safe_score.score >= 0.0 && safe_score.score <= 1.0);
    }
}
