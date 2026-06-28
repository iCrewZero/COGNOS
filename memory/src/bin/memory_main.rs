// COGNOS Memory Service binary entrypoint.
// v0: stub — prints startup info and exits.
// Owner: iCrewZero
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new("info"))
        .with_target(false)
        .init();

    tracing::info!("cognos-memory starting (v0 stub — no persistence yet)");
    // v1: open the JSONL store, start the embedder, serve gRPC.
    tokio::signal::ctrl_c().await.ok();
    tracing::info!("cognos-memory stopped");
}