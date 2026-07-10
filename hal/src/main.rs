//! HAL daemon entrypoint — starts the Human Approval Layer service.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use cognos_ipc_grpc::agent::{self, AgentSpec};
use cognos_ipc_grpc::pipeline_metrics::{log_stage, METRICS};
use cognos_ipc_grpc::proto::v1::{CapabilityViolation, HalGateRequest, HalGateResponse};
use cognos_ipc_grpc::server::{CognosServer, HalGateHandler, ServerConfig};

use cognos_hal::action_validator::ActionValidator;
use cognos_hal::hal_types::{
    BehavioralMetrics, CapabilityContext, HALContext, IntentSeverity, ProvenanceConfidence,
    ProvenanceData, SessionContext, SyscallSensitivity,
};
use cognos_hal::{
    score_action, IrreversibilityLevel, PatternMatchLevel, ProposedAction, RiskLevel, ScopeLevel,
    SystemContext, TimeAnomalyLevel, TrustContextLevel, UserHistoryLevel, VibeFlagLevel,
};

/// Default bind address for HAL's own gate RPC surface. Overridable via
/// `COGNOS_HAL_BIND` so tests can pick a free port.
const DEFAULT_HAL_BIND: &str = "127.0.0.1:7444";

// ─── HAL gate policy handler (transport adapter only) ──────────────────────────
//
// This adapter is *wiring*, not policy: it translates a wire `HalGateRequest`
// into HAL's existing public policy inputs and translates the existing policy
// output back to a wire `HalGateResponse`. It does NOT implement or alter any
// scoring formula, weight, threshold, or hard floor. All decisions are produced
// by the unmodified `cognos_hal` library:
//   * dangerous-path / destructive-pattern detection ← `ActionValidator::validate`
//   * the risk score and its band                     ← `score_action`
// See docs/HAL_AUDIT.md for the security review of this handler.

struct PolicyHalGate;

impl HalGateHandler for PolicyHalGate {
    fn evaluate(&self, req: &HalGateRequest) -> HalGateResponse {
        let started = std::time::Instant::now();
        let op = req.op.as_str();
        let path = req.device.as_str();

        // 1. Reuse HAL's existing rule set to classify the target path. We build
        //    a neutral context and read only the violations HAL reports; the
        //    dangerous-path list and destructive patterns live in `hal/src` and
        //    are used verbatim.
        let validation = ActionValidator::validate(&neutral_context(op, path), 0.0, 1.0);
        let dangerous = validation
            .violated_rules
            .iter()
            .any(|v| v == "dangerous_path_access");
        let destructive = validation
            .violated_rules
            .iter()
            .any(|v| v == "destructive_pattern");

        // 2. Delegate the risk decision to HAL's unmodified scorer. Only the
        //    inputs are mapped here; the formula and floors are HAL's.
        let is_delete = is_delete_op(op);
        let proposal = ProposedAction {
            action_type: op.to_string(),
            target: path.to_string(),
            agent: "agent.orchestrator".to_string(),
            irreversibility: if is_delete {
                IrreversibilityLevel::Irreversible
            } else {
                IrreversibilityLevel::FullyReversible
            },
            // A dangerous system path widens the blast radius to kernel-level,
            // which lets HAL's own kernel hard floor decide the band.
            scope: if dangerous {
                ScopeLevel::KernelLevel
            } else {
                ScopeLevel::SingleFileUserHome
            },
            trust_context: TrustContextLevel::KnownTrusted,
            time_anomaly: TimeAnomalyLevel::Normal,
            vibe_flag: VibeFlagLevel::None,
            user_history: UserHistoryLevel::Routine,
            pattern_match: PatternMatchLevel::ExactMatch,
            is_kernel_adjacent: dangerous,
            is_delete,
        };
        let score = score_action(&proposal, &SystemContext::default());

        // 3. Map HAL's decision band to the wire status. A destructive pattern
        //    is a hard block per HAL's validator; otherwise the risk band drives
        //    the outcome (Confirm → approval, Block → denied).
        let (status, violation) = if destructive {
            (
                "denied",
                Some(gate_violation(
                    &req.capability,
                    "destructive command pattern blocked by HAL",
                    path,
                    &req.trace_id,
                )),
            )
        } else {
            match score.level {
                RiskLevel::Silent | RiskLevel::Notify => ("granted", None),
                RiskLevel::Confirm => {
                    if req.allow_approval {
                        ("approval_required", None)
                    } else {
                        (
                            "denied",
                            Some(gate_violation(
                                &req.capability,
                                "user confirmation required but approval flow disabled",
                                path,
                                &req.trace_id,
                            )),
                        )
                    }
                }
                RiskLevel::Block => (
                    "denied",
                    Some(gate_violation(
                        &req.capability,
                        &score.explanation,
                        path,
                        &req.trace_id,
                    )),
                ),
            }
        };

        let grant_token = if status == "granted" {
            uuid::Uuid::new_v4().to_string()
        } else {
            String::new()
        };

        METRICS.record_hal_status(status);
        let gate_ms = started.elapsed().as_millis() as u64;
        log_stage(&req.trace_id, "hal_gate", gate_ms);
        tracing::info!(
            trace_id = %req.trace_id,
            stage = "hal_gate",
            latency_ms = gate_ms,
            hal_status = %status,
            op = %op,
            device = %path,
            "pipeline stage"
        );

        HalGateResponse {
            status: status.to_string(),
            grant_token,
            risk_score: score.score as f64,
            data: Vec::new(),
            violation,
            trace_id: req.trace_id.clone(),
        }
    }
}

/// True when the op names a delete/unlink. Kept intentionally simple; the
/// authoritative reversibility signal is HAL's own scorer, this only sets the
/// `is_delete` input.
fn is_delete_op(op: &str) -> bool {
    let lower = op.to_lowercase();
    lower.contains("delete")
        || lower.contains("unlink")
        || lower == "rm"
        || lower.starts_with("rm ")
        || lower.contains("remove")
}

/// Build a minimal, benign [`HALContext`] carrying only the fields
/// [`ActionValidator::validate`] consults for path/pattern classification
/// (`target_resource`, `requested_action`). Everything else is neutral so the
/// validator's *other* checks stay inert — we read only the path/pattern rules.
fn neutral_context(op: &str, path: &str) -> HALContext {
    HALContext {
        intent_id: String::new(),
        source_agent: "agent.orchestrator".to_string(),
        target_resource: path.to_string(),
        requested_action: op.to_string(),
        severity: IntentSeverity::Low,
        syscall_sensitivity: SyscallSensitivity::Safe,
        provenance: ProvenanceData {
            source_agent: "agent.orchestrator".to_string(),
            certificate_fingerprint: String::new(),
            trust_chain_hash: String::new(),
            signature_verified: true,
            replay_checked: true,
            confidence: ProvenanceConfidence::Trusted,
        },
        behavioral: BehavioralMetrics {
            anomaly_score: 0.0,
            volatility_score: 0.0,
            escalation_attempts: 0,
            historical_stability: 1.0,
            recent_failures: 0,
        },
        session: SessionContext {
            session_id: String::new(),
            user_present: true,
            active_workspace: String::new(),
            active_window_title: String::new(),
            requires_confirmation: false,
            user_attention_score: 1.0,
        },
        capabilities: CapabilityContext {
            granted_capabilities: Vec::new(),
            temporary_grants: Vec::new(),
            denied_capabilities: Vec::new(),
            capability_expiry_ms: 0,
        },
        metadata: HashMap::new(),
    }
}

fn gate_violation(required: &str, message: &str, path: &str, trace_id: &str) -> CapabilityViolation {
    CapabilityViolation {
        required: required.to_string(),
        held: String::new(),
        reason: "scope".to_string(),
        message: format!("{message} (target: {path})"),
        agent_id: "agent.hal".to_string(),
        trace_id: trace_id.to_string(),
    }
}

fn main() {
    env_logger::init();

    // Transport wiring only: register HAL as an agent of the central IPC
    // server, keep a heartbeat alive, and serve the HalGate RPC (delegating to
    // the unmodified HAL policy library). This does NOT touch risk scoring,
    // policy evaluation, audit, or any HAL decision logic — it is purely the
    // client/heartbeat/server plumbing. The daemon's blocking gate loop still
    // runs on the main thread (Unix only) exactly as before.
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime for IPC agent");

    let hal_bind = std::env::var("COGNOS_HAL_BIND")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_HAL_BIND.to_string());
    let hal_addr: SocketAddr = hal_bind.parse().expect("invalid COGNOS_HAL_BIND address");

    let ipc = rt.block_on(async move {
        // Serve HalGate on HAL's own endpoint so callers (e.g. the orchestrator)
        // reach HAL's real policy rather than the central server's stub.
        let mut server_cfg = ServerConfig::default();
        server_cfg.bind_addr = hal_bind.clone();
        server_cfg.self_capability = "hal.gate".to_string();
        let server = CognosServer::with_config(server_cfg)
            .with_hal_gate_handler(Arc::new(PolicyHalGate));
        tokio::spawn(async move {
            if let Err(e) = server.serve(hal_addr).await {
                log::error!("HAL gate server exited with error: {e}");
            }
        });
        log::info!("HAL gate RPC serving on {hal_addr}");

        agent::spawn(AgentSpec::from_env(
            "agent.hal",
            vec![
                "hal.gate".to_string(),
                "risk.score".to_string(),
                "audit.append".to_string(),
            ],
        ))
        .await
    });

    #[cfg(unix)]
    {
        // The heartbeat + gate-server tasks keep running on the runtime's worker
        // threads while the daemon blocks the main thread here.
        cognos_hal::HalDaemon::new().run();
        rt.block_on(ipc.stop());
    }

    #[cfg(not(unix))]
    {
        eprintln!(
            "cognos-hal daemon requires a Unix platform; IPC agent + HalGate RPC are running. Press Ctrl-C to exit."
        );
        rt.block_on(async {
            tokio::signal::ctrl_c().await.ok();
        });
        rt.block_on(ipc.stop());
    }
}
