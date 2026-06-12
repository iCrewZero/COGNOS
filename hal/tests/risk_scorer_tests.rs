//! Formal model conformance tests for the HAL risk scorer (docs/SPEC.md).

use cognos_hal::{
    score_action, IrreversibilityLevel, PatternMatchLevel, ProposedAction,
    RiskLevel, ScopeLevel, SystemContext, TimeAnomalyLevel, TrustContextLevel,
    UserHistoryLevel, VibeFlagLevel,
};

fn action() -> ProposedAction {
    ProposedAction {
        action_type: "open_files".into(),
        target: "~/projects/motor.py".into(),
        agent: "file_agent".into(),
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
fn benign_routine_action_is_silent() {
    let score = score_action(&action(), &SystemContext::default());
    assert!(score.score < 0.3, "score {} not in SILENT band", score.score);
    assert_eq!(score.level, RiskLevel::Silent);
}

#[test]
fn score_is_always_within_bounds() {
    // Negative raw scores (history/pattern subtraction) must clamp to 0.0.
    let benign = score_action(&action(), &SystemContext::default());
    assert!(benign.score >= 0.0);

    // Maximal risk inputs must clamp to <= 1.0.
    let mut worst = action();
    worst.irreversibility = IrreversibilityLevel::Irreversible;
    worst.scope = ScopeLevel::KernelLevel;
    worst.trust_context = TrustContextLevel::Unknown;
    worst.time_anomaly = TimeAnomalyLevel::UnusualTimeAndScope;
    worst.vibe_flag = VibeFlagLevel::AiGeneratedKernelAdjacent;
    worst.user_history = UserHistoryLevel::Never;
    worst.pattern_match = PatternMatchLevel::NoMatch;
    worst.is_kernel_adjacent = true;
    worst.is_delete = true;
    let score = score_action(&worst, &SystemContext::default());
    assert!(score.score <= 1.0);
    assert_eq!(score.level, RiskLevel::Block);
}

#[test]
fn delete_hard_floor_cannot_be_gamed_by_history() {
    // Routine history + exact pattern match on a delete: formula alone would
    // land far below 0.5 — the HAL-bypass-via-timing-attack mitigation
    // requires delete >= 0.5 regardless.
    let mut del = action();
    del.action_type = "delete_file".into();
    del.irreversibility = IrreversibilityLevel::Irreversible;
    del.is_delete = true;
    let score = score_action(&del, &SystemContext::default());
    assert!(score.score >= 0.5, "delete floored at 0.5, got {}", score.score);
    assert!(score.components.hard_floor_applied);
}

#[test]
fn kernel_adjacent_floor_is_confirm_or_higher() {
    let mut k = action();
    k.is_kernel_adjacent = true;
    let score = score_action(&k, &SystemContext::default());
    assert!(score.score >= 0.7);
    assert!(matches!(score.level, RiskLevel::Confirm | RiskLevel::Block));
}

#[test]
fn unreviewed_ai_code_always_blocks() {
    let mut v = action();
    v.vibe_flag = VibeFlagLevel::AiGeneratedUnreviewed;
    let score = score_action(&v, &SystemContext::default());
    assert!(score.score >= 0.8);
    assert_eq!(score.level, RiskLevel::Block);
}

#[test]
fn explanation_is_never_empty() {
    let score = score_action(&action(), &SystemContext::default());
    assert!(!score.explanation.trim().is_empty());
}
