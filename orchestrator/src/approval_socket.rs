//! Blocking HAL Unix-socket gate for orchestrator approval resume.
//!
//! When gRPC HalGate returns `approval_required`, the orchestrator opens a
//! synchronous gate on `COGNOS_HAL_SOCKET` (same path as `approval_flow`).
//! HAL may delegate to `COGNOS_HAL_UI_SOCKET` where `cognos approval watch`
//! listens.

#[cfg(unix)]
mod imp {
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    use cognos_hal::{GateReason, GateRequest, GateResponse};
    use cognos_ipc_grpc::approval_ui::{read_frame, write_frame};
    use uuid::Uuid;

    use crate::hal_gate::SideEffect;

    const DEFAULT_HAL_SOCKET: &str = "/run/cognos/hal.sock";

    pub fn approval_timeout_secs() -> u64 {
        std::env::var("COGNOS_APPROVAL_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(120)
    }

    fn hal_socket_path() -> String {
        std::env::var("COGNOS_HAL_SOCKET").unwrap_or_else(|_| DEFAULT_HAL_SOCKET.to_string())
    }

    /// Map orchestrator side-effect to the v0 gate request shape.
    pub fn side_effect_to_gate_request(action: &SideEffect, trace_id: &str) -> GateRequest {
        let agent = action
            .source_agent
            .strip_prefix("agent.")
            .unwrap_or(&action.source_agent)
            .to_string();
        let gate_action = map_gate_action(&action.op);
        let intent_id = Uuid::parse_str(trace_id).ok();
        GateRequest {
            request_id: Uuid::new_v4(),
            agent,
            action: gate_action,
            target: action.path.clone(),
            parameters: serde_json::json!({
                "capability": action.capability,
                "op": action.op,
            }),
            is_ai_generated: false,
            intent_id,
        }
    }

    fn map_gate_action(op: &str) -> String {
        let lower = op.to_lowercase();
        if lower.contains("delete") || lower.contains("unlink") || lower.contains("remove") {
            return "delete_file".to_string();
        }
        if lower.contains("install") {
            return "install_package".to_string();
        }
        if lower.contains("pkg") {
            return "install_package".to_string();
        }
        if lower.contains("config") {
            return "modify_config".to_string();
        }
        if lower.contains("create") || lower.contains("mkdir") {
            return "create_file".to_string();
        }
        if lower.contains("move") || lower.contains("rename") {
            return "move_file".to_string();
        }
        if lower.contains("execute") || lower.contains("binary") {
            return "execute_binary".to_string();
        }
        "open_file".to_string()
    }

    /// Synchronous gate round-trip (call from `spawn_blocking`).
    pub fn blocking_gate(action: &SideEffect, trace_id: &str) -> std::io::Result<GateResponse> {
        let request = side_effect_to_gate_request(action, trace_id);
        let mut stream = UnixStream::connect(hal_socket_path())?;
        let payload = serde_json::to_vec(&request).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
        })?;
        write_frame(&mut stream, &payload)?;
        let resp_bytes = read_frame(&mut stream)?;
        serde_json::from_slice(&resp_bytes).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
        })
    }

    pub fn reason_label(reason: &GateReason) -> &'static str {
        match reason {
            GateReason::AutoApproved => "auto_approved",
            GateReason::UserApproved => "user_approved",
            GateReason::AutoDenied => "auto_denied",
            GateReason::UserDenied => "user_denied",
            GateReason::TimeoutDeny => "timeout_deny",
            GateReason::HalError => "hal_error",
        }
    }

    /// Outer orchestrator timeout (distinct from HAL's internal 30s UI wait).
    pub fn gate_timeout() -> Duration {
        Duration::from_secs(approval_timeout_secs())
    }
}

#[cfg(unix)]
pub use imp::*;

#[cfg(not(unix))]
pub fn approval_timeout_secs() -> u64 {
    120
}
