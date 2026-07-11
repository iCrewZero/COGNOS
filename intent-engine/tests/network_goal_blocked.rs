//! Network goals are parsed then blocked with status=unsupported — never dispatched.

use std::net::TcpListener;
use std::process::{Child, Command, Stdio};

use cognos_ipc_grpc::client::{ClientConfig, CognosClient};
use cognos_ipc_grpc::proto::v1::Intent;
use cognos_ipc_grpc::server::{CognosServer, ServerConfig};

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    listener.local_addr().expect("local addr").port()
}

fn network_download_intent_json() -> String {
    r#"{
        "goal": "network_download",
        "domain": "downloads",
        "confidence": 0.9,
        "ambiguity_score": 0.1,
        "risk_estimate": 0.5,
        "hal_pre_score": 0.5,
        "required_context": [],
        "candidate_actions": [
            {"action": "download_file", "target": "https://mirror.internal/archive.tar", "confidence": 0.9, "recency_score": 0.5}
        ],
        "disambiguation_required": false,
        "disambiguation_question": null,
        "escalate_to_cloud": false
    }"#
    .to_string()
}

#[tokio::test]
async fn network_goal_returns_unsupported_without_action_graph() {
    let vllm = MockServer::start().await;
    let body = serde_json::json!({
        "choices": [{ "text": network_download_intent_json(), "index": 0 }]
    })
    .to_string();
    Mock::given(method("POST"))
        .and(path("/v1/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/json"))
        .mount(&vllm)
        .await;

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

    let intent_port = free_port();
    let intent_bind = format!("127.0.0.1:{intent_port}");
    let intent_endpoint = format!("http://{intent_bind}");

    let bin = env!("CARGO_BIN_EXE_cognos-intent");
    let child = Command::new(bin)
        .env("COGNOS_INTENT_BACKEND", "vllm")
        .env("COGNOS_INTENT_LLAMA_ENDPOINT", vllm.uri())
        .env("COGNOS_INTENT_MODEL", "Qwen/Qwen2.5-7B-Instruct-AWQ")
        .env("COGNOS_INTENT_TIMEOUT_MS", "5000")
        .env("COGNOS_INTENT_SCHEMA", "/nonexistent/schema.json")
        .env("COGNOS_INTENT_BIND", &intent_bind)
        .env("COGNOS_IPC_ENDPOINT", &central_endpoint)
        .env("COGNOS_IPC_SECRET", "")
        .env("RUST_LOG", "warn")
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn cognos-intent binary");
    let _guard = ChildGuard(child);

    let mut client = CognosClient::new(ClientConfig {
        agent_id: "test.network_block".to_string(),
        signing_secret: String::new(),
        endpoint: intent_endpoint.clone(),
        backoff_init_ms: 100,
        backoff_max_ms: 500,
        max_reconnect_attempts: 60,
        heartbeat_interval_ms: 5_000,
        request_timeout_ms: 3_000,
    });
    client
        .connect(&intent_endpoint)
        .await
        .expect("connect to cognos-intent");

    let intent = Intent {
        utterance: "fetch the telemetry archive from the internal mirror".to_string(),
        ..Default::default()
    };
    let resp = client
        .dispatch_intent(intent)
        .await
        .expect("DispatchIntent RPC should succeed");

    assert_eq!(
        resp.status, "unsupported",
        "network goal must be blocked: {}",
        resp.message
    );
    assert!(
        resp.message.contains("non supporté"),
        "message must explain v1 block: {}",
        resp.message
    );
    assert!(
        resp.action_graph.is_none(),
        "must not produce an action graph for network goals"
    );
}
