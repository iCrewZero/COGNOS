//! Subcommand implementations — each function talks to the relevant COGNOS
//! service over gRPC and prints results to stdout.
//!
//! Every handler accepts the clap-derived argument struct for its
//! subcommand, dials the CLI runtime ([`crate::runtime::CliRuntime`]) to
//! obtain a shared [`CognosClient`], performs one or more RPCs, and prints
//! the result to stdout in human-readable form (or JSON where requested).
//!
//! v0: stub implementation.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::runtime::{CliRuntime, RuntimeError};

// ─── Errors ─────────────────────────────────────────────────────────────────

/// Errors returned by any of the `cmd_*` handlers.
#[derive(Debug, Error)]
pub enum CliError {
    /// The CLI could not reach the requested COGNOS service.
    #[error("connection failed: {0}")]
    ConnectionFailed(String),
    /// The service accepted the request but returned an error.
    #[error("service error: {0}")]
    ServiceError(String),
    /// The requested entity (memory id, approval id, agent name) was not found.
    #[error("not found")]
    NotFound,
    /// The user-supplied arguments were invalid or contradictory.
    #[error("invalid args: {0}")]
    InvalidArgs(String),
    /// The operation was cancelled (Ctrl-C, deadline, or user abort).
    #[error("cancelled")]
    Cancelled,
}

impl From<RuntimeError> for CliError {
    fn from(e: RuntimeError) -> Self {
        match e {
            RuntimeError::ConnectFailed => {
                CliError::ConnectionFailed("connect failed".into())
            }
            RuntimeError::Timeout => CliError::Cancelled,
            RuntimeError::Cancelled => CliError::Cancelled,
            RuntimeError::Disconnected => {
                CliError::ConnectionFailed("disconnected".into())
            }
        }
    }
}

// ─── Argument structs ───────────────────────────────────────────────────────

/// Arguments for `cognos intent`.
#[derive(Debug, Clone, Serialize, Deserialize, clap::Args)]
pub struct IntentArgs {
    /// Natural-language intent text (quoted).
    #[arg(value_name = "TEXT")]
    pub text: String,

    /// Dry-run: decompose but do not enqueue the resulting task graph.
    #[arg(long)]
    pub dry_run: bool,

    /// Priority hint for the scheduler.
    #[arg(long, value_enum, default_value = "normal")]
    pub priority: Priority,
}

/// Arguments for `cognos approval`.
#[derive(Debug, Clone, Serialize, Deserialize, clap::Args)]
pub struct ApprovalArgs {
    /// List all pending HAL approvals.
    #[arg(long)]
    pub list: bool,

    /// Approve the approval with the given id.
    #[arg(long, value_name = "ID")]
    pub approve: Option<u64>,

    /// Deny the approval with the given id.
    #[arg(long, value_name = "ID")]
    pub deny: Option<u64>,

    /// Emit machine-readable JSON instead of a human-readable table.
    #[arg(long)]
    pub json: bool,
}

/// Arguments for `cognos memory`.
#[derive(Debug, Clone, Serialize, Deserialize, clap::Args)]
pub struct MemoryArgs {
    /// Full-text / semantic search query.
    #[arg(long, value_name = "QUERY")]
    pub search: Option<String>,

    /// List recent memories (paginated).
    #[arg(long)]
    pub list: bool,

    /// Forget (permanently delete) the memory with the given id.
    #[arg(long, value_name = "UUID")]
    pub forget: Option<Uuid>,

    /// Open the memory with the given id in $EDITOR for editing.
    #[arg(long, value_name = "UUID")]
    pub edit: Option<Uuid>,
}

/// Arguments for `cognos status`.
#[derive(Debug, Clone, Serialize, Deserialize, clap::Args)]
pub struct StatusArgs {
    /// Restrict the output to a single agent id.
    #[arg(long, value_name = "AGENT")]
    pub agent: Option<String>,

    /// Refresh the status every second until Ctrl-C.
    #[arg(long)]
    pub watch: bool,
}

/// Arguments for `cognos tui` (currently none).
#[derive(Debug, Clone, Default, Serialize, Deserialize, clap::Args)]
pub struct TuiArgs {}

// ─── Priority enum ──────────────────────────────────────────────────────────

/// Priority hint passed through the intent engine into the scheduler.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    /// Background / best-effort.
    Low,
    /// Default priority.
    Normal,
    /// Preempt Low and Normal work.
    High,
    /// Preempt everything except user-facing work.
    Critical,
}

impl Default for Priority {
    fn default() -> Self {
        Priority::Normal
    }
}

// ─── Placeholder domain types ───────────────────────────────────────────────

/// Decomposed task graph returned by the intent engine.
///
/// This is a CLI-local type used for display purposes. The canonical
/// task graph lives in `orchestrator::task_graph`. We keep a separate
/// display type here because the CLI may run without the orchestrator
/// crate in scope (e.g., when connecting to a remote IPC server).
///
/// TODO(v1): replace with a From<orchestrator::task_graph::TaskGraph> impl
/// that converts the canonical type into this display-friendly form.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CliTaskGraph {
    /// Root intent id (UUID-shaped string in v1).
    pub intent_id: String,
    /// Ordered task nodes.
    pub nodes: Vec<CliTaskNode>,
}

/// Single node of a [`CliTaskGraph`].
///
/// Renamed from TaskNode to avoid shadowing orchestrator::task_graph::TaskNode.
/// The "Cli" prefix makes it clear this is a display-only type.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CliTaskNode {
    /// Node id.
    pub id: String,
    /// Human-readable description.
    pub description: String,
    /// Agent id this node is assigned to.
    pub agent: String,
    /// Capabilities required to execute this node.
    pub capabilities: Vec<String>,
}

/// A pending HAL approval surfaced to the user.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HalApproval {
    /// Monotonic approval id.
    pub id: u64,
    /// Agent that requested the gated action.
    pub agent: String,
    /// Capability being requested.
    pub capability: String,
    /// Risk score (0.0–1.0).
    pub risk: f32,
    /// When the approval was requested.
    pub requested_at: DateTime<Utc>,
}

/// A memory entry returned by the memory fabric.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryEntry {
    /// Stable memory id.
    pub id: Uuid,
    /// Source path or uri.
    pub source: String,
    /// Snippet of the indexed content.
    pub snippet: String,
    /// Last-modified timestamp.
    pub updated_at: DateTime<Utc>,
}

/// Snapshot of one agent's runtime status.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentStatus {
    /// Agent id, e.g. "agent.coordinator".
    pub id: String,
    /// Lifecycle state: idle / running / blocked / crashed.
    pub state: String,
    /// Currently-executing task id, if any.
    pub current_task: Option<String>,
    /// Last heartbeat timestamp.
    pub last_heartbeat: Option<DateTime<Utc>>,
}

// ─── Handlers ───────────────────────────────────────────────────────────────

/// `cognos intent` — submit intent text to the cognos-intent service and
/// print the decomposed task graph.
pub async fn cmd_intent(args: IntentArgs) -> Result<(), CliError> {
    info!(dry_run = args.dry_run, priority = ?args.priority, "submitting intent");
    debug!(text = %args.text, "intent text");

    let rt = CliRuntime::connect(default_endpoint()).await?;
    // TODO(v1): rt.client().dispatch_intent(IntentRequest { ... }) and
    //           receive a streamed TaskGraph.
    let _ = rt;

    let graph = CliTaskGraph {
        intent_id: "00000000-0000-0000-0000-000000000000".to_string(),
        nodes: Vec::new(),
    };

    println!("intent:     {}", args.text);
    println!("priority:   {:?}", args.priority);
    println!("dry_run:    {}", args.dry_run);
    println!("intent_id:  {}", graph.intent_id);
    println!("nodes:      {}", graph.nodes.len());
    for node in &graph.nodes {
        println!(
            "  - [{}] {} (agent={}, caps={:?})",
            node.id, node.description, node.agent, node.capabilities
        );
    }
    Ok(())
}

/// `cognos approval` — list pending HAL approvals, optionally approve or
/// deny by id.
pub async fn cmd_approval(args: ApprovalArgs) -> Result<(), CliError> {
    // Validate flag combinations.
    if args.approve.is_some() && args.deny.is_some() {
        return Err(CliError::InvalidArgs(
            "--approve and --deny are mutually exclusive".into(),
        ));
    }
    if !args.list && args.approve.is_none() && args.deny.is_none() {
        // Default behaviour when no flag is given: list.
        return cmd_approval(ApprovalArgs {
            list: true,
            ..args
        })
        .await;
    }

    let rt = CliRuntime::connect(default_endpoint()).await?;
    // TODO(v1): rt.client().list_pending_approvals() /
    //           rt.client().resolve_approval(id, Approve|Deny).
    let _ = rt;

    if let Some(id) = args.approve {
        info!(id, "approving HAL gate");
        println!("approved: {}", id);
        return Ok(());
    }
    if let Some(id) = args.deny {
        info!(id, "denying HAL gate");
        println!("denied:   {}", id);
        return Ok(());
    }

    // --list
    let pending: Vec<HalApproval> = Vec::new();
    if args.json {
        println!("{}", serde_json::to_string_pretty(&pending).unwrap_or_default());
    } else {
        println!(
            "{:<8} {:<20} {:<24} {:>6}  {}",
            "ID", "AGENT", "CAPABILITY", "RISK", "REQUESTED"
        );
        println!("{}", "-".repeat(80));
        for a in &pending {
            println!(
                "{:<8} {:<20} {:<24} {:>6.2}  {}",
                a.id, a.agent, a.capability, a.risk, a.requested_at
            );
        }
        if pending.is_empty() {
            println!("(no pending approvals)");
        }
    }
    Ok(())
}

/// `cognos memory` — search, list, edit, or forget memories.
pub async fn cmd_memory(args: MemoryArgs) -> Result<(), CliError> {
    let n_flags = [
        args.search.is_some(),
        args.list,
        args.forget.is_some(),
        args.edit.is_some(),
    ]
    .iter()
    .filter(|&&b| b)
    .count();
    if n_flags != 1 {
        return Err(CliError::InvalidArgs(
            "exactly one of --search / --list / --forget / --edit is required".into(),
        ));
    }

    let rt = CliRuntime::connect(default_endpoint()).await?;
    // TODO(v1): rt.client().query_memory(MemoryQuery { ... }) /
    //           rt.client().forget_memory(id) / rt.client().edit_memory(id).
    let _ = rt;

    if let Some(query) = args.search {
        info!(query = %query, "memory search");
        let results: Vec<MemoryEntry> = Vec::new();
        println!("search:    {}", query);
        println!("results:   {}", results.len());
        for m in &results {
            println!(
                "  - {}  {}  ({} bytes)",
                m.id,
                m.source,
                m.snippet.len()
            );
        }
        return Ok(());
    }

    if args.list {
        info!("listing recent memories");
        let entries: Vec<MemoryEntry> = Vec::new();
        println!("{:<36}  {:<30}  {}", "UUID", "SOURCE", "UPDATED");
        println!("{}", "-".repeat(86));
        for m in &entries {
            println!("{:<36}  {:<30}  {}", m.id, m.source, m.updated_at);
        }
        if entries.is_empty() {
            println!("(memory fabric is empty — files are indexed at idle time)");
        }
        return Ok(());
    }

    if let Some(id) = args.forget {
        info!(%id, "forgetting memory");
        println!("forgotten: {}", id);
        return Ok(());
    }

    if let Some(id) = args.edit {
        info!(%id, "opening memory in $EDITOR");
        // TODO(v1): spawn $EDITOR on a temp file containing the memory
        //           payload, then PUT the edited body back to the memory
        //           service.
        warn!(%id, "edit not implemented in v0 — would open $EDITOR");
        println!("edit:      {} (not implemented in v0)", id);
        return Ok(());
    }

    // Should not reach here — all flag combinations are handled above.
    // Return a proper error instead of panicking. Owner: iCrewZero
    Err(CliError::InvalidArgs("no action selected (internal error)".into()))
}

/// `cognos status` — show agent statuses, system metrics, and the current
/// scenario.
pub async fn cmd_status(args: StatusArgs) -> Result<(), CliError> {
    let rt = CliRuntime::connect(default_endpoint()).await?;
    // TODO(v1): rt.client().list_agents() + rt.client().system_metrics() +
    //           rt.client().current_scenario().
    let _ = rt;

    if args.watch {
        info!(agent = ?args.agent, "watching status (Ctrl-C to stop)");
        // TODO(v1): loop with tokio::time::interval(Duration::from_secs(1))
        //           until the runtime's cancel flag is set.
        warn!("watch mode is a v0 stub — printing a single snapshot");
    }

    let agents: Vec<AgentStatus> = match args.agent.as_deref() {
        Some(name) => vec![AgentStatus {
            id: name.to_string(),
            ..Default::default()
        }],
        None => Vec::new(),
    };

    println!("{:<24} {:<10} {:<24} {}", "AGENT", "STATE", "CURRENT TASK", "LAST HEARTBEAT");
    println!("{}", "-".repeat(80));
    for a in &agents {
        println!(
            "{:<24} {:<10} {:<24} {}",
            a.id,
            a.state,
            a.current_task.as_deref().unwrap_or("-"),
            a.last_heartbeat
                .map(|t| t.to_rfc3339())
                .unwrap_or_else(|| "-".into()),
        );
    }
    if agents.is_empty() {
        println!("(no agents registered — daemons not running?)");
    }
    Ok(())
}

/// `cognos tui` — launch the interactive TUI.
pub async fn cmd_tui(_args: TuiArgs) -> Result<(), CliError> {
    info!("launching TUI");
    let mut tui = crate::tui::Tui::new().map_err(|e| {
        warn!(error = %e, "TUI init failed");
        CliError::ServiceError(e.to_string())
    })?;
    tui.run().await.map_err(|e| {
        warn!(error = %e, "TUI loop exited with error");
        CliError::ServiceError(e.to_string())
    })
}

/// `cognos version` — print version and build info.
pub async fn cmd_version() -> Result<(), CliError> {
    // TODO(v1): pull these from built structs (env! + built crate) rather
    //           than hard-coding them.
    println!("COGNOS/OS cli v0.0.0");
    println!("  build:     stub");
    println!("  commit:    unknown");
    // This is the crate version, not the rustc compiler version.
    // Owner: iCrewZero
    println!("  version:   {}", env!("CARGO_PKG_VERSION"));
    println!("  endpoint:  {}", default_endpoint());
    Ok(())
}

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Default gRPC endpoint the CLI dials. Overridable via config in v1.
fn default_endpoint() -> String {
    // TODO(v1): read from CliConfig (cli.toml or --config) and fall back
    //           to the COGNOS_CLI_ENDPOINT env var.
    crate::runtime::DEFAULT_ENDPOINT.to_string()
}

// v0: stub implementation
