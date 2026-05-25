/// Authenticated gRPC Agent IPC for COGNOS/OS.
///
/// Every agent communicates exclusively through this layer.
/// No direct function calls between agents. No unauthenticated channels.
/// This is a hard security boundary.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use uuid::Uuid;

// ─── Protocol types ───────────────────────────────────────────────────────────

/// Message type enum — matches the protobuf schema.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u32)]
pub enum AgentMessageType {
    IntentDispatch = 0,
    MemoryQuery = 1,
    MemoryResult = 2,
    SecurityAlert = 3,
    ResourceHint = 4,
    FileOperation = 5,
    HalGateRequest = 6,
    HalGateResponse = 7,
    Heartbeat = 8,
    CapabilityViolation = 9,
}

/// The wire message format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMessage {
    pub message_id: Uuid,
    pub sender_id: String,
    pub recipient_id: String,
    pub r#type: AgentMessageType,
    pub payload: Vec<u8>,
    pub timestamp_ms: i64,
    pub session_id: String,
}

/// A connected agent's session state.
struct AgentSession {
    agent_name: String,
    session_token: Uuid,
    token_expires_at: Instant,
    last_heartbeat: Instant,
    message_count_this_second: u32,
    last_rate_window: Instant,
}

// ─── Capability lattice: allowed message types per agent ──────────────────────

fn allowed_message_types(agent: &str) -> HashSet<AgentMessageType> {
    use AgentMessageType::*;
    match agent {
        "planner"     => [IntentDispatch, MemoryQuery, HalGateRequest].into(),
        "memory"      => [MemoryResult, HalGateRequest].into(),
        "security"    => [SecurityAlert, HalGateRequest, CapabilityViolation].into(),
        "scheduler"   => [ResourceHint].into(),
        "file"        => [FileOperation, HalGateRequest].into(),
        "coding"      => [HalGateRequest, MemoryQuery, FileOperation].into(),
        "ui"          => [HalGateRequest].into(),
        "coordinator" => {
            // Coordinator can route anything
            [
                IntentDispatch, MemoryQuery, MemoryResult, SecurityAlert,
                ResourceHint, FileOperation, HalGateRequest, HalGateResponse,
                Heartbeat, CapabilityViolation,
            ]
            .into()
        }
        _ => HashSet::new(), // unknown agents get nothing
    }
}

/// All agents known to the system.
const KNOWN_AGENTS: &[&str] = &[
    "planner", "memory", "security", "scheduler",
    "file", "coding", "ui", "coordinator",
];

fn is_known_agent(name: &str) -> bool {
    KNOWN_AGENTS.contains(&name)
}

// ─── Coordinator ─────────────────────────────────────────────────────────────

/// The central IPC hub. Authenticates agents, enforces capability lattice,
/// routes messages, and rate-limits.
pub struct AgentCoordinator {
    sessions: Arc<Mutex<HashMap<String, AgentSession>>>,
    audit_log: std::path::PathBuf,
    /// Channel map: agent name → sender half of the agent's message channel
    routes: Arc<Mutex<HashMap<String, tokio::sync::mpsc::Sender<AgentMessage>>>>,
}

/// Reasons a connection or message can be rejected.
#[derive(Debug, Clone, PartialEq)]
pub enum IpcRejection {
    UnknownAgent(String),
    CertificateCnMismatch { claimed: String, actual_cn: String },
    ExpiredToken,
    CapabilityViolation { agent: String, message_type: AgentMessageType },
    RateLimitExceeded { agent: String },
    MalformedMessage(String),
}

impl AgentCoordinator {
    pub fn new() -> Self {
        let audit_log = dirs::home_dir()
            .unwrap_or_else(|| "/tmp".into())
            .join(".cognos/audit.log");

        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            audit_log,
            routes: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Authenticate a connecting agent using its TLS certificate CN.
    ///
    /// The coordinator verifies that the certificate CN matches the claimed
    /// agent name. Unknown agents are rejected before any message is processed.
    pub async fn authenticate(
        &self,
        claimed_name: &str,
        cert_cn: &str,
    ) -> Result<Uuid, IpcRejection> {
        // 1. Reject unknown agents immediately
        if !is_known_agent(claimed_name) {
            self.audit_security_event("unknown_agent_rejected", claimed_name, "");
            return Err(IpcRejection::UnknownAgent(claimed_name.to_string()));
        }

        // 2. Verify the TLS certificate CN matches the claimed name
        if cert_cn != claimed_name {
            self.audit_security_event(
                "cert_cn_mismatch",
                claimed_name,
                &format!("claimed={}, actual_cn={}", claimed_name, cert_cn),
            );
            return Err(IpcRejection::CertificateCnMismatch {
                claimed: claimed_name.to_string(),
                actual_cn: cert_cn.to_string(),
            });
        }

        // 3. Issue a session token (expires in 300 seconds)
        let token = Uuid::new_v4();
        let session = AgentSession {
            agent_name: claimed_name.to_string(),
            session_token: token,
            token_expires_at: Instant::now() + Duration::from_secs(300),
            last_heartbeat: Instant::now(),
            message_count_this_second: 0,
            last_rate_window: Instant::now(),
        };

        self.sessions.lock().await.insert(claimed_name.to_string(), session);
        log::info!("Agent '{}' authenticated, token={}", claimed_name, token);
        Ok(token)
    }

    /// Route a message from one agent to another, enforcing all policies.
    pub async fn route(
        &self,
        message: AgentMessage,
        sender_token: Uuid,
    ) -> Result<(), IpcRejection> {
        let mut sessions = self.sessions.lock().await;

        // 1. Verify sender session
        let session = sessions
            .get_mut(&message.sender_id)
            .ok_or_else(|| IpcRejection::UnknownAgent(message.sender_id.clone()))?;

        if session.session_token != sender_token {
            return Err(IpcRejection::ExpiredToken);
        }
        if Instant::now() > session.token_expires_at {
            return Err(IpcRejection::ExpiredToken);
        }

        // 2. Rate limiting: max 100 messages per second per agent
        let now = Instant::now();
        if now.duration_since(session.last_rate_window) >= Duration::from_secs(1) {
            session.message_count_this_second = 0;
            session.last_rate_window = now;
        }
        session.message_count_this_second += 1;
        if session.message_count_this_second > 100 {
            return Err(IpcRejection::RateLimitExceeded {
                agent: message.sender_id.clone(),
            });
        }

        // 3. Capability lattice enforcement
        let allowed = allowed_message_types(&message.sender_id);
        if !allowed.contains(&message.r#type) {
            drop(sessions); // release lock before async audit
            self.audit_security_event(
                "capability_violation",
                &message.sender_id,
                &format!("{:?}", message.r#type),
            );
            // Also route a CapabilityViolation alert to the Security Agent
            self.alert_security_agent(&message).await;
            return Err(IpcRejection::CapabilityViolation {
                agent: message.sender_id.clone(),
                message_type: message.r#type,
            });
        }

        // 4. Log every message (truncated payload)
        self.log_message(&message);
        drop(sessions);

        // 5. Route to recipient
        let routes = self.routes.lock().await;
        if let Some(sender) = routes.get(&message.recipient_id) {
            let _ = sender.send(message).await;
        }

        Ok(())
    }

    /// Register an agent's inbound message channel.
    pub async fn register_agent(
        &self,
        agent_name: &str,
        sender: tokio::sync::mpsc::Sender<AgentMessage>,
    ) {
        self.routes.lock().await.insert(agent_name.to_string(), sender);
    }

    /// Process a heartbeat from an agent (resets dead-agent detection timer).
    pub async fn heartbeat(&self, agent_name: &str) {
        if let Some(session) = self.sessions.lock().await.get_mut(agent_name) {
            session.last_heartbeat = Instant::now();
        }
    }

    /// Detect agents that haven't sent a heartbeat in 10 seconds.
    pub async fn detect_dead_agents(&self) -> Vec<String> {
        let sessions = self.sessions.lock().await;
        sessions
            .iter()
            .filter(|(_, s)| {
                Instant::now().duration_since(s.last_heartbeat) > Duration::from_secs(10)
            })
            .map(|(name, _)| name.clone())
            .collect()
    }

    // ─── Private helpers ──────────────────────────────────────────────────────

    fn log_message(&self, msg: &AgentMessage) {
        let payload_preview = if msg.payload.len() > 80 {
            format!("{}... ({} bytes)", hex::encode(&msg.payload[..40]), msg.payload.len())
        } else {
            hex::encode(&msg.payload)
        };

        let line = format!(
            r#"{{"ts":"{}","agent":"coordinator","action":"route_message","sender":"{}","recipient":"{}","type":"{:?}","msg_id":"{}","payload_preview":"{}"}}"#,
            chrono::Utc::now().to_rfc3339(),
            msg.sender_id,
            msg.recipient_id,
            msg.r#type,
            msg.message_id,
            payload_preview,
        );
        self.write_audit(&line);
    }

    fn audit_security_event(&self, event: &str, agent: &str, detail: &str) {
        let line = format!(
            r#"{{"ts":"{}","agent":"coordinator","action":"{}","source_agent":"{}","detail":"{}","severity":"security"}}"#,
            chrono::Utc::now().to_rfc3339(),
            event,
            agent,
            detail,
        );
        self.write_audit(&line);
        log::warn!("Security event: {} agent={} detail={}", event, agent, detail);
    }

    async fn alert_security_agent(&self, offending_msg: &AgentMessage) {
        let alert = AgentMessage {
            message_id: Uuid::new_v4(),
            sender_id: "coordinator".to_string(),
            recipient_id: "security".to_string(),
            r#type: AgentMessageType::SecurityAlert,
            payload: serde_json::to_vec(&serde_json::json!({
                "event": "capability_violation",
                "agent": offending_msg.sender_id,
                "message_type": format!("{:?}", offending_msg.r#type),
            }))
            .unwrap_or_default(),
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
            session_id: offending_msg.session_id.clone(),
        };

        let routes = self.routes.lock().await;
        if let Some(sender) = routes.get("security") {
            let _ = sender.send(alert).await;
        }
    }

    fn write_audit(&self, line: &str) {
        if let Some(parent) = self.audit_log.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.audit_log)
        {
            use std::io::Write;
            let _ = writeln!(f, "{}", line);
        }
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    async fn make_coordinator() -> AgentCoordinator {
        AgentCoordinator::new()
    }

    #[tokio::test]
    async fn unknown_agent_is_rejected_on_auth() {
        let c = make_coordinator().await;
        let result = c.authenticate("evil-agent", "evil-agent").await;
        assert!(matches!(result, Err(IpcRejection::UnknownAgent(_))));
    }

    #[tokio::test]
    async fn cert_cn_mismatch_is_rejected() {
        let c = make_coordinator().await;
        let result = c.authenticate("planner", "memory").await; // CN says memory, claimed planner
        assert!(matches!(result, Err(IpcRejection::CertificateCnMismatch { .. })));
    }

    #[tokio::test]
    async fn valid_agent_gets_session_token() {
        let c = make_coordinator().await;
        let result = c.authenticate("planner", "planner").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn capability_violation_is_rejected() {
        let c = make_coordinator().await;
        let token = c.authenticate("scheduler", "scheduler").await.unwrap();

        // Register a dummy recipient
        let (tx, _rx) = tokio::sync::mpsc::channel(10);
        c.register_agent("memory", tx).await;

        let msg = AgentMessage {
            message_id: Uuid::new_v4(),
            sender_id: "scheduler".to_string(),
            recipient_id: "memory".to_string(),
            r#type: AgentMessageType::IntentDispatch, // SCHEDULER cannot send IntentDispatch
            payload: vec![],
            timestamp_ms: 0,
            session_id: "test".to_string(),
        };

        let result = c.route(msg, token).await;
        assert!(matches!(result, Err(IpcRejection::CapabilityViolation { .. })));
    }

    #[tokio::test]
    async fn valid_message_routes_successfully() {
        let c = make_coordinator().await;
        let token = c.authenticate("scheduler", "scheduler").await.unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::channel(10);
        c.register_agent("planner", tx).await;

        let msg = AgentMessage {
            message_id: Uuid::new_v4(),
            sender_id: "scheduler".to_string(),
            recipient_id: "planner".to_string(),
            r#type: AgentMessageType::ResourceHint, // scheduler CAN send ResourceHint
            payload: vec![],
            timestamp_ms: 0,
            session_id: "test".to_string(),
        };

        let result = c.route(msg, token).await;
        assert!(result.is_ok());
        assert!(rx.recv().await.is_some());
    }

    #[tokio::test]
    async fn rate_limit_enforced() {
        let c = make_coordinator().await;
        let token = c.authenticate("scheduler", "scheduler").await.unwrap();

        let (tx, _rx) = tokio::sync::mpsc::channel(200);
        c.register_agent("planner", tx).await;

        let mut rejected = false;
        for _ in 0..105 {
            let msg = AgentMessage {
                message_id: Uuid::new_v4(),
                sender_id: "scheduler".to_string(),
                recipient_id: "planner".to_string(),
                r#type: AgentMessageType::ResourceHint,
                payload: vec![],
                timestamp_ms: 0,
                session_id: "test".to_string(),
            };
            if c.route(msg, token).await.is_err() {
                rejected = true;
                break;
            }
        }
        assert!(rejected, "Rate limit was not enforced after 100 messages/second");
    }
}
