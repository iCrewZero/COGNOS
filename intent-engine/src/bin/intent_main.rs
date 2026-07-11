//! COGNOS intent engine daemon.
//!
//! Wires the parsing pipeline to the IPC bus:
//!   * loads [`IntentConfig`] (defaults → `/etc/cognos/intent.toml` → env),
//!   * builds an [`HttpVllmBackend`] (or legacy [`HttpLlamaBackend`]) wrapped
//!     in a [`FallbackBackend`] (keyword classifier) behind an [`IntentParser`],
//!   * serves `DispatchIntent` on its own gRPC endpoint (`ipc.bind`), returning
//!     a constructed [`ActionGraph`] serialized to proto,
//!   * registers as agent `agent.intent-engine` on the central IPC bus and
//!     keeps a heartbeat alive.
//!
//! Owner: iCrewZero

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use cognos_ipc_grpc::agent::{self, AgentSpec, DEFAULT_HEARTBEAT_INTERVAL_MS, DEFAULT_MAX_FAILURES};
use cognos_ipc_grpc::pipeline_metrics::METRICS;
use cognos_ipc_grpc::proto::v1::{
    Intent, IntentActionEdge, IntentActionGraph, IntentActionNode, IntentResponse,
    IntentSchemaProto,
};
use cognos_ipc_grpc::server::{CognosServer, IntentHandler, ServerConfig};

use cognos_intent_engine::action_graph::ActionGraph;
use cognos_intent_engine::backends::fallback::FallbackBackend;
use cognos_intent_engine::backends::http_llama::HttpLlamaBackend;
use cognos_intent_engine::backends::http_vllm::{HttpVllmBackend, VLLM_SOURCE};
use cognos_intent_engine::backends::mock_llama::MockLlmBackend;
use cognos_intent_engine::config::{InferenceBackendKind, IntentConfig, AGENT_ID};
use cognos_intent_engine::non_executable_reason;
use cognos_intent_engine::parser::{IntentParser, IntentSchema, ParseResult, SessionContext};
use cognos_intent_engine::backends::KEYWORD_FALLBACK_SOURCE;

/// Backend stack used by the daemon (mock, vLLM, or legacy llama + keyword fallback).
enum ServiceBackend {
    Mock(FallbackBackend<MockLlmBackend>),
    Vllm(FallbackBackend<HttpVllmBackend>),
    Llama(FallbackBackend<HttpLlamaBackend>),
}

impl ServiceBackend {
    async fn parse(
        &self,
        parser: &mut IntentParser,
        raw_input: &str,
        session: &SessionContext,
    ) -> Result<ParseResult, cognos_intent_engine::parser::IntentError> {
        match self {
            Self::Mock(b) => parser.parse(raw_input, session, b).await,
            Self::Vllm(b) => parser.parse(raw_input, session, b).await,
            Self::Llama(b) => parser.parse(raw_input, session, b).await,
        }
    }

    fn stamps_vllm_source(&self) -> bool {
        matches!(self, Self::Vllm(_))
    }
}

/// `DispatchIntent` handler: owns the parser + backend and turns an incoming
/// [`Intent`] into an [`IntentResponse`] carrying an action graph.
struct IntentService {
    // The parser holds a KV cache and requires `&mut self`; a mutex serializes
    // access so the handler stays `Send + Sync` behind an `Arc`.
    parser: tokio::sync::Mutex<IntentParser>,
    backend: ServiceBackend,
}

#[async_trait::async_trait]
impl IntentHandler for IntentService {
    async fn handle(&self, intent: &Intent) -> IntentResponse {
        let trace_id = if intent.trace_id.is_empty() {
            uuid::Uuid::new_v4().to_string()
        } else {
            intent.trace_id.clone()
        };
        METRICS.record_intent_request();
        let started = std::time::Instant::now();

        // Prefer the free-text utterance; fall back to the canonical action.
        let raw_input = if !intent.utterance.is_empty() {
            intent.utterance.clone()
        } else {
            intent.action.clone()
        };
        let session = build_session(intent);

        let result = {
            let mut parser = self.parser.lock().await;
            self.backend.parse(&mut parser, &raw_input, &session).await
        };

        let parse_ms = started.elapsed().as_millis() as u64;
        let now_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0) as u64;
        match result {
            Ok(parsed) => {
                let mut schema = parsed.schema;
                if self.backend.stamps_vllm_source()
                    && schema.source.as_deref() != Some(KEYWORD_FALLBACK_SOURCE)
                {
                    schema.source = Some(VLLM_SOURCE.to_string());
                }
                if let Some(reason) = non_executable_reason(&schema.goal) {
                    tracing::info!(
                        trace_id = %trace_id,
                        goal = %schema.goal,
                        "non-executable goal blocked before dispatch"
                    );
                    return IntentResponse {
                        intent_id: if !intent.intent_id.is_empty() {
                            intent.intent_id.clone()
                        } else {
                            schema.intent_id.to_string()
                        },
                        status: "unsupported".to_string(),
                        result_json: serde_json::to_vec(&schema).unwrap_or_default(),
                        message: format!("{reason}: {}", schema.goal),
                        violation: None,
                        completed_at_ns: now_ns,
                        action_graph: None,
                        trace_id,
                    };
                }
                let source = schema.source.as_deref().unwrap_or("llm");
                tracing::info!(
                    trace_id = %trace_id,
                    stage = "parse_llm",
                    latency_ms = parse_ms,
                    cache_hit = parsed.cache_hit,
                    source = %source,
                    "pipeline stage"
                );
                let graph = ActionGraph::from_schema(&schema);
                let proto_graph = graph_to_proto(&graph, &schema);
                let intent_id = if !intent.intent_id.is_empty() {
                    intent.intent_id.clone()
                } else {
                    schema.intent_id.to_string()
                };
                let result_json = serde_json::to_vec(&schema).unwrap_or_default();
                IntentResponse {
                    intent_id,
                    status: "ok".to_string(),
                    result_json,
                    message: format!(
                        "parsed intent '{}' into {} action(s)",
                        schema.goal,
                        proto_graph.nodes.len()
                    ),
                    violation: None,
                    completed_at_ns: now_ns,
                    action_graph: Some(proto_graph),
                    trace_id,
                }
            }
            Err(e) => {
                tracing::info!(
                    trace_id = %trace_id,
                    stage = "parse_llm",
                    latency_ms = parse_ms,
                    error = %e,
                    "pipeline stage failed"
                );
                IntentResponse {
                    intent_id: intent.intent_id.clone(),
                    status: "failed".to_string(),
                    result_json: Vec::new(),
                    message: format!("intent parse failed: {e}"),
                    violation: None,
                    completed_at_ns: now_ns,
                    action_graph: None,
                    trace_id,
                }
            }
        }
    }
}

/// Derive a [`SessionContext`] for the request. If the caller embedded one in
/// `args_json` we honor it; otherwise we synthesize a minimal context stamped
/// with the current wall-clock time.
fn build_session(intent: &Intent) -> SessionContext {
    if !intent.args_json.is_empty() {
        if let Ok(ctx) = serde_json::from_slice::<SessionContext>(&intent.args_json) {
            return ctx;
        }
    }
    SessionContext {
        last_active_domain: None,
        last_active_files: Vec::new(),
        current_time: chrono::Utc::now().format("%H:%M").to_string(),
        time_since_last_session: None,
    }
}

/// Serialize an [`IntentSchema`] to its proto twin.
fn schema_to_proto(schema: &IntentSchema) -> IntentSchemaProto {
    IntentSchemaProto {
        intent_id: schema.intent_id.to_string(),
        raw_input: schema.raw_input.clone(),
        goal: schema.goal.clone(),
        domain: schema.domain.clone().unwrap_or_default(),
        confidence: schema.confidence as f64,
        ambiguity_score: schema.ambiguity_score as f64,
        risk_estimate: schema.risk_estimate as f64,
        required_context: schema.required_context.clone(),
        disambiguation_required: schema.disambiguation_required,
        disambiguation_question: schema.disambiguation_question.clone().unwrap_or_default(),
        hal_pre_score: schema.hal_pre_score as f64,
        escalate_to_cloud: schema.escalate_to_cloud,
        source: schema.source.clone().unwrap_or_default(),
    }
}

/// Serialize an [`ActionGraph`] (+ its originating schema) to proto. Nodes are
/// emitted in deterministic execution order.
fn graph_to_proto(graph: &ActionGraph, schema: &IntentSchema) -> IntentActionGraph {
    let ordered = graph.execution_order().unwrap_or_else(|_| graph.nodes());
    let nodes = ordered
        .iter()
        .map(|n| IntentActionNode {
            node_id: n.node_id.to_string(),
            intent_id: n.intent_id.to_string(),
            action: n.action.clone(),
            target: n.target.clone(),
            confidence: n.confidence as f64,
            hal_pre_score: n.hal_pre_score as f64,
        })
        .collect();
    let deps = graph
        .dependencies()
        .into_iter()
        .map(|(from, to)| IntentActionEdge {
            from_node: from.to_string(),
            to_node: to.to_string(),
        })
        .collect();
    IntentActionGraph {
        nodes,
        deps,
        intent: Some(schema_to_proto(schema)),
    }
}

/// Parse a very small subset of CLI args: `--config <path>`.
fn config_path_from_args() -> Option<String> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" | "-c" => return args.next(),
            other if other.starts_with("--config=") => {
                return Some(other["--config=".len()..].to_string());
            }
            _ => {}
        }
    }
    None
}

/// Build the backend stack. When `MOCK_LLM=1`, use the deterministic mock
/// backend. Otherwise vLLM (default) or legacy llama-server with keyword fallback.
fn build_backend(cfg: &IntentConfig) -> Result<ServiceBackend, String> {
    if std::env::var("MOCK_LLM")
        .ok()
        .filter(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .is_some()
    {
        tracing::info!("MOCK_LLM enabled — using deterministic mock inference backend");
        return Ok(ServiceBackend::Mock(FallbackBackend::new(MockLlmBackend::new())));
    }

    match cfg.backend {
        InferenceBackendKind::Llama => {
            let primary = match std::fs::read_to_string(&cfg.grammar_path) {
                Ok(grammar) => {
                    tracing::info!("loaded GBNF grammar from {}", cfg.grammar_path);
                    HttpLlamaBackend::with_grammar(
                        &cfg.llama_endpoint,
                        &cfg.model,
                        cfg.timeout,
                        grammar,
                    )?
                }
                Err(_) => {
                    tracing::info!(
                        "grammar file {} not found; using embedded grammar",
                        cfg.grammar_path
                    );
                    HttpLlamaBackend::new(&cfg.llama_endpoint, &cfg.model, cfg.timeout)?
                }
            };
            Ok(ServiceBackend::Llama(FallbackBackend::new(primary)))
        }
        InferenceBackendKind::Vllm => {
            tracing::info!(
                "vLLM backend: endpoint={} model={} schema={}",
                cfg.llama_endpoint,
                cfg.model,
                cfg.schema_path
            );
            let primary = HttpVllmBackend::with_schema_path(
                &cfg.llama_endpoint,
                &cfg.model,
                cfg.timeout,
                &cfg.schema_path,
            )?;
            Ok(ServiceBackend::Vllm(FallbackBackend::new(primary)))
        }
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let cfg = IntentConfig::load(config_path_from_args().as_deref());
    tracing::info!(
        "cognos-intent starting: inference={:?} endpoint={} model={} ipc_bind={} ipc_endpoint={}",
        cfg.backend,
        cfg.llama_endpoint,
        cfg.model,
        cfg.ipc_bind,
        cfg.ipc_endpoint
    );

    let backend = match build_backend(&cfg) {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("failed to build inference backend: {e}");
            std::process::exit(1);
        }
    };

    let service = Arc::new(IntentService {
        parser: tokio::sync::Mutex::new(IntentParser::new()),
        backend,
    });

    // 1. Serve DispatchIntent on our own endpoint so callers reach the real
    //    parser (mirrors HAL serving HalGate on its own bind).
    let bind_addr: SocketAddr = match cfg.ipc_bind.parse() {
        Ok(a) => a,
        Err(e) => {
            tracing::error!("invalid ipc.bind address '{}': {e}", cfg.ipc_bind);
            std::process::exit(1);
        }
    };
    let mut server_cfg = ServerConfig::default();
    server_cfg.bind_addr = cfg.ipc_bind.clone();
    server_cfg.self_capability = "intent.engine".to_string();
    let server = CognosServer::with_config(server_cfg).with_intent_handler(service);
    tokio::spawn(async move {
        if let Err(e) = server.serve(bind_addr).await {
            tracing::error!("intent DispatchIntent server exited with error: {e}");
        }
    });
    tracing::info!("intent DispatchIntent RPC serving on {bind_addr}");

    // 2. Register on the central IPC bus + heartbeat (non-fatal if the bus is
    //    down, matching the other services).
    let ipc = agent::spawn(AgentSpec {
        agent_id: AGENT_ID.to_string(),
        endpoint: cfg.ipc_endpoint.clone(),
        signing_secret: cfg.secret.clone(),
        capabilities: vec!["intent.parse".to_string(), "intent.dispatch".to_string()],
        heartbeat_interval: Duration::from_millis(DEFAULT_HEARTBEAT_INTERVAL_MS),
        max_failures: DEFAULT_MAX_FAILURES,
    })
    .await;

    // 3. Block until shutdown.
    tokio::signal::ctrl_c().await.ok();
    tracing::info!("cognos-intent shutting down");
    ipc.stop().await;
}
