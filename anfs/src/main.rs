//! ANFS — AI-Native File System entrypoint. Mounts a FUSE overlay that adds
//! semantic tagging, journaling, and per-agent access control on top of a
//! backing directory.
//!
//! `cognos-anfs` is a long-running FUSE daemon. On startup it:
//!   1. Parses CLI args (mountpoint, backing directory, config path).
//!   2. Initialises structured logging via `tracing_subscriber`.
//!   3. Loads the ANFS configuration file (TOML or JSON).
//!   4. Constructs the [`overlay::AnfsOverlay`] filesystem from the journal,
//!      cache, and security subsystems.
//!   5. Installs SIGINT/SIGTERM handlers for a clean unmount.
//!   6. Enters the FUSE session loop (blocking).
//!
//! v0: stub implementation — the FUSE session loop is not actually started;
//! the daemon only parses args, sets up logging, constructs the overlay, and
//! exits after a signal is received.

mod cache;
mod journal;
mod overlay;
mod security;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

// ─── Errors ─────────────────────────────────────────────────────────────────

/// Top-level error type for the ANFS daemon entrypoint.
#[derive(Debug, Error)]
pub enum AnfsError {
    /// CLI invocation was invalid (missing or unknown arguments).
    #[error("invalid arguments: {0}")]
    InvalidArgs(String),
    /// The mountpoint directory does not exist or is not a directory.
    #[error("mountpoint invalid: {0}")]
    InvalidMountpoint(PathBuf),
    /// The backing directory could not be opened or read.
    #[error("backing directory invalid: {0}")]
    InvalidBacking(PathBuf),
    /// The configuration file could not be loaded or parsed.
    #[error("config error: {0}")]
    Config(String),
    /// The FUSE session could not be started or was interrupted.
    #[error("fuse session error: {0}")]
    Fuse(String),
}

// ─── CLI argument parsing ────────────────────────────────────────────────────

/// Parsed command-line arguments for `cognos-anfs`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliArgs {
    /// Directory where the FUSE overlay will be mounted.
    pub mountpoint: PathBuf,
    /// Real directory that ANFS overlays (typically the user's home).
    pub backing: PathBuf,
    /// Path to the ANFS daemon config file.
    pub config: PathBuf,
    /// Run in foreground (do not daemonize).
    pub foreground: bool,
    /// Mount options forwarded to libfuse (`-o …`).
    pub mount_options: Vec<String>,
}

impl CliArgs {
    /// Parse `std::env::args` into a [`CliArgs`].
    ///
    /// v0: hand-rolled parser. TODO(v1): replace with `clap`-derive for
    /// proper `--help` text, env-var fallback, and shell completion.
    pub fn parse() -> Result<Self> {
        let mut args = std::env::args().skip(1);
        let mut mountpoint = None;
        let mut backing = None;
        let mut config = None;
        let mut foreground = false;
        let mut mount_options = Vec::new();

        while let Some(a) = args.next() {
            match a.as_str() {
                "--mountpoint" => mountpoint = args.next().map(PathBuf::from),
                "--backing" => backing = args.next().map(PathBuf::from),
                "--config" => config = args.next().map(PathBuf::from),
                "--foreground" | "-f" => foreground = true,
                "-o" => {
                    if let Some(opt) = args.next() {
                        mount_options.push(opt);
                    }
                }
                "--help" | "-h" => {
                    print_usage();
                    std::process::exit(0);
                }
                other => {
                    return Err(anyhow!(AnfsError::InvalidArgs(format!(
                        "unknown argument: {other}"
                    ))));
                }
            }
        }

        let mountpoint = mountpoint.ok_or_else(|| {
            anyhow!(AnfsError::InvalidArgs("missing --mountpoint".into()))
        })?;
        let backing = backing.ok_or_else(|| {
            anyhow!(AnfsError::InvalidArgs("missing --backing".into()))
        })?;
        let config = config.unwrap_or_else(|| PathBuf::from("/etc/cognos/anfs.toml"));

        Ok(Self {
            mountpoint,
            backing,
            config,
            foreground,
            mount_options,
        })
    }
}

/// Print usage to stderr and return.
fn print_usage() {
    eprintln!(
        "cognos-anfs — AI-Native File System FUSE daemon\n\n\
         USAGE:\n    \
         cognos-anfs --mountpoint <dir> --backing <dir> [--config <path>] \
         [--foreground] [-o <opts>]\n\n\
         OPTIONS:\n    \
         --mountpoint <dir>   Where to mount the FUSE overlay\n    \
         --backing <dir>      Real directory ANFS sits on top of\n    \
         --config <path>      Path to the ANFS config file (default /etc/cognos/anfs.toml)\n    \
         --foreground, -f     Do not daemonize\n    \
         -o <opts>            Comma-separated FUSE mount options\n"
    );
}

// ─── Configuration ───────────────────────────────────────────────────────────

/// Daemon-wide configuration loaded from `--config`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnfsConfig {
    /// Path to the journal file (relative to the backing dir if not absolute).
    pub journal_path: PathBuf,
    /// Maximum size of the semantic data cache in bytes.
    pub cache_max_bytes: u64,
    /// Maximum number of entries in the metadata cache.
    pub cache_metadata_capacity: usize,
    /// Default capability lattice for the security subsystem.
    pub default_lattice: String,
    /// Audit log path (relative to the backing dir if not absolute).
    pub audit_log: PathBuf,
}

impl Default for AnfsConfig {
    fn default() -> Self {
        Self {
            journal_path: PathBuf::from(".cognos/anfs/journal.log"),
            cache_max_bytes: 64 * 1024 * 1024,
            cache_metadata_capacity: 4096,
            default_lattice: "user".to_string(),
            audit_log: PathBuf::from(".cognos/anfs/audit.log"),
        }
    }
}

impl AnfsConfig {
    /// Load config from a file. Format is detected by extension:
    /// `.toml` → TOML, `.json` (or anything else) → JSON.
    ///
    /// v0: TOML parsing is not implemented; only JSON is supported.
    /// TODO(v1): pull in the `toml` crate and deserialize uniformly.
    pub fn load(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("reading config {}", path.display()))?;
        if path.extension().map(|e| e == "toml").unwrap_or(false) {
            // TODO(v1): toml::from_str(&raw)
            Err(anyhow!(AnfsError::Config(
                "TOML parsing not implemented in v0".into()
            )))
        } else {
            serde_json::from_str(&raw)
                .map_err(|e| anyhow!(AnfsError::Config(format!("json: {e}"))))
        }
    }
}

// ─── Daemon ──────────────────────────────────────────────────────────────────

/// Top-level ANFS daemon handle.
///
/// Owns the parsed CLI args and loaded configuration; constructs the overlay
/// and drives the FUSE session loop.
pub struct AnfsDaemon {
    /// Parsed CLI arguments.
    pub args: CliArgs,
    /// Loaded daemon configuration.
    pub config: AnfsConfig,
    /// Shutdown flag flipped by the signal handler.
    pub shutdown: Arc<AtomicBool>,
}

impl AnfsDaemon {
    /// Construct from parsed CLI args + loaded config.
    pub fn new(args: CliArgs, config: AnfsConfig) -> Self {
        Self {
            args,
            config,
            shutdown: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Validate that the mountpoint and backing directory are usable.
    pub fn validate_paths(&self) -> Result<()> {
        if !self.args.mountpoint.is_dir() {
            return Err(anyhow!(AnfsError::InvalidMountpoint(
                self.args.mountpoint.clone()
            )));
        }
        if !self.args.backing.is_dir() {
            return Err(anyhow!(AnfsError::InvalidBacking(
                self.args.backing.clone()
            )));
        }
        Ok(())
    }

    /// Install SIGINT/SIGTERM handlers that flip the shutdown flag.
    ///
    /// v0: installs a placeholder handler. TODO(v1): use `signal-hook` or
    /// `tokio::signal` to catch SIGTERM/SIGINT and trigger a clean
    /// `fuser::unmount` of the mountpoint.
    pub fn install_signal_handlers(&self) -> Result<()> {
        let shutdown = Arc::clone(&self.shutdown);
        install_signal_flag(shutdown);
        Ok(())
    }

    /// Enter the FUSE session loop. Blocks until the filesystem is unmounted
    /// or a shutdown signal is received.
    ///
    /// v0: does NOT actually mount FUSE — only logs intent, constructs the
    /// overlay, and waits for a signal. Real session setup is scheduled
    /// for v1 via `fuser::spawn_mount` / `fuser::MountOption`.
    pub fn run(self) -> Result<()> {
        self.validate_paths()?;
        self.install_signal_handlers()?;
        let shutdown = Arc::clone(&self.shutdown);

        info!(
            mountpoint = %self.args.mountpoint.display(),
            backing = %self.args.backing.display(),
            config = %self.args.config.display(),
            "cognos-anfs starting (v0 stub)"
        );

        // Construct the overlay filesystem (journal + cache + security wired in).
        let overlay = overlay::AnfsOverlay::new(
            self.args.backing.clone(),
            self.config.clone(),
        );
        info!(
            backing = ?overlay.backing,
            journal_seq = overlay.journal.next_seq(),
            cache_bytes = overlay.cache.bytes_used(),
            "overlay constructed (v0 stub — not mounted)"
        );

        // TODO(v1): mount via fuser::spawn_mount(&overlay, &self.args.mountpoint, &mount_options)
        //           and block on the session until unmount. Until then we
        //           just poll the shutdown flag.
        while !shutdown.load(Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(200));
        }

        info!("cognos-anfs unmounting (v0 stub)");
        // TODO(v1): call fuser to unmount the mountpoint cleanly, then
        // journal.checkpoint() + cache.flush_dirty() before exit.
        Ok(())
    }
}

/// Install a Ctrl-C / SIGTERM handler that flips `shutdown`.
///
/// v0: spawns a no-op thread; real signal handling is TODO(v1).
fn install_signal_flag(shutdown: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        // TODO(v1):
        //   signal_hook::flag::register(signal_hook::consts::SIGTERM,
        //                              Arc::clone(&shutdown))
        //       .expect("register SIGTERM");
        //   signal_hook::flag::register(signal_hook::consts::SIGINT,
        //                              Arc::clone(&shutdown))
        //       .expect("register SIGINT");
        // For v0 we leave the flag untouched and rely on the FUSE session
        // being interruptible by Ctrl-C once the real mount is wired in.
        let _ = &shutdown;
    });
    warn!("signal handler installed as no-op (v0 stub)");
}

// ─── Entry point ─────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    // Initialise structured logging. Honors `RUST_LOG`.
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_target(true)
        .init();

    let args = CliArgs::parse()?;
    let config = AnfsConfig::load(&args.config).unwrap_or_default();
    let daemon = AnfsDaemon::new(args, config);

    match daemon.run() {
        Ok(()) => {
            info!("cognos-anfs exited cleanly");
            Ok(())
        }
        Err(e) => {
            error!(error = %e, "cognos-anfs exited with error");
            Err(e)
        }
    }
}

// v0: stub implementation
