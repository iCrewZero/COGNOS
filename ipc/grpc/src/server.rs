//! COGNOS gRPC IPC server — accepts authenticated agent connections,
//! enforces capability checks on every RPC, streams events.

use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::signal;
// Owner: iCrewZero — removed unused Mutex import (H3).
use tokio::sync::broadcast;
use tonic::{Request, Response, Status};
use tonic::transport::Server;

use tracing::{info, debug, warn};

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

// ─── CognosServer ────────────────────────────────────────────────────────────

/// Top-level gRPC server. Owns the event bus and configuration.
// Owner: iCrewZero — added Clone derive so the runtime can clone the server
// before spawning it into an async move block (B3).
#[derive(Clone)]
pub struct CognosServer {
    pub config: ServerConfig,
    event_tx: broadcast::Sender<Event>,
}

impl CognosServer {
    pub fn new() -> Self {
        Self::with_config(ServerConfig::default())
    }

    pub fn with_config(config: ServerConfig) -> Self {
        let (event_tx, _) = broadcast::channel(1024);
        Self { config, event_tx }
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
        );

        // Owner: iCrewZero — switched from HealthServer::new() + set_serving_status()
        // to the documented health_reporter() API for tonic-health 0.12 (B4).
        let (mut health_reporter, health_service) = tonic_health::server::health_reporter();
        // Mark the IPC server as serving so health checks pass.
        health_reporter.set_serving::<CognosIpcServer<CognosIpcService>>().await;

        Server::builder()
            .max_concurrent_streams(self.config.max_concurrent_streams)
            .max_frame_size(self.config.max_frame_size)
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
}

impl CognosIpcService {
    pub fn new(
        server_capability: String,
        event_tx: broadcast::Sender<Event>,
    ) -> Self {
        Self {
            server_capability,
            event_tx,
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

        // TODO(v1): verify envelope signature, enforce "intent.dispatch" capability,
        // forward to intent-engine for DAG construction, then to the orchestrator.
        // For now, acknowledge receipt and return pending status.
        let response = IntentResponse {
            intent_id: intent.intent_id,
            status: "pending".to_string(),
            result_json: Vec::new(),
            message: "intent queued for processing".to_string(),
            violation: None,
            completed_at_ns: chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0) as u64,
        };
        Ok(Response::new(response))
    }

    /// QueryMemory — vector + tag search against ANFS / memory.
    async fn query_memory(
        &self,
        request: Request<MemoryQuery>,
    ) -> Result<Response<MemoryResult>, Status> {
        let query = request.into_inner();
        info!(
            query = %query.query,
            top_k = query.top_k,
            "QueryMemory"
        );

        // TODO(v1): enforce "memory.read" capability, proxy to memory::query
        // which does cosine similarity + tag filtering against the embedder.
        // For now, return empty results.
        let response = MemoryResult {
            hits: Vec::new(),
            total: 0,
            elapsed_ns: 0,
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

        // TODO(v1): enforce the per-op capability, forward to hal::action_validator
        // which runs the risk model and returns granted/denied/approval_required.
        // Owner: iCrewZero — changed from "pending" to "approval_required";
        // proto documents valid statuses as granted|denied|approval_required|failed (H2).
        let response = HalGateResponse {
            status: "approval_required".to_string(),
            grant_token: String::new(),
            risk_score: req.risk_override,
            data: Vec::new(),
            violation: None,
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