//! End-to-end DispatchIntent round-trip against the real `cognos-intent` binary.
//!
//! Topology:
//!   * an in-process central `CognosServer` (the "IPC server" the daemon
//!     registers against),
//!   * an in-process wiremock standing in for vLLM,
//!   * the compiled `cognos-intent` binary as a child process, serving its own
//!     `DispatchIntent` endpoint,
//!   * a `CognosClient` that sends a `DispatchIntent` to the daemon and asserts
//!     it gets back a valid proto action graph.
//!
//! The daemon reaches the mock vLLM over HTTP; the mock returns a schema-valid
//! intent, so the daemon's primary (vLLM) path is exercised — not the keyword
//! fallback.

use std::net::TcpListener;
use std::process::{Child, Command, Stdio};

use cognos_ipc_grpc::client::{ClientConfig, CognosClient};
use cognos_ipc_grpc::proto::v1::Intent;
use cognos_ipc_grpc::server::{CognosServer, ServerConfig};

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Kills the child process (and its server) when the test ends.
struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Grab a free localhost TCP port by binding to :0 and immediately releasing it.
fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    listener.local_addr().expect("local addr").port()
}

/// A schema-valid intent the mock llama returns. Benign file.open so the graph
/// has exactly one node and no risk-gating surprises.
fn mock_intent_json() -> String {
    r#"{
        "goal": "open_file",
        "domain": "system",
        "confidence": 0.88,
        "ambiguity_score": 0.1,
        "risk_estimate": 0.15,
        "hal_pre_score": 0.15,
        "required_context": [],
        "candidate_actions": [
            {"action": "open_file", "target": "/home/user/notes.txt", "confidence": 0.88, "recency_score": 0.5}
        ],
        "disambiguation_required": false,
        "disambiguation_question": null,
        "escalate_to_cloud": false
    }"#
    .to_string()
}

#[tokio::test]
async fn dispatch_intent_roundtrip_returns_action_graph() {
    // 1. Mock vLLM: POST /v1/completions → OpenAI-style completion.
    let vllm = MockServer::start().await;
    let body = serde_json::json!({
        "choices": [{ "text": mock_intent_json(), "index": 0 }]
    })
    .to_string();
    Mock::given(method("POST"))
        .and(path("/v1/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/json"))
        .mount(&vllm)
        .await;

    // 2. In-process central IPC server (registration target for the daemon).
    let central_port = free_port();
    let central_addr = format!("127.0.0.1:{central_port}");
    let central_endpoint = format!("http://{central_addr}");
    {
        let mut cfg = ServerConfig::default();
        cfg.bind_addr = central_addr.clone();
        cfg.self_capability = "ipc.server".to_string();
        let server = CognosServer::with_config(cfg);
        let addr = central_addr.parse().expect("central addr");
        tokio::spawn(async move {
            let _ = server.serve(addr).await;
        });
    }

    // 3. Spawn the real cognos-intent binary.
    let intent_port = free_port();
    let intent_bind = format!("127.0.0.1:{intent_port}");
    let intent_endpoint = format!("http://{intent_bind}");

    let bin = env!("CARGO_BIN_EXE_cognos-intent");
    let child = Command::new(bin)
        .env("COGNOS_INTENT_BACKEND", "vllm")
        .env("COGNOS_INTENT_LLAMA_ENDPOINT", vllm.uri())
        .env("COGNOS_INTENT_MODEL", "Qwen/Qwen2.5-7B-Instruct-AWQ")
        .env("COGNOS_INTENT_TIMEOUT_MS", "5000")
        .env("COGNOS_INTENT_SCHEMA", "/nonexistent/intent.schema.json")
        .env("COGNOS_INTENT_BIND", &intent_bind)
        .env("COGNOS_IPC_ENDPOINT", &central_endpoint)
        .env("COGNOS_IPC_SECRET", "")
        .env("RUST_LOG", "warn")
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn cognos-intent binary");
    let _guard = ChildGuard(child);

    // 4. Connect to the daemon's own DispatchIntent endpoint (retry while it
    //    starts up).
    let mut client = CognosClient::new(ClientConfig {
        agent_id: "test.dispatch".to_string(),
        signing_secret: String::new(),
        endpoint: intent_endpoint.clone(),
        backoff_init_ms: 100,
        backoff_max_ms: 500,
        max_reconnect_attempts: 60, // ~ up to 30s of dialing while the bin boots
        heartbeat_interval_ms: 5_000,
        request_timeout_ms: 3_000,
    });
    client
        .connect(&intent_endpoint)
        .await
        .expect("connect to cognos-intent DispatchIntent server");

    // 5. Round-trip a DispatchIntent.
    let intent = Intent {
        utterance: "open my notes".to_string(),
        ..Default::default()
    };
    let resp = client
        .dispatch_intent(intent)
        .await
        .expect("DispatchIntent RPC should succeed");

    // 6. Assert we got a valid action graph back.
    assert_eq!(resp.status, "ok", "unexpected status: {} ({})", resp.status, resp.message);
    let graph = resp
        .action_graph
        .expect("response must carry an action_graph");
    assert!(
        !graph.nodes.is_empty(),
        "action graph must have at least one node"
    );
    assert_eq!(graph.nodes[0].action, "open_file");
    assert_eq!(graph.nodes[0].target, "/home/user/notes.txt");

    let embedded = graph.intent.expect("graph carries the parsed intent");
    assert_eq!(embedded.goal, "open_file");
    assert_eq!(embedded.raw_input, "open my notes");
    assert_eq!(embedded.source, "vllm");
}
