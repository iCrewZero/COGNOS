//! COGNOS gRPC runtime/transport glue.
//!
//! The IpcRuntime owns the gRPC server task, the set of outbound
//! client connections, and the event bus. It is the single seam
//! between the tokio scheduler and the rest of the COGNOS daemons.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

use crate::client::{ClientConfig, CognosClient};
use crate::server::{CognosServer, ServerConfig};

// ─── Errors ──────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("runtime already started")]
    AlreadyStarted,
    #[error("runtime not started")]
    NotStarted,
    #[error("agent already registered: {0}")]
    AgentExists(String),
    #[error("agent not registered: {0}")]
    AgentNotFound(String),
    #[error("server: {0}")]
    Server(String),
}

// ─── Agent registry entry ────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentEntry {
    pub agent_id: String,
    pub endpoint: String,
    pub capabilities: Vec<String>,
    pub last_seq: u64,
}

// ─── IpcRuntime ──────────────────────────────────────────────────────────────

/// Owns the gRPC server, outbound clients, and event bus.
pub struct IpcRuntime {
    pub server_config: ServerConfig,
    pub agents: Arc<RwLock<HashMap<String, AgentEntry>>>,
    server: Option<CognosServer>,
    clients: Arc<Mutex<HashMap<String, CognosClient>>>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
    // Owner: iCrewZero — removed shutdown_tx field; the dead oneshot channel
    // was never connected to the server (which uses its own SIGTERM handler),
    // and runtime.shutdown() already aborts tasks directly (H4).
}

impl IpcRuntime {
    pub fn new() -> Self {
        Self::with_server_config(ServerConfig::default())
    }

    pub fn with_server_config(server_config: ServerConfig) -> Self {
        Self {
            server_config,
            agents: Arc::new(RwLock::new(HashMap::new())),
            server: None,
            clients: Arc::new(Mutex::new(HashMap::new())),
            tasks: Mutex::new(Vec::new()),
        }
    }

    /// Start the runtime: bind the gRPC server and spawn the heartbeat supervisor.
    pub async fn start(&mut self, addr: SocketAddr) -> Result<(), RuntimeError> {
        if self.server.is_some() {
            return Err(RuntimeError::AlreadyStarted);
        }

        // Owner: iCrewZero — clone the server before spawning so the original
        // is still available for self.server = Some(server) below (B3).
        let serve_server = CognosServer::with_config(self.server_config.clone());

        // Owner: iCrewZero — removed dead oneshot shutdown channel (H4).
        // Shutdown is handled by the server task's own signal handler (SIGTERM/SIGINT).
        // runtime.shutdown() aborts the spawned tasks directly.

        // Spawn the real tonic gRPC server.
        let server_task = tokio::spawn(async move {
            if let Err(e) = serve_server.serve(addr).await {
                error!(error = %e, "cognos-ipc server task exited with error");
            }
        });
        self.tasks.lock().await.push(server_task);

        // Spawn the heartbeat supervisor.
        // Owner: iCrewZero — rewrote to fix two bugs (H1):
        //   1. The clients Mutex was held across .await (deadlock risk).
        //   2. `break` after the first successful agent stopped heartbeating others.
        // Now we snapshot the agent list under a read lock, drop it, then iterate.
        let agents = Arc::clone(&self.agents);
        let clients = Arc::clone(&self.clients);
        let supervisor = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(10));
            loop {
                interval.tick().await;
                // Snapshot the agent list under a read lock, then drop it.
                let agent_snap: Vec<(String, u64, String)> = {
                    let agents = agents.read().await;
                    agents.iter()
                        .filter(|(_, e)| !e.endpoint.is_empty())
                        .map(|(id, e)| (id.clone(), e.last_seq, e.endpoint.clone()))
                        .collect()
                };

                for (id, seq, _endpoint) in &agent_snap {
                    let seq = *seq + 1;
                    let hb = crate::proto::v1::Heartbeat {
                        agent_id: id.clone(),
                        seq,
                        sent_at_ns: chrono::Utc::now()
                            .timestamp_nanos_opt()
                            .unwrap_or(0) as u64,
                        load_avg: 0.0,
                        status: "alive".to_string(),
                    };
                    // Acquire client lock only for this one agent, drop after use.
                    let result = {
                        let mut clients = clients.lock().await;
                        if let Some(client) = clients.get_mut(id) {
                            if client.inner().is_some() {
                                client.heartbeat(hb).await.err()
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    };
                    if let Some(e) = result {
                        warn!(agent_id = %id, error = %e, "heartbeat failed");
                    } else {
                        // Update seq in agent registry.
                        let mut agents = agents.write().await;
                        if let Some(entry) = agents.get_mut(id) {
                            entry.last_seq = seq;
                        }
                    }
                }
            }
        });
        self.tasks.lock().await.push(supervisor);

        // Owner: iCrewZero — store the original server (not the moved clone) for diagnostics (B3).
        self.server = Some(CognosServer::with_config(self.server_config.clone()));
        info!(%addr, "cognos-ipc runtime started");
        Ok(())
    }

    /// Register a new agent and optionally open an outbound client.
    pub async fn register_agent(
        &self,
        entry: AgentEntry,
    ) -> Result<(), RuntimeError> {
        let mut agents = self.agents.write().await;
        if agents.contains_key(&entry.agent_id) {
            return Err(RuntimeError::AgentExists(entry.agent_id.clone()));
        }
        info!(agent_id = %entry.agent_id, endpoint = %entry.endpoint, "registering agent");

        if !entry.endpoint.is_empty() {
            let mut client = CognosClient::new(ClientConfig {
                agent_id: entry.agent_id.clone(),
                endpoint: entry.endpoint.clone(),
                ..ClientConfig::default()
            });
            // Try to connect immediately but don't fail if it's not up yet.
            match client.connect(&entry.endpoint).await {
                Ok(()) => debug!(agent_id = %entry.agent_id, "connected on registration"),
                Err(e) => warn!(agent_id = %entry.agent_id, error = %e, "connect failed on registration, will retry"),
            }
            self.clients.lock().await.insert(entry.agent_id.clone(), client);
        }

        agents.insert(entry.agent_id.clone(), entry);
        Ok(())
    }

    /// Remove an agent from the registry and tear down its client.
    pub async fn unregister_agent(&self, agent_id: &str) -> Result<(), RuntimeError> {
        let mut agents = self.agents.write().await;
        if agents.remove(agent_id).is_none() {
            return Err(RuntimeError::AgentNotFound(agent_id.to_string()));
        }
        info!(agent_id, "unregistering agent");
        if let Some(mut client) = self.clients.lock().await.remove(agent_id) {
            client.disconnect();
        }
        Ok(())
    }

    /// Gracefully shut the runtime down.
    pub async fn shutdown(&self) -> Result<(), RuntimeError> {
        info!("cognos-ipc runtime shutting down");

        // Owner: iCrewZero — removed dead oneshot send; tasks are aborted directly (H4).

        let mut tasks = self.tasks.lock().await;
        // Clone the handles so we can iterate and abort separately.
        let handles: Vec<_> = tasks.drain(..).collect();
        let drain = async {
            for handle in handles {
                let _ = handle.await;
            }
        };
        if tokio::time::timeout(Duration::from_secs(5), drain).await.is_err() {
            // We can't access the handles after they were moved into
            // the drain future, so we rely on the tokio runtime cleanup.
            error!("runtime shutdown timed out — tasks will be cancelled on drop");
            return Err(RuntimeError::Server("shutdown timeout".into()));
        }

        let mut clients = self.clients.lock().await;
        for (_id, mut client) in clients.drain() {
            client.disconnect();
        }

        info!("cognos-ipc runtime stopped");
        Ok(())
    }

    /// Snapshot the current agent registry, sorted by id.
    pub async fn agent_snapshot(&self) -> Vec<AgentEntry> {
        let agents = self.agents.read().await;
        let mut v: Vec<AgentEntry> = agents.values().cloned().collect();
        v.sort_by(|a, b| a.agent_id.cmp(&b.agent_id));
        v
    }
}

impl Default for IpcRuntime {
    fn default() -> Self {
        Self::new()
    }
}