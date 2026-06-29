"""
planner.py — Planning agent for COGNOS/OS.

Receives raw or pre-processed intents from the coordinator, breaks them
into ordered steps, checks capability requirements, and optionally
consults the intent-engine (via IPC) for disambiguation before
returning a structured plan.

The planner never executes anything. It produces a plan that the
coordinator assembles into an ActionSet for HAL gating.
"""

from __future__ import annotations

import logging
import uuid
from dataclasses import dataclass, field
from typing import Any

from shared.ipc import AgentIpcClient

log = logging.getLogger("cognos.planner")

# ─── Types ───────────────────────────────────────────────────────────────────


@dataclass
class PlanStep:
    """A single step in an execution plan."""
    step_id: str = ""
    description: str = ""
    capability: str = ""
    action: str = ""
    args: dict = field(default_factory=dict)
    requires_hal_gate: bool = True
    confidence: float = 0.8
    depends_on: list[str] = field(default_factory=list)

    def __post_init__(self):
        if not self.step_id:
            self.step_id = str(uuid.uuid4())


@dataclass
class Plan:
    """An ordered execution plan produced by the planner."""
    plan_id: str = ""
    goal: str = ""
    steps: list[PlanStep] = field(default_factory=list)
    confidence: float = 0.8
    requires_disambiguation: bool = False
    disambiguation_options: list[dict] = field(default_factory=list)
    context: dict = field(default_factory=dict)

    def __post_init__(self):
        if not self.plan_id:
            self.plan_id = str(uuid.uuid4())


# ─── Action classification ──────────────────────────────────────────────────

# Maps keyword patterns to canonical action types. The planner checks
# the intent text against these rules, similar to the Rust-side
# classify_intent in orchestrator/runtime.rs, but with more detail
# because the planner produces full step sequences.
_ACTION_RULES: list[tuple[list[str], str]] = [
    (["open", "launch", "start app"], "file.open"),
    (["find", "search", "locate", "look for"], "file.search"),
    (["move", "rename", "rename file"], "file.move"),
    (["delete", "remove", "trash"], "file.delete"),
    (["install package", "install"], "pkg.install"),
    (["uninstall", "remove package"], "pkg.uninstall"),
    (["update", "upgrade package"], "pkg.update"),
    (["config", "settings", "preference", "configure"], "system.config"),
    (["permission", "grant", "revoke access"], "security.permission"),
    (["audit", "security check", "scan"], "security.audit"),
    (["code", "implement", "write code", "write function"], "coding.implement"),
    (["refactor", "restructure", "clean up code"], "coding.refactor"),
    (["debug", "fix bug", "fix error", "troubleshoot"], "coding.debug"),
    (["test", "run test", "check test"], "coding.test"),
    (["summarize", "explain", "what is", "how does"], "knowledge.query"),
    (["schedule", "remind", "timer", "alarm"], "system.schedule"),
]


def classify_action(text: str) -> str:
    """Map natural-language text to a canonical action type."""
    lower = text.lower()
    for keywords, action in _ACTION_RULES:
        if any(kw in lower for kw in keywords):
            return action
    return "intent.general"


# ─── Step decomposition ─────────────────────────────────────────────────────

def decompose(action: str, goal: str, context: dict) -> list[PlanStep]:
    """
    Break a classified action into an ordered list of PlanSteps.

    Each step carries the capability it needs so HAL can gate it.
    The steps are returned in dependency order (later steps may
    depend on earlier ones).
    """
    if action in ("file.open",):
        return [
            PlanStep(
                description=f"Resolve target for: {goal}",
                capability="memory.read",
                action="resolve_target",
                args={"query": goal},
            ),
            PlanStep(
                description=f"Open: {goal}",
                capability="file.read",
                action="execute_open",
                args={"target": goal},
                depends_on=["__prev__"],
                confidence=0.9,
            ),
        ]

    if action in ("file.search"):
        return [
            PlanStep(
                description=f"Search memory for context: {goal}",
                capability="memory.read",
                action="memory_search",
                args={"query": goal, "top_k": 10},
            ),
            PlanStep(
                description=f"Execute file search: {goal}",
                capability="file.read",
                action="file_search",
                args={"pattern": goal},
                depends_on=["__prev__"],
            ),
        ]

    if action in ("file.move", "file.rename"):
        return [
            PlanStep(
                description=f"Locate source for move/rename: {goal}",
                capability="file.read",
                action="locate_source",
                args={"query": goal},
            ),
            PlanStep(
                description="Verify target path exists or is creatable",
                capability="file.read",
                action="check_target",
                depends_on=["__prev__"],
            ),
            PlanStep(
                description=f"Execute move/rename: {goal}",
                capability="file.write",
                action="execute_move",
                depends_on=["__prev__"],
                confidence=0.85,
            ),
        ]

    if action in ("file.delete",):
        return [
            PlanStep(
                description=f"Locate target for deletion: {goal}",
                capability="file.read",
                action="locate_target",
                args={"query": goal},
            ),
            PlanStep(
                description="Confirm deletion scope (no unexpected wildcards)",
                capability="security.review",
                action="confirm_delete_scope",
                depends_on=["__prev__"],
            ),
            PlanStep(
                description=f"Execute deletion: {goal}",
                capability="file.write",
                action="execute_delete",
                depends_on=["__prev__"],
                confidence=0.7,
            ),
        ]

    if action in ("pkg.install",):
        return [
            PlanStep(
                description=f"Security review for install: {goal}",
                capability="security.review",
                action="security_check",
                args={"operation": "install", "goal": goal},
            ),
            PlanStep(
                description="Check repository trust and signatures",
                capability="security.review",
                action="check_repo_trust",
                depends_on=["__prev__"],
            ),
            PlanStep(
                description=f"Execute package install: {goal}",
                capability="pkg.execute",
                action="pkg_install",
                args={"goal": goal},
                depends_on=["__prev__"],
            ),
        ]

    if action in ("pkg.uninstall",):
        return [
            PlanStep(
                description=f"Check reverse dependencies for: {goal}",
                capability="pkg.read",
                action="check_reverse_deps",
                args={"package": goal},
            ),
            PlanStep(
                description=f"Execute package uninstall: {goal}",
                capability="pkg.execute",
                action="pkg_uninstall",
                args={"goal": goal},
                depends_on=["__prev__"],
            ),
        ]

    if action in ("pkg.update",):
        return [
            PlanStep(
                description="Check for available updates",
                capability="pkg.read",
                action="check_updates",
            ),
            PlanStep(
                description=f"Apply updates: {goal}",
                capability="pkg.execute",
                action="pkg_update",
                args={"goal": goal},
                depends_on=["__prev__"],
            ),
        ]

    if action in ("coding.implement"):
        return [
            PlanStep(
                description=f"Plan implementation: {goal}",
                capability="coding.plan",
                action="create_plan",
                args={"goal": goal},
            ),
            PlanStep(
                description="Check existing code context and patterns",
                capability="memory.read",
                action="code_context_search",
                args={"query": goal},
            ),
            PlanStep(
                description=f"Generate/modify code: {goal}",
                capability="coding.execute",
                action="execute_code",
                args={"goal": goal},
                depends_on=["__prev__", "__prev2__"],
            ),
            PlanStep(
                description="Run tests and validate output",
                capability="coding.validate",
                action="validate",
                depends_on=["__prev__"],
            ),
        ]

    if action in ("coding.refactor",):
        return [
            PlanStep(
                description=f"Analyze code structure for refactoring: {goal}",
                capability="coding.plan",
                action="analyze_structure",
                args={"goal": goal},
            ),
            PlanStep(
                description=f"Apply refactoring: {goal}",
                capability="coding.execute",
                action="execute_refactor",
                args={"goal": goal},
                depends_on=["__prev__"],
            ),
            PlanStep(
                description="Run tests to verify refactoring didn't break anything",
                capability="coding.validate",
                action="validate",
                depends_on=["__prev__"],
            ),
        ]

    if action in ("coding.debug", "coding.fix bug"):
        return [
            PlanStep(
                description=f"Gather error context: {goal}",
                capability="memory.read",
                action="gather_error_context",
                args={"query": goal},
            ),
            PlanStep(
                description="Analyze root cause",
                capability="coding.plan",
                action="analyze_root_cause",
                args={"goal": goal},
                depends_on=["__prev__"],
            ),
            PlanStep(
                description=f"Apply fix: {goal}",
                capability="coding.execute",
                action="apply_fix",
                args={"goal": goal},
                depends_on=["__prev__"],
            ),
            PlanStep(
                description="Verify fix resolves the issue",
                capability="coding.validate",
                action="validate",
                depends_on=["__prev__"],
            ),
        ]

    if action in ("coding.test",):
        return [
            PlanStep(
                description=f"Discover and run tests: {goal}",
                capability="coding.validate",
                action="run_tests",
                args={"goal": goal},
            ),
            PlanStep(
                description="Collect and report test results",
                capability="coding.validate",
                action="report_results",
                depends_on=["__prev__"],
            ),
        ]

    if action in ("security.audit", "security.check"):
        return [
            PlanStep(
                description=f"Gather system state: {goal}",
                capability="security.read",
                action="gather_state",
                args={"goal": goal},
            ),
            PlanStep(
                description=f"Run security analysis: {goal}",
                capability="security.analyze",
                action="analyze",
                args={"goal": goal},
                depends_on=["__prev__"],
            ),
            PlanStep(
                description="Generate audit report",
                capability="security.report",
                action="generate_report",
                depends_on=["__prev__"],
            ),
        ]

    if action in ("security.permission",):
        return [
            PlanStep(
                description=f"Review permission request: {goal}",
                capability="security.review",
                action="review_permission",
                args={"goal": goal},
            ),
        ]

    if action in ("system.config",):
        return [
            PlanStep(
                description=f"Read current configuration: {goal}",
                capability="system.read",
                action="read_config",
                args={"goal": goal},
            ),
            PlanStep(
                description=f"Validate proposed change: {goal}",
                capability="security.review",
                action="validate_config_change",
                depends_on=["__prev__"],
            ),
            PlanStep(
                description=f"Apply configuration change: {goal}",
                capability="system.write",
                action="apply_config",
                args={"goal": goal},
                depends_on=["__prev__"],
            ),
        ]

    if action in ("knowledge.query",):
        return [
            PlanStep(
                description=f"Search knowledge base: {goal}",
                capability="memory.read",
                action="knowledge_search",
                args={"query": goal, "top_k": 5},
            ),
        ]

    if action in ("system.schedule",):
        return [
            PlanStep(
                description=f"Parse schedule request: {goal}",
                capability="system.read",
                action="parse_schedule",
                args={"goal": goal},
            ),
            PlanStep(
                description="Create scheduled task entry",
                capability="system.write",
                action="create_schedule_entry",
                depends_on=["__prev__"],
            ),
        ]

    # Fallback: general intent that needs disambiguation
    return [
        PlanStep(
            description=f"Disambiguate intent: {goal}",
            capability="intent.disambiguate",
            action="disambiguate",
            args={"goal": goal},
        ),
        PlanStep(
            description=f"Execute (pending disambiguation): {goal}",
            capability="general.execute",
            action="execute",
            args={"goal": goal},
            depends_on=["__prev__"],
            confidence=0.5,
        ),
    ]


# ─── Planner agent ──────────────────────────────────────────────────────────


class Planner:
    """
    Planning agent for COGNOS/OS.

    Takes an intent, classifies it, decomposes it into steps,
    and returns a structured Plan. Optionally uses the IPC client
    to query the intent-engine for disambiguation or memory for context.
    """

    def __init__(self, ipc_client: AgentIpcClient | None = None):
        self._ipc = ipc_client

    async def plan(self, goal: str, context: dict | None = None) -> Plan:
        """
        Main entry point. Produce a Plan from a natural-language goal.

        Args:
            goal: The user's intent text.
            context: Optional pre-populated context (from memory, prior turns, etc.)

        Returns:
            A Plan with ordered steps, each tagged with capabilities.
        """
        ctx = context or {}
        action = classify_action(goal)
        log.info("Classified goal '%s' as action '%s'", goal, action)

        # If we have an IPC client, optionally query memory for context
        memory_hits = []
        if self._ipc and self._ipc.is_connected:
            try:
                mem_result = await self._ipc.query_memory(
                    query=goal, top_k=5, namespace="planner"
                )
                memory_hits = mem_result.get("hits", [])
            except Exception as e:
                log.warning("Memory query failed (non-fatal): %s", e)

        steps = decompose(action, goal, ctx)

        # Wire step dependencies properly (replace __prev__ tokens)
        self._resolve_dependencies(steps)

        # Calculate overall plan confidence from step confidences
        if steps:
            avg_confidence = sum(s.confidence for s in steps) / len(steps)
        else:
            avg_confidence = 0.0

        # Check if the plan needs disambiguation
        needs_disambig = (
            action == "intent.general"
            and not memory_hits
            and avg_confidence < 0.6
        )

        plan = Plan(
            goal=goal,
            steps=steps,
            confidence=round(avg_confidence, 3),
            requires_disambiguation=needs_disambig,
            context={"memory_hits": memory_hits, "classified_action": action},
        )

        log.info(
            "Plan %s: %d steps, confidence=%.2f, disambiguate=%s",
            plan.plan_id[:8],
            len(steps),
            plan.confidence,
            needs_disambig,
        )
        return plan

    async def plan_with_hal_request(self, goal: str, context: dict | None = None) -> Plan:
        """
        Produce a plan and, if it involves HAL-gated steps, prepare
        the HAL gate request payloads as part of each step's args.
        """
        plan = await self.plan(goal, context)

        for step in plan.steps:
            if step.requires_hal_gate:
                # Make a shallow copy of args so we don't mutate the PlanStep
                step.args = dict(step.args)
                step.args["_hal_gate"] = {
                    "capability": step.capability,
                    "allow_approval": True,
                }

        return plan

    def _resolve_dependencies(self, steps: list[PlanStep]) -> None:
        """
        Replace __prev__ / __prev2__ tokens in depends_on with actual
        step_ids from the preceding steps in the list.
        """
        prev_ids: list[str] = []
        for step in steps:
            resolved_deps: list[str] = []
            for dep in step.depends_on:
                if dep == "__prev__" and prev_ids:
                    resolved_deps.append(prev_ids[-1])
                elif dep == "__prev2__" and len(prev_ids) >= 2:
                    resolved_deps.append(prev_ids[-2])
                else:
                    resolved_deps.append(dep)
            step.depends_on = resolved_deps
            prev_ids.append(step.step_id)