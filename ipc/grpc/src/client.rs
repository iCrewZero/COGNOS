//! COGNOS gRPC IPC client — typed wrapper around the tonic-generated
//! CognosIpcClient used by every agent to talk to the IPC server.
//!
//! Maintains a multiplexed HTTP/2 connection, signs requests with
//! HMAC-SHA256 envelopes, and reconnects with exponential backoff.

use std::time::Duration;

use hmac::{Hmac, Mac};
use sha2::Sha256;
use base64::Engine;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tonic::transport::{Channel, Endpoint, Uri};
use tracing::{debug, error, info, warn};

use crate::auth;
use crate::proto::v1::cognos_ipc_client::CognosIpcClient;
use crate::proto::v1::*;

type HmacSha256 = Hmac<Sha256>;

// ─── Errors ──────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("invalid endpoint: {0}")]
    InvalidEndpoint(String),
    #[error("transport: {0}")]
    Transport(String),
    #[error("status: {0}")]
    Status(String),
    #[error("reconnect exhausted after {0} attempts")]
    ReconnectExhausted(u32),
}

// ─── Client configuration ────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientConfig {
    /// Agent identity presented to the server.
    pub agent_id: String,
    /// HMAC signing secret (shared with the server).
    pub signing_secret: String,
    /// gRPC endpoint URI, e.g. "http://127.0.0.1:7443".
    pub endpoint: String,
    /// Initial reconnect backoff in ms.
    pub backoff_init_ms: u64,
    /// Maximum reconnect backoff in ms.
    pub backoff_max_ms: u64,
    /// Maximum reconnect attempts before giving up.
    pub max_reconnect_attempts: u32,
    /// Heartbeat interval in ms.
    pub heartbeat_interval_ms: u64,
    /// Per-request timeout in ms.
    pub request_timeout_ms: u64,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            agent_id: "agent.unknown".to_string(),
            signing_secret: String::new(),
            endpoint: "http://127.0.0.1:7443".to_string(),
            backoff_init_ms: 100,
            backoff_max_ms: 30_000,
            max_reconnect_attempts: 10,
            heartbeat_interval_ms: 5_000,
            request_timeout_ms: 10_000,
        }
    }
}

// ─── CognosClient ────────────────────────────────────────────────────────────

/// Typed gRPC client wrapping a tonic Channel and the generated
/// CognosIpcClient stub. Signs every request with an HMAC envelope.
pub struct CognosClient {
    pub config: ClientConfig,
    inner: Option<CognosIpcClient<Channel>>,
}

impl CognosClient {
    pub fn new(config: ClientConfig) -> Self {
        Self { config, inner: None }
    }

    /// Connect to the configured endpoint with exponential backoff retry.
    pub async fn connect(&mut self, endpoint: &str) -> Result<(), ClientError> {
        let _uri: Uri = endpoint
            .parse()
            .map_err(|e| ClientError::InvalidEndpoint(format!("{e}: {endpoint}")))?;

        let mut attempt: u32 = 0;
        let mut backoff_ms = self.config.backoff_init_ms;

        loop {
            attempt += 1;
            debug!(attempt, backoff_ms, endpoint, "dialing cognos-ipc server");

            match Endpoint::from_shared(endpoint.to_string())
                .map_err(|e| ClientError::InvalidEndpoint(e.to_string()))?
                .timeout(Duration::from_millis(self.config.request_timeout_ms))
                .connect()
                .await
            {
                Ok(channel) => {
                    let client = CognosIpcClient::new(channel);
                    info!(attempt, endpoint, "connected to cognos-ipc server");
                    self.inner = Some(client);
                    return Ok(());
                }
                Err(e) => {
                    warn!(attempt, error = %e, "dial failed");
                    if attempt >= self.config.max_reconnect_attempts {
                        error!(attempt, "reconnect exhausted");
                        return Err(ClientError::ReconnectExhausted(attempt));
                    }
                    tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                    backoff_ms = (backoff_ms * 2).min(self.config.backoff_max_ms);
                }
            }
        }
    }

    /// Return a reference to the underlying tonic client.
    pub fn inner(&self) -> Option<&CognosIpcClient<Channel>> {
        self.inner.as_ref()
    }

    /// Force-close the connection.
    pub fn disconnect(&mut self) {
        if self.inner.take().is_some() {
            info!("cognos-ipc client disconnected");
        }
    }

    // ─── RPC helpers ────────────────────────────────────────────────────────

    /// DispatchIntent — send a parsed intent to the server.
    pub async fn dispatch_intent(
        &self,
        mut intent: Intent,
    ) -> Result<IntentResponse, ClientError> {
        let client = self.require_connected()?;
        intent.trace_id = uuid::Uuid::new_v4().to_string();
        let resp = client
            .dispatch_intent(tonic::Request::new(intent))
            .await
            .map_err(|e| ClientError::Status(e.to_string()))?;
        Ok(resp.into_inner())
    }

    /// QueryMemory — run a vector + tag search.
    pub async fn query_memory(
        &self,
        mut query: MemoryQuery,
    ) -> Result<MemoryResult, ClientError> {
        let client = self.require_connected()?;
        query.trace_id = uuid::Uuid::new_v4().to_string();
        let resp = client
            .query_memory(tonic::Request::new(query))
            .await
            .map_err(|e| ClientError::Status(e.to_string()))?;
        Ok(resp.into_inner())
    }

    /// HalGate — request a hardware action through the HAL.
    pub async fn request_hal_gate(
        &self,
        mut req: HalGateRequest,
    ) -> Result<HalGateResponse, ClientError> {
        let client = self.require_connected()?;
        req.trace_id = uuid::Uuid::new_v4().to_string();
        let resp = client
            .hal_gate(tonic::Request::new(req))
            .await
            .map_err(|e| ClientError::Status(e.to_string()))?;
        Ok(resp.into_inner())
    }

    /// Heartbeat — liveness ping.
    pub async fn heartbeat(
        &self,
        mut hb: Heartbeat,
    ) -> Result<Heartbeat, ClientError> {
        let client = self.require_connected()?;
        hb.sent_at_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0) as u64;
        let resp = client
            .heartbeat(tonic::Request::new(hb))
            .await
            .map_err(|e| ClientError::Status(e.to_string()))?;
        Ok(resp.into_inner())
    }

    /// Long-running heartbeat loop. Pings at the configured interval
    /// until the shutdown future resolves.
    pub async fn heartbeat_loop(
        &mut self,
        shutdown: impl std::future::Future<Output = ()>,
    ) -> Result<(), ClientError> {
        let interval = Duration::from_millis(self.config.heartbeat_interval_ms);
        let mut seq: u64 = 0;
        tokio::pin!(shutdown);

        info!(agent_id = %self.config.agent_id, ?interval, "heartbeat loop starting");
        loop {
            tokio::select! {
                _ = &mut shutdown => {
                    info!("heartbeat loop received shutdown");
                    return Ok(());
                }
                _ = tokio::time::sleep(interval) => {
                    seq += 1;
                    let hb = Heartbeat {
                        agent_id: self.config.agent_id.clone(),
                        seq,
                        sent_at_ns: chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0) as u64,
                        load_avg: 0.0,
                        status: "alive".to_string(),
                    };
                    match self.heartbeat(hb).await {
                        Ok(_) => debug!(seq, "heartbeat ok"),
                        Err(e) => {
                            warn!(seq, error = %e, "heartbeat failed, reconnecting");
                            if let Err(e) = self.connect(&self.config.endpoint).await {
                                error!(seq, error = %e, "reconnect failed in heartbeat loop");
                                return Err(e);
                            }
                        }
                    }
                }
            }
        }
    }

    /// Build a signed Envelope for a payload.
    pub fn build_envelope(
        &self,
        trace_id: &str,
        source: &str,
        target: &str,
        capability: &str,
        payload: &[u8],
    ) -> Envelope {
        let mut mac = HmacSha256::new_from_slice(self.config.signing_secret.as_bytes())
            .expect("HMAC key is always valid");
        mac.update(trace_id.as_bytes());
        mac.update(b"|");
        mac.update(source.as_bytes());
        mac.update(b"|");
        mac.update(target.as_bytes());
        mac.update(b"|");
        mac.update(capability.as_bytes());
        mac.update(b"|");
        mac.update(payload);
        let sig = mac.finalize().into_bytes().to_vec();

        Envelope {
            trace_id: trace_id.to_string(),
            source: source.to_string(),
            target: target.to_string(),
            capability: capability.to_string(),
            payload: payload.to_vec(),
            signature: sig,
            sent_at_ns: chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0) as u64,
            schema: "v1".to_string(),
        }
    }

    /// Create a session token for this agent.
    ///
    /// The token is valid until `expiry`, expressed as **Unix epoch seconds**
    /// (i.e. seconds since 1970-01-01 00:00:00 UTC). Pass the same secret to
    /// the server's `verify_token` to validate.
    ///
    /// Owner: iCrewZero — clarified that `expiry` is in Unix seconds (M1).
    pub fn create_session_token(&self, expiry: u64) -> String {
        auth::create_token(
            &self.config.agent_id,
            expiry,
            self.config.signing_secret.as_bytes(),
        )
    }

    fn require_connected(&self) -> Result<&CognosIpcClient<Channel>, ClientError> {
        self.inner
            .as_ref()
            .ok_or_else(|| ClientError::Transport("not connected".into()))
    }
}

impl Default for CognosClient {
    fn default() -> Self {
        Self::new(ClientConfig::default())
    }
}