//! Client-side "service agent" bootstrap.
//!
//! The orchestrator, scheduler, and HAL daemons are all *agents* of the
//! central COGNOS IPC server: each opens an outbound [`CognosClient`],
//! registers its identity and capabilities, and keeps a heartbeat alive on a
//! background tokio task. This module packages that lifecycle so every service
//! `main.rs` can wire IPC in a couple of lines — which matters especially for
//! HAL, whose only permitted change is its `main.rs`.
//!
//! Reconnect uses the **same backoff policy as the Python client**
//! (`agents/shared/ipc.py`): exponential base 0.5s, cap 10s, equal jitter, and
//! an explicit WARNING-logged degrade after `max_failures` consecutive
//! failures (after which the loop keeps retrying at the capped cadence).
//!
//! Capability registration is currently *declarative*: the server has no
//! `Register` RPC yet (a v1 TODO — see docs/ROADMAP.md), so we record the
//! agent's capabilities in a local [`IpcRuntime`] registry via
//! [`IpcRuntime::register_agent`]. The live channel is the `CognosClient`.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::client::{ClientConfig, CognosClient};
use crate::proto::v1::Heartbeat;
use crate::runtime::{AgentEntry, IpcRuntime};

/// Exponential backoff base, matching the Python client (`BACKOFF_BASE_S`).
pub const BACKOFF_BASE_MS: u64 = 500;
/// Exponential backoff cap, matching the Python client (`BACKOFF_MAX_S`).
pub const BACKOFF_MAX_MS: u64 = 10_000;
/// Default heartbeat interval.
pub const DEFAULT_HEARTBEAT_INTERVAL_MS: u64 = 5_000;
/// Consecutive reconnect failures before the WARNING-logged degrade.
pub const DEFAULT_MAX_FAILURES: u32 = 3;
/// Per-RPC request timeout.
pub const DEFAULT_REQUEST_TIMEOUT_MS: u64 = 5_000;

const ENDPOINT_ENV: &str = "COGNOS_IPC_ENDPOINT";
const SECRET_ENV: &str = "COGNOS_IPC_SECRET";
const DEFAULT_ENDPOINT: &str = "http://127.0.0.1:7443";

/// Declarative description of a service acting as an IPC agent.
#[derive(Debug, Clone)]
pub struct AgentSpec {
    /// Agent identity presented to the server, e.g. `"agent.orchestrator"`.
    pub agent_id: String,
    /// gRPC endpoint of the central IPC server, e.g. `"http://127.0.0.1:7443"`.
    pub endpoint: String,
    /// HMAC signing secret shared with the server (may be empty in dev).
    pub signing_secret: String,
    /// Capabilities this service declares.
    pub capabilities: Vec<String>,
    /// Heartbeat interval.
    pub heartbeat_interval: Duration,
    /// Consecutive reconnect failures before the WARNING-logged degrade.
    pub max_failures: u32,
}

impl AgentSpec {
    /// Build a spec from the service environment.
    ///
    /// Reads `COGNOS_IPC_ENDPOINT` (default `http://127.0.0.1:7443`) and
    /// `COGNOS_IPC_SECRET` (default empty) — the same knobs the Python client
    /// and the Rust server use.
    pub fn from_env(agent_id: impl Into<String>, capabilities: Vec<String>) -> Self {
        let endpoint = std::env::var(ENDPOINT_ENV)
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| DEFAULT_ENDPOINT.to_string());
        let signing_secret = std::env::var(SECRET_ENV).unwrap_or_default();
        Self {
            agent_id: agent_id.into(),
            endpoint,
            signing_secret,
            capabilities,
            heartbeat_interval: Duration::from_millis(DEFAULT_HEARTBEAT_INTERVAL_MS),
            max_failures: DEFAULT_MAX_FAILURES,
        }
    }
}

/// Handle to a running IPC agent. Dropping it detaches the heartbeat task;
/// call [`AgentHandle::stop`] for a graceful shutdown.
pub struct AgentHandle {
    shutdown: Arc<Notify>,
    task: JoinHandle<()>,
    registry: IpcRuntime,
}

impl AgentHandle {
    /// Signal the heartbeat task to stop and wait for it to finish.
    pub async fn stop(self) {
        self.shutdown.notify_one();
        let _ = self.task.await;
    }

    /// Snapshot of the capabilities this agent registered locally.
    pub async fn registered_capabilities(&self) -> Vec<AgentEntry> {
        self.registry.agent_snapshot().await
    }
}

/// Normalize an endpoint to a tonic-parseable URI.
///
/// tonic's `Endpoint` requires a scheme; a bare `host:port` (as the Python
/// client accepts) is rejected. Prepend `http://` when no scheme is present.
fn normalize_endpoint(endpoint: &str) -> String {
    if endpoint.contains("://") {
        endpoint.to_string()
    } else {
        format!("http://{endpoint}")
    }
}

/// Equal-jitter exponential backoff, matching Python's `_backoff_delay`.
///
/// `attempt` is 1-based. Returns a delay in `[raw/2, raw]` where
/// `raw = min(BACKOFF_MAX_MS, BACKOFF_BASE_MS * 2^(attempt-1))`.
fn backoff_delay(attempt: u32) -> Duration {
    let shift = attempt.saturating_sub(1).min(20);
    let raw = BACKOFF_BASE_MS
        .saturating_mul(1u64 << shift)
        .min(BACKOFF_MAX_MS);
    let half = raw / 2;
    let jitter = if half > 0 { pseudo_rand() % (half + 1) } else { 0 };
    Duration::from_millis(half + jitter)
}

/// Cheap non-cryptographic jitter source (avoids pulling in the `rand` crate;
/// jitter only needs to de-synchronize reconnect storms, not be secure).
fn pseudo_rand() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    // xorshift-style mix so low bits vary.
    let mut x = nanos.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x ^= x >> 30;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 27;
    x
}

/// (Re)connect the client and send a registration heartbeat, retrying with
/// the Python backoff+jitter policy. Returns `true` once connected and
/// registered, or `false` after `max_failures` attempts (degraded — the
/// caller keeps retrying on the next heartbeat tick).
async fn connect_and_register(
    client: &mut CognosClient,
    endpoint: &str,
    agent_id: &str,
    max_failures: u32,
) -> bool {
    let mut attempt: u32 = 0;
    loop {
        attempt += 1;
        // `max_reconnect_attempts` is 1 on this client, so `connect` is a
        // single dial; the retry cadence below is ours (Python policy).
        match client.connect(endpoint).await {
            Ok(()) => {
                let hb = Heartbeat {
                    agent_id: agent_id.to_string(),
                    seq: 0,
                    sent_at_ns: 0,
                    load_avg: 0.0,
                    status: "register".to_string(),
                };
                match client.heartbeat(hb).await {
                    Ok(_) => {
                        info!(
                            agent_id,
                            endpoint, attempt, "registered + connected to ipc server"
                        );
                        return true;
                    }
                    Err(e) => warn!(
                        agent_id, attempt, error = %e,
                        "connected but registration heartbeat failed"
                    ),
                }
            }
            Err(e) => warn!(agent_id, attempt, endpoint, error = %e, "ipc connect failed"),
        }

        if attempt >= max_failures {
            warn!(
                agent_id,
                attempts = attempt,
                "ipc server unreachable after {max_failures} attempts — degraded (no live IPC), will keep retrying"
            );
            return false;
        }
        tokio::time::sleep(backoff_delay(attempt)).await;
    }
}

/// Bring up a service as an IPC agent: register its capabilities, connect to
/// the central IPC server, and start a background heartbeat loop.
///
/// Returns immediately with an [`AgentHandle`]; the connect + registration
/// happen inside the spawned task so a down server never blocks service
/// startup (matching the Python client's non-fatal connect).
pub async fn spawn(spec: AgentSpec) -> AgentHandle {
    // 1. Register identity + capabilities in the local runtime registry.
    let registry = IpcRuntime::new();
    if let Err(e) = registry
        .register_agent(AgentEntry {
            agent_id: spec.agent_id.clone(),
            // Empty endpoint: this is a declarative capability record, not a
            // second outbound connection. The live channel is the client below.
            endpoint: String::new(),
            capabilities: spec.capabilities.clone(),
            last_seq: 0,
        })
        .await
    {
        warn!(agent_id = %spec.agent_id, error = %e, "capability registration failed");
    }
    info!(
        agent_id = %spec.agent_id,
        capabilities = ?spec.capabilities,
        "agent capabilities registered"
    );

    // 2. Build the outbound client. `max_reconnect_attempts = 1` makes
    //    `connect()` a single dial so this module owns the retry cadence.
    let endpoint = normalize_endpoint(&spec.endpoint);
    let cfg = ClientConfig {
        agent_id: spec.agent_id.clone(),
        signing_secret: spec.signing_secret.clone(),
        endpoint: endpoint.clone(),
        backoff_init_ms: BACKOFF_BASE_MS,
        backoff_max_ms: BACKOFF_MAX_MS,
        max_reconnect_attempts: 1,
        heartbeat_interval_ms: spec.heartbeat_interval.as_millis() as u64,
        request_timeout_ms: DEFAULT_REQUEST_TIMEOUT_MS,
    };
    let mut client = CognosClient::new(cfg);

    let shutdown = Arc::new(Notify::new());
    let task_shutdown = Arc::clone(&shutdown);
    let interval = spec.heartbeat_interval;
    let max_failures = spec.max_failures.max(1);
    let agent_id = spec.agent_id.clone();

    // 3. Heartbeat loop as a tokio task.
    let task = tokio::spawn(async move {
        connect_and_register(&mut client, &endpoint, &agent_id, max_failures).await;

        let mut seq: u64 = 0;
        let mut ticker = tokio::time::interval(interval);
        // The first tick fires immediately; skip it so we don't double-beat
        // right after the registration heartbeat above.
        ticker.tick().await;

        loop {
            tokio::select! {
                _ = task_shutdown.notified() => {
                    info!(agent_id, "ipc agent stopping");
                    client.disconnect();
                    return;
                }
                _ = ticker.tick() => {
                    seq += 1;
                    let hb = Heartbeat {
                        agent_id: agent_id.clone(),
                        seq,
                        sent_at_ns: 0,
                        load_avg: 0.0,
                        status: "alive".to_string(),
                    };
                    match client.heartbeat(hb).await {
                        Ok(_) => debug!(agent_id, seq, "heartbeat ok"),
                        Err(e) => {
                            warn!(agent_id, seq, error = %e, "heartbeat failed — reconnecting");
                            connect_and_register(&mut client, &endpoint, &agent_id, max_failures).await;
                        }
                    }
                }
            }
        }
    });

    AgentHandle {
        shutdown,
        task,
        registry,
    }
}
