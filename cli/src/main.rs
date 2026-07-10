//! COGNOS CLI — the primary user interface. Dispatches subcommands to the
//! appropriate service: intent parsing, HAL approval, memory browsing,
//! agent status, and the interactive TUI.
//!
//! The `cognos` binary is a thin clap front-end. Argument parsing happens
//! here; all real work lives in [`commands`] (gRPC dispatch), [`runtime`]
//! (connection management), and [`tui`] (the interactive dashboard).
//!
//! v0: stub implementation.

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

mod approval_watch;
mod commands;
mod runtime;
mod tui;

// ─── CLI definition ─────────────────────────────────────────────────────────

/// Top-level `cognos` command.
#[derive(Debug, Parser)]
#[command(
    name = "cognos",
    version,
    about = "COGNOS/OS command-line interface",
    long_about = "COGNOS/OS command-line interface — dispatches to the \
                  intent engine, HAL, memory fabric, agent mesh, and the \
                  interactive TUI."
)]
pub struct Cli {
    /// Increase log verbosity (-v info, -vv debug, -vvv trace).
    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Path to an alternative config file (default: ~/.cognos/cli.toml).
    #[arg(long, value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// Subcommand to dispatch.
    #[command(subcommand)]
    pub command: Command,
}

/// All `cognos` subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Parse a natural-language intent and show the resulting task graph.
    Intent(commands::IntentArgs),
    /// List pending HAL approvals and optionally approve / deny by id.
    Approval(commands::ApprovalArgs),
    /// Search, list, edit, or forget memories.
    Memory(commands::MemoryArgs),
    /// Show agent statuses, system metrics, and the current scenario.
    Status(commands::StatusArgs),
    /// Launch the interactive terminal UI.
    Tui(commands::TuiArgs),
    /// Print version and build info.
    Version,
}

// ─── Entrypoint ─────────────────────────────────────────────────────────────

/// CLI entrypoint. Parses arguments, sets up logging, and dispatches to the
/// appropriate subcommand handler in [`commands`].
#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    init_logging(cli.verbose);

    info!(verbose = cli.verbose, "cognos CLI starting");
    // TODO(v1): load ~/.cognos/cli.toml (or cli.config) and pass a typed
    //           CliConfig down into the runtime / commands layer.
    let _ = cli.config.as_ref();

    let result = match cli.command {
        Command::Intent(args) => commands::cmd_intent(args).await,
        Command::Approval(args) => commands::cmd_approval(args).await,
        Command::Memory(args) => commands::cmd_memory(args).await,
        Command::Status(args) => commands::cmd_status(args).await,
        Command::Tui(args) => commands::cmd_tui(args).await,
        Command::Version => commands::cmd_version().await,
    };

    if let Err(ref e) = result {
        warn!(error = %e, "subcommand failed");
    }

    // Owner: iCrewZero — use Into::into to preserve the structured CliError
    // type instead of converting to an untyped string.
    result.map_err(Into::into)
}

// ─── Logging setup ──────────────────────────────────────────────────────────

/// Initialise the `tracing_subscriber` based on the `-v` count.
///
/// - `0`  → warn (with the `cognos` target at info)
/// - `1`  → info
/// - `2`  → debug
/// - `3+` → trace
fn init_logging(verbose: u8) {
    let filter = match verbose {
        0 => EnvFilter::new("warn,cognos=info"),
        1 => EnvFilter::new("info"),
        2 => EnvFilter::new("debug"),
        _ => EnvFilter::new("trace"),
    };
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}

// v0: stub implementation
