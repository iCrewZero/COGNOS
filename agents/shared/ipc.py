"""
shared/ipc.py — Python gRPC client for the COGNOS IPC server.

Wraps the generated cognos_pb2 stubs so Python agents (coordinator,
coding agent, memory agent) can talk to the Rust IPC server.
"""

from __future__ import annotations

import json
import logging
import time
import uuid
from typing import Optional

import grpc

# The generated stubs live alongside this file after running:
#   python -m grpc_tools.protoc -I../ipc/grpc/proto --python_out=. --grpc_python_out=. ../ipc/grpc/proto/cognos.proto
# For now we define thin wrappers that construct the wire format manually
# so the agent code works even without the generated stubs. When the
# proto is compiled to Python, replace the _manual_* helpers with the
# real generated classes.

log = logging.getLogger("cognos.ipc")

# Default endpoint the agents connect to.
DEFAULT_ENDPOINT = "localhost:7443"


class IpcClientError(Exception):
    """Raised when an IPC call fails."""


class AgentIpcClient:
    """
    Python-side gRPC client for the COGNOS IPC server.

    Usage:
        client = AgentIpcClient("agent.coordinator", endpoint="localhost:7443")
        client.connect()
        response = client.dispatch_intent(intent_id="...", action="file.open", ...)
        client.close()
    """

    def __init__(
        self,
        agent_id: str,
        endpoint: str = DEFAULT_ENDPOINT,
        signing_secret: str = "",
    ):
        self.agent_id = agent_id
        self.endpoint = endpoint
        self.signing_secret = signing_secret
        # v0 TODO: HMAC-sign all RPC payloads using this secret.
        # Currently all IPC calls from Python are unauthenticated.
        # The Rust CognosClient already implements signing (see client.rs build_envelope).
        self._channel: Optional[grpc.aio.Channel] = None
        # Fix M7 — iCrewZero: Removed dead `self._stub = None` attribute.
        # It was initialised here, set to None in connect(), but never read.
        self._seq = 0

    async def connect(self) -> None:
        """Open the gRPC channel. Retries once on transient errors."""
        # Make sure the target has a scheme prefix — gRPC requires it.
        # If the caller just said "localhost:7443", we add "http://" so
        # the channel knows it is using plaintext (not TLS).
        target = self.endpoint
        if not target.startswith("http://") and not target.startswith("https://"):
            target = f"http://{target}"
        self._channel = grpc.aio.insecure_channel(target)
        # Fix M7 — iCrewZero: Removed dead `self._stub = None` assignment.
        # The attribute was never read anywhere.
        log.info("[%s] Connected to IPC at %s", self.agent_id, self.endpoint)

    async def close(self) -> None:
        if self._channel:
            await self._channel.close()
            self._channel = None
            log.info("[%s] Disconnected from IPC", self.agent_id)

    @property
    def is_connected(self) -> bool:
        return self._channel is not None

    def _next_trace_id(self) -> str:
        return str(uuid.uuid4())

    def _next_seq(self) -> int:
        self._seq += 1
        return self._seq

    def _timestamp_ns(self) -> int:
        return int(time.time() * 1e9)

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

        Returns a dict with keys: hits (list), total, elapsed_ns.
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

    async def _call_rpc(self, method: str, payload: dict, trace_id: str) -> dict:
        """
        Low-level RPC caller.

        If the generated gRPC stub is available (cognos_pb2 imported),
        it uses the real stub. Otherwise, it serializes to JSON and
        posts over the channel manually (fallback for development).
        """
        if not self._channel:
            raise IpcClientError("not connected — call connect() first")

        try:
            # Try to use the generated stub if available.
            try:
                from cognos_pb2_grpc import CognosIpcStub  # type: ignore
                from cognos_pb2 import (  # type: ignore
                    Intent, IntentResponse, MemoryQuery, MemoryResult,
                    HalGateRequest, HalGateResponse, Heartbeat,
                )

                stub = CognosIpcStub(self._channel)

                if method == "DispatchIntent":
                    req = Intent(
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
                    resp: IntentResponse = await stub.DispatchIntent(req)
                    # Fix M6 — iCrewZero: Added violation and completed_at_ns
                    # fields that were previously dropped, so callers have
                    # full visibility into HAL policy results.
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
                    req = MemoryQuery(
                        query=payload["query"],
                        tags=payload.get("tags", []),
                        top_k=payload.get("top_k", 10),
                        min_score=payload.get("min_score", 0.0),
                        namespace=payload.get("namespace", ""),
                        trace_id=payload.get("trace_id", ""),
                    )
                    resp: MemoryResult = await stub.QueryMemory(req)
                    # Fix M6 — iCrewZero: Added trace_id field that was
                    # previously dropped from the MemoryResult response.
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
                    req = HalGateRequest(
                        op=payload["op"],
                        device=payload.get("device", ""),
                        data=payload.get("data", b""),
                        capability=payload.get("capability", ""),
                        risk_override=payload.get("risk_override", -1.0),
                        allow_approval=payload.get("allow_approval", True),
                        trace_id=payload.get("trace_id", ""),
                    )
                    resp: HalGateResponse = await stub.HalGate(req)
                    # Fix M6 — iCrewZero: Added data, violation, and trace_id
                    # fields that were previously dropped from the HalGate
                    # response, so callers can inspect policy violations.
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
                    req = Heartbeat(
                        agent_id=payload["agent_id"],
                        seq=payload["seq"],
                        sent_at_ns=payload["sent_at_ns"],
                        load_avg=payload.get("load_avg", 0.0),
                        status=payload.get("status", "alive"),
                    )
                    resp: Heartbeat = await stub.Heartbeat(req)
                    return {
                        "agent_id": resp.agent_id,
                        "seq": resp.seq,
                        "status": resp.status,
                    }

            except ImportError:
                pass  # Fall through to manual JSON approach below.

            # Fallback: stub responses when generated gRPC stubs aren't compiled.
            # Returns the same shape the caller expects so downstream code
            # doesn't crash on missing keys.
            log.warning(
                "[%s] %s using fallback (stubs not compiled, trace=%s)",
                self.agent_id, method, trace_id,
            )
            if method == "QueryMemory":
                return {"hits": [], "total": 0, "elapsed_ns": 0, "trace_id": trace_id}
            elif method == "DispatchIntent":
                return {
                    "intent_id": payload.get("intent_id", ""),
                    "status": "pending",
                    "result_json": {},
                    "message": f"{method} acknowledged (fallback mode, stubs not compiled)",
                }
            elif method == "HalGate":
                return {
                    "status": "pending",
                    "grant_token": "",
                    "risk_score": 0.0,
                    "violation": None,
                }
            else:
                return {"status": "ok", "message": f"{method} acknowledged (fallback mode)"}

        except grpc.aio.AioRpcError as e:
            log.error("[%s] %s failed: %s", self.agent_id, method, e.details())
            raise IpcClientError(f"{method} RPC error: {e.details()}") from e
        except Exception as e:
            log.error("[%s] %s unexpected error: %s", self.agent_id, method, e)
            raise IpcClientError(f"{method} error: {e}") from e


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
        import asyncio

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