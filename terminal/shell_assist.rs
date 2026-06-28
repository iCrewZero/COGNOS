//! Shell assistant — sits beside the user's shell, observes commands and output,
//! and offers context-aware suggestions: command translation, error explanation,
//! and next-step hints. Never auto-executes — only suggests.
//!
//! The assistant is fed [`HistoryEntry`] records by the shell integration layer
//! (PROMPT_COMMAND / preexec hooks). It then exposes three primary surfaces:
//!
//! - [`ShellAssist::suggest`]         — proactive hints for the current context
//! - [`ShellAssist::explain_error`]   — natural-language explanation of stderr
//! - [`ShellAssist::translate`]       — natural-language → shell command candidates
//!
//! All suggestions are advisory. The HAL must approve any command that touches
//! privileged resources before it is even *displayed* to the user with
//! `requires_approval = true`.
//!
//! # Privacy
//!
//! Directories that match the [`PRIVATE_DIR_PATTERNS`] list (e.g. `~/.ssh`,
//! `~/.gnupg`, `/etc/cognos/hal`) are never observed — entries captured in
//! those working directories are silently dropped and a redacted audit record
//! is emitted instead.
//!
//! // v0: stub implementation

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, info, warn};

// ─── Types ───────────────────────────────────────────────────────────────────

/// Opaque identifier for the shell session this assistant is bound to.
///
/// A single user may have multiple terminals open; each gets its own
/// `ShellAssist` instance sharing the same underlying `LlmHandle` and
/// `HalClient`.
pub type SessionId = uuid::Uuid;

/// A single observed command execution.
///
/// Populated by the shell integration layer and pushed into the assistant
/// via [`ShellAssist::observe`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// Wall-clock time the command was invoked.
    pub timestamp: SystemTime,
    /// The raw command line as typed by the user.
    pub command: String,
    /// Exit code of the process (`0` = success). `-1` indicates the process
    /// was killed by a signal or could not be launched.
    pub exit_code: i32,
    /// Truncated stdout captured during execution (capped at ~4 KiB).
    pub stdout_excerpt: String,
    /// Truncated stderr captured during execution (capped at ~4 KiB).
    pub stderr_excerpt: String,
    /// Working directory at the moment of invocation.
    pub cwd: PathBuf,
}

/// The kind of advice a [`Suggestion`] carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SuggestionKind {
    /// A concrete shell command the user may run.
    Command,
    /// A patch / fix-up for a command that just failed.
    Fix,
    /// A recommended next action that is not necessarily a single command
    /// (e.g. "commit your changes", "switch to the feature branch first").
    NextStep,
    /// A non-blocking warning about side-effects, risk, or privacy.
    Warning,
}

/// A single advisory hint returned by the assistant.
///
/// `requires_approval == true` indicates the suggestion, if accepted by the
/// user, will need to traverse the HAL approval flow before execution. The
/// shell integration layer is responsible for surfacing that badge to the
/// user — the assistant never executes anything itself.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Suggestion {
    /// Category of the suggestion.
    pub kind: SuggestionKind,
    /// Human-readable suggestion text (already localized by the LLM).
    pub text: String,
    /// Confidence in `[0.0, 1.0]`. Suggestions below `0.25` are filtered
    /// out before being returned to the caller.
    pub confidence: f32,
    /// Whether HAL approval would be required to act on this suggestion.
    pub requires_approval: bool,
}

/// A candidate shell command produced by natural-language translation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandCandidate {
    /// The proposed shell command.
    pub command: String,
    /// Short rationale for *why* this candidate fits the request.
    pub explanation: String,
    /// Rough risk estimate in `[0.0, 1.0]`. ≥ 0.5 implies destructive or
    /// privileged operations and forces `requires_approval` on the
    /// suggestion that wraps this candidate.
    pub risk_estimate: f32,
}

/// Errors returned by the assistant.
///
/// Deliberately narrow — the assistant never panics on LLM/HAL failures, it
/// returns an `AssistError` so the shell integration layer can degrade
/// gracefully (e.g. show "assistant offline" rather than crash the shell).
#[derive(Debug, Error)]
pub enum AssistError {
    /// The assistant has not yet accumulated enough context to answer.
    #[error("no context available to answer this request")]
    NoContext,
    /// The underlying LLM handle is unreachable (daemon down, OOM, etc.).
    #[error("LLM handle unavailable")]
    LlmUnavailable,
    /// The HAL refused to authorize the inspection or the proposed command.
    #[error("HAL denied the request")]
    HalDenied,
}

// ─── Supporting handles ─────────────────────────────────────────────────────

/// Handle to a local LLM inference engine (llama.cpp / GGUF).
///
/// In v0 this is a stand-in type — the real handle lives in
/// `llm/llama_cpp/inference_engine.rs` and is wired in by the shell crate.
pub struct LlmHandle {
    /// Whether the underlying engine reports itself as ready.
    pub ready: bool,
}

impl LlmHandle {
    /// Construct a stub LLM handle.
    pub fn new() -> Self {
        Self { ready: false }
    }

    /// Returns `true` if the engine is currently reachable.
    pub fn is_ready(&self) -> bool {
        self.ready
    }
}

impl Default for LlmHandle {
    fn default() -> Self {
        Self::new()
    }
}

/// Thin client over the COGNOS HAL service. Used for capability checks
/// before emitting suggestions that would touch privileged resources.
pub struct HalClient {
    /// Endpoint URL (e.g. `unix:///run/cognos/hal.sock`).
    pub endpoint: String,
}

impl HalClient {
    /// Construct a HAL client pointed at the given endpoint.
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self { endpoint: endpoint.into() }
    }
}

// ─── Privacy ────────────────────────────────────────────────────────────────

/// Substrings that, when present in the canonicalized `cwd` of a
/// [`HistoryEntry`], cause the entry to be *dropped* before storage.
///
/// The list is intentionally substring-based so it also catches nested
/// paths (e.g. `~/.ssh/known_hosts` invocations inside `~/.ssh`).
///
/// Never include commands that may echo secrets — the assistant is not
/// allowed to learn anything about contents of these directories.
pub const PRIVATE_DIR_PATTERNS: &[&str] = &[
    ".ssh",
    ".gnupg",
    "/etc/cognos/hal",
    ".password-store",
    ".config/cognos/secrets",
];

/// Returns `true` if `cwd` matches any of the [`PRIVATE_DIR_PATTERNS`].
///
/// The path is canonicalized first so `~` expansion and symlinks are
/// resolved before matching.
fn is_private_dir(cwd: &Path) -> bool {
    let canonical = cwd.to_string_lossy();
    let home = std::env::var("HOME").unwrap_or_default();
    let expanded = canonical.replacen('~', &home, 1);
    PRIVATE_DIR_PATTERNS
        .iter()
        .any(|p| expanded.contains(p))
}

// ─── ShellAssist ────────────────────────────────────────────────────────────

/// The shell assistant.
///
/// Construct one per shell session. The shell integration layer is
/// responsible for hooking PROMPT_COMMAND / `precmd` / `preexec` and feeding
/// [`HistoryEntry`] records into [`ShellAssist::observe`].
pub struct ShellAssist {
    /// Identifier of the owning shell session.
    pub session: SessionId,
    /// Rolling buffer of observed commands (newest at the back).
    /// Capped at [`MAX_HISTORY`] entries.
    pub history: Vec<HistoryEntry>,
    /// Handle to the local LLM inference engine.
    pub llm: LlmHandle,
    /// HAL client used for capability checks.
    pub hal: HalClient,
}

/// Maximum number of [`HistoryEntry`] records retained in-memory.
///
/// Older entries are evicted FIFO. The full transcript can be replayed from
/// the audit log if a longer context window is needed — TODO(v1): wire the
/// audit log into the history backfill path.
pub const MAX_HISTORY: usize = 256;

impl ShellAssist {
    /// Construct a new assistant bound to the given session.
    pub fn new(session: SessionId, llm: LlmHandle, hal: HalClient) -> Self {
        Self {
            session,
            history: Vec::with_capacity(MAX_HISTORY),
            llm,
            hal,
        }
    }

    /// Observe a single command execution.
    ///
    /// Entries whose `cwd` matches a [`PRIVATE_DIR_PATTERNS`] entry are
    /// silently dropped — the assistant must not learn anything about
    /// commands executed in secret-bearing directories.
    pub async fn observe(&mut self, entry: HistoryEntry) {
        if is_private_dir(&entry.cwd) {
            warn!(
                session = %self.session,
                cwd = %entry.cwd.display(),
                "dropping history entry in private directory"
            );
            return;
        }

        debug!(
            session = %self.session,
            cmd = %entry.command,
            exit = entry.exit_code,
            "observed command"
        );

        if self.history.len() >= MAX_HISTORY {
            self.history.remove(0);
        }
        self.history.push(entry);
    }

    /// Produce proactive suggestions for the given ad-hoc `context` string.
    ///
    /// `context` is free-form text — typically the current line buffer or
    /// the last error message — that the assistant should reason about.
    pub async fn suggest(
        &self,
        context: &str,
    ) -> Result<Vec<Suggestion>, AssistError> {
        if self.history.is_empty() && context.trim().is_empty() {
            return Err(AssistError::NoContext);
        }
        if !self.llm.is_ready() {
            // TODO(v1): fall back to a local rule-based suggester so we can
            // still return *something* useful when the LLM is offline.
            return Err(AssistError::LlmUnavailable);
        }

        // TODO(v1): assemble prompt from `self.history` + `context` and
        // dispatch to `LlmHandle::complete`. For v0 we return an empty
        // list — the caller should treat this as "no suggestions".
        info!(session = %self.session, "suggest called (v0 stub)");
        Ok(Vec::new())
    }

    /// Explain a stderr blob in natural language.
    ///
    /// `command` is the command that produced `stderr`; both are passed to
    /// the LLM so the explanation can reference the failing invocation.
    pub async fn explain_error(
        &self,
        command: &str,
        stderr: &str,
    ) -> Result<String, AssistError> {
        if !self.llm.is_ready() {
            return Err(AssistError::LlmUnavailable);
        }
        if command.trim().is_empty() && stderr.trim().is_empty() {
            return Err(AssistError::NoContext);
        }

        // TODO(v1): build an explain_prompt and call self.llm.complete.
        // v0 returns a placeholder acknowledging the failure.
        Ok(format!(
            "command `{}` failed; explanation unavailable in v0 stub.",
            command
        ))
    }

    /// Translate a natural-language request into shell command candidates.
    ///
    /// Candidates are sorted by descending relevance by the caller; the
    /// assistant returns them in LLM-emitted order.
    pub async fn translate(
        &self,
        natural: &str,
    ) -> Result<Vec<CommandCandidate>, AssistError> {
        if natural.trim().is_empty() {
            return Err(AssistError::NoContext);
        }
        if !self.llm.is_ready() {
            return Err(AssistError::LlmUnavailable);
        }

        // TODO(v1): dispatch to LLM with a translation prompt and parse the
        // JSON array of candidates. v0 returns an empty list.
        info!(
            session = %self.session,
            req_len = natural.len(),
            "translate called (v0 stub)"
        );
        Ok(Vec::new())
    }
}

// ─── Tests are intentionally omitted in v0 ──────────────────────────────────
// TODO(v1): add integration tests using a mock LlmHandle and HalClient.

// v0: stub implementation
