//! Scheduler binary — starts the COGNOS adaptive resource scheduler.
//!
//! In v1 this will:
//!   1. Connect to the IPC server
//!   2. Start the eBPF telemetry reader
//!   3. Run the 1Hz control loop: sample → predict → apply policy
//!
//! Owner: iCrewZero

use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new("info"))
        .with_target(false)
        .init();

    tracing::info!("cognos-scheduler starting");

    // In v1, we would create a SchedulerDaemon, connect to IPC,
    // and start the telemetry + control loop. For now we just
    // log readiness and wait for ctrl-c.
    tracing::info!("cognos-scheduler ready (v0: no eBPF telemetry yet)");

    tokio::signal::ctrl_c().await.ok();
    tracing::info!("cognos-scheduler stopped");
}