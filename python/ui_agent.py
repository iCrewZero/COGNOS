"""COGNOS UI Agent — bridges the Rust shell with Python-based UI logic, providing higher-level intent interpretation, natural-language prompts, and UI orchestration. Talks to the Rust services over gRPC."""

import asyncio
import logging
from dataclasses import dataclass, field
from typing import Optional, List, Dict, Any
from pathlib import Path
# import grpc  # TODO(v1): wire up the real grpcio client once the proto stubs are generated

logger = logging.getLogger("cognos.ui_agent")


@dataclass
class UIAgentConfig:
    """Runtime configuration for the UI agent.

    Attributes:
        grpc_endpoint: Unix socket or host:port the Rust shell listens on.
        intent_timeout: Seconds to wait for an intent ack from the shell.
        approval_timeout: Seconds to wait for a human approval decision.
        max_retries: Number of gRPC retries before surfacing an error.
    """

    grpc_endpoint: str = "unix:///run/cognos/cli.sock"
    intent_timeout: float = 5.0
    approval_timeout: float = 30.0
    max_retries: int = 3


@dataclass
class Intent:
    """A natural-language intent submitted by the user."""

    text: str
    user_id: str
    session_id: str
    priority: str = "normal"
    context: Dict[str, Any] = field(default_factory=dict)


@dataclass
class Approval:
    """A pending approval decision surfaced by the HAL approval flow."""

    id: int
    action: str
    risk_score: float
    agent: str
    timestamp: str


@dataclass
class AgentStatus:
    """Snapshot of a single agent's runtime state."""

    agent_id: str
    state: str
    current_task: Optional[str]
    trust_score: float
    capabilities: List[str]


class UIAgent:
    """High-level orchestrator that bridges the Rust shell with Python UI logic.

    The Rust shell owns the terminal/wayland surface; this agent is responsible
    for intent interpretation, prompt rendering, approval UX and event fan-out.
    Communication with the Rust services happens over gRPC.
    """

    def __init__(self, config: UIAgentConfig) -> None:
        self.config = config
        self._client = None  # TODO(v1): grpc.aio.Channel once wired
        self._event_subscribers: List[Any] = []

    async def connect(self) -> None:
        """Open the gRPC channel to the Rust shell."""
        logger.info("connecting to gRPC endpoint %s", self.config.grpc_endpoint)
        # TODO(v1): self._client = CognosStub(grpc.aio.insecure_channel(...))

    async def disconnect(self) -> None:
        """Close the gRPC channel and flush subscribers."""
        logger.info("disconnecting UI agent")
        # TODO(v1): await self._client.channel.close()

    async def submit_intent(self, intent: Intent) -> str:
        """Submit a natural-language intent; returns the assigned task_id."""
        logger.info("submit_intent: %r (priority=%s)", intent.text, intent.priority)
        # TODO(v1): call CognosIpc.DispatchIntent with retries + intent_timeout
        return ""

    async def list_pending_approvals(self) -> List[Approval]:
        """Return all approvals currently awaiting a human decision."""
        logger.debug("list_pending_approvals")
        # TODO(v1): call CognosIpc.ListApprovals
        return []

    async def approve(self, approval_id: int) -> bool:
        """Approve a pending action by ID."""
        logger.info("approve: id=%s", approval_id)
        # TODO(v1): call CognosIpc.ResolveApproval(approved=true)
        return True

    async def deny(self, approval_id: int) -> bool:
        """Deny a pending action by ID."""
        logger.info("deny: id=%s", approval_id)
        # TODO(v1): call CognosIpc.ResolveApproval(approved=false)
        return True

    async def list_agents(self) -> List[AgentStatus]:
        """Return the runtime status of every registered agent."""
        logger.debug("list_agents")
        # TODO(v1): call CognosIpc.ListAgents
        return []

    async def subscribe_events(self, callback) -> None:
        """Register a callback invoked on every streamed event from the shell."""
        logger.debug("subscribe_events: new subscriber registered")
        self._event_subscribers.append(callback)
        # TODO(v1): open CognosIpc.StreamEvents and fan out to subscribers

    async def run_forever(self) -> None:
        """Main event loop: stream events and fan them out to subscribers."""
        logger.info("UI agent event loop started")
        # TODO(v1): async for event in self._client.stream_events(): ...
        try:
            await asyncio.Event().wait()
        except asyncio.CancelledError:
            logger.info("UI agent event loop cancelled")
            raise


async def main() -> None:
    """Entrypoint: connect a UIAgent and run forever."""
    logging.basicConfig(level=logging.INFO)
    agent = UIAgent(UIAgentConfig())
    await agent.connect()
    try:
        await agent.run_forever()
    finally:
        await agent.disconnect()


if __name__ == "__main__":
    asyncio.run(main())

# v0: stub — gRPC client wiring is TODO
