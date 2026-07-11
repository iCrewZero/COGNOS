"""End-to-end IPC round-trip test against the real Rust CognosIpc server.

Flow under test (the coordinator's remote path):
    AgentIpcClient.connect()  → channel up + Heartbeat registration
    AgentIpcClient.query_memory(...) → QueryMemory RPC → Rust memory responder
    assert on the returned hit content (proves the request reached the real
    server and came back — a client-side fallback stub can't reproduce it).

The Rust server binary (`cognos-ipc-server`) is started as a pytest fixture.
If the binary can't be located or built, the server-dependent tests SKIP
(they never fail the suite). A separate fallback test runs without a server
to prove the degraded path still works.

Locating the binary (first match wins):
    1. $COGNOS_IPC_SERVER_BIN — explicit path to the built binary.
    2. $CARGO_TARGET_DIR/{debug,release}/cognos-ipc-server[.exe]
    3. <repo>/ipc/grpc/target/{debug,release}/... and <repo>/target/...
    4. `cargo build --bin cognos-ipc-server` (best-effort; skipped if cargo
       is absent or the build fails).
"""
from __future__ import annotations

import asyncio
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import time

import pytest

AGENTS_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
REPO_ROOT = os.path.dirname(AGENTS_DIR)
MANIFEST = os.path.join(REPO_ROOT, "ipc", "grpc", "Cargo.toml")
BIN_NAME = "cognos-ipc-server"

if AGENTS_DIR not in sys.path:
    sys.path.insert(0, AGENTS_DIR)

from shared.ipc import AgentIpcClient  # noqa: E402


# ─── Binary discovery / build ────────────────────────────────────────────────

def _exe(name: str) -> str:
    return name + (".exe" if os.name == "nt" else "")


def _candidate_target_dirs() -> list[str]:
    dirs: list[str] = []
    env_target = os.environ.get("CARGO_TARGET_DIR")
    if env_target:
        dirs.append(env_target)
    dirs.append(os.path.join(REPO_ROOT, "ipc", "grpc", "target"))
    dirs.append(os.path.join(REPO_ROOT, "target"))
    return dirs


def _find_prebuilt() -> str | None:
    explicit = os.environ.get("COGNOS_IPC_SERVER_BIN")
    if explicit and os.path.isfile(explicit):
        return explicit
    for target in _candidate_target_dirs():
        for profile in ("debug", "release"):
            candidate = os.path.join(target, profile, _exe(BIN_NAME))
            if os.path.isfile(candidate):
                return candidate
    return None


def _try_build() -> tuple[str | None, str]:
    """Best-effort `cargo build`. Returns (binary_path_or_None, log)."""
    cargo = shutil.which("cargo")
    if not cargo:
        return None, "cargo not found on PATH"
    if not os.path.isfile(MANIFEST):
        return None, f"manifest not found: {MANIFEST}"
    try:
        proc = subprocess.run(
            [cargo, "build", "--bin", BIN_NAME, "--manifest-path", MANIFEST],
            capture_output=True,
            text=True,
            timeout=900,
        )
    except (subprocess.TimeoutExpired, OSError) as e:
        return None, f"cargo build failed to run: {e}"
    if proc.returncode != 0:
        return None, f"cargo build exited {proc.returncode}:\n{proc.stderr[-2000:]}"
    return _find_prebuilt(), "built"


def _free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def _wait_for_port(host: str, port: int, timeout: float) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            with socket.create_connection((host, port), timeout=0.5):
                return True
        except OSError:
            time.sleep(0.1)
    return False


# ─── Server fixture ──────────────────────────────────────────────────────────

@pytest.fixture(scope="module")
def ipc_endpoint():
    """Start the real Rust IPC server on a free port; yield its endpoint.

    Skips (never fails) when the binary is neither prebuilt nor buildable.
    """
    binary = _find_prebuilt()
    if binary is None:
        binary, log = _try_build()
        if binary is None:
            pytest.skip(f"cognos-ipc-server binary unavailable — {log}")

    port = _free_port()
    host = "127.0.0.1"
    endpoint = f"{host}:{port}"

    env = os.environ.copy()
    env["COGNOS_IPC_BIND"] = endpoint

    log_file = tempfile.NamedTemporaryFile(
        prefix="cognos-ipc-server-", suffix=".log", delete=False, mode="w+",
    )
    proc = subprocess.Popen(
        [binary],
        env=env,
        stdout=log_file,
        stderr=subprocess.STDOUT,
    )

    try:
        if not _wait_for_port(host, port, timeout=20.0):
            proc.terminate()
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()
            log_file.flush()
            log_file.seek(0)
            output = log_file.read()
            pytest.fail(
                f"cognos-ipc-server did not open {endpoint} within 20s.\n"
                f"binary: {binary}\n--- server log ---\n{output}"
            )
        yield endpoint
    finally:
        if proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()
        log_file.close()
        try:
            os.unlink(log_file.name)
        except OSError:
            pass


# ─── Tests: real server round-trip ───────────────────────────────────────────

def test_connect_and_register(ipc_endpoint):
    """connect() brings the channel up and registers via a Heartbeat."""
    async def run():
        client = AgentIpcClient(
            "agent.coordinator",
            endpoint=ipc_endpoint,
            connect_timeout=5.0,
            rpc_timeout=5.0,
            max_failures=5,
        )
        await client.connect()
        try:
            assert client.is_connected
            assert not client.in_fallback_mode
            # A direct heartbeat also round-trips and the server answers "ok".
            hb = await client.heartbeat(status="alive")
            assert hb["status"] == "ok"
        finally:
            await client.close()

    asyncio.run(run())


def test_query_memory_roundtrip(ipc_endpoint):
    """coordinator → QueryMemory → memory responder → assert on content."""
    query = "find my rust notes"

    async def run():
        client = AgentIpcClient(
            "agent.coordinator",
            endpoint=ipc_endpoint,
            connect_timeout=5.0,
            rpc_timeout=5.0,
            max_failures=5,
        )
        await client.connect()
        assert not client.in_fallback_mode, "must use the real server, not fallback"
        try:
            return await client.query_memory(
                query=query,
                tags=["project:cognos", "kind:note"],
                namespace="notes",
                top_k=5,
            )
        finally:
            await client.close()

    result = asyncio.run(run())

    # The real server returns a deterministic echo hit; the fallback returns
    # an empty result — so a populated, query-derived hit proves the RPC hit
    # the live server end-to-end.
    assert result["total"] == 1
    assert len(result["hits"]) == 1

    hit = result["hits"][0]
    assert hit["object_id"] == f"echo:{query}"
    assert hit["score"] == pytest.approx(1.0)
    assert hit["payload"]["echo"] == query
    assert hit["payload"]["namespace"] == "notes"
    assert "project:cognos" in hit["tags"]
    assert "kind:note" in hit["tags"]
    assert result["trace_id"], "server must echo the request trace_id"


def test_coordinator_send_routes_to_memory(ipc_endpoint):
    """The coordinator's high-level send() routes MEMORY_QUERY to the server."""
    async def run():
        client = AgentIpcClient(
            "agent.coordinator",
            endpoint=ipc_endpoint,
            connect_timeout=5.0,
            rpc_timeout=5.0,
            max_failures=5,
        )
        await client.connect()
        try:
            return await client.send(
                "memory", "MEMORY_QUERY",
                {"query": "hello memory", "namespace": "sess", "tags": ["t1"]},
            )
        finally:
            await client.close()

    result = asyncio.run(run())
    assert result["total"] == 1
    hit = result["hits"][0]
    assert hit["payload"]["echo"] == "hello memory"
    assert hit["payload"]["namespace"] == "sess"


# ─── Test: fallback path still works with NO server ──────────────────────────

def test_fallback_when_server_absent():
    """No server → connect() degrades to fallback; RPCs return stub shapes.

    This runs without the fixture and must not raise: it proves the existing
    fallback mode is preserved.
    """
    dead_port = _free_port()  # nothing is listening here

    async def run():
        client = AgentIpcClient(
            "agent.coordinator",
            endpoint=f"127.0.0.1:{dead_port}",
            connect_timeout=1.0,
            rpc_timeout=1.0,
            max_failures=2,
        )
        await client.connect()
        try:
            assert client.in_fallback_mode, "connect() should degrade to fallback"
            result = await client.query_memory(query="anything")
            return result
        finally:
            await client.close()

    result = asyncio.run(run())
    assert result["total"] == 0
    assert result["hits"] == []
    assert "trace_id" in result
