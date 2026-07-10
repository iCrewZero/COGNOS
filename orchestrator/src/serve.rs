//! Orchestrator ingress — serves `DispatchIntent` on the orchestrator endpoint
//! and runs the full submit → HAL gate → execute → respond pipeline.

use std::net::SocketAddr;
use std::sync::Arc;

use cognos_ipc_grpc::pipeline_metrics::log_stage;
use cognos_ipc_grpc::proto::v1::{Intent, IntentResponse};
use cognos_ipc_grpc::server::{CognosServer, IntentHandler, ServerConfig};
use tokio::sync::Mutex;
use tracing::{error, info};

use crate::runtime::OrchestratorRuntime;

/// Shared orchestrator state behind the gRPC ingress.
pub struct OrchestratorService {
    runtime: Arc<Mutex<OrchestratorRuntime>>,
}

impl OrchestratorService {
    pub fn new(runtime: Arc<Mutex<OrchestratorRuntime>>) -> Self {
        Self { runtime }
    }
}

#[async_trait::async_trait]
impl IntentHandler for OrchestratorService {
    async fn handle(&self, intent: &Intent) -> IntentResponse {
        let ingress_started = std::time::Instant::now();
        let trace_id = if intent.trace_id.is_empty() {
            uuid::Uuid::new_v4().to_string()
        } else {
            intent.trace_id.clone()
        };
        let utterance = if !intent.utterance.is_empty() {
            intent.utterance.clone()
        } else {
            intent.action.clone()
        };

        let mut rt = self.runtime.lock().await;
        let response = match rt.submit_and_execute(&utterance, &intent.session_id, &trace_id).await {
            Ok(report) => {
                let ingress_ms = ingress_started.elapsed().as_millis() as u64;
                log_stage(&trace_id, "ingress_total", ingress_ms);
                tracing::info!(
                    trace_id = %trace_id,
                    stage = "ingress_total",
                    latency_ms = ingress_ms,
                    success = report.success,
                    "pipeline stage"
                );
                let result_json = serde_json::to_vec(&report).unwrap_or_default();
                IntentResponse {
                    intent_id: report.intent_id.clone(),
                    status: if report.success { "ok" } else { "failed" }.to_string(),
                    result_json,
                    message: report.summary.clone(),
                    violation: None,
                    completed_at_ns: chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0) as u64,
                    action_graph: None,
                    trace_id: trace_id.clone(),
                }
            }
            Err(e) => {
                let ingress_ms = ingress_started.elapsed().as_millis() as u64;
                log_stage(&trace_id, "ingress_total", ingress_ms);
                IntentResponse {
                    intent_id: intent.intent_id.clone(),
                    status: "failed".to_string(),
                    result_json: Vec::new(),
                    message: e.to_string(),
                    violation: None,
                    completed_at_ns: chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0) as u64,
                    action_graph: None,
                    trace_id,
                }
            }
        };
        response
    }
}

/// Bind the orchestrator ingress server (`DispatchIntent` → full pipeline).
pub async fn serve_ingress(
    runtime: Arc<Mutex<OrchestratorRuntime>>,
    bind: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let addr: SocketAddr = bind.parse()?;
    let handler = Arc::new(OrchestratorService::new(runtime));
    let mut cfg = ServerConfig::default();
    cfg.bind_addr = bind.to_string();
    cfg.self_capability = "intent.dispatch".to_string();
    let server = CognosServer::with_config(cfg).with_intent_handler(handler);
    info!(%bind, "orchestrator DispatchIntent ingress serving");
    server.serve(addr).await.map_err(|e| {
        error!(error = %e, "orchestrator ingress server failed");
        Box::new(e) as Box<dyn std::error::Error + Send + Sync>
    })
}
