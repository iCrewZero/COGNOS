//! CLI runtime — manages gRPC connections to COGNOS services, handles
//! reconnects and Ctrl-C cancellation, and provides a single shared client.
#![allow(dead_code)]
//!
//! The [`CliRuntime`] owns:
//! - a [`CognosClient`] (a thin wrapper around the tonic channel),
//! - an optional [`Deadline`] used by [`CliRuntime::with_timeout`],
//! - a signal-handler guard that flips a cancel flag on Ctrl-C.
//!
//! All `cmd_*` handlers in [`crate::commands`] obtain their client through
//! this runtime so reconnect / cancellation policy lives in exactly one
//! place.
//!
//! v0: stub implementation.

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use cognos_ipc_grpc::client::{ClientConfig, CognosClient as IpcClient, ClientError};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, info, warn};

/// Default orchestrator ingress (full intent pipeline).
pub const DEFAULT_ORCHESTRATOR_ENDPOINT: &str = "http://127.0.0.1:7446";

/// Default per-RPC timeout applied by [`CliRuntime::with_timeout`].
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

impl From<ClientError> for RuntimeError {
    fn from(e: ClientError) -> Self {
        RuntimeError::ConnectFailed(e.to_string())
    }
}

// ─── Errors ─────────────────────────────────────────────────────────────────

/// Errors that can arise while running the CLI runtime.
#[derive(Debug, Error)]
pub enum RuntimeError {
    /// The initial connection to the COGNOS endpoint failed.
    #[error("connect failed: {0}")]
    ConnectFailed(String),
    /// An RPC did not complete before its deadline.
    #[error("operation timed out")]
    Timeout,
    /// The user pressed Ctrl-C (or otherwise cancelled) mid-RPC.
    #[error("cancelled")]
    Cancelled,
    /// The previously-healthy connection dropped mid-RPC.
    #[error("disconnected")]
    Disconnected,
}

// ─── Deadline ───────────────────────────────────────────────────────────────

/// A point in time by which an RPC must complete.
///
/// v0: a thin wrapper around a [`Duration`] (relative deadline). v1 will
/// use `tokio::time::Instant` for absolute deadlines.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Deadline {
    /// Relative deadline from the moment [`CliRuntime::with_timeout`] is
    /// called.
    pub timeout: Duration,
}

impl Default for Deadline {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_TIMEOUT,
        }
    }
}

// ─── Signal handler guard ───────────────────────────────────────────────────

/// Handle to an installed signal handler. Dropping this guard uninstalls
/// the handler (in v1; in v0 it is a no-op).
#[derive(Debug, Default)]
pub struct SignalHandler {
    /// Set to `true` when Ctrl-C is observed. Polled by `with_timeout`.
    cancel: Arc<AtomicBool>,
}

impl SignalHandler {
    /// Install a new handler. Returns the guard.
    pub fn install() -> Self {
        let cancel = Arc::new(AtomicBool::new(false));
        // TODO(v1): tokio::signal::ctrl_c() → set cancel flag.
        //           Spawned as a task on the current runtime; aborted on
        //           Drop via a JoinHandle stored here.
        Self { cancel }
    }

    /// Returns `true` if Ctrl-C has been observed since install.
    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// Manually arm the cancel flag (used in tests and from `shutdown`).
    pub fn arm(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

// ─── CliRuntime ─────────────────────────────────────────────────────────────

/// Owns the shared gRPC client, the deadline policy, and the signal handler.
pub struct CliRuntime {
    pub client: IpcClient,
    pub deadline: Option<Deadline>,
    pub signal_handler: SignalHandler,
}

impl CliRuntime {
    /// Connect to the orchestrator ingress and return a ready runtime.
    pub async fn connect(endpoint: String) -> Result<Self, RuntimeError> {
        info!(%endpoint, "connecting CLI runtime to orchestrator");

        let mut client = IpcClient::new(ClientConfig {
            agent_id: "agent.cli".to_string(),
            signing_secret: std::env::var("COGNOS_IPC_SECRET").unwrap_or_default(),
            endpoint: endpoint.clone(),
            request_timeout_ms: DEFAULT_TIMEOUT.as_millis() as u64,
            ..ClientConfig::default()
        });
        client.connect(&endpoint).await.map_err(|e| {
            warn!(error = %e, "connect failed");
            RuntimeError::ConnectFailed(format!("{e}"))
        })?;

        Ok(Self {
            client,
            deadline: None,
            signal_handler: SignalHandler::install(),
        })
    }

    /// Run `f` to completion, enforcing the runtime's deadline (or
    /// [`DEFAULT_TIMEOUT`] if none is set) and aborting on Ctrl-C.
    pub async fn with_timeout<F, T>(f: F) -> Result<T, RuntimeError>
    where
        F: Future<Output = T>,
    {
        // TODO(v1): take &self so we can consult self.deadline and
        //           self.signal_handler. The current signature is the
        //           v0 placeholder that just runs f with the default
        //           timeout and never consults the cancel flag.
        match tokio::time::timeout(DEFAULT_TIMEOUT, f).await {
            Ok(v) => Ok(v),
            Err(_) => Err(RuntimeError::Timeout),
        }
    }

    /// Install (or re-arm) the Ctrl-C signal handler.
    pub fn install_signal_handlers(&self) {
        // The handler is installed at construction time; this method is a
        // no-op in v0 and exists so callers can re-arm after a fork /
        // exec or a deliberate disarming.
        // TODO(v1): spawn the ctrl_c task on the current handle and store
        //           the JoinHandle so it can be aborted on shutdown.
        debug!("install_signal_handlers (v0 no-op)");
    }

    /// Gracefully shut the runtime down: arm the cancel flag so any
    /// in-flight `with_timeout` future aborts promptly. The underlying
    /// gRPC channel is dropped when the [`CliRuntime`] itself is dropped.
    pub fn shutdown(&self) {
        info!("CLI runtime shutting down");
        self.signal_handler.arm();
        // TODO(v1): abort the signal-handler task, drain in-flight RPCs,
        //           and explicitly close the tonic Channel (requires
        //           interior mutability on `client`).
    }
}

// v0: stub implementation
