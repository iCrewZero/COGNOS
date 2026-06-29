//! ANFS per-agent access control — gates every VFS operation against a
//! capability lattice and a path-restriction policy, emitting an audit entry
//! for each check (allow or deny).
//!
//! Restricted paths:
//!   * `~/.cognos/`   — only the HAL and the ANFS daemon itself may touch.
//!   * `~/.ssh/`      — only the security agent (and the user) may touch.
//!   * `/etc/cognos/` — only the HAL and the security agent may touch.
//!
//! Every call to [`AnfsSecurity::check`] emits an audit record to the
//! configured audit handle, regardless of outcome, so that the audit chain
//! has a complete picture of attempted file access.
//!
//! v0: stub implementation — the lattice is a flat allow-set, the audit
//! handle is an in-memory ring buffer, and `check` always returns `Ok(())`
//! for non-restricted paths and `Err(PathRestricted)` for restricted ones.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::debug;

// ─── Errors ─────────────────────────────────────────────────────────────────

/// Outcome of a denied security check.
#[derive(Debug, Error)]
pub enum SecurityError {
    /// The agent is not known to the lattice.
    #[error("unknown agent: {0}")]
    UnknownAgent(String),
    /// The agent is known but lacks the required capability.
    #[error("denied: agent {agent} lacks {op:?} on {path}")]
    Denied {
        /// Agent that was denied.
        agent: String,
        /// Operation that was attempted.
        op: FileOp,
        /// Path that was targeted.
        path: String,
    },
    /// The path is restricted and the agent is not on its allow-list.
    #[error("path restricted: {path} (agent {agent})")]
    PathRestricted {
        /// Restricted path that was targeted.
        path: String,
        /// Agent that attempted access.
        agent: String,
    },
    /// The operation requires explicit HAL approval (interactive gate).
    #[error("needs HAL approval: agent {agent} op {op:?} on {path}")]
    NeedsHalApproval {
        /// Agent requesting the operation.
        agent: String,
        /// Operation being gated.
        op: FileOp,
        /// Path being targeted.
        path: String,
    },
}

// ─── File operations ────────────────────────────────────────────────────────

/// A VFS operation being checked against the security policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileOp {
    /// Read file contents.
    Read,
    /// Write file contents (existing file).
    Write,
    /// Create a new file or directory.
    Create,
    /// Delete a file or directory.
    Delete,
    /// Rename a file or directory.
    Rename,
    /// Read metadata (`getattr`).
    GetAttr,
    /// Write metadata (`chmod`, `chown`, `utimes`, xattr set).
    SetAttr,
}

impl FileOp {
    /// Whether this operation mutates the filesystem.
    pub fn is_mutating(&self) -> bool {
        matches!(
            self,
            FileOp::Write
                | FileOp::Create
                | FileOp::Delete
                | FileOp::Rename
                | FileOp::SetAttr
        )
    }
}

// ─── Capability lattice ──────────────────────────────────────────────────────

/// A node in the capability lattice.
///
/// v0: a flat string label. TODO(v1): delegate to
/// `hal::capability_lattice::LatticeNode` for real partial-order math.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct Capability(pub String);

impl Capability {
    /// The all-powerful root capability.
    pub fn root() -> Self {
        Self("cognos.root".to_string())
    }

    /// The HAL-only capability.
    pub fn hal() -> Self {
        Self("cognos.hal".to_string())
    }

    /// The security-agent capability.
    pub fn security() -> Self {
        Self("cognos.security".to_string())
    }

    /// The ANFS-daemon capability.
    pub fn anfs() -> Self {
        Self("cognos.anfs".to_string())
    }
}

/// The capability lattice used by ANFS to decide which agents may perform
/// which file operations.
///
/// v0: a flat `HashMap<AgentId, HashSet<Capability>>` with no real lattice
/// structure. TODO(v1): reuse `hal::capability_lattice::CapabilityLattice`
/// for proper escalation-path reasoning.
pub struct CapabilityLattice {
    /// Map from agent id to the set of capabilities that agent holds.
    pub agents: HashMap<String, HashSet<Capability>>,
}

impl CapabilityLattice {
    /// Construct an empty lattice.
    pub fn new() -> Self {
        Self {
            agents: HashMap::new(),
        }
    }

    /// Construct a lattice seeded with the default COGNOS agents.
    pub fn with_defaults() -> Self {
        let mut lat = Self::new();
        lat.grant("cognos.hal", Capability::hal());
        lat.grant("cognos.hal", Capability::root());
        lat.grant("cognos.security", Capability::security());
        lat.grant("cognos.anfs", Capability::anfs());
        lat.grant("cognos.anfs", Capability::hal());
        lat
    }

    /// Grant a capability to an agent.
    pub fn grant(&mut self, agent: &str, cap: Capability) {
        self.agents
            .entry(agent.to_string())
            .or_default()
            .insert(cap);
    }

    /// Whether `agent` holds `cap`.
    pub fn holds(&self, agent: &str, cap: &Capability) -> bool {
        self.agents
            .get(agent)
            .map(|set| set.contains(cap))
            .unwrap_or(false)
    }

    /// Whether `agent` holds *any* of the capabilities in `caps`.
    pub fn holds_any(&self, agent: &str, caps: &HashSet<Capability>) -> bool {
        match self.agents.get(agent) {
            Some(held) => caps.iter().any(|c| held.contains(c)),
            None => false,
        }
    }

    /// Whether `agent` is known to the lattice at all.
    pub fn knows(&self, agent: &str) -> bool {
        self.agents.contains_key(agent)
    }
}

impl Default for CapabilityLattice {
    fn default() -> Self {
        Self::with_defaults()
    }
}

// ─── Audit ───────────────────────────────────────────────────────────────────

/// One audit record emitted by a security check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    /// UTC timestamp of the check.
    pub ts: DateTime<Utc>,
    /// Agent that was checked.
    pub agent: String,
    /// Operation that was checked.
    pub op: FileOp,
    /// Path that was checked.
    pub path: String,
    /// `true` if the check allowed the operation, `false` if it denied.
    pub allowed: bool,
    /// Human-readable reason for the decision.
    pub reason: String,
}

/// Handle to the audit sink used by [`AnfsSecurity`].
///
/// v0: an in-memory ring buffer of the last 1024 entries. TODO(v1): delegate
/// to `hal::audit_chain::AuditChain` for hash-chained durable audit.
pub struct AuditHandle {
    /// In-memory ring buffer.
    pub buffer: VecDeque<AuditEntry>,
    /// Maximum entries retained.
    pub capacity: usize,
    /// Path to the on-disk audit log (for v1 persistence).
    pub path: PathBuf,
}

impl AuditHandle {
    /// Construct a new audit handle with the given on-disk path.
    pub fn new(path: &Path) -> Self {
        Self {
            buffer: VecDeque::with_capacity(1024),
            capacity: 1024,
            path: path.to_path_buf(),
        }
    }

    /// Append an audit entry to the ring buffer.
    pub fn emit(&mut self, entry: AuditEntry) {
        if self.buffer.len() >= self.capacity {
            self.buffer.pop_front();
        }
        // TODO(v1): also append to the on-disk audit chain (hash-chained).
        self.buffer.push_back(entry);
    }

    /// Iterate over the in-memory audit entries (oldest first).
    pub fn iter(&self) -> impl Iterator<Item = &AuditEntry> {
        self.buffer.iter()
    }

    /// Number of in-memory audit entries.
    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    /// Whether the in-memory audit buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }
}

// ─── AnfsSecurity ────────────────────────────────────────────────────────────

/// Per-agent access control for ANFS file operations.
///
/// Combines a [`CapabilityLattice`] (who-can-do-what) with an [`AuditHandle`]
/// (what-was-attempted) and a path-restriction policy (which paths are gated
/// to which agents).
pub struct AnfsSecurity {
    /// Capability lattice mapping agents → capabilities.
    pub lattice: CapabilityLattice,
    /// Audit sink for every check.
    pub audit: AuditHandle,
}

impl AnfsSecurity {
    /// Construct a new security gate with the given lattice name and audit path.
    pub fn new(lattice_name: &str, audit_path: &Path) -> Self {
        let lattice = match lattice_name {
            "user" => CapabilityLattice::with_defaults(),
            other => {
                // TODO(v1): load a named lattice from the config.
                debug!(lattice = other, "unknown lattice name, falling back to defaults");
                CapabilityLattice::with_defaults()
            }
        };
        Self {
            lattice,
            audit: AuditHandle::new(audit_path),
        }
    }

    /// Check whether `agent` may perform `op` on `path`.
    ///
    /// Every call emits an audit entry regardless of outcome. Restricted
    /// paths are gated to specific agents:
    ///
    ///   * `~/.cognos/`     → HAL + ANFS daemon only.
    ///   * `~/.ssh/`        → security agent only.
    ///   * `/etc/cognos/`   → HAL + security agent only.
    ///
    /// v0: the check is a flat path-prefix match; the capability lattice is
    /// consulted only for agent existence and path-restriction membership.
    /// TODO(v1): real lattice math and HAL-approval escalation for high-risk
    /// ops (e.g. bulk delete, write to `/etc/cognos/`).
    pub fn check(
        &mut self,
        agent: &str,
        path: &Path,
        op: FileOp,
    ) -> Result<(), SecurityError> {
        let path_str = path.display().to_string();
        let ts = Utc::now();

        // Unknown agent → reject + audit.
        if !self.lattice.knows(agent) {
            self.audit.emit(AuditEntry {
                ts,
                agent: agent.to_string(),
                op,
                path: path_str.clone(),
                allowed: false,
                reason: "unknown_agent".to_string(),
            });
            return Err(SecurityError::UnknownAgent(agent.to_string()));
        }

        // Path restriction check.
        if let Some(required) = self.restricted_path_requirements(&path_str) {
            if !self.lattice.holds_any(agent, &required) {
                let reason = format!(
                    "path_restricted:{}",
                    required
                        .iter()
                        .map(|c| c.0.as_str())
                        .collect::<Vec<_>>()
                        .join("|")
                );
                self.audit.emit(AuditEntry {
                    ts,
                    agent: agent.to_string(),
                    op,
                    path: path_str.clone(),
                    allowed: false,
                    reason,
                });
                return Err(SecurityError::PathRestricted {
                    path: path_str,
                    agent: agent.to_string(),
                });
            }
        }

        // TODO(v1): real capability-lattice check; for v0 any known agent
        // with a non-restricted path is allowed. High-risk mutating ops on
        // system paths should escalate via SecurityError::NeedsHalApproval.
        self.audit.emit(AuditEntry {
            ts,
            agent: agent.to_string(),
            op,
            path: path_str.clone(),
            allowed: true,
            reason: "default_allow".to_string(),
        });
        debug!(
            agent,
            op = ?op,
            path = %path_str,
            "security check allowed (v0 stub)"
        );
        Ok(())
    }

    /// Determine which capabilities are required to touch `path`, if any.
    ///
    /// Returns `None` for unrestricted paths. For restricted paths returns
    /// the set of capabilities, *any* of which satisfies the gate.
    fn restricted_path_requirements(&self, path: &str) -> Option<HashSet<Capability>> {
        // Expand `~/` to the user's home directory for matching.
        let home = std::env::var("HOME").unwrap_or_default();
        let cognos_dir = format!("{home}/.cognos");
        let ssh_dir = format!("{home}/.ssh");

        let mut allowed = HashSet::new();
        if path.starts_with(&cognos_dir) || path == &cognos_dir {
            // ~/.cognos/ → HAL + ANFS daemon.
            allowed.insert(Capability::hal());
            allowed.insert(Capability::anfs());
            return Some(allowed);
        }
        if path.starts_with("/etc/cognos/") || path == "/etc/cognos" {
            // /etc/cognos/ → HAL + security agent.
            allowed.insert(Capability::hal());
            allowed.insert(Capability::security());
            return Some(allowed);
        }
        if path.starts_with(&ssh_dir) || path == &ssh_dir {
            // ~/.ssh/ → security agent only.
            allowed.insert(Capability::security());
            return Some(allowed);
        }
        None
    }

    /// Iterate over the in-memory audit log.
    pub fn audit_entries(&self) -> impl Iterator<Item = &AuditEntry> {
        self.audit.iter()
    }
}

impl Default for AnfsSecurity {
    fn default() -> Self {
        Self::new("user", Path::new(".cognos/anfs/audit.log"))
    }
}

// v0: stub implementation
