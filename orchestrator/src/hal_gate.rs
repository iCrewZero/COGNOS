//! HAL gating for the orchestrator.
//!
//! Every side-effecting action the orchestrator is about to dispatch to an
//! agent must first pass through HAL. This module builds the [`HalGateRequest`]
//! from a proposed [`SideEffect`], sends it over the IPC bus via the
//! orchestrator's [`CognosClient`], and interprets the HAL response into a
//! [`Decision`].
//!
//! The decision maps the wire status HAL returns:
//!   * `granted`           → [`Decision::Granted`]
//!   * `approval_required` → [`Decision::ApprovalRequired`]
//!   * `denied` / `failed` → [`Decision::Denied`]  (fail closed)

use cognos_ipc_grpc::client::{ClientError, CognosClient};
use cognos_ipc_grpc::proto::v1::HalGateRequest;
use thiserror::Error;

/// A side-effecting action the orchestrator wants to perform, described in the
/// terms HAL gates on: the operation, the target path/resource, the capability
/// it requires, and the agent that would perform it.
#[derive(Debug, Clone)]
pub struct SideEffect {
    /// Operation name, e.g. `"file.delete"`, `"execute_open"`.
    pub op: String,
    /// Target resource / path, e.g. `"/etc/passwd"`, `"~/notes.txt"`.
    pub path: String,
    /// Capability the action requires, e.g. `"file.delete"`.
    pub capability: String,
    /// Agent that would carry out the action (the gate request's source).
    pub source_agent: String,
}

impl SideEffect {
    pub fn new(
        op: impl Into<String>,
        path: impl Into<String>,
        capability: impl Into<String>,
        source_agent: impl Into<String>,
    ) -> Self {
        Self {
            op: op.into(),
            path: path.into(),
            capability: capability.into(),
            source_agent: source_agent.into(),
        }
    }
}

/// The outcome of a HAL gate evaluation.
#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    /// HAL approved the action. `grant_token` must accompany the follow-up op.
    Granted { grant_token: String, risk_score: f64 },
    /// HAL requires an explicit user approval before the action may proceed.
    ApprovalRequired { risk_score: f64 },
    /// HAL denied the action outright.
    Denied { reason: String, risk_score: f64 },
}

impl Decision {
    /// True only for [`Decision::Granted`] — the sole state in which the
    /// orchestrator may dispatch the action to an agent without further steps.
    pub fn is_granted(&self) -> bool {
        matches!(self, Decision::Granted { .. })
    }
}

#[derive(Debug, Error)]
pub enum GateError {
    #[error("HAL gate RPC failed: {0}")]
    Rpc(#[from] ClientError),
    #[error("HAL returned an unknown status: {0}")]
    UnknownStatus(String),
}

/// Which capabilities may cause side effects and therefore MUST be gated.
///
/// Read-only work (memory/query/plan/search/analysis) is dispatched without a
/// gate; anything that can mutate the machine (write/delete/execute/install/…)
/// is gated. When in doubt this errs toward gating.
pub fn is_side_effecting(capability: &str) -> bool {
    let c = capability.to_lowercase();
    const SIDE_EFFECT_MARKERS: &[&str] = &[
        "write", "delete", "remove", "execute", "install", "uninstall", "update",
        "modify", "pkg", "spawn", "kill", "mount", "net.send", "hal",
    ];
    SIDE_EFFECT_MARKERS.iter().any(|m| c.contains(m))
}

/// Send `action` to HAL through `client` and interpret the response.
///
/// `client` must already be connected to a HAL gate endpoint. The gate request
/// lets HAL compute the risk (`risk_override = -1.0`) and allows the approval
/// flow (`allow_approval = true`), so a risky-but-not-blocked action comes back
/// as [`Decision::ApprovalRequired`] rather than a hard denial.
pub async fn gate_action(
    client: &CognosClient,
    action: &SideEffect,
    trace_id: &str,
) -> Result<Decision, GateError> {
    let request = HalGateRequest {
        op: action.op.clone(),
        device: action.path.clone(),
        data: Vec::new(),
        capability: action.capability.clone(),
        risk_override: -1.0,
        allow_approval: true,
        trace_id: trace_id.to_string(),
    };

    let response = client.request_hal_gate(request).await?;
    let risk_score = response.risk_score;

    match response.status.as_str() {
        "granted" => Ok(Decision::Granted {
            grant_token: response.grant_token,
            risk_score,
        }),
        "approval_required" => Ok(Decision::ApprovalRequired { risk_score }),
        "denied" | "failed" => {
            let reason = response
                .violation
                .map(|v| v.message)
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| format!("HAL {}", response.status));
            Ok(Decision::Denied { reason, risk_score })
        }
        other => Err(GateError::UnknownStatus(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_only_capabilities_are_not_gated() {
        assert!(!is_side_effecting("memory.read"));
        assert!(!is_side_effecting("file.read"));
        assert!(!is_side_effecting("coding.plan"));
        assert!(!is_side_effecting("intent.disambiguate"));
    }

    #[test]
    fn mutating_capabilities_are_gated() {
        assert!(is_side_effecting("file.delete"));
        assert!(is_side_effecting("file.write"));
        assert!(is_side_effecting("pkg.execute"));
        assert!(is_side_effecting("coding.execute"));
    }
}
