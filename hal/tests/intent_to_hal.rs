//! Cross-crate integration: the Phase 1 pipeline end to end.
//!
//! LLM JSON → schema validation → disambiguation auto-resolve →
//! action graph ordering → HAL R(A) scoring.
//!
//! This is the sovereignty contract in executable form: the intent engine
//! proposes, HAL scores, and hard floors hold regardless of how the
//! proposal was generated.

use cognos_hal::{
    score_action, IrreversibilityLevel, PatternMatchLevel, ProposedAction,
    RiskLevel, ScopeLevel, SystemContext, TimeAnomalyLevel, TrustContextLevel,
    UserHistoryLevel, VibeFlagLevel,
};
use cognos_intent_engine::{
    parse_llm_output, ActionGraph, ActionNode, DisambiguationEngine,
};

/// LLM output for "open my robotics work" with a clear recency winner,
/// matching the example schema in docs/SPEC.md.
fn robotics_intent_json() -> &'static str {
    r#"{
        "intent_id": "550e8400-e29b-41d4-a716-446655440000",
        "raw_input": "open my robotics work",
        "goal": "open_workspace",
        "domain": "robotics",
        "confidence": 0.82,
        "ambiguity_score": 0.65,
        "risk_estimate": 0.14,
        "hal_pre_score": 0.14,
        "required_context": ["recent_project"],
        "candidate_actions": [
            {
                "action": "open_files",
                "target": "~/projects/robo-arm/motor.py",
                "confidence": 0.71,
                "recency_score": 0.9
            },
            {
                "action": "open_files",
                "target": "~/projects/pid-tuning/",
                "confidence": 0.45,
                "recency_score": 0.3
            }
        ],
        "disambiguation_required": true,
        "disambiguation_question": "The motor driver from March or the PID tuning project?",
        "session_context": {
            "last_active_domain": "robotics",
            "last_active_files": ["motor.py", "config.yaml"],
            "current_time": "14:32",
            "time_since_last_session": "2h"
        },
        "escalate_to_cloud": false
    }"#
}

/// Map an ordered action node to a HAL proposal. In the full system the
/// orchestrator does this with live trust/behavior inputs; here we pin
/// known levels so threshold assertions are deterministic.
fn to_proposal(node: &ActionNode) -> ProposedAction {
    ProposedAction {
        action_type: node.action.clone(),
        target: node.target.clone(),
        agent: "file_agent".into(),
        irreversibility: IrreversibilityLevel::FullyReversible,
        scope: ScopeLevel::SingleFileUserHome,
        trust_context: TrustContextLevel::KnownTrusted,
        time_anomaly: TimeAnomalyLevel::Normal,
        vibe_flag: VibeFlagLevel::None,
        user_history: UserHistoryLevel::Frequent,
        pattern_match: PatternMatchLevel::PartialMatch,
        is_kernel_adjacent: false,
        is_delete: false,
    }
}

#[test]
fn open_workspace_intent_executes_silently() {
    // 1. Validate the LLM output against the intent schema.
    let schema = parse_llm_output(robotics_intent_json())
        .expect("spec example must validate");
    assert!(schema.disambiguation_required);

    // 2. Disambiguation: recency gap > 0.4 → auto-resolve, zero questions.
    let engine = DisambiguationEngine::load();
    let resolved = engine
        .try_auto_resolve(&schema)
        .expect("large recency gap must auto-resolve without a question");
    assert!(!resolved.was_disambiguated);
    assert_eq!(resolved.selected_action.target, "~/projects/robo-arm/motor.py");

    // 3. Action graph: ordered, audit-linked to the intent.
    let graph = ActionGraph::from_resolved(&resolved, &schema);
    let order = graph.execution_order().expect("single node orders");
    assert_eq!(order.len(), 1);
    assert_eq!(order[0].intent_id, schema.intent_id);

    // 4. HAL scores the node: benign open → SILENT band.
    let score = score_action(&to_proposal(&order[0]), &SystemContext::default());
    assert_eq!(score.level, RiskLevel::Silent, "explanation: {}", score.explanation);
}

#[test]
fn hal_floors_hold_regardless_of_intent_provenance() {
    // Same pipeline, but the resolved action is a delete. However the
    // intent was produced, HAL's hard floor must hold (>= 0.5, NOTIFY+).
    let schema = parse_llm_output(robotics_intent_json()).expect("validates");
    let engine = DisambiguationEngine::load();
    let resolved = engine.try_auto_resolve(&schema).expect("auto-resolves");
    let graph = ActionGraph::from_resolved(&resolved, &schema);
    let order = graph.execution_order().expect("orders");

    let mut proposal = to_proposal(&order[0]);
    proposal.action_type = "delete_file".into();
    proposal.irreversibility = IrreversibilityLevel::Irreversible;
    proposal.is_delete = true;
    proposal.user_history = UserHistoryLevel::Routine;
    proposal.pattern_match = PatternMatchLevel::ExactMatch;

    let score = score_action(&proposal, &SystemContext::default());
    assert!(score.score >= 0.5);
    assert_ne!(score.level, RiskLevel::Silent);
}

#[test]
fn ai_generated_code_from_intent_pipeline_always_blocks() {
    // Threat model: "HAL operates on the action graph, not raw LLM output."
    // An AI-generated-code action arriving via the intent pipeline must
    // floor at 0.8 (BLOCK) even with perfect history.
    let schema = parse_llm_output(robotics_intent_json()).expect("validates");
    let engine = DisambiguationEngine::load();
    let resolved = engine.try_auto_resolve(&schema).expect("auto-resolves");
    let graph = ActionGraph::from_resolved(&resolved, &schema);
    let order = graph.execution_order().expect("orders");

    let mut proposal = to_proposal(&order[0]);
    proposal.action_type = "apply_generated_patch".into();
    proposal.vibe_flag = VibeFlagLevel::AiGeneratedUnreviewed;
    proposal.user_history = UserHistoryLevel::Routine;
    proposal.pattern_match = PatternMatchLevel::ExactMatch;

    let score = score_action(&proposal, &SystemContext::default());
    assert_eq!(score.level, RiskLevel::Block);
    assert!(score.components.hard_floor_applied);
}
