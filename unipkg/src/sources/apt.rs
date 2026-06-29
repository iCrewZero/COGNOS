//! APT source adapter — wraps `apt-cache` / `apt-get` / `dpkg` so the unipkg
//! resolver can treat APT like any other pluggable source.
//!
//! v0 mirrors the behavior of the inline `AptSource` in `crate::main` but
//! exposes it as a module so the resolver can call it polymorphically. The
//! actual install path delegates to `apt-get install -y` so existing apt
//! workflows continue to work unchanged.

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

use super::super::resolver::{ResolvedPackage, SourceKind};
use serde::{Deserialize, Serialize};

// ─── Types ────────────────────────────────────────────────────────────────────

/// APT-specific package metadata (extra fields beyond `ResolvedPackage`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AptMetadata {
    pub maintainer: String,
    pub section: String,
    pub dependencies: Vec<String>,
    pub origin: String,
}

/// Handle to the local APT source. Stateless — every call shells out.
#[derive(Debug, Clone, Copy)]
pub struct AptSource;

// ─── Errors ───────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum AptError {
    #[error("apt binary not found")]
    NotAvailable,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("apt command failed: {0}")]
    CommandFailed(String),
}

// ─── Implementation ───────────────────────────────────────────────────────────

impl AptSource {
    /// Returns true if `apt-get` is on PATH and reports a sane version.
    pub fn available() -> bool {
        Command::new("apt-get")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// The `SourceKind` discriminator for this source.
    pub fn kind() -> SourceKind {
        SourceKind::Apt
    }

    /// Search apt's package index by name substring.
    pub fn search(query: &str) -> Result<Vec<ResolvedPackage>, AptError> {
        if !Self::available() {
            return Err(AptError::NotAvailable);
        }
        let out = Command::new("apt-cache")
            .args(["search", "--names-only", query])
            .output()?;
        if !out.status.success() {
            return Err(AptError::CommandFailed(
                String::from_utf8_lossy(&out.stderr).into_owned(),
            ));
        }
        let results: Vec<ResolvedPackage> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|line| {
                let mut parts = line.splitn(2, " - ");
                let name = parts.next()?.trim().to_string();
                let desc = parts.next().unwrap_or("").trim().to_string();
                Some(ResolvedPackage {
                    name: name.clone(),
                    display_name: name.clone(),
                    version: String::new(),
                    description: desc,
                    source: SourceKind::Apt,
                    installed: Self::is_installed(&name),
                    size_kb: None,
                    score: 0.0,
                })
            })
            .collect();
        Ok(results)
    }

    /// Install a package by name. Requires root. v0: passes `-y` so the
    /// command never blocks on a prompt.
    pub fn install(name: &str) -> Result<bool, AptError> {
        if !Self::available() {
            return Err(AptError::NotAvailable);
        }
        let status = Command::new("apt-get").args(["install", "-y", name]).status()?;
        Ok(status.success())
    }

    /// Remove a package by name. Does not purge config by default.
    pub fn remove(name: &str) -> Result<bool, AptError> {
        if !Self::available() {
            return Err(AptError::NotAvailable);
        }
        let status = Command::new("apt-get").args(["remove", "-y", name]).status()?;
        Ok(status.success())
    }

    /// `apt-get update && apt-get upgrade -y`. Returns (success, packages_upgraded).
    pub fn update_all() -> Result<(bool, u32), AptError> {
        if !Self::available() {
            return Err(AptError::NotAvailable);
        }
        Command::new("apt-get").arg("update").status()?;
        let out = Command::new("apt-get").args(["upgrade", "-y"]).output()?;
        let count = String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| l.contains("upgraded"))
            .count() as u32;
        Ok((out.status.success(), count))
    }

    /// True if the package is currently installed locally (via dpkg).
    pub fn is_installed(name: &str) -> bool {
        Command::new("dpkg")
            .args(["-l", name])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// Stream `apt-get install` progress lines to the caller. v0: returns a
    /// reader over stderr; v1 will parse the new JSON progress format.
    pub fn install_streaming(
        name: &str,
    ) -> Result<impl BufRead, AptError> {
        if !Self::available() {
            return Err(AptError::NotAvailable);
        }
        let child = Command::new("apt-get")
            .args(["install", "-y", "--status-fd", "1", name])
            .stderr(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()?;
        // TODO(v1): real status-fd parsing; for now we hand back stderr.
        Ok(BufReader::new(child.stderr.expect("piped stderr")))
    }
}

// v0: stub implementation — see `crate::main::AptSource` for the original inline version
