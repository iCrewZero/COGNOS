//! End-to-end intent pipeline — binary orchestration mirroring `scripts/dev_e2e.sh`.
//!
//! Runs the full stack with `MOCK_LLM=1` (deterministic) plus an offline path
//! where llama is unreachable and keyword fallback completes the pipeline.

mod approval_responder;
mod support;

use approval_responder::UiResponderMode;

use cognos_intent_engine::backends::fallback::KEYWORD_FALLBACK_SOURCE;
use cognos_intent_engine::backends::mock_llama::{
    GOLDEN_AMBIGUOUS_UTTERANCE, GOLDEN_APPROVAL_UTTERANCE, GOLDEN_BENIGN_UTTERANCE,
    GOLDEN_CONFIRM_DELETE_UTTERANCE, GOLDEN_HOME_DELETE_UTTERANCE, MOCK_LLM_SOURCE,
};

use support::{
    benign_target_dir, hal_note_contains, remove_dir_if_exists, E2eApprovalConfig,
    E2eCluster, IntentSchemaLite, LlmMode,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[cfg_attr(not(target_os = "linux"), ignore = "binary E2E requires Linux")]
async fn golden_benign_mkdir_granted_and_created() {
    let cluster = E2eCluster::start(LlmMode::Mock).await;
    let target = benign_target_dir();
    remove_dir_if_exists(&target);

    let intent_resp = cluster
        .dispatch_intent_engine(GOLDEN_BENIGN_UTTERANCE)
        .await;
    assert_eq!(intent_resp.status, "ok", "{}", intent_resp.message);
    let schema = IntentSchemaLite::from_response(&intent_resp);
    assert_eq!(schema.source.as_deref(), Some(MOCK_LLM_SOURCE));
    assert_eq!(schema.goal, "create_dir");

    let report = cluster
        .dispatch_orchestrator(GOLDEN_BENIGN_UTTERANCE)
        .await;
    assert!(report.success, "report: {:?}", report);
    assert_eq!(report.tasks.len(), 1);
    let task = &report.tasks[0];
    assert_eq!(task.action, "create_dir");
    assert_eq!(task.target, "/tmp/test");
    assert_eq!(task.status, "succeeded");
    assert!(
        hal_note_contains(&task.hal_decision, "granted"),
        "expected HAL granted, got {:?}",
        task.hal_decision
    );
    assert!(
        target.is_dir(),
        "expected {} to exist after benign mkdir E2E",
        target.display()
    );

    if let Some(lat) = &report.latency {
        assert!(
            lat.total_ms < 2000,
            "mock E2E latency regression guard: {}ms (limit 2000ms)",
            lat.total_ms
        );
    }

    let (hal_m, intent_m, orch_m) = cluster.pipeline_metrics().await;
    assert!(
        hal_m.as_ref().map(|m| m.hal_granted).unwrap_or(0) >= 1,
        "HAL granted counter should be visible after E2E"
    );
    assert!(
        intent_m.as_ref().map(|m| m.parser_cache_misses).unwrap_or(0) >= 1,
        "parser cache_miss counter should be visible after E2E"
    );
    assert!(
        orch_m.as_ref().map(|m| m.last_total_latency_ms).unwrap_or(0.0) > 0.0,
        "orchestrator latency sample should be recorded"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[cfg_attr(not(target_os = "linux"), ignore = "binary E2E requires Linux")]
async fn golden_dangerous_delete_hal_blocks_execution() {
    let cluster = E2eCluster::start_with(
        LlmMode::Mock,
        E2eApprovalConfig {
            ui_responder: Some(UiResponderMode::Deny),
            approval_timeout_secs: None,
        },
    )
    .await;

    let intent_resp = cluster
        .dispatch_intent_engine(GOLDEN_APPROVAL_UTTERANCE)
        .await;
    assert_eq!(intent_resp.status, "ok", "{}", intent_resp.message);
    let schema = IntentSchemaLite::from_response(&intent_resp);
    assert_eq!(schema.source.as_deref(), Some(MOCK_LLM_SOURCE));
    assert!(!schema.disambiguation_required);

    let report = cluster
        .dispatch_orchestrator_raw(GOLDEN_APPROVAL_UTTERANCE)
        .await
        .1;
    assert!(!report.success, "dangerous delete must not succeed: {:?}", report);
    assert_eq!(report.tasks.len(), 1);
    let task = &report.tasks[0];
    assert_eq!(task.action, "delete_files");
    assert_eq!(task.target, "/boot");
    assert!(
        task.status == "denied" || task.status == "awaiting_hal",
        "expected HAL gate deny or park, got status={} hal={:?}",
        task.status,
        task.hal_decision
    );
    assert!(
        hal_note_contains(&task.hal_decision, "approval")
            || hal_note_contains(&task.hal_decision, "denied")
            || hal_note_contains(&task.hal_decision, "user"),
        "HAL must require approval or deny, got {:?}",
        task.hal_decision
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[cfg_attr(not(target_os = "linux"), ignore = "binary E2E requires Linux")]
async fn golden_ambiguous_disambiguation_no_destructive_effects() {
    let cluster = E2eCluster::start(LlmMode::Mock).await;
    let target = benign_target_dir();
    remove_dir_if_exists(&target);

    let intent_resp = cluster
        .dispatch_intent_engine(GOLDEN_AMBIGUOUS_UTTERANCE)
        .await;
    assert_eq!(intent_resp.status, "ok", "{}", intent_resp.message);
    let schema = IntentSchemaLite::from_response(&intent_resp);
    assert_eq!(schema.source.as_deref(), Some(MOCK_LLM_SOURCE));
    assert!(schema.disambiguation_required);
    assert_eq!(schema.candidate_actions.len(), 2);

    let graph = intent_resp
        .action_graph
        .expect("ambiguous golden must return action graph");
    assert_eq!(graph.nodes.len(), 2);
    assert!(graph.nodes.iter().all(|n| n.action == "open_files"));

    let report = cluster
        .dispatch_orchestrator(GOLDEN_AMBIGUOUS_UTTERANCE)
        .await;
    assert_eq!(report.tasks.len(), 2, "two parallel open_files candidates");
    for task in &report.tasks {
        assert_eq!(task.action, "open_files");
        assert!(
            task.hal_decision.is_none(),
            "read-only open_files must not hit HAL gate, got {:?}",
            task.hal_decision
        );
        assert_eq!(task.status, "succeeded");
    }
    assert!(
        !target.exists(),
        "ambiguous intent must not create /tmp/test as a side effect"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[cfg_attr(not(target_os = "linux"), ignore = "binary E2E requires Linux")]
async fn offline_keyword_fallback_completes_pipeline() {
    let cluster = E2eCluster::start(LlmMode::OfflineKeywordFallback).await;
    let target = benign_target_dir();
    remove_dir_if_exists(&target);

    let intent_resp = cluster
        .dispatch_intent_engine(GOLDEN_BENIGN_UTTERANCE)
        .await;
    assert_eq!(intent_resp.status, "ok", "{}", intent_resp.message);
    let schema = IntentSchemaLite::from_response(&intent_resp);
    assert_eq!(
        schema.source.as_deref(),
        Some(KEYWORD_FALLBACK_SOURCE),
        "offline path must use keyword_fallback provenance"
    );
    assert_eq!(schema.goal, "create_dir");

    let report = cluster
        .dispatch_orchestrator(GOLDEN_BENIGN_UTTERANCE)
        .await;
    assert!(report.success, "offline fallback pipeline must complete: {:?}", report);
    assert_eq!(report.tasks.len(), 1);
    let task = &report.tasks[0];
    assert_eq!(task.action, "create_dir");
    assert_eq!(task.status, "succeeded");
    assert!(
        hal_note_contains(&task.hal_decision, "granted"),
        "expected HAL granted on offline path, got {:?}",
        task.hal_decision
    );
    assert!(
        target.is_dir(),
        "offline keyword fallback must still create {}",
        target.display()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[cfg_attr(not(target_os = "linux"), ignore = "binary E2E requires Linux")]
async fn golden_approval_socket_auto_approve_completes_home_delete() {
    let cluster = E2eCluster::start_with(
        LlmMode::Mock,
        E2eApprovalConfig {
            ui_responder: Some(UiResponderMode::Approve),
            approval_timeout_secs: None,
        },
    )
    .await;

    let report = cluster
        .dispatch_orchestrator_raw(GOLDEN_HOME_DELETE_UTTERANCE)
        .await
        .1;
    assert_eq!(report.tasks.len(), 1);
    let task = &report.tasks[0];
    assert_eq!(task.action, "install_package");
    assert_eq!(task.target, "e2e-test-tool");
    assert!(
        task.status == "succeeded" || task.status == "failed",
        "approval path must complete execution attempt, got {:?}",
        task
    );
    assert!(
        hal_note_contains(&task.hal_decision, "user_approved")
            || hal_note_contains(&task.hal_decision, "granted"),
        "expected user approval via UI socket, got {:?}",
        task.hal_decision
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[cfg_attr(not(target_os = "linux"), ignore = "binary E2E requires Linux")]
async fn golden_approval_timeout_denies_when_ui_hangs() {
    let cluster = E2eCluster::start_with(
        LlmMode::Mock,
        E2eApprovalConfig {
            ui_responder: Some(UiResponderMode::Hang),
            approval_timeout_secs: Some(2),
        },
    )
    .await;

    let report = cluster
        .dispatch_orchestrator_raw(GOLDEN_CONFIRM_DELETE_UTTERANCE)
        .await
        .1;
    assert!(!report.success, "timeout must not succeed: {:?}", report);
    assert_eq!(report.tasks.len(), 1);
    let task = &report.tasks[0];
    assert_eq!(task.status, "denied");
    assert!(
        hal_note_contains(&task.hal_decision, "timeout"),
        "expected orchestrator timeout deny, got {:?}",
        task.hal_decision
    );
}
