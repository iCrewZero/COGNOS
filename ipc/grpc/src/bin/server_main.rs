//! IPC server binary — starts the COGNOS gRPC endpoint.

use std::net::SocketAddr;

use tracing_subscriber::EnvFilter;

use cognos_ipc_grpc::runtime::IpcRuntime;
use cognos_ipc_grpc::server::ServerConfig;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new("info"))
        .with_target(false)
        .init();

    let mut config = ServerConfig::default();
    // Allow the bind address to be overridden from the environment so tests
    // (and deployments) can pick a free port instead of the fixed default.
    if let Ok(bind) = std::env::var("COGNOS_IPC_BIND") {
        if !bind.is_empty() {
            config.bind_addr = bind;
        }
    }
    let addr: SocketAddr = config.bind_addr.parse().expect("invalid bind address");

    let mut runtime = IpcRuntime::with_server_config(config);
    if let Err(e) = runtime.start(addr).await {
        eprintln!("IPC runtime failed to start: {e}");
        std::process::exit(1);
    }

    // Block until ctrl-c.
    tokio::signal::ctrl_c().await.ok();
    runtime.shutdown().await.ok();
}
