/// HAL v0 — Human Approval Layer Skeleton for COGNOS/OS.
///
/// THIS FILE IS HUMAN-WRITTEN ONLY.
/// No AI authorship. No AI commits. CI enforces this.
///
/// v0 establishes the process, protocol, and enforcement boundary.
/// The full risk scoring model, trust calibration, and behavioral analysis
/// come in v1 (Phase 3). Do not add v1 features here.
///
/// Process architecture:
///   - Separate process owned by cognos system user
///   - Unix socket: /run/cognos/hal.sock (permissions: 0600)
///   - Agents wait synchronously for HAL response before proceeding
///   - Maximum wait: 30 seconds; after that, deny + log timeout_deny

use std::collections::HashSet;
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ─── Protocol types ───────────────────────────────────────────────────────────

/// A request from an agent, asking HAL to gate an action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateRequest {
    pub request_id: Uuid,
    pub agent: String,
    pub action: String,
    /// The resource the action targets (file path, package name, etc.)
    pub target: String,
    /// Action-specific parameters
    pub parameters: serde_json::Value,
    /// True if the action was produced by AI-generated code
    pub is_ai_generated: bool,
    /// The intent that triggered this action (for audit correlation)
    pub intent_id: Option<Uuid>,
}

/// HAL's response to a gate request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateResponse {
    pub request_id: Uuid,
    pub approved: bool,
    pub reason: GateReason,
    pub hal_score: f32,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum GateReason {
    AutoApproved,
    UserApproved,
    AutoDenied,
    UserDenied,
    TimeoutDeny,
    HalError,
}

/// HAL's assessment of what to do with a request before showing UI.
#[derive(Debug, Clone, PartialEq)]
enum HalLevel {
    /// Score < 0.3: execute silently, write to audit log.
    Silent,
    /// Score 0.3–0.6: show toast notification.
    Notify,
    /// Score 0.6–0.8: show confirmation dialog, wait for user.
    Confirm,
    /// Score ≥ 0.8: full explanation + explicit approve/deny.
    Block,
}

// ─── Known agents ─────────────────────────────────────────────────────────────

const KNOWN_AGENTS: &[&str] = &[
    "planner", "memory", "security", "scheduler",
    "file", "coding", "ui",
];

fn is_known_agent(name: &str) -> bool {
    KNOWN_AGENTS.contains(&name)
}

// ─── v0 simplified risk scoring ───────────────────────────────────────────────

/// Compute HAL level using the simplified v0 rules.
/// Full formal risk model is implemented in risk_scorer.rs (v1).
fn compute_hal_level_v0(req: &GateRequest) -> (HalLevel, f32) {
    // Immediate Block: unknown agent or HAL self-modification attempt
    if !is_known_agent(&req.agent) {
        return (HalLevel::Block, 1.0);
    }
    if req.action == "modify_hal" {
        return (HalLevel::Block, 1.0);
    }

    // Block: target outside user home, AI-generated binary execution
    if req.action == "execute_binary" && req.is_ai_generated {
        return (HalLevel::Block, 0.9);
    }
    if is_outside_user_home(&req.target) {
        return (HalLevel::Block, 0.85);
    }

    // Confirm: package install, config modify, any delete, any AI-generated action
    if matches!(
        req.action.as_str(),
        "install_package" | "modify_config" | "delete_file"
    ) || req.is_ai_generated
    {
        let score = if req.action == "delete_file" { 0.75 } else { 0.65 };
        return (HalLevel::Confirm, score);
    }

    // Notify: file moves and creates, known cognos service starts
    if matches!(req.action.as_str(), "move_file" | "create_file" | "start_service") {
        return (HalLevel::Notify, 0.4);
    }

    // Silent: read-only and known-safe actions
    (HalLevel::Silent, 0.15)
}

fn is_outside_user_home(target: &str) -> bool {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
    !target.is_empty()
        && !target.starts_with(&home)
        && !target.starts_with("~/")
        && target.starts_with('/')
}

// ─── HAL daemon ──────────────────────────────────────────────────────────────

pub struct HalDaemon {
    socket_path: std::path::PathBuf,
    notification_socket: std::path::PathBuf,
    hal_ui_socket: std::path::PathBuf,
    audit_log: std::path::PathBuf,
    running: Arc<AtomicBool>,
}

impl HalDaemon {
    pub fn new() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        Self {
            socket_path: "/run/cognos/hal.sock".into(),
            notification_socket: "/run/cognos/notifications.sock".into(),
            hal_ui_socket: "/run/cognos/hal-ui.sock".into(),
            audit_log: std::path::PathBuf::from(home).join(".cognos/audit.log"),
            running: Arc::new(AtomicBool::new(true)),
        }
    }

    /// Start the HAL daemon. Blocks until shutdown signal.
    pub fn run(&self) {
        // Ensure socket directory exists
        if let Some(parent) = self.socket_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        // Remove stale socket
        let _ = std::fs::remove_file(&self.socket_path);

        let listener = match UnixListener::bind(&self.socket_path) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("[hal] Failed to bind socket: {}", e);
                std::process::exit(1);
            }
        };

        // Set socket permissions: 0600
        let _ = std::fs::set_permissions(
            &self.socket_path,
            std::os::unix::fs::PermissionsExt::from_mode(0o600),
        );

        log::info!("[hal] v0 listening on {:?}", self.socket_path);

        listener
            .set_nonblocking(false)
            .expect("Failed to configure listener");

        for stream in listener.incoming() {
            if !self.running.load(Ordering::Relaxed) {
                break;
            }
            match stream {
                Ok(s) => {
                    let audit = self.audit_log.clone();
                    let notif = self.notification_socket.clone();
                    let ui = self.hal_ui_socket.clone();
                    std::thread::spawn(move || {
                        handle_connection(s, audit, notif, ui);
                    });
                }
                Err(e) => {
                    log::error!("[hal] Accept error: {}", e);
                }
            }
        }
    }

    /// Graceful shutdown.
    pub fn shutdown(&self) {
        self.running.store(false, Ordering::Relaxed);
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

// ─── Connection handler ───────────────────────────────────────────────────────

fn handle_connection(
    mut stream: UnixStream,
    audit_log: std::path::PathBuf,
    notification_socket: std::path::PathBuf,
    hal_ui_socket: std::path::PathBuf,
) {
    // Read length-prefixed message
    let mut len_buf = [0u8; 4];
    if stream.read_exact(&mut len_buf).is_err() {
        return;
    }
    let msg_len = u32::from_be_bytes(len_buf) as usize;

    if msg_len > 64 * 1024 {
        // Reject absurdly large messages
        write_response(&mut stream, deny_response(Uuid::nil(), GateReason::HalError, 1.0));
        return;
    }

    let mut payload = vec![0u8; msg_len];
    if stream.read_exact(&mut payload).is_err() {
        return;
    }

    let request: GateRequest = match serde_json::from_slice(&payload) {
        Ok(r) => r,
        Err(e) => {
            log::warn!("[hal] Malformed request: {}", e);
            write_response(&mut stream, deny_response(Uuid::nil(), GateReason::HalError, 1.0));
            return;
        }
    };

    // Security events: unknown agent or modify_hal attempt
    if !is_known_agent(&request.agent) {
        log::warn!("[hal] Unknown agent '{}' — denied + logged as security_alert", request.agent);
        audit_entry(&audit_log, &request, 1.0, "block", "auto_denied", "unknown_agent");
        write_response(
            &mut stream,
            deny_response(request.request_id, GateReason::AutoDenied, 1.0),
        );
        return;
    }
    if request.action == "modify_hal" {
        log::warn!("[hal] modify_hal attempt by '{}' — BLOCK + security_alert", request.agent);
        audit_entry(&audit_log, &request, 1.0, "block", "auto_denied", "modify_hal_attempt");
        write_response(
            &mut stream,
            deny_response(request.request_id, GateReason::AutoDenied, 1.0),
        );
        return;
    }

    // Compute level and score
    let (level, score) = compute_hal_level_v0(&request);

    let response = match level {
        HalLevel::Silent => {
            audit_entry(&audit_log, &request, score, "silent", "auto_approved", "");
            approve_response(request.request_id, GateReason::AutoApproved, score)
        }
        HalLevel::Notify => {
            send_notification(&notification_socket, &request, score);
            audit_entry(&audit_log, &request, score, "notify", "auto_approved", "");
            approve_response(request.request_id, GateReason::AutoApproved, score)
        }
        HalLevel::Confirm | HalLevel::Block => {
            // Set 30-second timeout for user response
            let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
            let result = request_ui_decision(&hal_ui_socket, &request, score, &level);
            let (approved, reason) = result.unwrap_or((false, GateReason::TimeoutDeny));
            let outcome = if approved { "approved" } else { "denied" };
            audit_entry(&audit_log, &request, score, &format!("{:?}", level).to_lowercase(), outcome, "");
            GateResponse {
                request_id: request.request_id,
                approved,
                reason,
                hal_score: score,
                timestamp: chrono::Utc::now(),
            }
        }
    };

    write_response(&mut stream, response);
}

// ─── UI delegation ────────────────────────────────────────────────────────────

/// Send a non-blocking toast notification.
fn send_notification(socket_path: &Path, req: &GateRequest, score: f32) {
    let msg = serde_json::json!({
        "type": "notify",
        "action": req.action,
        "target": req.target,
        "agent": req.agent,
        "hal_score": score,
    });
    if let Ok(mut s) = UnixStream::connect(socket_path) {
        let payload = serde_json::to_vec(&msg).unwrap_or_default();
        let len = (payload.len() as u32).to_be_bytes();
        let _ = s.write_all(&len);
        let _ = s.write_all(&payload);
    }
    // If shell not running, log and continue (auto-approve for notify level)
}

/// Request a confirm/block dialog from the shell UI. Returns (approved, reason).
fn request_ui_decision(
    socket_path: &Path,
    req: &GateRequest,
    score: f32,
    level: &HalLevel,
) -> Option<(bool, GateReason)> {
    // Try to reach the shell UI
    match UnixStream::connect(socket_path) {
        Ok(mut s) => {
            let msg = serde_json::json!({
                "type": if *level == HalLevel::Block { "block_dialog" } else { "confirm_dialog" },
                "request_id": req.request_id,
                "action": req.action,
                "target": req.target,
                "agent": req.agent,
                "hal_score": score,
                "is_ai_generated": req.is_ai_generated,
            });
            let payload = serde_json::to_vec(&msg).unwrap_or_default();
            let len = (payload.len() as u32).to_be_bytes();
            let _ = s.write_all(&len);
            let _ = s.write_all(&payload);

            // Read response (up to 30 seconds)
            let _ = s.set_read_timeout(Some(Duration::from_secs(30)));
            let mut len_buf = [0u8; 4];
            if s.read_exact(&mut len_buf).is_err() {
                return None; // timeout → caller uses TimeoutDeny
            }
            let resp_len = u32::from_be_bytes(len_buf) as usize;
            let mut resp_buf = vec![0u8; resp_len];
            if s.read_exact(&mut resp_buf).is_err() {
                return None;
            }
            let resp: serde_json::Value = serde_json::from_slice(&resp_buf).ok()?;
            let approved = resp["approved"].as_bool().unwrap_or(false);
            let reason = if approved {
                GateReason::UserApproved
            } else {
                GateReason::UserDenied
            };
            Some((approved, reason))
        }
        Err(_) => {
            // Shell not running — terminal fallback
            terminal_fallback(req, level)
        }
    }
}

/// Terminal fallback when no Wayland session is running.
fn terminal_fallback(req: &GateRequest, level: &HalLevel) -> Option<(bool, GateReason)> {
    eprintln!(
        "\n[HAL] {} Request\n  Action: {}\n  Target: {}\n  Agent:  {}\n  AI-generated: {}",
        if *level == HalLevel::Block { "BLOCK" } else { "CONFIRM" },
        req.action,
        req.target,
        req.agent,
        req.is_ai_generated,
    );

    if *level == HalLevel::Block {
        eprint!("\nType 'yes' to allow, anything else to deny: ");
    } else {
        eprint!("\n[y/N] Allow? ");
    }

    let mut input = String::new();
    std::io::stdin().read_line(&mut input).ok()?;
    let approved = input.trim() == "yes" || input.trim().to_lowercase() == "y";
    let reason = if approved { GateReason::UserApproved } else { GateReason::UserDenied };
    Some((approved, reason))
}

// ─── Wire helpers ─────────────────────────────────────────────────────────────

fn write_response(stream: &mut UnixStream, response: GateResponse) {
    let payload = serde_json::to_vec(&response).unwrap_or_default();
    let len = (payload.len() as u32).to_be_bytes();
    let _ = stream.write_all(&len);
    let _ = stream.write_all(&payload);
}

fn approve_response(id: Uuid, reason: GateReason, score: f32) -> GateResponse {
    GateResponse {
        request_id: id,
        approved: true,
        reason,
        hal_score: score,
        timestamp: chrono::Utc::now(),
    }
}

fn deny_response(id: Uuid, reason: GateReason, score: f32) -> GateResponse {
    GateResponse {
        request_id: id,
        approved: false,
        reason,
        hal_score: score,
        timestamp: chrono::Utc::now(),
    }
}

// ─── Audit ────────────────────────────────────────────────────────────────────

fn audit_entry(
    log_path: &Path,
    req: &GateRequest,
    score: f32,
    level: &str,
    outcome: &str,
    note: &str,
) {
    let entry = serde_json::json!({
        "ts": chrono::Utc::now().to_rfc3339(),
        "agent": req.agent,
        "action": req.action,
        "target": req.target,
        "hal_score": score,
        "hal_level": level,
        "outcome": outcome,
        "request_id": req.request_id,
        "intent_id": req.intent_id,
        "is_ai_generated": req.is_ai_generated,
        "note": note,
    });

    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
    {
        let _ = writeln!(f, "{}", serde_json::to_string(&entry).unwrap_or_default());
    }
}

// ─── Tests (human-authored) ───────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn req(agent: &str, action: &str, target: &str, ai: bool) -> GateRequest {
        GateRequest {
            request_id: Uuid::new_v4(),
            agent: agent.to_string(),
            action: action.to_string(),
            target: target.to_string(),
            parameters: serde_json::Value::Null,
            is_ai_generated: ai,
            intent_id: None,
        }
    }

    #[test]
    fn unknown_agent_is_block() {
        let r = req("evil", "open_file", "~/test.py", false);
        let (level, score) = compute_hal_level_v0(&r);
        assert_eq!(level, HalLevel::Block);
        assert_eq!(score, 1.0);
    }

    #[test]
    fn modify_hal_is_always_block() {
        for agent in KNOWN_AGENTS {
            let r = req(agent, "modify_hal", "/etc/cognos/hal", false);
            let (level, score) = compute_hal_level_v0(&r);
            assert_eq!(level, HalLevel::Block, "modify_hal by {} should be Block", agent);
            assert_eq!(score, 1.0);
        }
    }

    #[test]
    fn open_file_is_silent() {
        let r = req("file", "open_file", "~/motor.py", false);
        let (level, score) = compute_hal_level_v0(&r);
        assert_eq!(level, HalLevel::Silent);
        assert!(score < 0.3);
    }

    #[test]
    fn delete_file_is_confirm() {
        let r = req("file", "delete_file", "~/motor.py", false);
        let (level, score) = compute_hal_level_v0(&r);
        assert_eq!(level, HalLevel::Confirm);
        assert!(score >= 0.6 && score < 0.8);
    }

    #[test]
    fn ai_generated_action_is_confirm_or_block() {
        let r = req("coding", "open_file", "~/motor.py", true);
        let (level, _) = compute_hal_level_v0(&r);
        assert!(
            level == HalLevel::Confirm || level == HalLevel::Block,
            "AI-generated action should be at least Confirm level"
        );
    }

    #[test]
    fn target_outside_home_is_block() {
        let r = req("file", "open_file", "/etc/passwd", false);
        let (level, score) = compute_hal_level_v0(&r);
        assert_eq!(level, HalLevel::Block);
        assert!(score >= 0.8);
    }

    #[test]
    fn execute_ai_binary_is_block() {
        let r = req("coding", "execute_binary", "~/tmp/generated.sh", true);
        let (level, score) = compute_hal_level_v0(&r);
        assert_eq!(level, HalLevel::Block);
        assert!(score >= 0.8);
    }
}
