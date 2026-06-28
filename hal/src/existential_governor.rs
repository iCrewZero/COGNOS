//! Existential governor — guards against actions that threaten system existence.
//!
//!
//! Most HAL policy is about *risk*: how reversible is this action, how
//! trusted is the agent, how anomalous is the time. The existential governor
//! is about something simpler and more absolute: would this action *end* or
//! *maim* the system that HAL is supposed to protect?
//!
//! Resources protected by the governor are non-negotiable: HAL itself, the
//! audit chain, the kernel, the memory store, and the authentication keys.
//! Any action that threatens one of these is classified as
//! [`ExistentialVerdict::Catastrophic`] and routed to mandatory human review
//! regardless of trust, reputation, or autonomy level.
//!
//! v0: stub implementation. Threat detection is pattern-matching on resource
//! paths; a learned classifier is TODO(v1).

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{error, warn};

// v0: stub implementation

/// Type alias for an agent identifier (matches the rest of the crate).
pub type AgentId = String;

// ─── Threat Types ───────────────────────────────────────────────────────────────

/// The category of protected resource an action threatens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ThreatType {
    /// HAL's own binaries, configs, or runtime state.
    HalItself,
    /// The tamper-evident audit chain.
    AuditChain,
    /// The kernel or kernel-adjacent interfaces.
    Kernel,
    /// The persistent memory store (vectors, episodic logs, etc.).
    MemoryStore,
    /// Authentication keys, tokens, or capability grants.
    AuthKeys,
}

impl ThreatType {
    /// Human-readable description of the threat.
    pub fn description(&self) -> &'static str {
        match self {
            Self::HalItself => "modification of HAL itself",
            Self::AuditChain => "tampering with the audit chain",
            Self::Kernel => "kernel or kernel-adjacent modification",
            Self::MemoryStore => "destruction or corruption of the memory store",
            Self::AuthKeys => "compromise of authentication keys or tokens",
        }
    }
}

// ─── Verdict ────────────────────────────────────────────────────────────────────

/// The governor's verdict on an action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExistentialVerdict {
    /// The action does not threaten any protected resource.
    Safe,
    /// The action threatens a protected resource but is not catastrophic
    /// (e.g. appending to a memory store with a valid capability). v0 never
    /// emits this; all threats escalate to Catastrophic.
    Threatens(ThreatType),
    /// The action is catastrophic and must be routed to mandatory human
    /// review regardless of trust or autonomy level.
    Catastrophic,
}

// ─── Action Descriptor ──────────────────────────────────────────────────────────

/// Minimal descriptor of an action proposed for existential evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExistentialAction {
    /// Agent proposing the action.
    pub agent: AgentId,
    /// Action name (e.g. "write_file", "delete_directory").
    pub action: String,
    /// Target resource path or identifier.
    pub target: String,
    /// Whether the action is destructive (delete, overwrite, format).
    pub destructive: bool,
    /// When the action was proposed (for audit correlation).
    pub proposed_at: DateTime<Utc>,
}

// ─── Existential Governor ───────────────────────────────────────────────────────

/// The existential governor.
pub struct ExistentialGovernor {
    /// Resource path prefixes protected by the governor.
    protected_resources: HashSet<String>,
    /// Whether the kill switches are currently armed.
    kill_switches: KillSwitchState,
}

/// State of the governor's kill switches.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KillSwitchState {
    /// Whether HAL-self-protection is armed.
    pub hal_self_armed: bool,
    /// Whether audit-chain protection is armed.
    pub audit_chain_armed: bool,
    /// Whether kernel protection is armed.
    pub kernel_armed: bool,
    /// Whether memory-store protection is armed.
    pub memory_store_armed: bool,
    /// Whether auth-key protection is armed.
    pub auth_keys_armed: bool,
}

impl KillSwitchState {
    /// Build a state with all kill switches armed (the safe default).
    pub fn all_armed() -> Self {
        Self {
            hal_self_armed: true,
            audit_chain_armed: true,
            kernel_armed: true,
            memory_store_armed: true,
            auth_keys_armed: true,
        }
    }
}

impl Default for ExistentialGovernor {
    fn default() -> Self {
        Self::new()
    }
}

impl ExistentialGovernor {
    /// Build a new governor with all kill switches armed and the default
    /// protected-resource set.
    pub fn new() -> Self {
        let mut protected = HashSet::new();
        for path in DEFAULT_PROTECTED_RESOURCES {
            protected.insert((*path).to_string());
        }
        Self {
            protected_resources: protected,
            kill_switches: KillSwitchState::all_armed(),
        }
    }

    /// Add a protected resource path prefix.
    pub fn protect(&mut self, path: impl Into<String>) {
        self.protected_resources.insert(path.into());
    }

    /// Evaluate the existential threat of an action.
    pub fn evaluate(&self, action: &ExistentialAction) -> ExistentialVerdict {
        let threat = self.classify(action);
        match threat {
            None => ExistentialVerdict::Safe,
            Some(t) => {
                if self.kill_switch_armed(t) {
                    error!(
                        agent = %action.agent,
                        action = %action.action,
                        target = %action.target,
                        threat = t.description(),
                        "existential_governor: CATASTROPHIC threat detected"
                    );
                    // Per the spec, any threat to HAL itself is catastrophic
                    // and mandates human review.
                    ExistentialVerdict::Catastrophic
                } else {
                    warn!(
                        agent = %action.agent,
                        action = %action.action,
                        target = %action.target,
                        threat = t.description(),
                        "existential_governor: threat detected but kill switch disarmed"
                    );
                    ExistentialVerdict::Threatens(t)
                }
            }
        }
    }

    /// Whether the kill switch for a given threat type is currently armed.
    pub fn kill_switch_armed(&self, t: ThreatType) -> bool {
        match t {
            ThreatType::HalItself => self.kill_switches.hal_self_armed,
            ThreatType::AuditChain => self.kill_switches.audit_chain_armed,
            ThreatType::Kernel => self.kill_switches.kernel_armed,
            ThreatType::MemoryStore => self.kill_switches.memory_store_armed,
            ThreatType::AuthKeys => self.kill_switches.auth_keys_armed,
        }
    }

    /// Disarm a kill switch (operator action only; v0 has no auth on this).
    pub fn disarm(&mut self, t: ThreatType) {
        match t {
            ThreatType::HalItself => self.kill_switches.hal_self_armed = false,
            ThreatType::AuditChain => self.kill_switches.audit_chain_armed = false,
            ThreatType::Kernel => self.kill_switches.kernel_armed = false,
            ThreatType::MemoryStore => self.kill_switches.memory_store_armed = false,
            ThreatType::AuthKeys => self.kill_switches.auth_keys_armed = false,
        }
    }

    /// Classify an action against the protected-resource set.
    ///
    /// v0 uses simple prefix matching on the action target. v1 will use a
    /// proper resource ontology and capability-aware reasoning.
    // TODO(v1): replace prefix matching with a resource-ontology lookup
    // that accounts for symlinks, bind mounts, and capability scopes.
    fn classify(&self, action: &ExistentialAction) -> Option<ThreatType> {
        let target = action.target.as_str();
        for path in &self.protected_resources {
            if !target.starts_with(path.as_str()) {
                continue;
            }
            // Heuristic: HAL-self paths always map to HalItself.
            if path.starts_with("/opt/cognos/hal") || path.starts_with("/etc/cognos/hal") {
                return Some(ThreatType::HalItself);
            }
            if path.starts_with("/var/lib/cognos/audit") {
                return Some(ThreatType::AuditChain);
            }
            if path.starts_with("/sys/kernel")
                || path.starts_with("/proc/sys")
                || path.starts_with("/lib/modules")
            {
                return Some(ThreatType::Kernel);
            }
            if path.starts_with("/var/lib/cognos/memory") {
                return Some(ThreatType::MemoryStore);
            }
            if path.starts_with("/etc/cognos/keys")
                || path.starts_with("/var/lib/cognos/tokens")
            {
                return Some(ThreatType::AuthKeys);
            }
        }
        None
    }
}

/// The default set of protected resource path prefixes.
pub const DEFAULT_PROTECTED_RESOURCES: &[&str] = &[
    "/opt/cognos/hal",
    "/etc/cognos/hal",
    "/var/lib/cognos/audit",
    "/sys/kernel",
    "/proc/sys",
    "/lib/modules",
    "/var/lib/cognos/memory",
    "/etc/cognos/keys",
    "/var/lib/cognos/tokens",
];

// ─── Errors ─────────────────────────────────────────────────────────────────────

/// Errors returned by the existential governor.
#[derive(Debug, Error)]
pub enum ExistentialGovernorError {
    /// An attempt was made to disarm a kill switch via an unauthorized path.
    #[error("unauthorized disarm attempt for threat type {0:?}")]
    UnauthorizedDisarm(ThreatType),
}
