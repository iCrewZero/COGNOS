//! Flatpak source adapter — wraps the `flatpak` CLI so the unipkg resolver
//! can treat Flatpak like any other pluggable source.
//!
//! v0 mirrors the behavior of the inline `FlatpakSource` in `crate::main` but
//! exposes it as a module. Flatpak is the preferred source for graphical apps
//! because of its strong sandboxing; APT remains preferred for system libs.

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

use super::super::resolver::{ResolvedPackage, SourceKind};
use serde::{Deserialize, Serialize};

// ─── Types ────────────────────────────────────────────────────────────────────

/// Flatpak-specific package metadata (extra fields beyond `ResolvedPackage`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FlatpakMetadata {
    pub app_id: String,
    pub remote: String,
    pub branch: String,
    pub runtime: String,
    pub arch: String,
}

/// The remotes configured on this system. v0: stub list — real list comes
/// from `flatpak remotes --columns=name`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FlatpakRemotes {
    pub names: Vec<String>,
}

/// Handle to the local Flatpak source. Stateless — every call shells out.
#[derive(Debug, Clone, Copy)]
pub struct FlatpakSource;

// ─── Errors ───────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum FlatpakError {
    #[error("flatpak binary not found")]
    NotAvailable,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("flatpak command failed: {0}")]
    CommandFailed(String),
}

// ─── Implementation ───────────────────────────────────────────────────────────

impl FlatpakSource {
    /// Returns true if `flatpak` is on PATH and reports a sane version.
    pub fn available() -> bool {
        Command::new("flatpak")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// The `SourceKind` discriminator for this source.
    pub fn kind() -> SourceKind {
        SourceKind::Flatpak
    }

    /// List configured remotes (flathub, flathub-beta, ...).
    pub fn remotes() -> Result<FlatpakRemotes, FlatpakError> {
        if !Self::available() {
            return Err(FlatpakError::NotAvailable);
        }
        let out = Command::new("flatpak").args(["remotes", "--columns=name"]).output()?;
        if !out.status.success() {
            return Err(FlatpakError::CommandFailed(
                String::from_utf8_lossy(&out.stderr).into_owned(),
            ));
        }
        let names = String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        Ok(FlatpakRemotes { names })
    }

    /// Search Flatpak's app index. v0 uses `flatpak search` which queries the
    /// AppStream data of all configured remotes.
    pub fn search(query: &str) -> Result<Vec<ResolvedPackage>, FlatpakError> {
        if !Self::available() {
            return Err(FlatpakError::NotAvailable);
        }
        let out = Command::new("flatpak").args(["search", query]).output()?;
        if !out.status.success() {
            // `flatpak search` exits non-zero on no results in some versions —
            // treat that as an empty list rather than an error.
            return Ok(Vec::new());
        }
        let stdout = String::from_utf8_lossy(&out.stdout);
        let results = Self::parse_search_table(&stdout);
        Ok(results)
    }

    /// Install a Flatpak app by its app-id (e.g. `com.visualstudio.code`).
    pub fn install(app_id: &str, remote: Option<&str>) -> Result<bool, FlatpakError> {
        if !Self::available() {
            return Err(FlatpakError::NotAvailable);
        }
        let remote = remote.unwrap_or("flathub");
        let status = Command::new("flatpak")
            .args(["install", "-y", remote, app_id])
            .status()?;
        Ok(status.success())
    }

    /// Uninstall a Flatpak app by app-id.
    pub fn remove(app_id: &str) -> Result<bool, FlatpakError> {
        if !Self::available() {
            return Err(FlatpakError::NotAvailable);
        }
        let status = Command::new("flatpak").args(["uninstall", "-y", app_id]).status()?;
        Ok(status.success())
    }

    /// `flatpak update -y`. Returns (success, count_updated).
    pub fn update_all() -> Result<(bool, u32), FlatpakError> {
        if !Self::available() {
            return Err(FlatpakError::NotAvailable);
        }
        let out = Command::new("flatpak").args(["update", "-y"]).output()?;
        let count = String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| l.contains("Updating"))
            .count() as u32;
        Ok((out.status.success(), count))
    }

    /// True if the Flatpak app is currently installed locally.
    pub fn is_installed(app_id: &str) -> bool {
        Command::new("flatpak")
            .args(["info", app_id])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// Stream `flatpak install` progress to the caller.
    pub fn install_streaming(
        app_id: &str,
        remote: Option<&str>,
    ) -> Result<impl BufRead, FlatpakError> {
        if !Self::available() {
            return Err(FlatpakError::NotAvailable);
        }
        let remote = remote.unwrap_or("flathub");
        let child = Command::new("flatpak")
            .args(["install", "-y", "--noninteractive", remote, app_id])
            .stderr(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()?;
        Ok(BufReader::new(child.stderr.expect("piped stderr")))
    }

    // ─── Helpers ─────────────────────────────────────────────────────────────

    /// Parse `flatpak search`'s columnar output. The format is:
    /// ```text
    /// Name        Application ID             Version   Branch   Remotes
    /// ─────────────────────────────────────────────────────────────────────
    /// VS Code     com.visualstudio.code      stable    stable   flathub
    /// ```
    /// v0 uses a tolerant column split; v1 will switch to `--columns=` parse.
    fn parse_search_table(stdout: &str) -> Vec<ResolvedPackage> {
        // TODO(v1): use `flatpak search --columns=name,application,version,branch,remote`
        //           to get machine-parseable output. For v0 we do a best-effort split.
        let mut out = Vec::new();
        let mut saw_header = false;
        for line in stdout.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if !saw_header {
                // First non-empty line is the column header.
                saw_header = true;
                continue;
            }
            if trimmed.starts_with('─') || trimmed.starts_with('-') {
                continue;
            }
            // Best-effort: split on 2+ spaces.
            let cols: Vec<&str> = trimmed.split_ascii_whitespace().collect();
            if cols.len() < 2 {
                continue;
            }
            let display_name = cols[0].to_string();
            let app_id = cols[1].to_string();
            let version = cols.get(2).map(|s| s.to_string()).unwrap_or_default();
            out.push(ResolvedPackage {
                name: app_id.clone(),
                display_name: display_name.clone(),
                version,
                description: String::new(),
                source: SourceKind::Flatpak,
                installed: Self::is_installed(&app_id),
                size_kb: None,
                score: 0.0,
            });
        }
        out
    }
}

// v0: stub implementation — see `crate::main::FlatpakSource` for the original inline version
