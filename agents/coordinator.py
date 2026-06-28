"""
Agent Coordinator / Orchestrator for COGNOS/OS.

Receives resolved IntentSchema, decomposes it into tasks, delegates to agents,
handles conflicts, and assembles the final ActionSet for HAL to gate.

Proposes actions only — never gates, never executes directly.
"""

from __future__ import annotations

import asyncio
import json
import logging
import time
import uuid
from dataclasses import dataclass, field, asdict
from datetime import datetime, UTC
from pathlib import Path
from typing import Any

from shared.ipc import AgentIpcClient

log = logging.getLogger("cognos.coordinator")

AUDIT_LOG = Path.home() / ".cognos" / "audit.log"
DEFAULT_AGENT_TIMEOUT = 0.8   # seconds
DEGRADED_THRESHOLD = 0.20     # >20% failure rate → degraded
UNAVAILABLE_THRESHOLD = 0.50  # >50% failure rate → unavailable


# ─── Types ───────────────────────────────────────────────────────────────────

@dataclass
class ProposedAction:
    action: str
    target: str
    agent: str
    confidence: float
    reversible: bool
    hal_pre_score: float
    parameters: dict = field(default_factory=dict)
    conflict_flag: bool = False
    conflict_note: str = ""


@dataclass
class ActionSet:
    actions: list[ProposedAction]
    context: dict
    confidence: float
    requires_disambiguation: bool
    assembled_by: str = "coordinator"
    intent_id: str = ""
    latency_ms: float = 0.0


@dataclass
class AgentHealth:
    name: str
    total_requests: int = 0
    failed_requests: int = 0
    is_degraded: bool = False
    is_unavailable: bool = False

    @property
    def failure_rate(self) -> float:
        if self.total_requests == 0:
            return 0.0
        return self.failed_requests / self.total_requests

    def record_result(self, success: bool) -> None:
        self.total_requests += 1
        if not success:
            self.failed_requests += 1
        # Keep only last 60 seconds of counts (simplified: per-request rolling)
        self.is_degraded = self.failure_rate > DEGRADED_THRESHOLD
        self.is_unavailable = self.failure_rate > UNAVAILABLE_THRESHOLD


# ─── Coordinator ─────────────────────────────────────────────────────────────

class Coordinator:
    """
    Central orchestrator for COGNOS/OS agent pipeline.

    All agent communication goes through AgentIpcClient (authenticated gRPC).
    No direct filesystem access. No HAL calls — proposes only.
    """


    # ─── Local agent registry ─────────────────────────────────────────────
    # In v0, all agents run in the same Python process. The coordinator
    # instantiates them here and dispatches directly. When agents move to
    # separate processes in v1, this registry is replaced by IPC calls.
    #
    # Owner: iCrewZero

    def __init__(self, ipc_client: AgentIpcClient):
        self._ipc = ipc_client
        self._health: dict[str, AgentHealth] = {
            name: AgentHealth(name)
            for name in ("planner", "memory", "security", "scheduler",
                         "file", "coding")
        }

        # Lazy-loaded local agent instances.
        self._agents: dict[str, Any] = {}

    def _get_agent(self, name: str) -> Any:
        """Get or lazily create a local agent instance."""
        if name in self._agents:
            return self._agents[name]

        try:
            if name == "memory":
                from memory import MemoryAgent
                self._agents[name] = MemoryAgent()
            elif name == "security":
                from security import SecurityAgent
                self._agents[name] = SecurityAgent()
            elif name == "scheduler":
                from scheduler import SchedulerAgent
                self._agents[name] = SchedulerAgent()
            elif name == "file":
                from file_agent import FileAgent
                self._agents[name] = FileAgent()
            elif name == "coding":
                from coding_agent import CodingAgent
                self._agents[name] = CodingAgent()
            elif name == "planner":
                from planner import Planner
                self._agents[name] = Planner(ipc_client=self._ipc)
            else:
                log.warning("Unknown local agent: %s", name)
                return None
        # Fix H2 — iCrewZero: Changed from `except ImportError` to
        # `except Exception` so that TypeError (e.g. B1's missing __init__)
        # and other construction errors are caught and logged instead of
        # bubbling up as an unhandled crash.
        except Exception as e:
            log.error("Cannot instantiate agent '%s': %s", name, e)
            return None

        log.debug("Instantiated local agent: %s", name)
        return self._agents[name]



    @classmethod
    async def create(cls, endpoint: str = "localhost:7443", signing_secret: str = "") -> "Coordinator":
        """Factory: create a coordinator with a connected IPC client.

        This is the recommended way to create a Coordinator — it handles
        the IPC connection so you don't have to remember to call connect().
        """
        # Fix M4 — iCrewZero: Removed redundant import of AgentIpcClient;
        # it is already imported at module level (line 22).
        ipc = AgentIpcClient("agent.coordinator", endpoint=endpoint, signing_secret=signing_secret)
        await ipc.connect()
        return cls(ipc)

    async def handle_intent(self, schema: dict) -> ActionSet:
        """
        Main entry point. Decomposes an IntentSchema into an ActionSet.
        """
        start = time.monotonic()
        intent_id = schema.get("intent_id", str(uuid.uuid4()))
        goal = schema.get("goal", "")
        # Fix M4 — iCrewZero: Removed unused variable `hal_pre` that was
        # never referenced after this line.

        # 1. Always query Memory Agent first (parallel with scheduler notification)
        memory_task = asyncio.create_task(
            self._call_agent("memory", "MEMORY_QUERY", schema)
        )
        # 2. Notify Scheduler Agent (non-blocking)
        asyncio.create_task(
            self._call_agent("scheduler", "RESOURCE_HINT", {"goal": goal}, critical=False)
        )

        # 3. Primary agent routing
        primary_task = asyncio.create_task(
            self._route_primary(goal, schema)
        )

        # Gather with timeouts
        memory_result, primary_result = await asyncio.gather(
            memory_task,
            primary_task,
            return_exceptions=True,
        )

        # 4. Assemble ActionSet
        actions: list[ProposedAction] = []
        context: dict = {}
        min_confidence = 1.0

        # Fix M3 — iCrewZero: The memory agent may return dataclass
        # instances (e.g. MemorySearchResult) in its result dicts.  Those
        # aren't JSON-serializable.  Convert them to plain dicts here so
        # downstream JSON.dumps (audit log, IPC) doesn't crash.
        if isinstance(memory_result, dict):
            mem_data = memory_result.get("hits", memory_result.get("data", []))
            from dataclasses import asdict as _asdict
            if isinstance(mem_data, list):
                mem_data = [
                    _asdict(r) if hasattr(r, '__dataclass_fields__') else r
                    for r in mem_data
                ]
            context["memory"] = mem_data
            min_confidence = min(min_confidence, memory_result.get("confidence", 1.0))

        if isinstance(primary_result, list):
            actions.extend(primary_result)
        elif isinstance(primary_result, dict):
            proposed = primary_result.get("actions", [])
            actions.extend(proposed)
            min_confidence = min(min_confidence, primary_result.get("confidence", 1.0))

        # 5. Conflict resolution
        actions = self._resolve_conflicts(actions, context)

        # Critical agent failure → reduce confidence
        if isinstance(memory_result, Exception):
            log.error("Memory agent timeout/error: %s", memory_result)
            min_confidence = min(min_confidence, 0.5)

        # Fix H1 — iCrewZero: When asyncio.gather(..., return_exceptions=True)
        # returns an Exception for the primary agent, the old code only
        # checked isinstance(list/dict) and silently dropped it, producing
        # an ActionSet with zero actions and confidence 1.0.  Now we catch
        # the exception, log it, and pull confidence down to 0.3 so the
        # caller knows something went wrong.
        if isinstance(primary_result, Exception):
            log.error("Primary agent error: %s", primary_result)
            min_confidence = min(min_confidence, 0.3)

        elapsed = (time.monotonic() - start) * 1000
        action_set = ActionSet(
            actions=actions,
            context=context,
            confidence=max(0.0, min(1.0, min_confidence)),
            requires_disambiguation=schema.get("disambiguation_required", False),
            intent_id=intent_id,
            latency_ms=elapsed,
        )

        self._audit(schema, action_set)
        return action_set

    async def _route_primary(self, goal: str, schema: dict) -> Any:
        """Route to the primary agent based on the intent goal."""
        open_goals = {"open_workspace", "find_files", "retrieve_context"}
        coding_goals = {"coding_task", "refactor", "implement", "debug"}
        security_goals = {"security_concern", "audit_app", "check_permissions"}
        install_goals = {"install_package", "uninstall_package"}
        config_goals = {"system_config", "modify_settings"}

        if goal in open_goals:
            # File Agent + Memory Agent (memory already running in parallel)
            return await self._call_agent("file", "FILE_OPERATION", {
                "operation": "open_workspace",
                "schema": schema,
            })

        elif goal in coding_goals:
            return await self._call_agent("coding", "INTENT_DISPATCH", schema)

        elif goal in security_goals:
            return await self._call_agent("security", "SECURITY_ALERT", schema)

        elif goal in install_goals:
            # Install: Security Agent review + File Agent for execution
            security_resp = await self._call_agent("security", "INSTALL_TRUST", {
                "package": schema.get("package", schema.get("goal", "")),
                "source": schema.get("source", "apt"),
                "schema": schema
            })
            if security_resp and security_resp.get("approved", True):
                return await self._call_agent("file", "FILE_OPERATION", {
                    "operation": "install", "schema": schema
                })

        elif goal in config_goals:
            # System config: HAL gate first (via planner), then agent
            return await self._call_agent("planner", "HAL_GATE_REQUEST", schema)

        # General / unknown
        return await self._call_agent("planner", "INTENT_DISPATCH", schema)

    def _resolve_conflicts(
        self, actions: list[ProposedAction], context: dict
    ) -> list[ProposedAction]:
        """
        If two actions target the same resource differently,
        keep the higher-confidence one and flag both for user review.
        """
        seen_targets: dict[str, ProposedAction] = {}
        resolved: list[ProposedAction] = []

        for action in actions:
            if action.target in seen_targets:
                existing = seen_targets[action.target]
                # Conflict detected
                log.warning(
                    "Conflict: %s vs %s both targeting %s",
                    existing.agent, action.agent, action.target
                )
                self._audit_conflict(existing, action)

                if action.confidence >= existing.confidence:
                    # Replace with higher confidence, flag both
                    existing.conflict_flag = True
                    existing.conflict_note = f"Also proposed by {action.agent}"
                    action.conflict_flag = True
                    action.conflict_note = f"Conflicts with {existing.agent}"
                    seen_targets[action.target] = action
                    resolved = [a for a in resolved if a.target != action.target]
                    resolved.append(action)
                else:
                    existing.conflict_flag = True
                    existing.conflict_note = f"Also proposed by {action.agent}"
            else:
                seen_targets[action.target] = action
                resolved.append(action)

        return resolved

    # ─── Agent communication ──────────────────────────────────────────────────

    async def _call_agent(
        self,
        agent: str,
        msg_type: str,
        payload: dict,
        critical: bool = True,
    ) -> dict | None:
        """
        Send a message to an agent and await response.
        Tries local dispatch first (v0: same-process), falls back to IPC.
        Tracks health per-agent. Returns None on failure.
        """
        health = self._health.get(agent)
        if health is None:
            log.warning("Unknown agent '%s' — no health tracking", agent)
            health = AgentHealth(agent)

        if health.is_unavailable and not critical:
            log.warning("Skipping unavailable non-critical agent '%s'", agent)
            return None

        # v0: Try local agent dispatch first.
        local = self._get_agent(agent)
        if local is not None:
            try:
                # Planner is special — it has plan() not handle_message()
                if agent == "planner" and hasattr(local, "plan"):
                    result = await asyncio.wait_for(
                        local.plan(payload.get("goal", ""), payload.get("schema", {})),
                        timeout=DEFAULT_AGENT_TIMEOUT,
                    )
                    health.record_result(True)
                    if isinstance(result, dict):
                        return result
                    # Fix M4 — iCrewZero: Removed redundant local import;
                    # `asdict` is already imported at module level (line 17).
                    plan_dict = asdict(result) if hasattr(result, '__dataclass_fields__') else {"data": result}
                    if "steps" in plan_dict:
                        plan_dict["actions"] = plan_dict.pop("steps")
                    return plan_dict
                elif hasattr(local, "handle_message"):
                    from shared.types import AgentMessage
                    msg = AgentMessage(type=msg_type, payload=payload, sender="coordinator")
                    result = await asyncio.wait_for(
                        local.handle_message(msg),
                        timeout=DEFAULT_AGENT_TIMEOUT,
                    )
                    health.record_result(True)
                    if isinstance(result, dict):
                        return result
                    return {"status": "ok", "data": result}
            except asyncio.TimeoutError:
                log.warning("Local agent '%s' timed out after %.1fs", agent, DEFAULT_AGENT_TIMEOUT)
                health.record_result(False)
                if health.is_unavailable:
                    log.error("Agent '%s' marked unavailable", agent)
                return None
            except Exception as e:
                log.error("Local agent '%s' error: %s", agent, e)
                health.record_result(False)
                return None

        # v1 fallback: dispatch via IPC to a remote agent process.
        try:
            result = await asyncio.wait_for(
                self._ipc.send(agent, msg_type, payload),
                timeout=DEFAULT_AGENT_TIMEOUT,
            )
            health.record_result(True)
            return result
        except asyncio.TimeoutError:
            log.warning("Agent '%s' timed out after %.1fs", agent, DEFAULT_AGENT_TIMEOUT)
            health.record_result(False)
            if health.is_unavailable:
                log.error("Agent '%s' marked unavailable — attempting restart", agent)
                await self._restart_agent(agent)
            return None
        except Exception as e:
            log.error("Agent '%s' error: %s", agent, e)
            health.record_result(False)
            return None

    # Fix M4 — iCrewZero: Removed the dead _with_timeout() method.
    # It was never called anywhere in the codebase.

    async def _restart_agent(self, agent: str) -> None:
        """Attempt to restart a failed agent via systemd dbus."""
        try:
            proc = await asyncio.create_subprocess_exec(
                "sudo", "systemctl", "restart", f"cognos-{agent}.service",
                stdout=asyncio.subprocess.DEVNULL,
                stderr=asyncio.subprocess.DEVNULL,
            )
            await asyncio.wait_for(proc.wait(), timeout=5.0)
            log.info("Restarted agent '%s'", agent)
        except Exception as e:
            log.error("Failed to restart agent '%s': %s", agent, e)

    # ─── Audit ───────────────────────────────────────────────────────────────

    def _audit(self, schema: dict, action_set: ActionSet) -> None:
        entry = {
            "ts": datetime.now(UTC).isoformat(),
            "agent": "coordinator",
            "action": "intent_handled",
            "intent_id": action_set.intent_id,
            "goal": schema.get("goal"),
            "action_count": len(action_set.actions),
            "confidence": action_set.confidence,
            "latency_ms": round(action_set.latency_ms, 2),
            "hal_pre_score": schema.get("hal_pre_score", 0.0),
            "outcome": "assembled",
        }
        self._write_audit(entry)

    def _audit_conflict(self, a: ProposedAction, b: ProposedAction) -> None:
        entry = {
            "ts": datetime.now(UTC).isoformat(),
            "agent": "coordinator",
            "action": "conflict_detected",
            "target": a.target,
            "agent_a": a.agent,
            "agent_b": b.agent,
            "confidence_a": a.confidence,
            "confidence_b": b.confidence,
            "resolution": "higher_confidence_wins",
            "outcome": "conflict_logged",
        }
        self._write_audit(entry)

    def _write_audit(self, entry: dict) -> None:
        try:
            AUDIT_LOG.parent.mkdir(parents=True, exist_ok=True)
            with open(AUDIT_LOG, "a") as f:
                f.write(json.dumps(entry) + "\n")
        except OSError as e:
            log.error("Audit write failed: %s", e)
