//! Subcommand implementations — each function talks to the relevant COGNOS
//! service over gRPC and prints results to stdout.
//!
//! Every handler accepts the clap-derived argument struct for its
//! subcommand, dials the CLI runtime ([`crate::runtime::CliRuntime`]) to
//! obtain a shared [`CognosClient`], performs one or more RPCs, and prints
//! the result to stdout in human-readable form (or JSON where requested).
//!
//! v0: stub implementation.
#![allow(dead_code)]

use std::time::Instant;

use chrono::{DateTime, Utc};
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, info, warn};
use cognos_ipc_grpc::client::{ClientConfig, CognosClient};
use cognos_ipc_grpc::pipeline_metrics::log_stage;
use cognos_ipc_grpc::proto::v1::{Intent, PipelineMetrics, PipelineMetricsRequest};
use uuid::Uuid;

use crate::approval_watch;
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
            RuntimeError::ConnectFailed(msg) => CliError::ConnectionFailed(msg),
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

/// Arguments for `cognos approval` (deferred list/approve/deny).
#[derive(Debug, Clone, clap::Args)]
pub struct ApprovalArgs {
    #[command(subcommand)]
    pub command: Option<ApprovalCommand>,

    /// List all pending HAL approvals (deferred queue).
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

/// Real-time or deferred approval subcommands.
#[derive(Debug, Clone, Subcommand)]
pub enum ApprovalCommand {
    /// Listen on the HAL UI socket and decide interactively (blocking).
    Watch(approval_watch::WatchArgs),
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

/// `cognos intent` — submit intent text to the orchestrator ingress and
/// print per-task status plus HAL decisions.
pub async fn cmd_intent(args: IntentArgs) -> Result<(), CliError> {
    info!(dry_run = args.dry_run, priority = ?args.priority, "submitting intent");
    debug!(text = %args.text, "intent text");

    if args.dry_run {
        println!("dry-run: would submit intent to orchestrator");
        println!("  text:     {}", args.text);
        println!("  priority: {:?}", args.priority);
        return Ok(());
    }

    let endpoint = default_endpoint();
    let rt = CliRuntime::connect(endpoint).await?;
    let trace_id = Uuid::new_v4().to_string();
    let intent = Intent {
        intent_id: Uuid::new_v4().to_string(),
        utterance: args.text.clone(),
        session_id: "cli".to_string(),
        trace_id: trace_id.clone(),
        ..Default::default()
    };

    let dispatch_started = Instant::now();
    tracing::info!(
        trace_id = %trace_id,
        stage = "cli_dispatch_start",
        utterance = %args.text,
        "pipeline stage"
    );

    let resp = rt
        .client
        .dispatch_intent(intent)
        .await
        .map_err(|e| CliError::ServiceError(e.to_string()))?;

    let cli_ms = dispatch_started.elapsed().as_millis() as u64;
    log_stage(&trace_id, "cli_dispatch", cli_ms);
    tracing::info!(
        trace_id = %trace_id,
        stage = "cli_dispatch",
        latency_ms = cli_ms,
        status = %resp.status,
        "pipeline stage"
    );

    println!("trace_id:   {}", resp.trace_id);
    println!("intent_id:  {}", resp.intent_id);
    println!("status:     {}", resp.status);
    println!("message:    {}", resp.message);

    if !resp.result_json.is_empty() {
        if let Ok(report) = serde_json::from_slice::<serde_json::Value>(&resp.result_json) {
            if let Some(latency) = report.get("latency") {
                let total = latency.get("total_ms").and_then(|v| v.as_u64()).unwrap_or(0);
                let parse = latency.get("parse_ms").and_then(|v| v.as_u64()).unwrap_or(0);
                let orch = latency
                    .get("orchestrate_ms")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let exec = latency.get("execute_ms").and_then(|v| v.as_u64()).unwrap_or(0);
                println!(
                    "latency:    total={total}ms parse={parse}ms orchestrate={orch}ms execute={exec}ms"
                );
            }
            if let Some(tasks) = report.get("tasks").and_then(|t| t.as_array()) {
                println!("\nTasks:");
                for t in tasks {
                    let action = t.get("action").and_then(|v| v.as_str()).unwrap_or("-");
                    let target = t.get("target").and_then(|v| v.as_str()).unwrap_or("-");
                    let status = t.get("status").and_then(|v| v.as_str()).unwrap_or("-");
                    let hal = t
                        .get("hal_decision")
                        .and_then(|v| v.as_str())
                        .unwrap_or("-");
                    let msg = t
                        .get("message")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    println!("  [{status}] {action} → {target}");
                    println!("           HAL: {hal}");
                    if !msg.is_empty() {
                        println!("           {msg}");
                    }
                }
            }
            let success = report
                .get("success")
                .and_then(|v| v.as_bool())
                .unwrap_or(resp.status == "ok");
            if !success {
                return Err(CliError::ServiceError(report
                    .get("summary")
                    .and_then(|v| v.as_str())
                    .unwrap_or("intent execution failed")
                    .to_string()));
            }
        }
    } else if resp.status != "ok" {
        return Err(CliError::ServiceError(resp.message));
    }

    Ok(())
}

/// `cognos approval` — list pending HAL approvals, optionally approve or
/// deny by id.
pub async fn cmd_approval(args: ApprovalArgs) -> Result<(), CliError> {
    if let Some(ApprovalCommand::Watch(watch_args)) = args.command {
        return approval_watch::cmd_approval_watch(watch_args).await;
    }

    // Validate flag combinations.
    if args.approve.is_some() && args.deny.is_some() {
        return Err(CliError::InvalidArgs(
            "--approve and --deny are mutually exclusive".into(),
        ));
    }
    let args = if !args.list && args.approve.is_none() && args.deny.is_none() {
        ApprovalArgs { list: true, ..args }
    } else {
        args
    };

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
    let _rt = CliRuntime::connect(default_endpoint()).await?;

    if args.watch {
        info!(agent = ?args.agent, "watching status (Ctrl-C to stop)");
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

    println!();
    println!("── Pipeline metrics ──");

    let hal = fetch_pipeline_metrics(&hal_endpoint()).await;
    println!("HAL (cognos-hal):");
    print_hal_metrics(&hal);

    let intent = fetch_pipeline_metrics(&intent_endpoint()).await;
    println!("Intent parser (cognos-intent):");
    print_parser_metrics(&intent);

    let orch = fetch_pipeline_metrics(&default_endpoint()).await;
    println!("Latency (cognos-orchestrator):");
    print_latency_metrics(&orch);

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
    std::env::var("COGNOS_ORCHESTRATOR_ENDPOINT")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| crate::runtime::DEFAULT_ORCHESTRATOR_ENDPOINT.to_string())
}

fn hal_endpoint() -> String {
    std::env::var("COGNOS_HAL_ENDPOINT")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "http://127.0.0.1:7444".to_string())
}

fn intent_endpoint() -> String {
    std::env::var("COGNOS_INTENT_ENDPOINT")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "http://127.0.0.1:7445".to_string())
}

async fn fetch_pipeline_metrics(endpoint: &str) -> Option<PipelineMetrics> {
    let mut client = CognosClient::new(ClientConfig {
        agent_id: "cli.status".to_string(),
        endpoint: endpoint.to_string(),
        max_reconnect_attempts: 2,
        request_timeout_ms: 3_000,
        ..ClientConfig::default()
    });
    if client.connect(endpoint).await.is_err() {
        return None;
    }
    client
        .get_pipeline_metrics(PipelineMetricsRequest::default())
        .await
        .ok()
}

fn print_hal_metrics(m: &Option<PipelineMetrics>) {
    match m {
        Some(s) => {
            println!("  granted:            {}", s.hal_granted);
            println!("  denied:             {}", s.hal_denied);
            println!("  approval_required:  {}", s.hal_approval_required);
        }
        None => println!("  (unreachable)"),
    }
}

fn print_parser_metrics(m: &Option<PipelineMetrics>) {
    match m {
        Some(s) => {
            println!("  cache hits:         {}", s.parser_cache_hits);
            println!("  cache misses:       {}", s.parser_cache_misses);
            println!("  fallback uses:      {}", s.parser_fallback_uses);
            println!("  intent requests:    {}", s.intent_requests);
        }
        None => println!("  (unreachable)"),
    }
}

fn print_latency_metrics(m: &Option<PipelineMetrics>) {
    match m {
        Some(s) => {
            if s.last_trace_id.is_empty() {
                println!("  (no samples yet)");
            } else {
                println!("  last trace_id:      {}", s.last_trace_id);
                println!(
                    "  last total:         {:.0}ms (parse {:.0} / orchestrate {:.0} / execute {:.0})",
                    s.last_total_latency_ms,
                    s.last_parse_latency_ms,
                    s.last_orchestrate_latency_ms,
                    s.last_execute_latency_ms,
                );
            }
        }
        None => println!("  (unreachable)"),
    }
}

// v0: stub implementation
