/// HAL Permissions — capability lattice enforcement for COGNOS/OS.
///
/// THIS FILE IS HUMAN-WRITTEN ONLY. Zero AI authorship.
///
/// Anything not explicitly ALLOW-listed is implicitly DENY.
/// Violations are logged as security events and rejected before any
/// message reaches the target agent.

use std::collections::{HashMap, HashSet};
use serde::{Deserialize, Serialize};

// ─── Capability types ─────────────────────────────────────────────────────────

/// Every capability type in the system, explicitly enumerated.
/// The list is closed — new capabilities require a spec update and human review.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Capability {
    // Filesystem
    ReadUserHome,
    WriteUserHome,
    ReadFileMeta,
    MoveFileUserHome,
    CreateFileUserHome,
    ListDirectory,
    DeleteFile,       // Always HAL-gated; moves to recycle

    // Process / exec
    OpenApp,
    ExecuteBinary,    // Requires Security scan + HAL Block

    // Memory / ChromaDB
    ReadMemoryDb,
    WriteMemoryDb,
    QueryMemory,

    // Network
    OutboundApiOnly,  // Only to user-specified endpoints

    // Kernel / system
    ReadEbpfTelemetry,
    WriteSchedHints,
    AdjustCgroupWeights,
    SwitchCpuGovernor,

    // IPC
    SendIntentDispatch,
    SendMemoryQuery,
    SendMemoryResult,
    SendSecurityAlert,
    SendResourceHint,
    SendFileOperation,
    SendHalGateRequest,
    SendHalGateResponse,
    SendHeartbeat,
    SendCapabilityViolation,

    // HAL
    ReadHalConfig,
    ModifyHal,        // DENY for all agents

    // UI
    RenderUi,
    DisplayNotification,
    ReadAgentStatus,

    // Security
    ReadAppBehaviorLogs,
    ReadAppArmorLogs,
    StaticAnalysisGenerated,
    RaiseHalAlert,
    RecommendPermissionChanges,

    // Clipboard
    ReadClipboard,    // Per-request grant only
}

// ─── Agent names ──────────────────────────────────────────────────────────────

pub const ALL_AGENTS: &[&str] = &[
    "planner", "memory", "security", "scheduler",
    "file", "coding", "ui", "coordinator",
];

// ─── Lattice ──────────────────────────────────────────────────────────────────

/// The capability lattice. Defines exactly what each agent is allowed to do.
/// Built once at startup from the hard-coded spec — not configurable at runtime.
pub struct CapabilityLattice {
    allow: HashMap<String, HashSet<Capability>>,
}

impl CapabilityLattice {
    /// Build the lattice from the formal spec.
    pub fn new() -> Self {
        use Capability::*;
        let mut allow: HashMap<String, HashSet<Capability>> = HashMap::new();

        // ── Planner ───────────────────────────────────────────────────────────
        allow.insert("planner".into(), [
            QueryMemory,
            SendIntentDispatch,
            SendMemoryQuery,
            SendHalGateRequest,
            SendHeartbeat,
        ].into());

        // ── Memory ────────────────────────────────────────────────────────────
        allow.insert("memory".into(), [
            ReadUserHome,
            ReadFileMeta,
            ReadMemoryDb,
            WriteMemoryDb,
            QueryMemory,
            SendMemoryResult,
            SendHalGateRequest,
            SendHeartbeat,
        ].into());

        // ── Security ──────────────────────────────────────────────────────────
        allow.insert("security".into(), [
            ReadAppBehaviorLogs,
            ReadAppArmorLogs,
            StaticAnalysisGenerated,
            RaiseHalAlert,
            RecommendPermissionChanges,
            SendSecurityAlert,
            SendHalGateRequest,
            SendCapabilityViolation,
            SendHeartbeat,
        ].into());

        // ── Scheduler ─────────────────────────────────────────────────────────
        allow.insert("scheduler".into(), [
            ReadEbpfTelemetry,
            WriteSchedHints,
            AdjustCgroupWeights,
            SwitchCpuGovernor,
            SendResourceHint,
            SendHeartbeat,
        ].into());

        // ── File ──────────────────────────────────────────────────────────────
        allow.insert("file".into(), [
            ReadUserHome,
            WriteUserHome,
            ReadFileMeta,
            MoveFileUserHome,
            CreateFileUserHome,
            ListDirectory,
            DeleteFile,
            OpenApp,
            SendFileOperation,
            SendHalGateRequest,
            SendHeartbeat,
        ].into());

        // ── Coding ────────────────────────────────────────────────────────────
        allow.insert("coding".into(), [
            ReadUserHome,
            ReadFileMeta,
            SendHalGateRequest,
            SendMemoryQuery,
            SendFileOperation,
            SendHeartbeat,
            // Code is written to temp first — WriteUserHome only after HAL approval
        ].into());

        // ── UI ────────────────────────────────────────────────────────────────
        allow.insert("ui".into(), [
            RenderUi,
            DisplayNotification,
            ReadAgentStatus,
            SendHalGateRequest,
            SendHeartbeat,
        ].into());

        // ── Coordinator ───────────────────────────────────────────────────────
        allow.insert("coordinator".into(), [
            SendIntentDispatch,
            SendMemoryQuery,
            SendMemoryResult,
            SendSecurityAlert,
            SendResourceHint,
            SendFileOperation,
            SendHalGateRequest,
            SendHalGateResponse,
            SendHeartbeat,
            SendCapabilityViolation,
            ReadAgentStatus,
        ].into());

        // ModifyHal is explicitly absent from every agent — enforced below.
        Self { allow }
    }

    /// Check if an agent is allowed to use a capability.
    ///
    /// Returns Ok(()) if allowed, Err(violation) if not.
    pub fn check(&self, agent: &str, capability: &Capability) -> Result<(), CapabilityViolation> {
        // ModifyHal is always denied for every agent, no exceptions.
        if capability == &Capability::ModifyHal {
            return Err(CapabilityViolation {
                agent: agent.to_string(),
                capability: capability.clone(),
                reason: "ModifyHal is denied for all agents — this is a security event".to_string(),
            });
        }

        // Unknown agent: deny everything
        if !ALL_AGENTS.contains(&agent) {
            return Err(CapabilityViolation {
                agent: agent.to_string(),
                capability: capability.clone(),
                reason: format!("'{}' is not a known agent", agent),
            });
        }

        // Check the allow list
        let allowed = self.allow.get(agent)
            .map(|set| set.contains(capability))
            .unwrap_or(false);

        if allowed {
            Ok(())
        } else {
            Err(CapabilityViolation {
                agent: agent.to_string(),
                capability: capability.clone(),
                reason: format!(
                    "Capability {:?} is not in the allow list for agent '{}'",
                    capability, agent
                ),
            })
        }
    }

    /// Assert a capability is allowed, panicking with a clear message if not.
    /// Use in agent code where a violation would be a programming error.
    pub fn assert_allowed(&self, agent: &str, cap: &Capability) {
        if let Err(v) = self.check(agent, cap) {
            panic!("Capability lattice violation: {}", v.reason);
        }
    }

    /// Return the full allow set for an agent.
    pub fn allowed_capabilities(&self, agent: &str) -> HashSet<Capability> {
        self.allow.get(agent).cloned().unwrap_or_default()
    }
}

impl Default for CapabilityLattice {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Violation type ───────────────────────────────────────────────────────────

/// A capability lattice violation — logged and raised as a security alert.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityViolation {
    pub agent: String,
    pub capability: Capability,
    pub reason: String,
}

impl std::fmt::Display for CapabilityViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CapabilityViolation[{}]: {:?} — {}", self.agent, self.capability, self.reason)
    }
}

impl std::error::Error for CapabilityViolation {}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn lattice() -> CapabilityLattice { CapabilityLattice::new() }

    #[test]
    fn file_agent_can_read_user_home() {
        assert!(lattice().check("file", &Capability::ReadUserHome).is_ok());
    }

    #[test]
    fn security_agent_cannot_write_filesystem() {
        let result = lattice().check("security", &Capability::WriteUserHome);
        assert!(result.is_err(), "Security agent must not be able to write filesystem");
    }

    #[test]
    fn modify_hal_denied_for_all_agents() {
        for agent in ALL_AGENTS {
            let result = lattice().check(agent, &Capability::ModifyHal);
            assert!(result.is_err(), "ModifyHal must be denied for agent '{}'", agent);
        }
    }

    #[test]
    fn unknown_agent_denied_everything() {
        let result = lattice().check("evil-agent", &Capability::ReadUserHome);
        assert!(result.is_err());
        assert!(result.unwrap_err().reason.contains("not a known agent"));
    }

    #[test]
    fn scheduler_cannot_execute_binary() {
        assert!(lattice().check("scheduler", &Capability::ExecuteBinary).is_err());
    }

    #[test]
    fn ui_cannot_modify_agent_behavior() {
        assert!(lattice().check("ui", &Capability::WriteUserHome).is_err());
        assert!(lattice().check("ui", &Capability::ReadMemoryDb).is_err());
    }

    #[test]
    fn coding_agent_cannot_write_without_hal() {
        // Coding agent gets WriteUserHome only after HAL approval at runtime;
        // it is NOT in the static lattice — it writes to temp first.
        assert!(lattice().check("coding", &Capability::WriteUserHome).is_err());
    }

    #[test]
    fn planner_has_no_filesystem_access() {
        assert!(lattice().check("planner", &Capability::ReadUserHome).is_err());
        assert!(lattice().check("planner", &Capability::WriteUserHome).is_err());
    }

    #[test]
    fn coordinator_can_route_all_message_types() {
        use Capability::*;
        let l = lattice();
        for cap in &[
            SendIntentDispatch, SendMemoryQuery, SendMemoryResult,
            SendSecurityAlert, SendResourceHint, SendFileOperation,
            SendHalGateRequest, SendHalGateResponse, SendHeartbeat,
        ] {
            assert!(l.check("coordinator", cap).is_ok(), "Coordinator missing {:?}", cap);
        }
    }
}
