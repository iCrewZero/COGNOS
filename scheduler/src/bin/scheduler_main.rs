//! Scheduler binary — starts the COGNOS adaptive resource scheduler.
//!
//! In v1 this will:
//!   1. Connect to the IPC server
//!   2. Start the eBPF telemetry reader
//!   3. Run the 1Hz control loop: sample → predict → apply policy
//!
//! Owner: iCrewZero

use tracing_subscriber::EnvFilter;

use cognos_ipc_grpc::agent::{self, AgentSpec};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new("info"))
        .with_target(false)
        .init();

    tracing::info!("cognos-scheduler starting");

    // Register with the central IPC server as an agent and keep a heartbeat
    // alive on a background task (address/secret from the service environment).
    let ipc = agent::spawn(AgentSpec::from_env(
        "agent.scheduler",
        vec![
            "resource.telemetry".to_string(),
            "resource.policy".to_string(),
            "sched.hint".to_string(),
        ],
    ))
    .await;

    tracing::info!("cognos-scheduler ready");

    tokio::signal::ctrl_c().await.ok();

    ipc.stop().await;
    tracing::info!("cognos-scheduler stopped");
}