"""
shared/base_agent.py — Base class for all COGNOS/OS Python agents.
All agents inherit from this. Handles IPC lifecycle and message dispatch.
"""
from __future__ import annotations

import asyncio
import logging
from typing import Any

log = logging.getLogger("cognos.base_agent")


class AgentMessage:
    def __init__(self, type: str, payload: dict, sender: str = ""):
        self.type = type
        self.payload = payload
        self.sender = sender


class BaseAgent:
    def __init__(self, name: str):
        self.name = name
        self._running = False
        self._queue: asyncio.Queue = asyncio.Queue()

    async def handle_message(self, msg: AgentMessage) -> Any:
        """Override in subclass to handle incoming messages."""
        log.debug("[%s] Unhandled message: %s", self.name, msg.type)
        return None

    async def run(self) -> None:
        """Start message processing loop."""
        self._running = True
        log.info("[%s] Agent started", self.name)
        while self._running:
            try:
                msg = await asyncio.wait_for(self._queue.get(), timeout=1.0)
                await self.handle_message(msg)
            except asyncio.TimeoutError:
                pass
            except Exception as e:
                log.error("[%s] Error handling message: %s", self.name, e)

    async def send(self, msg: AgentMessage) -> None:
        await self._queue.put(msg)

    def stop(self) -> None:
        self._running = False