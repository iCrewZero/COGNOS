"""
shared/ipc.py — Python gRPC client for the COGNOS IPC server.

Wraps the generated cognos_pb2 stubs so Python agents (coordinator,
coding agent, memory agent) can talk to the Rust IPC server.

Connection model
────────────────
`connect()` opens the channel, waits for the transport to come up, and
"registers" the agent with a Heartbeat round-trip. Both the connect and
every RPC are guarded by:

  * a per-RPC timeout (config / $COGNOS_IPC_TIMEOUT, default 5s);
  * exponential backoff + jitter on reconnect (base 0.5s, cap 10s);
  * an explicit switch to FALLBACK mode after `max_failures`
    (config / $COGNOS_IPC_MAX_FAILURES, default 3), logged at WARNING.

Fallback mode returns the same response shape agent code expects, so the
pipeline keeps working without a live server — it just proves nothing.
"""

from __future__ import annotations

import asyncio
import json
import logging
import os
import random
import time
import uuid
from typing import Optional

import grpc

try:
    from auth import create_token, resolve_secret, DEFAULT_TOKEN_TTL_S
except ImportError:  # pragma: no cover — depends on how agents/ is on sys.path
    from agents.auth import create_token, resolve_secret, DEFAULT_TOKEN_TTL_S

# The generated stubs live in the `proto` package after running:
#   python agents/generate_proto.py   (or `make proto`)
# They are never committed by hand — always generated. Detect them once at
# import time; if they are missing, the client runs in fallback mode so agent
# code keeps working (but the real gRPC contract is not exercised).
try:
    from proto.cognos_pb2_grpc import CognosIpcStub  # type: ignore
    from proto import cognos_pb2 as _pb  # type: ignore
    _STUBS_AVAILABLE = True
except ImportError:  # pragma: no cover
    try:
        from agents.proto.cognos_pb2_grpc import CognosIpcStub  # type: ignore
        from agents.proto import cognos_pb2 as _pb  # type: ignore
        _STUBS_AVAILABLE = True
    except ImportError:
        CognosIpcStub = None  # type: ignore
        _pb = None  # type: ignore
        _STUBS_AVAILABLE = False

log = logging.getLogger("cognos.ipc")

# ─── Configuration defaults (overridable via constructor args or env) ─────────
DEFAULT_ENDPOINT = "localhost:7443"
DEFAULT_RPC_TIMEOUT_S = 5.0
DEFAULT_CONNECT_TIMEOUT_S = 3.0
DEFAULT_MAX_FAILURES = 3
BACKOFF_BASE_S = 0.5
BACKOFF_MAX_S = 10.0

ENDPOINT_ENV = "COGNOS_IPC_ENDPOINT"
TIMEOUT_ENV = "COGNOS_IPC_TIMEOUT"
MAX_FAILURES_ENV = "COGNOS_IPC_MAX_FAILURES"


class IpcClientError(Exception):
    """Raised when an IPC call fails."""


class AgentIpcClient:
    """
    Python-side gRPC client for the COGNOS IPC server.

    Usage:
        client = AgentIpcClient("agent.coordinator", endpoint="localhost:7443")
        await client.connect()
        response = await client.query_memory(query="...")
        await client.close()
    """

    def __init__(
        self,
        agent_id: str,
        endpoint: Optional[str] = None,
        signing_secret: str = "",
        *,
        rpc_timeout: Optional[float] = None,
        connect_timeout: Optional[float] = None,
        max_failures: Optional[int] = None,
    ):
        self.agent_id = agent_id
        # Address comes from the explicit arg, then $COGNOS_IPC_ENDPOINT, then
        # the built-in default — matching the Rust ServerConfig bind address.
        self.endpoint = endpoint or os.environ.get(ENDPOINT_ENV) or DEFAULT_ENDPOINT
        self.signing_secret = signing_secret

        # Per-RPC deadline and the connect reachability probe deadline.
        self.rpc_timeout = float(
            rpc_timeout if rpc_timeout is not None
            else os.environ.get(TIMEOUT_ENV, DEFAULT_RPC_TIMEOUT_S)
        )
        self.connect_timeout = float(
            connect_timeout if connect_timeout is not None else DEFAULT_CONNECT_TIMEOUT_S
        )
        # How many connect/reconnect attempts before falling back.
        self.max_failures = int(
            max_failures if max_failures is not None
            else os.environ.get(MAX_FAILURES_ENV, DEFAULT_MAX_FAILURES)
        )

        # HMAC-SHA256 session-token auth, matching the Rust IPC auth module
        # (ipc/grpc/src/auth.rs). The secret is resolved from the explicit
        # arg or $COGNOS_IPC_SECRET; see agents/auth.py. Every outgoing RPC
        # carries a freshly-minted (and cached) token in its gRPC metadata.
        self._secret = resolve_secret(signing_secret)
        self._token: Optional[str] = None
        self._token_expiry: int = 0

        self._channel: Optional[grpc.aio.Channel] = None
        self._stub = None
        self._fallback_mode = False
        self._registered = False
        self._seq = 0

    # ─── Connection lifecycle ─────────────────────────────────────────────

    async def connect(self) -> None:
        """Open the channel, register the agent, and enter live mode.

        Retries with exponential backoff + jitter. After `max_failures`
        attempts it switches to fallback mode (logged WARNING) instead of
        raising, so agent code keeps working without a live server.
        """
        if not _STUBS_AVAILABLE:
            log.warning(
                "[%s] proto stubs not compiled — running in FALLBACK mode",
                self.agent_id,
            )
            self._fallback_mode = True
            return
        await self._establish(context="connect")

    async def close(self) -> None:
        await self._close_channel()
        log.info("[%s] Disconnected from IPC", self.agent_id)

    @property
    def is_connected(self) -> bool:
        return self._channel is not None and not self._fallback_mode

    @property
    def in_fallback_mode(self) -> bool:
        return self._fallback_mode

    async def _establish(self, context: str) -> None:
        """(Re)open channel + register, retrying with backoff.

        On success, clears fallback mode. If every attempt fails, switches
        to fallback mode and logs a WARNING (the explicit degrade path).
        """
        last_err: Optional[Exception] = None
        for attempt in range(1, self.max_failures + 1):
            try:
                await self._close_channel()
                await self._open_channel()
                await self._register()
                self._fallback_mode = False
                log.info(
                    "[%s] IPC %s ok at %s (attempt %d/%d)",
                    self.agent_id, context, self.endpoint, attempt, self.max_failures,
                )
                return
            except Exception as e:  # connection or registration failure
                last_err = e
                log.warning(
                    "[%s] IPC %s attempt %d/%d failed: %s",
                    self.agent_id, context, attempt, self.max_failures, e,
                )
                if attempt < self.max_failures:
                    await asyncio.sleep(self._backoff_delay(attempt))

        await self._close_channel()
        self._fallback_mode = True
        log.warning(
            "[%s] IPC unreachable after %d attempts (%s) — switching to FALLBACK mode",
            self.agent_id, self.max_failures, last_err,
        )

    async def _open_channel(self) -> None:
        # gRPC's name resolver expects a bare "host:port" authority (or a
        # scheme it understands like dns:/ipv4:). A leading http(s):// makes it
        # try to DNS-resolve the literal string and fail, so strip it here.
        target = self.endpoint
        for scheme in ("http://", "https://"):
            if target.startswith(scheme):
                target = target[len(scheme):]
                break
        self._channel = grpc.aio.insecure_channel(target)
        self._stub = CognosIpcStub(self._channel)
        # Reachability is probed by the registration RPC (`_register`), which
        # carries a `connect_timeout` deadline. We intentionally do NOT block
        # on `channel.channel_ready()` here: on a refused endpoint that future
        # can hang under the asyncio C-core loop even when wrapped in wait_for,
        # whereas a deadline'd unary RPC fails fast with UNAVAILABLE.

    async def _close_channel(self) -> None:
        if self._channel is not None:
            try:
                await self._channel.close()
            except Exception:  # pragma: no cover — best-effort teardown
                pass
        self._channel = None
        self._stub = None
        self._registered = False

    async def _register(self) -> None:
        """Announce this agent to the server via a Heartbeat round-trip.

        The server echoes an "ok" Heartbeat; a successful exchange means the
        channel is up and the session token was accepted. Raises on failure
        so `_establish` can retry / fall back.
        """
        resp = await self._invoke_stub(
            "Heartbeat",
            {
                "agent_id": self.agent_id,
                "seq": self._next_seq(),
                "sent_at_ns": self._timestamp_ns(),
                "load_avg": 0.0,
                "status": "register",
            },
            self._next_trace_id(),
            timeout=self.connect_timeout,
        )
        self._registered = True
        log.info(
            "[%s] registered with IPC server (peer=%s)",
            self.agent_id, resp.get("agent_id", "?"),
        )

    def _backoff_delay(self, attempt: int) -> float:
        """Exponential backoff (base 0.5s, cap 10s) with equal jitter.

        `attempt` is 1-based. Returns a delay in [raw/2, raw] seconds.
        """
        raw = min(BACKOFF_MAX_S, BACKOFF_BASE_S * (2 ** (attempt - 1)))
        half = raw / 2.0
        return half + random.uniform(0.0, half)

    # ─── Small helpers ────────────────────────────────────────────────────

    def _next_trace_id(self) -> str:
        return str(uuid.uuid4())

    def _next_seq(self) -> int:
        self._seq += 1
        return self._seq

    def _timestamp_ns(self) -> int:
        return int(time.time() * 1e9)

    def _session_token(self) -> str:
        """Return a cached HMAC session token, refreshing before it expires.

        Uses the same construction as Rust `auth::create_token`, so the token
        verifies with the server's `auth::verify_token`.
        """
        now = int(time.time())
        # Refresh ~60s early to avoid a token expiring mid-flight.
        if self._token is None or self._token_expiry - 60 <= now:
            self._token_expiry = now + DEFAULT_TOKEN_TTL_S
            self._token = create_token(self.agent_id, self._token_expiry, self._secret)
        return self._token

    def _auth_metadata(self) -> tuple:
        """gRPC metadata carrying the agent identity and its session token."""
        return (
            ("x-cognos-agent", self.agent_id),
            ("x-cognos-token", self._session_token()),
        )

    # ─── Public RPC surface ───────────────────────────────────────────────

    async def dispatch_intent(
        self,
        intent_id: str,
        action: str,
        utterance: str = "",
        args: dict | None = None,
        confidence: float = 0.0,
        requires: list[str] | None = None,
        session_id: str = "",
    ) -> dict:
        """
        Send a DispatchIntent RPC.

        Returns a dict with keys: intent_id, status, message, result_json.
        """
        trace_id = self._next_trace_id()
        payload = {
            "intent_id": intent_id,
            "utterance": utterance,
            "action": action,
            "args_json": json.dumps(args or {}).encode(),
            "confidence": confidence,
            "requires": requires or [],
            "session_id": session_id,
            "deadline_ns": 0,
            "trace_id": trace_id,
        }
        return await self._call_rpc("DispatchIntent", payload, trace_id)

    async def query_memory(
        self,
        query: str,
        tags: list[str] | None = None,
        top_k: int = 10,
        min_score: float = 0.0,
        namespace: str = "",
    ) -> dict:
        """
        Send a QueryMemory RPC.

        Returns a dict with keys: hits (list), total, elapsed_ns, trace_id.
        """
        trace_id = self._next_trace_id()
        payload = {
            "query": query,
            "tags": tags or [],
            "top_k": top_k,
            "min_score": min_score,
            "namespace": namespace,
            "trace_id": trace_id,
        }
        return await self._call_rpc("QueryMemory", payload, trace_id)

    async def hal_gate(
        self,
        op: str,
        device: str = "",
        data: bytes = b"",
        capability: str = "",
        allow_approval: bool = True,
    ) -> dict:
        """
        Send a HalGate RPC.

        Returns a dict with keys: status, grant_token, risk_score, violation.
        """
        trace_id = self._next_trace_id()
        payload = {
            "op": op,
            "device": device,
            "data": data,
            "capability": capability,
            "risk_override": -1.0,
            "allow_approval": allow_approval,
            "trace_id": trace_id,
        }
        return await self._call_rpc("HalGate", payload, trace_id)

    async def heartbeat(self, status: str = "alive", load_avg: float = 0.0) -> dict:
        """Send a Heartbeat RPC."""
        payload = {
            "agent_id": self.agent_id,
            "seq": self._next_seq(),
            "sent_at_ns": self._timestamp_ns(),
            "load_avg": load_avg,
            "status": status,
        }
        return await self._call_rpc("Heartbeat", payload, self._next_trace_id())

    # ─── RPC plumbing ─────────────────────────────────────────────────────

    async def _call_rpc(self, method: str, payload: dict, trace_id: str) -> dict:
        """
        Dispatch one RPC over the real stub, with reconnect + fallback.

        Order of handling:
          1. If already in fallback mode (or stubs missing) → fallback shape.
          2. Otherwise invoke the stub with the per-RPC timeout.
          3. On a transient gRPC/timeout error, reconnect (backoff + re-register)
             and retry once. If reconnect exhausts `max_failures`, degrade to
             fallback. If the retry still fails, raise IpcClientError.
        """
        if self._fallback_mode or not _STUBS_AVAILABLE:
            if not _STUBS_AVAILABLE and not self._fallback_mode:
                log.warning(
                    "[%s] proto stubs not compiled — FALLBACK for %s",
                    self.agent_id, method,
                )
                self._fallback_mode = True
            return self._fallback_response(method, payload, trace_id)

        if self._channel is None or self._stub is None:
            raise IpcClientError("not connected — call connect() first")

        try:
            return await self._invoke_stub(method, payload, trace_id)
        except (grpc.aio.AioRpcError, asyncio.TimeoutError) as e:
            detail = e.details() if isinstance(e, grpc.aio.AioRpcError) else f"timeout>{self.rpc_timeout}s"
            log.warning(
                "[%s] %s failed (%s) — attempting reconnect", self.agent_id, method, detail,
            )
            await self._establish(context="reconnect")
            if self._fallback_mode:
                return self._fallback_response(method, payload, trace_id)
            try:
                return await self._invoke_stub(method, payload, trace_id)
            except (grpc.aio.AioRpcError, asyncio.TimeoutError) as e2:
                detail2 = (
                    e2.details() if isinstance(e2, grpc.aio.AioRpcError)
                    else f"timeout>{self.rpc_timeout}s"
                )
                log.error(
                    "[%s] %s failed after reconnect: %s", self.agent_id, method, detail2,
                )
                raise IpcClientError(f"{method} RPC error: {detail2}") from e2
        except Exception as e:
            log.error("[%s] %s unexpected error: %s", self.agent_id, method, e)
            raise IpcClientError(f"{method} error: {e}") from e

    async def _invoke_stub(
        self, method: str, payload: dict, trace_id: str, timeout: Optional[float] = None,
    ) -> dict:
        """Raw stub call with a deadline. Raises on any error.

        `timeout` defaults to the configured per-RPC timeout; callers may
        override it (e.g. the registration probe uses `connect_timeout`).
        """
        stub = self._stub
        if stub is None:
            raise IpcClientError("stub not initialised")
        metadata = self._auth_metadata()
        if timeout is None:
            timeout = self.rpc_timeout

        if method == "DispatchIntent":
            req = _pb.Intent(
                intent_id=payload["intent_id"],
                utterance=payload.get("utterance", ""),
                action=payload["action"],
                args_json=payload.get("args_json", b"{}"),
                confidence=payload.get("confidence", 0.0),
                requires=payload.get("requires", []),
                session_id=payload.get("session_id", ""),
                deadline_ns=payload.get("deadline_ns", 0),
                trace_id=payload.get("trace_id", ""),
            )
            resp = await stub.DispatchIntent(req, metadata=metadata, timeout=timeout)
            return {
                "intent_id": resp.intent_id,
                "status": resp.status,
                "result_json": json.loads(resp.result_json) if resp.result_json else {},
                "message": resp.message,
                "violation": {
                    "required": resp.violation.required,
                    "reason": resp.violation.reason,
                    "message": resp.violation.message,
                } if resp.HasField("violation") else None,
                "completed_at_ns": resp.completed_at_ns,
            }

        elif method == "QueryMemory":
            req = _pb.MemoryQuery(
                query=payload["query"],
                tags=payload.get("tags", []),
                top_k=payload.get("top_k", 10),
                min_score=payload.get("min_score", 0.0),
                namespace=payload.get("namespace", ""),
                trace_id=payload.get("trace_id", ""),
            )
            resp = await stub.QueryMemory(req, metadata=metadata, timeout=timeout)
            return {
                "hits": [
                    {
                        "object_id": h.object_id,
                        "score": h.score,
                        "payload": json.loads(h.payload_json) if h.payload_json else {},
                        "tags": list(h.tags),
                    }
                    for h in resp.hits
                ],
                "total": resp.total,
                "elapsed_ns": resp.elapsed_ns,
                "trace_id": resp.trace_id,
            }

        elif method == "HalGate":
            req = _pb.HalGateRequest(
                op=payload["op"],
                device=payload.get("device", ""),
                data=payload.get("data", b""),
                capability=payload.get("capability", ""),
                risk_override=payload.get("risk_override", -1.0),
                allow_approval=payload.get("allow_approval", True),
                trace_id=payload.get("trace_id", ""),
            )
            resp = await stub.HalGate(req, metadata=metadata, timeout=timeout)
            return {
                "status": resp.status,
                "grant_token": resp.grant_token,
                "risk_score": resp.risk_score,
                "data": resp.data,
                "violation": {
                    "required": resp.violation.required,
                    "held": resp.violation.held,
                    "reason": resp.violation.reason,
                    "message": resp.violation.message,
                } if resp.HasField("violation") else None,
                "trace_id": resp.trace_id,
            }

        elif method == "Heartbeat":
            req = _pb.Heartbeat(
                agent_id=payload["agent_id"],
                seq=payload["seq"],
                sent_at_ns=payload["sent_at_ns"],
                load_avg=payload.get("load_avg", 0.0),
                status=payload.get("status", "alive"),
            )
            resp = await stub.Heartbeat(req, metadata=metadata, timeout=timeout)
            return {
                "agent_id": resp.agent_id,
                "seq": resp.seq,
                "status": resp.status,
            }

        raise IpcClientError(f"unknown RPC method: {method}")

    def _fallback_response(self, method: str, payload: dict, trace_id: str) -> dict:
        """Return a well-formed stub response when no live IPC is available.

        Same shape the real path returns so downstream code never trips on a
        missing key. This path proves nothing about the wire contract — it
        just keeps the agent pipeline alive when the server is down.
        """
        log.warning(
            "[%s] %s served from FALLBACK (no live IPC, trace=%s)",
            self.agent_id, method, trace_id,
        )
        if method == "QueryMemory":
            return {
                "hits": [], "total": 0, "elapsed_ns": 0,
                "trace_id": payload.get("trace_id", trace_id),
            }
        elif method == "DispatchIntent":
            return {
                "intent_id": payload.get("intent_id", ""),
                "status": "pending",
                "result_json": {},
                "message": f"{method} acknowledged (fallback mode, no live IPC)",
            }
        elif method == "HalGate":
            return {
                "status": "pending",
                "grant_token": "",
                "risk_score": 0.0,
                "violation": None,
            }
        elif method == "Heartbeat":
            return {
                "agent_id": self.agent_id,
                "seq": payload.get("seq", 0),
                "status": "ok",
            }
        return {"status": "ok", "message": f"{method} acknowledged (fallback mode)"}

    # ─── High-level routing ───────────────────────────────────────────────

    async def send(self, target_agent: str, msg_type: str, payload: dict) -> dict:
        """
        Route a message to the right RPC based on msg_type.
        This is the main dispatch method used by the coordinator.

        Args:
            target_agent: who we're sending to (e.g. "memory", "security")
            msg_type: the message type tag (e.g. "MEMORY_QUERY", "SECURITY_ALERT")
            payload: the message body as a dict

        Returns:
            The RPC response as a dict.
        """
        # Memory query → QueryMemory RPC
        if msg_type in ("MEMORY_QUERY", "MEMORY_RESULT"):
            return await self.query_memory(
                query=payload.get("query", ""),
                tags=payload.get("tags"),
                top_k=payload.get("top_k", 10),
                namespace=payload.get("namespace", ""),
            )

        # Intent dispatch → DispatchIntent RPC
        if msg_type in ("INTENT_DISPATCH", "FILE_OPERATION", "HAL_GATE_REQUEST"):
            return await self.dispatch_intent(
                intent_id=payload.get("intent_id", ""),
                action=payload.get("action", msg_type.lower()),
                utterance=payload.get("utterance", ""),
                args=payload.get("args", payload),
                confidence=payload.get("confidence", 0.0),
            )

        # HAL gate → HalGate RPC
        if msg_type == "HAL_GATE":
            return await self.hal_gate(
                op=payload.get("op", ""),
                device=payload.get("device", ""),
                capability=payload.get("capability", ""),
            )

        # Resource hint → ResourceHint RPC (not directly exposed, use heartbeat as ack)
        if msg_type == "RESOURCE_HINT":
            return await self.heartbeat(status="hint_received")

        # Security alert / unknown → dispatch as intent with the alert type
        return await self.dispatch_intent(
            intent_id=payload.get("intent_id", ""),
            action=f"agent.{msg_type.lower()}",
            utterance=payload.get("message", ""),
            args=payload,
        )

    async def heartbeat_loop(
        self,
        shutdown_event: asyncio.Event,
        interval: float = 5.0,
    ) -> None:
        """Long-running heartbeat loop. Runs until shutdown_event is set."""
        log.info("[%s] heartbeat loop starting (interval=%.1fs)", self.agent_id, interval)
        while not shutdown_event.is_set():
            try:
                await self.heartbeat()
            except IpcClientError as e:
                log.warning("[%s] heartbeat failed: %s", self.agent_id, e)
            try:
                await asyncio.wait_for(shutdown_event.wait(), timeout=interval)
            except asyncio.TimeoutError:
                pass
        log.info("[%s] heartbeat loop stopped", self.agent_id)
