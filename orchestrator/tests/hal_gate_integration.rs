//! End-to-end HAL gating: orchestrator → IPC → HAL policy → decision.
//!
//! Fixtures: the central `cognos-ipc-server` and the `cognos-hal` binary are
//! both started as child processes. The orchestrator opens a HAL gate client
//! and submits two actions:
//!   (a) a benign file open        → expected `Granted`
//!   (b) a delete under a dangerous path → expected `ApprovalRequired` or `Denied`
//!
//! The assertions prove the request reached HAL's *real* policy (the central
//! server's misroute stub answers `failed` with an explicit message — a
//! `Granted` on (a) can only come from HAL on :7444).
//!
//! Binaries are located from `CARGO_BIN_EXE_*`-style discovery: env override,
//! then the sibling of the test binary in `target/<profile>/`. If they cannot
//! be found or built, the test skips (it is a no-op in environments without a
//! compiled workspace); CI compiles the binaries first, so it runs there.

use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::Duration;

use cognos_ipc_grpc::client::{ClientConfig, CognosClient};
use cognos_orchestrator::hal_gate::{gate_action, Decision, SideEffect};

/// Kills a spawned fixture process when dropped.
struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Grab a free TCP port on localhost by binding to :0 and releasing it.
fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    listener.local_addr().expect("local addr").port()
}

/// Locate a workspace binary: env override first, then the sibling of the test
/// executable in `target/<profile>/`.
fn locate_bin(name: &str, env_var: &str) -> Option<PathBuf> {
    let file_name = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };

    if let Ok(p) = std::env::var(env_var) {
        let pb = PathBuf::from(p);
        if pb.is_file() {
            return Some(pb);
        }
    }

    // current_exe: <target>/<profile>/deps/<test>-<hash>[.exe]
    let mut dir = std::env::current_exe().ok()?;
    dir.pop(); // deps
    if dir.file_name().map(|f| f == "deps").unwrap_or(false) {
        dir.pop(); // <profile>
    }
    let candidate = dir.join(&file_name);
    if candidate.is_file() {
        return Some(candidate);
    }
    None
}

/// Try to build the required binaries if they are missing (local convenience).
fn try_build_bins() {
    // One invocation — repeated `-p`/`--bin` pairs only honor the last package.
    let _ = Command::new(env!("CARGO"))
        .args([
            "build",
            "-p",
            "cognos-hal",
            "-p",
            "cognos-ipc-grpc",
            "--bins",
        ])
        .status();
}

fn resolve_bins() -> Option<(PathBuf, PathBuf)> {
    if let (Some(ipc), Some(hal)) = (
        locate_bin("cognos-ipc-server", "COGNOS_IPC_SERVER_BIN"),
        locate_bin("cognos-hal", "COGNOS_HAL_BIN"),
    ) {
        return Some((ipc, hal));
    }
    try_build_bins();
    match (
        locate_bin("cognos-ipc-server", "COGNOS_IPC_SERVER_BIN"),
        locate_bin("cognos-hal", "COGNOS_HAL_BIN"),
    ) {
        (Some(ipc), Some(hal)) => Some((ipc, hal)),
        _ => None,
    }
}

/// Retry-connect a client to `endpoint` until it comes up or we give up.
async fn connect_with_wait(endpoint: &str) -> Option<CognosClient> {
    for _ in 0..60 {
        let mut client = CognosClient::new(ClientConfig {
            agent_id: "agent.orchestrator".to_string(),
            endpoint: endpoint.to_string(),
            max_reconnect_attempts: 1,
            request_timeout_ms: 2_000,
            ..ClientConfig::default()
        });
        if client.connect(endpoint).await.is_ok() {
            return Some(client);
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    None
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn orchestrator_gates_actions_through_hal() {
    let Some((ipc_bin, hal_bin)) = resolve_bins() else {
        eprintln!(
            "SKIP orchestrator_gates_actions_through_hal: \
             cognos-ipc-server / cognos-hal binaries not found (set COGNOS_IPC_SERVER_BIN / \
             COGNOS_HAL_BIN or build the workspace)."
        );
        return;
    };

    let ipc_port = free_port();
    let hal_port = free_port();
    let ipc_bind = format!("127.0.0.1:{ipc_port}");
    let hal_bind = format!("127.0.0.1:{hal_port}");
    let hal_endpoint = format!("http://{hal_bind}");
    let ipc_endpoint = format!("http://{ipc_bind}");

    // Fixture 1: the central IPC server.
    let _ipc = ChildGuard(
        Command::new(&ipc_bin)
            .env("COGNOS_IPC_BIND", &ipc_bind)
            .env("RUST_LOG", "warn")
            .spawn()
            .expect("spawn cognos-ipc-server"),
    );

    // Fixture 2: HAL — serves the real HalGate policy on its own endpoint and
    // registers with the central server.
    let _hal = ChildGuard(
        Command::new(&hal_bin)
            .env("COGNOS_HAL_BIND", &hal_bind)
            .env("COGNOS_IPC_ENDPOINT", &ipc_endpoint)
            .env("RUST_LOG", "warn")
            .spawn()
            .expect("spawn cognos-hal"),
    );

    // The orchestrator's HAL gate client.
    let client = connect_with_wait(&hal_endpoint)
        .await
        .expect("HAL gate endpoint never came up");

    // (a) Benign action: open a file in the user's home → HAL must grant.
    let benign = SideEffect::new(
        "file.open",
        "~/projects/notes.txt",
        "file.read",
        "agent.orchestrator",
    );
    let decision_a = gate_action(&client, &benign, "test-trace-a")
        .await
        .expect("gate_action(benign) failed");
    assert!(
        matches!(decision_a, Decision::Granted { .. }),
        "benign open must be granted by HAL, got {decision_a:?}"
    );

    // (b) Delete under a dangerous system path → HAL must NOT grant; it must
    //     require approval or deny outright, per HAL's existing rules.
    let dangerous = SideEffect::new(
        "file.delete",
        "/etc/passwd",
        "file.delete",
        "agent.orchestrator",
    );
    let decision_b = gate_action(&client, &dangerous, "test-trace-b")
        .await
        .expect("gate_action(dangerous delete) failed");
    assert!(
        matches!(
            decision_b,
            Decision::ApprovalRequired { .. } | Decision::Denied { .. }
        ),
        "delete under a dangerous path must be gated (approval_required or denied), got {decision_b:?}"
    );
    assert!(
        !decision_b.is_granted(),
        "delete under a dangerous path must never be granted"
    );
}
