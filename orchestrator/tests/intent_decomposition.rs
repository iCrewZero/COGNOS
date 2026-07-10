//! Intent decomposition integration tests — IPC path vs legacy fallback.
//!
//! (a) Mock intent-engine returns a 3-node graph → 3 dependent TaskNodes.
//! (b) Intent-engine unreachable → legacy `classify_intent` + `decompose_into_tasks`
//!     (not connected, and connected-then-down / closed port).
//! (c) Cyclic proto graph → clean rejection (no silent fallback).

use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cognos_ipc_grpc::client::{ClientConfig, CognosClient};
use cognos_ipc_grpc::proto::v1::{
    Intent, IntentActionEdge, IntentActionGraph, IntentActionNode, IntentResponse,
};
use cognos_ipc_grpc::server::{CognosServer, IntentHandler, ServerConfig};
use cognos_orchestrator::{Intent as OrchIntent, OrchestratorError, OrchestratorRuntime};

/// Mock intent-engine: returns a fixed action graph on every `DispatchIntent`.
struct MockIntentHandler {
    graph: IntentActionGraph,
    status: String,
}

#[async_trait]
impl IntentHandler for MockIntentHandler {
    async fn handle(&self, _intent: &Intent) -> IntentResponse {
        IntentResponse {
            intent_id: String::new(),
            status: self.status.clone(),
            result_json: Vec::new(),
            message: "mock intent-engine".into(),
            violation: None,
            completed_at_ns: 0,
            action_graph: if self.status == "ok" {
                Some(self.graph.clone())
            } else {
                None
            },
            trace_id: String::new(),
        }
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .expect("local addr")
        .port()
}

fn proto_node(id: &str, action: &str, target: &str) -> IntentActionNode {
    IntentActionNode {
        node_id: id.into(),
        intent_id: uuid::Uuid::new_v4().to_string(),
        action: action.into(),
        target: target.into(),
        confidence: 0.9,
        hal_pre_score: 0.1,
    }
}

fn three_node_chain_graph() -> IntentActionGraph {
    IntentActionGraph {
        nodes: vec![
            proto_node("n1", "create_dir", "~/project"),
            proto_node("n2", "create_file", "~/project/main.rs"),
            proto_node("n3", "open_files", "~/project/main.rs"),
        ],
        deps: vec![
            IntentActionEdge {
                from_node: "n1".into(),
                to_node: "n2".into(),
            },
            IntentActionEdge {
                from_node: "n2".into(),
                to_node: "n3".into(),
            },
        ],
        intent: None,
    }
}

fn cyclic_graph() -> IntentActionGraph {
    IntentActionGraph {
        nodes: vec![
            proto_node("a", "step_a", "t"),
            proto_node("b", "step_b", "t"),
        ],
        deps: vec![
            IntentActionEdge {
                from_node: "a".into(),
                to_node: "b".into(),
            },
            IntentActionEdge {
                from_node: "b".into(),
                to_node: "a".into(),
            },
        ],
        intent: None,
    }
}

async fn start_mock_intent_server(graph: IntentActionGraph) -> (String, tokio::task::JoinHandle<()>) {
    let port = free_port();
    let bind = format!("127.0.0.1:{port}");
    let endpoint = format!("http://{bind}");

    let mut cfg = ServerConfig::default();
    cfg.bind_addr = bind.clone();
    cfg.self_capability = "intent.engine".into();
    let server = CognosServer::with_config(cfg).with_intent_handler(Arc::new(MockIntentHandler {
        graph,
        status: "ok".into(),
    }));

    let addr = bind.parse().expect("socket addr");
    let handle = tokio::spawn(async move {
        let _ = server.serve(addr).await;
    });

    // Brief pause so the listener is up before the test dials.
    tokio::time::sleep(Duration::from_millis(50)).await;
    (endpoint, handle)
}

async fn connect_client(endpoint: &str) -> CognosClient {
    let mut client = CognosClient::new(ClientConfig {
        agent_id: "test.orchestrator".into(),
        signing_secret: String::new(),
        endpoint: endpoint.to_string(),
        backoff_init_ms: 50,
        backoff_max_ms: 200,
        max_reconnect_attempts: 40,
        request_timeout_ms: 2_000,
        ..Default::default()
    });
    client
        .connect(endpoint)
        .await
        .expect("connect to mock intent-engine");
    client
}

fn node_actions(runtime: &OrchestratorRuntime) -> Vec<String> {
    let order = runtime.graph.topological().expect("task graph topo sort");
    order
        .iter()
        .map(|id| {
            runtime
                .graph
                .get(*id)
                .and_then(|n| n.intent.get("action"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        })
        .collect()
}

fn assert_legacy_coding_fallback(runtime: &OrchestratorRuntime) {
    assert_eq!(runtime.graph.nodes.len(), 3);
    let caps: Vec<String> = runtime
        .graph
        .topological()
        .unwrap()
        .iter()
        .map(|id| {
            runtime.graph.get(*id).unwrap().intent["capability"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(
        caps,
        vec!["coding.plan", "coding.execute", "coding.validate"]
    );
    assert!(
        runtime
            .graph
            .nodes
            .values()
            .all(|n| n.intent.get("target").and_then(|v| v.as_str()) == Some(""))
    );
}

#[tokio::test]
async fn intent_engine_three_node_graph_produces_three_dependent_tasks() {
    let (endpoint, _server) = start_mock_intent_server(three_node_chain_graph()).await;
    let client = connect_client(&endpoint).await;

    let mut runtime = OrchestratorRuntime::new().await;
    runtime.attach_intent_client(client);

    let intent = OrchIntent::new("user.test", "set up my project workspace");
    let root = runtime.submit(intent).await.expect("submit via intent-engine");

    assert_eq!(runtime.graph.nodes.len(), 3);
    assert_eq!(
        node_actions(&runtime),
        vec!["create_dir", "create_file", "open_files"]
    );

    // Root is the first node in topological order (no predecessors).
    let topo = runtime.graph.topological().expect("topo");
    assert_eq!(root, topo[0]);

    // Two control edges n1→n2, n2→n3.
    assert_eq!(runtime.graph.edges.len(), 2);
}

#[tokio::test]
async fn intent_engine_not_connected_uses_legacy_keyword_path() {
    let mut runtime = OrchestratorRuntime::new().await;
    // Never dial — `dispatch_intent` fails instantly with a transport error.
    runtime.attach_intent_client(CognosClient::new(ClientConfig {
        agent_id: "test.orchestrator".into(),
        endpoint: "http://127.0.0.1:1".into(),
        signing_secret: String::new(),
        ..Default::default()
    }));

    let intent = OrchIntent::new("user.test", "please implement the new feature");
    runtime
        .submit(intent)
        .await
        .expect("legacy fallback should succeed");

    assert_legacy_coding_fallback(&runtime);
}

#[tokio::test]
async fn intent_engine_connected_then_down_uses_legacy_keyword_path() {
    // Prod path: client dialed successfully at startup, then the engine dies
    // (or the endpoint becomes unreachable) before the next DispatchIntent.
    let (endpoint, server) = start_mock_intent_server(three_node_chain_graph()).await;
    let client = connect_client(&endpoint).await;
    server.abort();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut runtime = OrchestratorRuntime::new().await;
    runtime.attach_intent_client(client);

    let intent = OrchIntent::new("user.test", "please implement the new feature");
    runtime
        .submit(intent)
        .await
        .expect("legacy fallback after transport failure on a dead endpoint");

    assert_legacy_coding_fallback(&runtime);
}

#[tokio::test]
async fn cyclic_proto_graph_is_rejected() {
    let (endpoint, _server) = start_mock_intent_server(cyclic_graph()).await;
    let client = connect_client(&endpoint).await;

    let mut runtime = OrchestratorRuntime::new().await;
    runtime.attach_intent_client(client);

    let intent = OrchIntent::new("user.test", "do something cyclic");
    let err = runtime
        .submit(intent)
        .await
        .expect_err("cycle must be rejected");

    match err {
        OrchestratorError::DecompositionFailed(msg) => {
            assert!(
                msg.contains("cycle"),
                "expected cycle rejection, got: {msg}"
            );
        }
        other => panic!("expected DecompositionFailed, got {other:?}"),
    }
    assert!(runtime.graph.nodes.is_empty(), "no tasks on cycle rejection");
}
