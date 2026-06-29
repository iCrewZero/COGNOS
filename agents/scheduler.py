"""
Scheduler Agent for COGNOS/OS.

Receives resource hints and scheduling requests from the coordinator,
forwards them to the Rust scheduler daemon via IPC. Also provides
local scheduling decisions for the Python agent pool.

In v0 this is a thin proxy. In v1 it will integrate with the LSTM
predictor and cgroup v2 weight adjustments.

Owner: iCrewZero
"""

from __future__ import annotations

import logging
from dataclasses import dataclass
from typing import Any

from shared.base_agent import BaseAgent
from shared.types import AgentMessage

log = logging.getLogger("cognos.scheduler")


@dataclass
class ScheduleHint:
    """A resource hint from an agent to the scheduler."""
    agent_id: str
    kind: str        # "cpu" | "gpu" | "memory" | "io" | "net"
    priority: int    # 0-100
    duration_ns: int = 0
    metadata: dict | None = None


class SchedulerAgent(BaseAgent):
    """
    Scheduler agent. Receives RESOURCE_HINT messages from the coordinator
    and applies local scheduling decisions. Forwards heavy scheduling
    to the Rust scheduler daemon via IPC in v1.
    """

    def __init__(self):
        super().__init__("scheduler")
        # Track active hints so we can adjust priorities.
        self._active_hints: dict[str, ScheduleHint] = {}

    async def handle_message(self, msg: AgentMessage) -> Any:
        """Route incoming messages by type."""
        log.info("[scheduler] Received: %s", msg.type)

        if msg.type == "RESOURCE_HINT":
            return await self._handle_resource_hint(msg)
        elif msg.type == "SCHEDULER_STATUS":
            return await self._get_status(msg)
        else:
            log.warning("[scheduler] Unknown message type: %s", msg.type)
            return {"status": "ignored", "reason": f"unknown type: {msg.type}"}

    async def _handle_resource_hint(self, msg: AgentMessage) -> dict:
        """
        Process a resource hint from an agent.

        The hint tells the scheduler that an agent is about to do something
        that needs CPU, GPU, memory, IO, or network resources. The scheduler
        can use this to pre-adjust cgroup weights or priority.

        In v0 this just acknowledges and tracks the hint locally.
        In v1 it forwards to the Rust scheduler via ResourceHint RPC.
        """
        payload = msg.payload
        hint = ScheduleHint(
            agent_id=payload.get("agent_id", msg.sender),
            kind=payload.get("kind", "cpu"),
            priority=payload.get("priority", 50),
            duration_ns=payload.get("duration_ns", 0),
            metadata=payload.get("metadata"),
        )

        # Track the hint.
        self._active_hints[hint.agent_id] = hint

        log.info(
            "[scheduler] Resource hint: agent=%s kind=%s priority=%d",
            hint.agent_id, hint.kind, hint.priority,
        )

        # In v0, just acknowledge. The coordinator doesn't block on this.
        return {
            "status": "acknowledged",
            "agent_id": hint.agent_id,
            "kind": hint.kind,
            "priority": hint.priority,
            "message": "hint registered (v0: local tracking only)",
        }

    async def _get_status(self, msg: AgentMessage) -> dict:
        """Return current scheduler state and active hints."""
        return {
            "status": "ok",
            "active_hints": len(self._active_hints),
            "hints": [
                {
                    "agent_id": h.agent_id,
                    "kind": h.kind,
                    "priority": h.priority,
                }
                for h in self._active_hints.values()
            ],
            "message": "v0 scheduler agent — local tracking only, no cgroup adjustments",
        }
