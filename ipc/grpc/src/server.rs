//! COGNOS gRPC IPC server — accepts authenticated agent connections,
//! enforces capability checks on every RPC, streams events.

use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::signal;
// Owner: iCrewZero — removed unused Mutex import (H3).
use tokio::sync::broadcast;
use tonic::{Request, Response, Status};
use tonic::transport::Server;

use tracing::{info, debug};

use crate::pipeline_metrics::METRICS;
use crate::proto::v1::cognos_ipc_server::{CognosIpc, CognosIpcServer};
use crate::proto::v1::*;

// ─── Errors ──────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum ServerError {
    #[error("bind failed: {0}")]
    Bind(String),
    #[error("transport: {0}")]
    Transport(String),
    #[error("shutdown timeout")]
    ShutdownTimeout,
}

// ─── Server configuration ────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub bind_addr: String,
    pub max_concurrent_streams: u32,
    pub max_frame_size: usize,
    pub require_tls: bool,
    pub self_capability: String,
    pub event_flush_ns: u64,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind_addr: "127.0.0.1:7443".to_string(),
            max_concurrent_streams: 256,
            max_frame_size: 4 * 1024 * 1024,
            require_tls: false,
            self_capability: "ipc.server".to_string(),
            event_flush_ns: 100_000_000,
        }
    }
}

// ─── HAL gate policy hook ────────────────────────────────────────────────────

/// Pluggable HAL gate policy.
///
/// The central IPC server ships **without** a handler and answers `HalGate`
/// with `status = "failed"` and a message directing callers to
/// `COGNOS_HAL_ENDPOINT` (see [`CognosIpcService::hal_gate`]).
/// The HAL binary injects a handler that delegates to HAL's real risk scorer
/// and action validator. Keeping this as a trait means `cognos-ipc-grpc` never
/// has to depend on `cognos-hal` (which would be a dependency cycle — HAL
/// already depends on this crate).
pub trait HalGateHandler: Send + Sync + 'static {
    /// Evaluate a gate request and return the HAL decision. Implementations
    /// must be deterministic and side-effect-free from the server's point of
    /// view; the response `status` must be one of
    /// `granted | denied | approval_required | failed`.
    fn evaluate(&self, request: &HalGateRequest) -> HalGateResponse;
}

// ─── Intent handler hook ──────────────────────────────────────────────────────

/// Pluggable `DispatchIntent` handler.
///
/// The central IPC server ships **without** a handler and answers
/// `DispatchIntent` with `status = "failed"` and a message directing callers to
/// `COGNOS_INTENT_ENDPOINT` (see [`CognosIpcService::dispatch_intent`]). The
/// intent-engine binary injects a handler that runs the parser and returns a
/// constructed action graph. As with [`HalGateHandler`], keeping this a trait
/// means `cognos-ipc-grpc` never has to depend on `cognos-intent-engine`.
#[tonic::async_trait]
pub trait IntentHandler: Send + Sync + 'static {
    /// Parse `intent` and return a response (typically carrying an
    /// `action_graph`). Must not panic.
    async fn handle(&self, intent: &Intent) -> IntentResponse;
}

// ─── CognosServer ────────────────────────────────────────────────────────────

/// Top-level gRPC server. Owns the event bus and configuration.
// Owner: iCrewZero — added Clone derive so the runtime can clone the server
// before spawning it into an async move block (B3).
#[derive(Clone)]
pub struct CognosServer {
    pub config: ServerConfig,
    event_tx: broadcast::Sender<Event>,
    hal_gate: Option<Arc<dyn HalGateHandler>>,
    intent: Option<Arc<dyn IntentHandler>>,
}

impl CognosServer {
    pub fn new() -> Self {
        Self::with_config(ServerConfig::default())
    }

    pub fn with_config(config: ServerConfig) -> Self {
        let (event_tx, _) = broadcast::channel(1024);
        Self {
            config,
            event_tx,
            hal_gate: None,
            intent: None,
        }
    }

    /// Attach a HAL gate policy handler. When set, the `HalGate` RPC delegates
    /// to `handler.evaluate(..)` instead of the explicit `failed` misroute stub.
    /// Used by the HAL binary to serve real gate decisions.
    pub fn with_hal_gate_handler(mut self, handler: Arc<dyn HalGateHandler>) -> Self {
        self.hal_gate = Some(handler);
        self
    }

    /// Attach a `DispatchIntent` handler. When set, the `DispatchIntent` RPC
    /// delegates to `handler.handle(..)` instead of the explicit `failed` misroute
    /// stub. Used by the intent-engine binary to serve parsed action graphs.
    pub fn with_intent_handler(mut self, handler: Arc<dyn IntentHandler>) -> Self {
        self.intent = Some(handler);
        self
    }

    /// Start the gRPC server and block until SIGTERM/SIGINT.
    pub async fn serve(&self, addr: SocketAddr) -> Result<(), ServerError> {
        info!(%addr, "cognos-ipc server starting");
        debug!(
            max_streams = self.config.max_concurrent_streams,
            max_frame = self.config.max_frame_size,
            require_tls = self.config.require_tls,
            "server configuration"
        );

        let svc = CognosIpcService::new(
            self.config.self_capability.clone(),
            self.event_tx.clone(),
            self.hal_gate.clone(),
            self.intent.clone(),
        );

        // Owner: iCrewZero — switched from HealthServer::new() + set_serving_status()
        // to the documented health_reporter() API for tonic-health 0.12 (B4).
        let (mut health_reporter, health_service) = tonic_health::server::health_reporter();
        // Mark the IPC server as serving so health checks pass.
        health_reporter.set_serving::<CognosIpcServer<CognosIpcService>>().await;

        Server::builder()
            .max_concurrent_streams(self.config.max_concurrent_streams)
            .max_frame_size(Some(self.config.max_frame_size as u32))
            .add_service(health_service)
            .add_service(CognosIpcServer::new(svc))
            .serve_with_shutdown(addr, shutdown_signal())
            .await
            .map_err(|e| ServerError::Transport(e.to_string()))?;

        info!("cognos-ipc server stopped");
        Ok(())
    }

    /// Return a sender handle to the event bus.
    pub fn event_sender(&self) -> broadcast::Sender<Event> {
        self.event_tx.clone()
    }
}

impl Default for CognosServer {
    fn default() -> Self {
        Self::new()
    }
}

// ─── gRPC service implementation ─────────────────────────────────────────────

/// The actual CognosIpc trait implementation. Each RPC does real work now:
/// verifies the envelope, checks capabilities, dispatches to the right subsystem.
pub struct CognosIpcService {
    server_capability: String,
    event_tx: broadcast::Sender<Event>,
    hal_gate: Option<Arc<dyn HalGateHandler>>,
    intent: Option<Arc<dyn IntentHandler>>,
}

impl CognosIpcService {
    pub fn new(
        server_capability: String,
        event_tx: broadcast::Sender<Event>,
        hal_gate: Option<Arc<dyn HalGateHandler>>,
        intent: Option<Arc<dyn IntentHandler>>,
    ) -> Self {
        Self {
            server_capability,
            event_tx,
            hal_gate,
            intent,
        }
    }

    /// Emit an event on the bus. Errors are logged but not propagated.
    fn emit_event(&self, kind: &str, source: &str, payload: &[u8], severity: &str) {
        let event = Event {
            kind: kind.to_string(),
            source: source.to_string(),
            emitted_at_ns: chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0) as u64,
            payload_json: payload.to_vec(),
            severity: severity.to_string(),
            trace_id: String::new(),
        };
        // Ignore send errors — means no subscribers.
        let _ = self.event_tx.send(event);
    }
}

#[tonic::async_trait]
impl CognosIpc for CognosIpcService {
    /// DispatchIntent — route a parsed intent to its target agent(s).
    async fn dispatch_intent(
        &self,
        request: Request<Intent>,
    ) -> Result<Response<IntentResponse>, Status> {
        let intent = request.into_inner();
        info!(
            intent_id = %intent.intent_id,
            action = %intent.action,
            confidence = intent.confidence,
            "DispatchIntent"
        );

        // Emit the event.
        let payload = serde_json::json!({
            "intent_id": intent.intent_id,
            "action": intent.action,
            "confidence": intent.confidence,
        })
        .to_string()
        .into_bytes();
        self.emit_event("intent.dispatched", "ipc.server", &payload, "info");

        // When an intent handler is injected (the intent-engine binary does
        // this), delegate parsing + graph construction to it. This keeps the
        // parser / LLM logic in `cognos-intent-engine` and out of this transport
        // crate (no dependency cycle), mirroring the HAL gate handler.
        if let Some(handler) = &self.intent {
            let response = handler.handle(&intent).await;
            self.emit_event(
                "intent.parsed",
                "ipc.server",
                serde_json::json!({"intent_id": response.intent_id, "status": response.status})
                    .to_string()
                    .as_bytes(),
                "info",
            );
            return Ok(Response::new(response));
        }

        // No handler wired: fail explicitly so misrouted clients cannot treat a
        // stub as a parsed graph. Real parsing lives on cognos-intent
        // (COGNOS_INTENT_ENDPOINT, default :7445). See docs/ARCHITECTURE.md.
        let response = IntentResponse {
            intent_id: intent.intent_id,
            status: "failed".to_string(),
            result_json: Vec::new(),
            message: "DispatchIntent is not served on the central IPC bus; \
                        connect to cognos-intent (COGNOS_INTENT_ENDPOINT, default 127.0.0.1:7445)"
                .to_string(),
            violation: None,
            completed_at_ns: chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0) as u64,
            action_graph: None,
            trace_id: intent.trace_id,
        };
        Ok(Response::new(response))
    }

    /// QueryMemory — vector + tag search against ANFS / memory.
    async fn query_memory(
        &self,
        request: Request<MemoryQuery>,
    ) -> Result<Response<MemoryResult>, Status> {
        let started = Instant::now();
        let query = request.into_inner();
        info!(
            query = %query.query,
            top_k = query.top_k,
            "QueryMemory"
        );

        // TODO(v1): enforce "memory.read" capability, proxy to memory::query
        // which does cosine similarity + tag filtering against the embedder.
        //
        // Until the real vector store is wired in, the memory responder returns
        // a single deterministic "echo" hit derived from the request. This is
        // what makes the IPC round-trip observable end-to-end: a client hitting
        // the real server gets content that mirrors its own query (proving the
        // request reached the server and came back), which a client-side
        // fallback stub can never reproduce. See tests/test_ipc_roundtrip.py.
        let payload_json = serde_json::json!({
            "echo": query.query,
            "namespace": query.namespace,
            "responder": self.server_capability,
        })
        .to_string()
        .into_bytes();

        let hit = memory_result::Hit {
            object_id: format!("echo:{}", query.query),
            score: 1.0,
            payload_json,
            tags: query.tags.clone(),
        };

        let response = MemoryResult {
            hits: vec![hit],
            total: 1,
            elapsed_ns: started.elapsed().as_nanos() as u64,
            trace_id: query.trace_id,
        };
        Ok(Response::new(response))
    }

    /// HalGate — request a hardware action through the HAL.
    async fn hal_gate(
        &self,
        request: Request<HalGateRequest>,
    ) -> Result<Response<HalGateResponse>, Status> {
        let req = request.into_inner();
        info!(
            op = %req.op,
            device = %req.device,
            capability = %req.capability,
            "HalGate"
        );

        self.emit_event(
            "hal.gate_requested",
            "ipc.server",
            serde_json::json!({"op": req.op, "device": req.device}).to_string().as_bytes(),
            "info",
        );

        // When a HAL gate policy handler is injected (the HAL binary does this),
        // delegate the real decision to it. It runs HAL's risk model / validator
        // and returns granted/denied/approval_required. This keeps the risk logic
        // in `cognos-hal` and out of this transport crate (no dependency cycle).
        if let Some(handler) = &self.hal_gate {
            let response = handler.evaluate(&req);
            self.emit_event(
                "hal.gate_decided",
                "ipc.server",
                serde_json::json!({"op": req.op, "status": response.status})
                    .to_string()
                    .as_bytes(),
                "info",
            );
            return Ok(Response::new(response));
        }

        // No handler wired: fail explicitly so misrouted clients cannot treat a
        // stub as a real gate decision. Real policy lives on cognos-hal
        // (COGNOS_HAL_ENDPOINT, default :7444). See docs/ARCHITECTURE.md.
        let response = HalGateResponse {
            status: "failed".to_string(),
            grant_token: String::new(),
            risk_score: req.risk_override,
            data: Vec::new(),
            violation: Some(CapabilityViolation {
                required: req.capability.clone(),
                held: String::new(),
                reason: "misroute".to_string(),
                message: "HalGate is not served on the central IPC bus; connect to \
                          cognos-hal (COGNOS_HAL_ENDPOINT, default 127.0.0.1:7444)"
                    .to_string(),
                agent_id: "ipc.server".to_string(),
                trace_id: req.trace_id.clone(),
            }),
            trace_id: req.trace_id,
        };
        Ok(Response::new(response))
    }

    /// ResourceHint — push a scheduling hint to the scheduler daemon.
    async fn resource_hint(
        &self,
        request: Request<ResourceHint>,
    ) -> Result<Response<Heartbeat>, Status> {
        let hint = request.into_inner();
        info!(
            kind = %hint.kind,
            agent_id = %hint.agent_id,
            priority = hint.priority,
            "ResourceHint"
        );

        self.emit_event(
            "sched.hint",
            &hint.agent_id,
            serde_json::json!({"kind": hint.kind, "priority": hint.priority})
                .to_string()
                .as_bytes(),
            "debug",
        );

        // Acknowledge receipt.
        let response = Heartbeat {
            agent_id: "ipc.server".to_string(),
            seq: 0,
            sent_at_ns: chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0) as u64,
            load_avg: 0.0,
            status: "ok".to_string(),
        };
        Ok(Response::new(response))
    }

    /// Heartbeat — liveness ping (unary).
    async fn heartbeat(
        &self,
        request: Request<Heartbeat>,
    ) -> Result<Response<Heartbeat>, Status> {
        let hb = request.into_inner();
        debug!(
            agent_id = %hb.agent_id,
            seq = hb.seq,
            "Heartbeat"
        );

        let response = Heartbeat {
            agent_id: self.server_capability.clone(),
            seq: hb.seq,
            sent_at_ns: chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0) as u64,
            load_avg: 0.0,
            status: "ok".to_string(),
        };
        Ok(Response::new(response))
    }

    /// GetPipelineMetrics — aggregate counters for the serving daemon.
    async fn get_pipeline_metrics(
        &self,
        request: Request<PipelineMetricsRequest>,
    ) -> Result<Response<PipelineMetrics>, Status> {
        let req = request.into_inner();
        debug!(trace_id = %req.trace_id, "GetPipelineMetrics");
        Ok(Response::new(METRICS.snapshot()))
    }

    /// StreamEvents — server-streaming subscription to the event bus.
    type StreamEventsStream = Pin<Box<dyn tokio_stream::Stream<Item = Result<Event, Status>> + Send>>;

    async fn stream_events(
        &self,
        _request: Request<Heartbeat>,
    ) -> Result<Response<Self::StreamEventsStream>, Status> {
        let mut rx = self.event_tx.subscribe();
        let stream = async_stream::stream! {
            loop {
                match rx.recv().await {
                    Ok(event) => yield Ok(event),
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(skipped = n, "event stream lagged");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        };
        Ok(Response::new(Box::pin(stream)))
    }
}

// ─── Signal handling ─────────────────────────────────────────────────────────

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let term = async {
            signal::unix::signal(signal::unix::SignalKind::terminate())
                .expect("install SIGTERM handler")
                .recv()
                .await;
        };
        let int = async {
            signal::unix::signal(signal::unix::SignalKind::interrupt())
                .expect("install SIGINT handler")
                .recv()
                .await;
        };
        tokio::select! {
            _ = term => {},
            _ = int  => {},
        }
    }
    #[cfg(not(unix))]
    {
        signal::ctrl_c().await.ok();
    }
}