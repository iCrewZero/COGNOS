//! Sandboxed installer — orchestrates package installation in an isolated namespace: mount namespace, PID namespace, net namespace, restricted capabilities. All writes go through ANFS so the audit chain captures them.
//!
//! The [`SandboxInstaller`] is the entry point for any operation that places
//! new code onto a COGNOS system. It refuses to install anything outside of a
//! sandbox built by `crate::isolation`, and it refuses to let that sandbox
//! touch the host filesystem directly — every byte the installer writes goes
//! through ANFS so the audit chain has a record of it.
//!
//! v0: stub implementation. The public surface (config, receipts, errors) is
//! in place and round-trips through serde, but `install` / `uninstall` /
//! `verify` all return descriptive errors. The actual sandbox construction,
//! package fetch, and integrity check land in v1 — see `TODO(v1)` markers.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, info, instrument, warn};
use uuid::Uuid;

// ─── Errors ──────────────────────────────────────────────────────────────────

/// Errors returned by [`SandboxInstaller`].
#[derive(Debug, Error)]
pub enum InstallError {
    /// The sandbox could not be created or entered.
    #[error("sandbox setup failed: {0}")]
    SandboxFailed(String),
    /// The package attempted a network operation that the configured
    /// [`NetPolicy`] forbids.
    #[error("network blocked by policy")]
    NetworkBlocked,
    /// The HAL refused to grant one or more of the requested capabilities.
    #[error("HAL denied capability grant")]
    HalDenied,
    /// The package's integrity check (signature, hash, or reproducible build
    /// attestation) failed.
    #[error("integrity check failed: {0}")]
    IntegrityCheckFailed(String),
    /// The target prefix does not have enough free space for the install.
    #[error("disk full on target prefix")]
    DiskFull,
}

// ─── Capability / Network policy ─────────────────────────────────────────────

/// A capability the installed package is requesting. The installer forwards
/// the union of these to the HAL for approval before the sandbox is allowed
/// to start.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Read access to the user's home directory.
    ReadHome,
    /// Write access to the user's home directory (subject to ANFS policy).
    WriteHome,
    /// Outbound network access (subject to [`NetPolicy`]).
    Network,
    /// Access to audio capture / playback devices.
    Audio,
    /// Access to camera devices.
    Camera,
    /// Access to GPU compute (CUDA / ROCm / Vulkan).
    Gpu,
    /// Permission to spawn child processes.
    Spawn,
    /// Permission to read system telemetry (CPU, memory, sensors).
    ReadTelemetry,
}

/// Network policy applied to the install sandbox. [`NetPolicy::Disabled`] is
/// the default — installers that need to fetch packages negotiate an
/// [`NetPolicy::ApiOnly`] grant with the HAL first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "mode", content = "allow")]
pub enum NetPolicy {
    /// No network access at all (the net namespace has no interfaces).
    Disabled,
    /// Only the listed host:port endpoints may be contacted.
    ApiOnly(Vec<String>),
    /// Full outbound network. Reserved for trusted first-party installers.
    Full,
}

impl Default for NetPolicy {
    fn default() -> Self {
        Self::Disabled
    }
}

// ─── Package references ──────────────────────────────────────────────────────

/// Where a package is being sourced from. Drives which fetcher the installer
/// hands the request to (apt, flatpak, snap, AppImage, or a local file).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "path")]
pub enum PackageSource {
    /// `apt-get install` style package.
    Apt,
    /// Flatpak remote.
    Flatpak,
    /// Snap store / snap file.
    Snap,
    /// AppImage single-file executable.
    AppImage,
    /// A local file path (already on disk, no fetch required).
    Local(PathBuf),
}

/// A reference to a package to install or upgrade. `name` and `version` are
/// the user-visible identifiers; `source` tells the installer how to obtain
/// the bits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageRef {
    /// Package name, e.g. `ripgrep` or `com.spotify.Client`.
    pub name: String,
    /// Version string as understood by the source (semver, apt epoch, …).
    pub version: String,
    /// Where the package comes from.
    pub source: PackageSource,
}

// ─── Install config ──────────────────────────────────────────────────────────

/// Configuration for a [`SandboxInstaller`] instance. One installer serves
/// many install calls but they all share the same target prefix and
/// capability envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallConfig {
    /// Where the package is fetched from (used for logging only — the actual
    /// source lives on each [`PackageRef`]).
    pub package_source: PackageSource,
    /// Filesystem prefix where the package's files will land (e.g.
    /// `/opt/cognos/pkgs/<name>`).
    pub target_prefix: PathBuf,
    /// Capabilities the installer is willing to request on a package's
    /// behalf. Packages cannot escalate beyond this set.
    pub capabilities: Vec<Capability>,
    /// Network policy for the install sandbox.
    pub network: NetPolicy,
    /// Host directories exposed read-only into the sandbox (typically
    /// `/usr`, `/lib`, `/etc/resolv.conf`).
    pub fs_roots: Vec<PathBuf>,
}

impl Default for InstallConfig {
    fn default() -> Self {
        Self {
            package_source: PackageSource::Local(PathBuf::new()),
            target_prefix: PathBuf::from("/opt/cognos/pkgs"),
            capabilities: Vec::new(),
            network: NetPolicy::default(),
            fs_roots: vec![
                PathBuf::from("/usr"),
                PathBuf::from("/lib"),
                PathBuf::from("/etc/resolv.conf"),
            ],
        }
    }
}

// ─── Install receipt ─────────────────────────────────────────────────────────

/// Proof that an install happened. Stored under
/// `~/.cognos/installer/receipts/<id>.json` and required for any later
/// `uninstall` or `verify` call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallReceipt {
    /// Unique receipt id (UUID v4).
    pub id: Uuid,
    /// Package that was installed.
    pub package: PackageRef,
    /// Version string recorded at install time (may differ from
    /// `package.version` if the source upgraded underneath us).
    pub version: String,
    /// Every file the installer wrote, with absolute paths inside the
    /// target prefix. Used by `uninstall` and `verify`.
    pub installed_files: Vec<PathBuf>,
    /// Capabilities the HAL actually granted (subset of the requested set).
    pub capabilities_granted: Vec<Capability>,
    /// Identifier of the sandbox that ran the install. Required to re-enter
    /// the sandbox for `verify` operations.
    pub sandbox_id: Uuid,
    /// When the install completed (UTC).
    pub timestamp: DateTime<Utc>,
}

// ─── Verify report ───────────────────────────────────────────────────────────

/// Result of [`SandboxInstaller::verify`]. Compares the on-disk state of an
/// install against the receipt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyReport {
    /// Receipt that was checked.
    pub receipt_id: Uuid,
    /// True iff every file in `receipt.installed_files` exists and matches
    /// its recorded hash.
    pub intact: bool,
    /// Files that were missing or modified since install.
    pub modified_files: Vec<PathBuf>,
    /// Capabilities still granted at verify time (may be fewer than at
    /// install time if the HAL has since revoked them).
    pub capabilities_active: Vec<Capability>,
    /// When the verify ran.
    pub checked_at: DateTime<Utc>,
}

// ─── SandboxRunner (placeholder) ─────────────────────────────────────────────

/// Handle to the lower-level sandbox runner (`crate::isolation::Sandbox`).
/// v0 keeps this opaque so the orchestrator can be unit-tested without
/// pulling in the libc layer.
#[derive(Debug, Default)]
pub struct SandboxRunner {
    /// v0: bookkeeping of sandboxes created by this runner. v1 will hold
    /// real `Sandbox` handles.
    sandboxes: HashMap<Uuid, ()>,
}

impl SandboxRunner {
    /// Construct a new runner with no live sandboxes.
    pub fn new() -> Self {
        Self::default()
    }
}

// ─── SandboxInstaller ────────────────────────────────────────────────────────

/// Orchestrates package installation inside an isolated sandbox.
///
/// Typical lifecycle:
///
/// 1. Build a [`SandboxInstaller`] from an [`InstallConfig`].
/// 2. Call [`install`][SandboxInstaller::install] with a [`PackageRef`].
/// 3. Persist the returned [`InstallReceipt`].
/// 4. Later, call [`verify`][SandboxInstaller::verify] with the receipt.
/// 5. Eventually call [`uninstall`][SandboxInstaller::uninstall].
///
/// All file writes performed by the installer go through ANFS so the audit
/// chain captures them; the installer never calls `open(2)` on the host
/// filesystem directly.
pub struct SandboxInstaller {
    /// Install configuration (prefix, capabilities, network policy, …).
    pub config: InstallConfig,
    /// Underlying sandbox runner.
    pub runner: SandboxRunner,
}

impl SandboxInstaller {
    /// Build a new installer from a config. Uses a default [`SandboxRunner`].
    pub fn new(config: InstallConfig) -> Self {
        Self {
            config,
            runner: SandboxRunner::new(),
        }
    }

    /// Install `pkg` into the configured target prefix.
    ///
    /// Steps (v1):
    ///   1. Fetch the package via its [`PackageSource`].
    ///   2. Verify integrity (signature + reproducible-build attestation).
    ///   3. Build a sandbox via `crate::isolation::create`.
    ///   4. Apply the seccomp profile from `crate::seccomp`.
    ///   5. Run the package's install hook inside the sandbox, with every
    ///      write redirected through ANFS.
    ///   6. Record the file manifest and emit an [`InstallReceipt`].
    #[instrument(skip(self, pkg), fields(name = %pkg.name, version = %pkg.version))]
    pub async fn install(&self, pkg: PackageRef) -> Result<InstallReceipt, InstallError> {
        let _ = &self.config;
        let _ = &self.runner;
        info!(name = %pkg.name, version = %pkg.version, "install: v0 stub");

        // TODO(v1): implement the full install pipeline. Until then, refuse
        // so callers do not silently get an empty receipt.
        Err(InstallError::SandboxFailed(
            "install pipeline not implemented in v0".to_string(),
        ))
    }

    /// Uninstall the package identified by `receipt`.
    ///
    /// v1 will re-enter the original sandbox (by `sandbox_id`) and run the
    /// package's uninstall hook, then delete every file listed in the
    /// receipt. v0 returns a descriptive error.
    #[instrument(skip(self, receipt), fields(id = %receipt.id))]
    pub async fn uninstall(&self, receipt: InstallReceipt) -> Result<(), InstallError> {
        let _ = &self.config;
        info!(id = %receipt.id, "uninstall: v0 stub");
        // TODO(v1): re-enter sandbox, run uninstall hook, delete files.
        let _ = receipt;
        Err(InstallError::SandboxFailed(
            "uninstall pipeline not implemented in v0".to_string(),
        ))
    }

    /// Verify that the install described by `receipt` is still intact.
    ///
    /// v1 will re-stat every file in `receipt.installed_files`, re-hash them,
    /// and re-query the HAL for the granted capabilities. v0 returns an empty
    /// (intact) report so callers can plumb the type through.
    #[instrument(skip(self, receipt), fields(id = %receipt.id))]
    pub async fn verify(&self, receipt: InstallReceipt) -> Result<VerifyReport, InstallError> {
        let _ = &self.config;
        debug!(id = %receipt.id, "verify: v0 stub");
        // TODO(v1): re-stat + re-hash every installed file, re-query HAL.
        warn!(
            id = %receipt.id,
            "verify: returning intact=true without checking files in v0"
        );
        Ok(VerifyReport {
            receipt_id: receipt.id,
            intact: true,
            modified_files: Vec::new(),
            capabilities_active: receipt.capabilities_granted,
            checked_at: Utc::now(),
        })
    }

    /// Convenience: read a receipt from a JSON file on disk. v1 will move
    /// this into a dedicated `receipts` module.
    pub fn load_receipt(_path: &Path) -> Result<InstallReceipt, InstallError> {
        // TODO(v1): tokio::fs::read + serde_json::from_slice.
        Err(InstallError::SandboxFailed(
            "receipt loading not implemented in v0".to_string(),
        ))
    }
}

impl Default for SandboxInstaller {
    fn default() -> Self {
        Self::new(InstallConfig::default())
    }
}

// v0: stub implementation
