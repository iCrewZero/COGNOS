//! COGNOS intent engine binary entrypoint.
//!
//! Loads config, starts the gRPC IPC client, and begins accepting
//! intent parsing requests.
//!
//! Owner: iCrewZero

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    tracing::info!("cognos-intent starting (v0 stub)");
    // TODO(v1): load intent.toml, connect to IPC server, serve intents
}
