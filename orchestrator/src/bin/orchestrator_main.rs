//! Orchestrator binary — starts the COGNOS orchestrator runtime.
//!
//! In v1 this will:
//!   1. Connect to the IPC server
//!   2. Register as an agent
//!   3. Accept intents via DispatchIntent RPC or stdin
//!   4. Run the DAG execution loop
//!
//! Owner: iCrewZero

use tracing_subscriber::EnvFilter;

use cognos_orchestrator::OrchestratorRuntime;

#[tokio::main]
async fn main() {
    // Set up logging so we can see what the orchestrator is doing.
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new("info"))
        .with_target(false)
        .init();

    tracing::info!("cognos-orchestrator starting");

    // Create the runtime — this sets up the task graph, event bus,
    // scheduler, and agent registry.
    let mut runtime = OrchestratorRuntime::new().await;

    // In v1, we would:
    //   1. Connect to the IPC gRPC server
    //   2. Subscribe to StreamEvents for incoming intents
    //   3. Register our capabilities with the agent registry
    //   4. Enter the dispatch loop: pop ready tasks, send to agents, collect results
    //
    // For now, just log that we started and wait for ctrl-c.
    tracing::info!("cognos-orchestrator ready (v0: no IPC connection yet)");

    // Block until ctrl-c or SIGTERM.
    tokio::signal::ctrl_c().await.ok();

    runtime.shutdown().await.ok();
    tracing::info!("cognos-orchestrator stopped");
}
