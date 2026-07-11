//! Shared fixtures for COGNOS binary E2E tests (cluster spawn, gRPC clients).

use super::approval_responder::{
    hal_gate_socket, hal_ui_socket, socket_dir, ApprovalUiResponder, UiResponderMode,
};

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use cognos_ipc_grpc::client::{ClientConfig, CognosClient};
use cognos_ipc_grpc::proto::v1::{Intent, IntentResponse, PipelineMetrics, PipelineMetricsRequest};
use cognos_orchestrator::runtime::ExecutionReport;

/// Kills a spawned fixture process when dropped.
pub struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// How the intent-engine inference stack is wired for a cluster.
#[derive(Debug, Clone, Copy)]
pub enum LlmMode {
    /// `MOCK_LLM=1` — deterministic mock backend, no network.
    Mock,
    /// Primary llama endpoint on a closed port → keyword fallback.
    OfflineKeywordFallback,
}

/// Full binary E2E clusters bind ephemeral ports and share `/tmp/test`; one
/// cluster at a time avoids cross-test races when `cargo test` runs in parallel.
static E2E_CLUSTER_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// HAL approval wiring for an E2E cluster (Unix UI socket + orchestrator timeout).
#[derive(Debug, Clone, Copy)]
pub struct E2eApprovalConfig {
    pub ui_responder: Option<UiResponderMode>,
    pub approval_timeout_secs: Option<u64>,
}

impl Default for E2eApprovalConfig {
    fn default() -> Self {
        Self {
            ui_responder: Some(UiResponderMode::Deny),
            approval_timeout_secs: None,
        }
    }
}

/// Running COGNOS service cluster with ephemeral ports.
pub struct E2eCluster {
    _cluster_guard: std::sync::MutexGuard<'static, ()>,
    _approval_ui: Option<ApprovalUiResponder>,
    _socket_dir: PathBuf,
    _children: Vec<ChildGuard>,
    _repo_root: PathBuf,
    _ipc_endpoint: String,
    _hal_endpoint: String,
    pub intent_endpoint: String,
    pub orch_endpoint: String,
}

impl E2eCluster {
    pub async fn start(llm_mode: LlmMode) -> Self {
        Self::start_with(llm_mode, E2eApprovalConfig::default()).await
    }

    pub async fn start_with(llm_mode: LlmMode, approval: E2eApprovalConfig) -> Self {
        let cluster_guard = E2E_CLUSTER_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let repo_root = repo_root();
        try_build_workspace_bins(&repo_root);

        let ipc_port = free_port();
        let hal_port = free_port();
        let intent_port = free_port();
        let orch_port = free_port();
        let llama_dead_port = match llm_mode {
            LlmMode::Mock => None,
            LlmMode::OfflineKeywordFallback => Some(free_port()),
        };

        let ipc_bind = format!("127.0.0.1:{ipc_port}");
        let hal_bind = format!("127.0.0.1:{hal_port}");
        let intent_bind = format!("127.0.0.1:{intent_port}");
        let orch_bind = format!("127.0.0.1:{orch_port}");

        let ipc_endpoint = format!("http://{ipc_bind}");
        let hal_endpoint = format!("http://{hal_bind}");
        let intent_endpoint = format!("http://{intent_bind}");
        let orch_endpoint = format!("http://{orch_bind}");

        let agents_dir = repo_root.join("agents");
        let mut children = Vec::new();

        let socket_dir = socket_dir("cluster");
        let _ = std::fs::create_dir_all(&socket_dir);
        let hal_sock = hal_gate_socket(&socket_dir);
        let hal_ui = hal_ui_socket(&socket_dir);
        let approval_ui = approval
            .ui_responder
            .map(|mode| ApprovalUiResponder::start(hal_ui.clone(), mode));

        let ipc_bin = require_bin("cognos-ipc-server", &repo_root);
        children.push(spawn(
            &ipc_bin,
            &[("COGNOS_IPC_BIND", &ipc_bind), ("RUST_LOG", "warn")],
        ));
        wait_for_tcp(&ipc_bind, "cognos-ipc-server").await;

        let scheduler_bin = require_bin("cognos-scheduler", &repo_root);
        children.push(spawn(
            &scheduler_bin,
            &[
                ("COGNOS_IPC_ENDPOINT", &ipc_endpoint),
                ("RUST_LOG", "warn"),
            ],
        ));

        let memory_bin = require_bin("cognos-memory", &repo_root);
        children.push(spawn(
            &memory_bin,
            &[
                ("COGNOS_IPC_ENDPOINT", &ipc_endpoint),
                ("RUST_LOG", "warn"),
            ],
        ));

        let hal_bin = require_bin("cognos-hal", &repo_root);
        let hal_env: Vec<(&str, String)> = vec![
            ("COGNOS_HAL_BIND", hal_bind.clone()),
            ("COGNOS_IPC_ENDPOINT", ipc_endpoint.clone()),
            ("COGNOS_HAL_SOCKET", hal_sock.to_string_lossy().into_owned()),
            ("COGNOS_HAL_UI_SOCKET", hal_ui.to_string_lossy().into_owned()),
            ("RUST_LOG", "warn".into()),
        ];
        children.push(spawn_env(&hal_bin, &hal_env));
        wait_for_tcp(&hal_bind, "cognos-hal").await;

        let intent_bin = require_bin("cognos-intent", &repo_root);
        let mut intent_env: Vec<(&str, String)> = vec![
            ("COGNOS_INTENT_BIND", intent_bind.clone()),
            ("COGNOS_IPC_ENDPOINT", ipc_endpoint.clone()),
            ("COGNOS_IPC_SECRET", String::new()),
            ("COGNOS_INTENT_GRAMMAR", "/nonexistent/intent.gbnf".into()),
            ("RUST_LOG", "warn".into()),
        ];
        match llm_mode {
            LlmMode::Mock => {
                intent_env.push(("MOCK_LLM", "1".into()));
            }
            LlmMode::OfflineKeywordFallback => {
                let dead = llama_dead_port.expect("dead port");
                intent_env.push((
                    "COGNOS_INTENT_LLAMA_ENDPOINT",
                    format!("http://127.0.0.1:{dead}"),
                ));
                intent_env.push(("COGNOS_INTENT_TIMEOUT_MS", "800".into()));
            }
        }
        children.push(spawn_env(&intent_bin, &intent_env));
        wait_for_tcp(&intent_bind, "cognos-intent").await;

        let orch_bin = require_bin("cognos-orchestrator", &repo_root);
        let mut orch_env: Vec<(&str, String)> = vec![
            ("COGNOS_ORCHESTRATOR_BIND", orch_bind.clone()),
            ("COGNOS_HAL_ENDPOINT", hal_endpoint.clone()),
            ("COGNOS_INTENT_ENDPOINT", intent_endpoint.clone()),
            ("COGNOS_IPC_ENDPOINT", ipc_endpoint.clone()),
            ("COGNOS_HAL_SOCKET", hal_sock.to_string_lossy().into_owned()),
            ("COGNOS_AGENTS_DIR", agents_dir.to_string_lossy().into_owned()),
            ("COGNOS_EXTRA_PATHS", "/tmp".into()),
            ("COGNOS_PYTHON", "python3".into()),
            ("RUST_LOG", "warn".into()),
        ];
        if let Some(secs) = approval.approval_timeout_secs {
            orch_env.push(("COGNOS_APPROVAL_TIMEOUT_SECS", secs.to_string()));
        }
        children.push(spawn_env(&orch_bin, &orch_env));
        wait_for_tcp(&orch_bind, "cognos-orchestrator").await;

        Self {
            _cluster_guard: cluster_guard,
            _approval_ui: approval_ui,
            _socket_dir: socket_dir,
            _children: children,
            _repo_root: repo_root,
            _ipc_endpoint: ipc_endpoint,
            _hal_endpoint: hal_endpoint,
            intent_endpoint,
            orch_endpoint,
        }
    }

    pub async fn dispatch_orchestrator(&self, utterance: &str) -> ExecutionReport {
        let (status, report) = self.dispatch_orchestrator_raw(utterance).await;
        assert_eq!(
            status, "ok",
            "orchestrator ingress failed: status={status} summary={}",
            report.summary
        );
        report
    }

    /// Full orchestrator ingress round-trip (any RPC status).
    pub async fn dispatch_orchestrator_raw(&self, utterance: &str) -> (String, ExecutionReport) {
        let resp = self.dispatch_raw(&self.orch_endpoint, utterance).await;
        let report: ExecutionReport = serde_json::from_slice(&resp.result_json).unwrap_or_else(|e| {
            panic!(
                "invalid ExecutionReport JSON: {e}; status={} message={} body={}",
                resp.status,
                resp.message,
                String::from_utf8_lossy(&resp.result_json)
            )
        });
        (resp.status, report)
    }

    pub async fn dispatch_intent_engine(&self, utterance: &str) -> IntentResponse {
        self.dispatch_raw(&self.intent_endpoint, utterance).await
    }

    pub async fn pipeline_metrics(&self) -> (Option<PipelineMetrics>, Option<PipelineMetrics>, Option<PipelineMetrics>) {
        let hal = fetch_metrics(&self._hal_endpoint).await;
        let intent = fetch_metrics(&self.intent_endpoint).await;
        let orch = fetch_metrics(&self.orch_endpoint).await;
        (hal, intent, orch)
    }

    async fn dispatch_raw(&self, endpoint: &str, utterance: &str) -> IntentResponse {
        let client = connect_client(endpoint, "e2e.test").await;
        client
            .dispatch_intent(Intent {
                utterance: utterance.to_string(),
                trace_id: "e2e-trace".to_string(),
                ..Default::default()
            })
            .await
            .expect("DispatchIntent RPC")
    }
}

pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

pub fn benign_target_dir() -> PathBuf {
    PathBuf::from("/tmp/test")
}

pub fn remove_dir_if_exists(path: &Path) {
    if path.exists() {
        let _ = std::fs::remove_dir_all(path);
    }
}

/// Grab a free TCP port on localhost.
pub fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    listener.local_addr().expect("local addr").port()
}

fn spawn(bin: &Path, env: &[(&str, &str)]) -> ChildGuard {
    let pairs: Vec<(&str, String)> = env
        .iter()
        .map(|(k, v)| (*k, (*v).to_string()))
        .collect();
    spawn_env(bin, &pairs)
}

fn spawn_env(bin: &Path, env: &[(&str, String)]) -> ChildGuard {
    let mut cmd = Command::new(bin);
    cmd.stdout(Stdio::null()).stderr(Stdio::inherit());
    for (k, v) in env {
        cmd.env(k, v);
    }
    ChildGuard(
        cmd.spawn()
            .unwrap_or_else(|e| panic!("spawn {}: {e}", bin.display())),
    )
}

async fn wait_for_tcp(bind: &str, label: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    loop {
        if tokio::net::TcpStream::connect(bind).await.is_ok() {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("timeout waiting for {label} on {bind}");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn connect_client(endpoint: &str, agent_id: &str) -> CognosClient {
    for _ in 0..60 {
        let mut client = CognosClient::new(ClientConfig {
            agent_id: agent_id.to_string(),
            signing_secret: String::new(),
            endpoint: endpoint.to_string(),
            backoff_init_ms: 100,
            backoff_max_ms: 500,
            max_reconnect_attempts: 3,
            heartbeat_interval_ms: 5_000,
            request_timeout_ms: 15_000,
        });
        if client.connect(endpoint).await.is_ok() {
            return client;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!("could not connect to {endpoint}");
}

fn try_build_workspace_bins(repo_root: &Path) {
    let _ = Command::new(env!("CARGO"))
        .current_dir(repo_root)
        .args([
            "build",
            "-p",
            "cognos-intent-engine",
            "-p",
            "cognos-intent-engine",
            "-p",
            "cognos-orchestrator",
            "-p",
            "cognos-hal",
            "--bins",
        ])
        .status();
}

fn locate_bin(name: &str) -> Option<PathBuf> {
    if let Ok(path) = std::env::var(format!("CARGO_BIN_EXE_{name}")) {
        let pb = PathBuf::from(path);
        if pb.is_file() {
            return Some(pb);
        }
    }

    let file_name = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };

    let mut dir = std::env::current_exe().ok()?;
    dir.pop();
    if dir.file_name().map(|f| f == "deps").unwrap_or(false) {
        dir.pop();
    }
    let candidate = dir.join(&file_name);
    if candidate.is_file() {
        return Some(candidate);
    }
    None
}

fn require_bin(name: &str, repo_root: &Path) -> PathBuf {
    if let Some(p) = locate_bin(name) {
        return p;
    }
    try_build_workspace_bins(repo_root);
    locate_bin(name).unwrap_or_else(|| {
        panic!(
            "required binary {name} not found — run `cargo build --workspace --bins` first"
        )
    })
}

#[derive(Debug, serde::Deserialize)]
pub struct IntentSchemaLite {
    pub source: Option<String>,
    pub disambiguation_required: bool,
    pub goal: String,
    pub candidate_actions: Vec<serde_json::Value>,
}

impl IntentSchemaLite {
    pub fn from_response(resp: &IntentResponse) -> Self {
        serde_json::from_slice(&resp.result_json).unwrap_or_else(|e| {
            panic!(
                "invalid intent schema JSON: {e}; body={}",
                String::from_utf8_lossy(&resp.result_json)
            )
        })
    }
}

pub fn hal_note_contains(hal: &Option<String>, needle: &str) -> bool {
    hal.as_deref()
        .map(|s| s.to_lowercase().contains(needle))
        .unwrap_or(false)
}

async fn fetch_metrics(endpoint: &str) -> Option<PipelineMetrics> {
    let mut client = CognosClient::new(ClientConfig {
        agent_id: "e2e.metrics".to_string(),
        signing_secret: String::new(),
        endpoint: endpoint.to_string(),
        max_reconnect_attempts: 3,
        request_timeout_ms: 5_000,
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
