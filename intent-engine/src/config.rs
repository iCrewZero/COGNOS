//! Runtime configuration for the intent-engine daemon.
//!
//! Resolution order (last wins):
//!   1. Built-in dev defaults (this module).
//!   2. `/etc/cognos/intent.toml` (or the path passed via `--config`).
//!   3. Environment variables (`COGNOS_INTENT_*`, `COGNOS_IPC_*`).
//!
//! A missing config file is not an error — the daemon runs on defaults +
//! env, which is what the dev workflow and the integration test rely on.

use std::path::Path;
use std::time::Duration;

use serde::Deserialize;

/// Default inference endpoint (vLLM OpenAI-compatible server).
pub const DEFAULT_LLAMA_ENDPOINT: &str = "http://127.0.0.1:8080";
/// Default model identifier (vLLM HF model name).
pub const DEFAULT_MODEL: &str = "Qwen/Qwen2.5-7B-Instruct-AWQ";
/// Default per-request timeout against the inference server (ms).
pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;
/// Default production JSON Schema path for vLLM structured output.
pub const DEFAULT_SCHEMA_PATH: &str = "/etc/cognos/intent-llm-output.schema.json";
/// Default GBNF grammar path (llama-server legacy backend only).
pub const DEFAULT_GRAMMAR_PATH: &str = "/etc/cognos/intent.gbnf";
/// Default central IPC bus endpoint (for agent registration/heartbeat).
pub const DEFAULT_IPC_ENDPOINT: &str = "http://127.0.0.1:7443";
/// Default bind address for this daemon's own `DispatchIntent` server.
pub const DEFAULT_IPC_BIND: &str = "127.0.0.1:7445";
/// Agent identity presented to the central IPC bus.
pub const AGENT_ID: &str = "agent.intent-engine";

/// Inference backend kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InferenceBackendKind {
    /// vLLM OpenAI-compatible server with JSON Schema structured output.
    Vllm,
    /// llama.cpp `llama-server` with GBNF grammar (legacy).
    Llama,
}

impl InferenceBackendKind {
    fn parse(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "llama" | "llama-server" | "llama_server" => Self::Llama,
            _ => Self::Vllm,
        }
    }
}

/// Fully-resolved intent-engine configuration.
#[derive(Debug, Clone)]
pub struct IntentConfig {
    /// Inference server base URL (vLLM or llama-server).
    pub llama_endpoint: String,
    /// Model identifier.
    pub model: String,
    /// Per-request timeout against the inference server.
    pub timeout: Duration,
    /// Production JSON Schema path (vLLM structured output).
    pub schema_path: String,
    /// GBNF grammar path (llama-server only).
    pub grammar_path: String,
    /// Which HTTP inference backend to use.
    pub backend: InferenceBackendKind,
    /// Central IPC bus endpoint (registration + heartbeat).
    pub ipc_endpoint: String,
    /// Bind address for this daemon's own `DispatchIntent` gRPC server.
    pub ipc_bind: String,
    /// HMAC signing secret shared with the IPC server (may be empty in dev).
    pub secret: String,
}

impl Default for IntentConfig {
    fn default() -> Self {
        Self {
            llama_endpoint: DEFAULT_LLAMA_ENDPOINT.to_string(),
            model: DEFAULT_MODEL.to_string(),
            timeout: Duration::from_millis(DEFAULT_TIMEOUT_MS),
            schema_path: DEFAULT_SCHEMA_PATH.to_string(),
            grammar_path: DEFAULT_GRAMMAR_PATH.to_string(),
            backend: InferenceBackendKind::Vllm,
            ipc_endpoint: DEFAULT_IPC_ENDPOINT.to_string(),
            ipc_bind: DEFAULT_IPC_BIND.to_string(),
            secret: String::new(),
        }
    }
}

// ─── On-disk representation ────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
struct RawConfig {
    #[serde(default)]
    llama: RawLlama,
    #[serde(default)]
    vllm: RawVllm,
    #[serde(default)]
    inference: RawInference,
    #[serde(default)]
    ipc: RawIpc,
}

#[derive(Debug, Default, Deserialize)]
struct RawInference {
    backend: Option<String>,
    endpoint: Option<String>,
    model: Option<String>,
    timeout_ms: Option<u64>,
    schema: Option<String>,
    grammar: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct RawVllm {
    endpoint: Option<String>,
    model: Option<String>,
    timeout_ms: Option<u64>,
    schema: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct RawLlama {
    endpoint: Option<String>,
    model: Option<String>,
    timeout_ms: Option<u64>,
    grammar: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct RawIpc {
    endpoint: Option<String>,
    bind: Option<String>,
    secret: Option<String>,
}

impl IntentConfig {
    /// Load config from defaults, then an optional TOML file, then env vars.
    ///
    /// `config_path` is typically `/etc/cognos/intent.toml` (from the systemd
    /// `--config` flag). A missing file is fine; a malformed file logs a
    /// warning and is ignored so the daemon still starts on defaults + env.
    pub fn load(config_path: Option<&str>) -> Self {
        let mut cfg = Self::default();

        if let Some(path) = config_path {
            cfg.apply_file(path);
        }

        cfg.apply_env();
        cfg
    }

    fn apply_file(&mut self, path: &str) {
        let p = Path::new(path);
        if !p.exists() {
            log::debug!("intent config file not found at {path}; using defaults + env");
            return;
        }
        let text = match std::fs::read_to_string(p) {
            Ok(t) => t,
            Err(e) => {
                log::warn!("could not read intent config {path}: {e}; using defaults + env");
                return;
            }
        };
        let raw: RawConfig = match toml::from_str(&text) {
            Ok(r) => r,
            Err(e) => {
                log::warn!("malformed intent config {path}: {e}; using defaults + env");
                return;
            }
        };

        if let Some(v) = raw.llama.endpoint {
            self.llama_endpoint = v;
        }
        if let Some(v) = raw.llama.model {
            self.model = v;
        }
        if let Some(v) = raw.llama.timeout_ms {
            self.timeout = Duration::from_millis(v);
        }
        if let Some(v) = raw.llama.grammar {
            self.grammar_path = v;
        }
        if let Some(v) = raw.vllm.endpoint {
            self.llama_endpoint = v;
        }
        if let Some(v) = raw.vllm.model {
            self.model = v;
        }
        if let Some(v) = raw.vllm.timeout_ms {
            self.timeout = Duration::from_millis(v);
        }
        if let Some(v) = raw.vllm.schema {
            self.schema_path = v;
        }
        if let Some(inf) = raw.inference.backend {
            self.backend = InferenceBackendKind::parse(&inf);
        }
        if let Some(v) = raw.inference.endpoint {
            self.llama_endpoint = v;
        }
        if let Some(v) = raw.inference.model {
            self.model = v;
        }
        if let Some(v) = raw.inference.timeout_ms {
            self.timeout = Duration::from_millis(v);
        }
        if let Some(v) = raw.inference.schema {
            self.schema_path = v;
        }
        if let Some(v) = raw.inference.grammar {
            self.grammar_path = v;
        }
        if let Some(v) = raw.ipc.endpoint {
            self.ipc_endpoint = v;
        }
        if let Some(v) = raw.ipc.bind {
            self.ipc_bind = v;
        }
        if let Some(v) = raw.ipc.secret {
            self.secret = v;
        }
    }

    fn apply_env(&mut self) {
        if let Some(v) = env_nonempty("COGNOS_INTENT_LLAMA_ENDPOINT") {
            self.llama_endpoint = v;
        }
        if let Some(v) = env_nonempty("COGNOS_INTENT_MODEL") {
            self.model = v;
        }
        if let Some(v) = env_nonempty("COGNOS_INTENT_TIMEOUT_MS") {
            match v.parse::<u64>() {
                Ok(ms) => self.timeout = Duration::from_millis(ms),
                Err(e) => log::warn!("invalid COGNOS_INTENT_TIMEOUT_MS ({v}): {e}; keeping default"),
            }
        }
        if let Some(v) = env_nonempty("COGNOS_INTENT_GRAMMAR") {
            self.grammar_path = v;
        }
        if let Some(v) = env_nonempty("COGNOS_INTENT_SCHEMA") {
            self.schema_path = v;
        }
        if let Some(v) = env_nonempty("COGNOS_INTENT_BACKEND") {
            self.backend = InferenceBackendKind::parse(&v);
        }
        if let Some(v) = env_nonempty("COGNOS_INTENT_BIND") {
            self.ipc_bind = v;
        }
        // Shared with the agent bootstrap and the Rust server/Python client.
        if let Some(v) = env_nonempty("COGNOS_IPC_ENDPOINT") {
            self.ipc_endpoint = v;
        }
        // Secret may legitimately be set to empty; read it unconditionally.
        if let Ok(v) = std::env::var("COGNOS_IPC_SECRET") {
            self.secret = v;
        }
    }
}

/// Read an env var, treating unset or empty as absent.
fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let cfg = IntentConfig::default();
        assert_eq!(cfg.llama_endpoint, DEFAULT_LLAMA_ENDPOINT);
        assert_eq!(cfg.model, DEFAULT_MODEL);
        assert_eq!(cfg.timeout, Duration::from_millis(DEFAULT_TIMEOUT_MS));
        assert_eq!(cfg.backend, InferenceBackendKind::Vllm);
        assert_eq!(cfg.ipc_endpoint, DEFAULT_IPC_ENDPOINT);
        assert_eq!(cfg.ipc_bind, DEFAULT_IPC_BIND);
    }

    #[test]
    fn missing_file_falls_back_to_defaults() {
        let cfg = IntentConfig::load(Some("/nonexistent/intent.toml"));
        assert_eq!(cfg.model, DEFAULT_MODEL);
    }

    #[test]
    fn file_values_are_applied() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("cognos_intent_test_{}.toml", std::process::id()));
        std::fs::write(
            &path,
            r#"
[llama]
endpoint = "http://127.0.0.1:9999"
model = "test-model"
timeout_ms = 1234

[ipc]
bind = "127.0.0.1:6000"
secret = "s3cr3t"
"#,
        )
        .unwrap();

        // Clear env so the file values are the ones observed.
        for k in [
            "COGNOS_INTENT_LLAMA_ENDPOINT",
            "COGNOS_INTENT_MODEL",
            "COGNOS_INTENT_TIMEOUT_MS",
            "COGNOS_INTENT_BIND",
            "COGNOS_IPC_ENDPOINT",
            "COGNOS_IPC_SECRET",
        ] {
            std::env::remove_var(k);
        }

        let cfg = IntentConfig::load(Some(path.to_str().unwrap()));
        std::fs::remove_file(&path).ok();

        assert_eq!(cfg.llama_endpoint, "http://127.0.0.1:9999");
        assert_eq!(cfg.model, "test-model");
        assert_eq!(cfg.timeout, Duration::from_millis(1234));
        assert_eq!(cfg.ipc_bind, "127.0.0.1:6000");
        assert_eq!(cfg.secret, "s3cr3t");
    }
}
