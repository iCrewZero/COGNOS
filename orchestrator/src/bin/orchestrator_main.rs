//! Orchestrator binary — starts the COGNOS orchestrator runtime.



use std::sync::Arc;



use tracing_subscriber::EnvFilter;



use cognos_ipc_grpc::agent::{self, AgentSpec};

use cognos_ipc_grpc::client::{ClientConfig, CognosClient};

use cognos_orchestrator::runtime::{AgentDescriptor, AgentId, OrchestratorRuntime};

use cognos_orchestrator::serve;

use tokio::sync::Mutex;



/// Default endpoint of HAL's gate RPC (HAL binds `COGNOS_HAL_BIND`, default

/// `127.0.0.1:7444`). Overridable via `COGNOS_HAL_ENDPOINT`.

const DEFAULT_HAL_ENDPOINT: &str = "http://127.0.0.1:7444";

/// Default endpoint of the intent-engine's `DispatchIntent` server

/// (`COGNOS_INTENT_BIND`, default `127.0.0.1:7445`). Overridable via

/// `COGNOS_INTENT_ENDPOINT`.

const DEFAULT_INTENT_ENDPOINT: &str = "http://127.0.0.1:7445";

/// Default orchestrator ingress (`DispatchIntent` full pipeline).

const DEFAULT_ORCHESTRATOR_BIND: &str = "127.0.0.1:7446";



#[tokio::main]

async fn main() {

    tracing_subscriber::fmt()

        .with_env_filter(EnvFilter::new("info"))

        .with_target(false)

        .init();



    tracing::info!("cognos-orchestrator starting");



    let runtime = Arc::new(Mutex::new(OrchestratorRuntime::new().await));



    // Register local agents the executor can route to.

    {

        let mut rt = runtime.lock().await;

        rt.agents.register(AgentDescriptor {

            id: AgentId("agent.file".to_string()),

            name: "file".to_string(),

            capabilities: vec!["file.read".into(), "file.write".into()],

            available: true,

        });

        rt.agents.register(AgentDescriptor {

            id: AgentId("agent.coordinator".to_string()),

            name: "coordinator".to_string(),

            capabilities: vec![

                "general.execute".into(),

                "intent.disambiguate".into(),

                "memory.read".into(),

            ],

            available: true,

        });

    }



    let hal_endpoint = std::env::var("COGNOS_HAL_ENDPOINT")

        .ok()

        .filter(|s| !s.is_empty())

        .unwrap_or_else(|| DEFAULT_HAL_ENDPOINT.to_string());

    let mut hal_client = CognosClient::new(ClientConfig {

        agent_id: "agent.orchestrator".to_string(),

        signing_secret: std::env::var("COGNOS_IPC_SECRET").unwrap_or_default(),

        endpoint: hal_endpoint.clone(),

        max_reconnect_attempts: 1,

        ..ClientConfig::default()

    });

    match hal_client.connect(&hal_endpoint).await {

        Ok(()) => tracing::info!(endpoint = %hal_endpoint, "connected to HAL gate"),

        Err(e) => tracing::warn!(endpoint = %hal_endpoint, error = %e, "HAL gate unreachable at startup"),

    }

    runtime.lock().await.attach_hal_gate(hal_client);



    let intent_endpoint = std::env::var("COGNOS_INTENT_ENDPOINT")

        .ok()

        .filter(|s| !s.is_empty())

        .unwrap_or_else(|| DEFAULT_INTENT_ENDPOINT.to_string());

    let mut intent_client = CognosClient::new(ClientConfig {

        agent_id: "agent.orchestrator".to_string(),

        signing_secret: std::env::var("COGNOS_IPC_SECRET").unwrap_or_default(),

        endpoint: intent_endpoint.clone(),

        max_reconnect_attempts: 1,

        ..ClientConfig::default()

    });

    match intent_client.connect(&intent_endpoint).await {

        Ok(()) => tracing::info!(endpoint = %intent_endpoint, "connected to intent-engine"),

        Err(e) => tracing::warn!(

            endpoint = %intent_endpoint,

            error = %e,

            "intent-engine unreachable at startup — will use keyword fallback on submit"

        ),

    }

    runtime.lock().await.attach_intent_client(intent_client);



    let orch_bind = std::env::var("COGNOS_ORCHESTRATOR_BIND")

        .ok()

        .filter(|s| !s.is_empty())

        .unwrap_or_else(|| DEFAULT_ORCHESTRATOR_BIND.to_string());

    let ingress_rt = Arc::clone(&runtime);

    let bind_for_server = orch_bind.clone();

    tokio::spawn(async move {

        if let Err(e) = serve::serve_ingress(ingress_rt, &bind_for_server).await {

            tracing::error!(error = %e, "orchestrator ingress server exited");

        }

    });

    tracing::info!(bind = %orch_bind, "orchestrator ingress ready (DispatchIntent → execute)");



    let ipc = agent::spawn(AgentSpec::from_env(

        "agent.orchestrator",

        vec![

            "intent.dispatch".to_string(),

            "task.orchestrate".to_string(),

            "memory.query".to_string(),

            "agent.coordinate".to_string(),

        ],

    ))

    .await;



    tracing::info!("cognos-orchestrator ready");



    tokio::signal::ctrl_c().await.ok();



    ipc.stop().await;

    tracing::info!("cognos-orchestrator stopped");

}


