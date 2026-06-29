"""
shared/base_agent.py — Base class for all COGNOS/OS Python agents.
All agents inherit from this. Handles IPC lifecycle and message dispatch.
"""
from __future__ import annotations

import asyncio
import logging
from typing import Any

# Import the canonical message type so all agents share one definition.
# The re-export below means existing code that does `from shared.base_agent import AgentMessage`
# still works — it gets the dataclass from types.py.
from shared.types import AgentMessage as _CanonicalAgentMessage

log = logging.getLogger("cognos.base_agent")


# AgentMessage is the canonical shared type — defined in types.py.
# We re-export it here so old imports like `from shared.base_agent import AgentMessage` still work.
AgentMessage = _CanonicalAgentMessage  # noqa: F811



class BaseAgent:
    # Fix M1 — iCrewZero: Defer asyncio.Queue creation to run() because
    # creating a Queue in __init__ (sync code) triggers DeprecationWarning
    # in Python 3.10+ and may raise RuntimeError in 3.12+ when no event
    # loop is running yet.  We initialise _queue to None here and create
    # it lazily inside the async run() method.
    def __init__(self, name: str):
        self.name = name
        self._running = False
        self._queue: asyncio.Queue | None = None

    async def handle_message(self, msg: AgentMessage) -> Any:
        """Override in subclass to handle incoming messages."""
        log.debug("[%s] Unhandled message: %s", self.name, msg.type)
        return None

    async def run(self) -> None:
        """Start message processing loop."""
        # Create the queue inside the running event loop to avoid
        # DeprecationWarning in Python 3.10+ and RuntimeError in 3.12+.
        self._queue = asyncio.Queue()
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
        # Fix M1 — iCrewZero: Guard against send() being called before
        # run() has had a chance to create the queue.  Log a warning and
        # drop the message rather than crashing with AttributeError.
        if self._queue is None:
            log.warning("[%s] send() called before run() — dropping message", self.name)
            return
        await self._queue.put(msg)

    def stop(self) -> None:
        self._running = False