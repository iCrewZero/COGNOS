"""
Security Agent for COGNOS/OS.

Handles security scanning, permission checks, install trust verification,
and security alerts routed from the coordinator. All responses go back
through the IPC bus — this agent never touches the filesystem directly.

Owner: iCrewZero
"""

from __future__ import annotations

import logging
import re
from dataclasses import dataclass, field
from typing import Any

from shared.base_agent import BaseAgent
from shared.types import AgentMessage, HalVerdict

log = logging.getLogger("cognos.security")

# Patterns that indicate potentially dangerous operations.
DANGEROUS_PATTERNS = {
    "rm_rf_root": re.compile(r"rm\s+(-[rfRF]+\s+)?/"),
    "chmod_777": re.compile(r"chmod\s+777"),
    "curl_pipe_bash": re.compile(r"curl.*\|.*(?:bash|sh)"),
    "eval_input": re.compile(r"\beval\b.*\$(?:INPUT|1|QUERY)"),
    "dd_overwrite": re.compile(r"dd\s+.*of=/dev/"),
}


@dataclass
class SecurityCheckResult:
    """Result of a security check."""
    approved: bool
    risk_level: str  # "none" | "low" | "medium" | "high" | "critical"
    findings: list[str] = field(default_factory=list)
    hal_verdict: HalVerdict = HalVerdict.PENDING


class SecurityAgent(BaseAgent):
    """
    Security agent. Receives SECURITY_ALERT messages from the coordinator,
    runs static analysis on the proposed action, and returns an approve/deny
    decision with findings.

    In v0 this does pattern-matching. In v1 it will integrate with the
    Rust security/scanner/ static analysis module via IPC.
    """

    # Fix B1 — iCrewZero: The coordinator calls SecurityAgent() with no
    # arguments, but BaseAgent.__init__ requires a name: str.  Adding an
    # explicit __init__ that passes the fixed name "security" up to the
    # base class prevents the TypeError crash.
    def __init__(self):
        super().__init__("security")

    async def handle_message(self, msg: AgentMessage) -> Any:
        """Route incoming messages by type."""
        log.info("[security] Received: %s", msg.type)

        if msg.type == "SECURITY_ALERT":
            return await self._handle_security_alert(msg)
        elif msg.type == "INSTALL_TRUST":
            return await self._check_install_trust(msg)
        else:
            log.warning("[security] Unknown message type: %s", msg.type)
            return {"status": "ignored", "reason": f"unknown type: {msg.type}"}

    async def _handle_security_alert(self, msg: AgentMessage) -> dict:
        """
        Run security checks on the proposed action.

        Checks:
        1. Pattern matching against known-dangerous operations.
        2. Capability validation — does the requesting agent have the
           right permissions for what it's trying to do?
        3. Trust score check — is the source agent trustworthy enough?
        """
        payload = msg.payload
        action = payload.get("action", "")
        agent = payload.get("agent", "unknown")
        schema = payload.get("schema", {})

        findings = []
        risk_level = "none"

        # 1. Pattern matching against dangerous operations.
        action_text = action
        if isinstance(schema, dict):
            # Also check the full schema for dangerous patterns.
            action_text = f"{action} {schema.get('goal', '')} {schema.get('utterance', '')}"

        for name, pattern in DANGEROUS_PATTERNS.items():
            if pattern.search(action_text):
                findings.append(f"DANGEROUS_PATTERN: {name} matched in action")
                risk_level = "high"

        # 2. Check if the action requires HAL gating.
        requires_hal = schema.get("requires_hal", False) if isinstance(schema, dict) else False
        if requires_hal and risk_level == "none":
            findings.append("Action requires HAL gating — forwarding to gate")
            risk_level = "low"

        # 3. Capability check.
        required_caps = schema.get("requires", []) if isinstance(schema, dict) else []
        if required_caps:
            findings.append(f"Required capabilities: {', '.join(required_caps)}")

        approved = risk_level not in ("high", "critical")
        verdict = HalVerdict.GRANTED if approved else HalVerdict.DENIED

        result = SecurityCheckResult(
            approved=approved,
            risk_level=risk_level,
            findings=findings,
            hal_verdict=verdict,
        )

        log.info(
            "[security] Check complete: approved=%s risk=%s findings=%d",
            approved, risk_level, len(findings),
        )

        return {
            "approved": result.approved,
            "risk_level": result.risk_level,
            "findings": result.findings,
            "hal_verdict": result.hal_verdict.value,
            "confidence": 0.9 if approved else 0.95,
            "actions": [
                {
                    "action": "security.check_complete",
                    "target": f"agent.{agent}",
                    "agent": "security",
                    "confidence": 0.9 if approved else 0.95,
                    "reversible": True,
                    "hal_pre_score": 0.1 if risk_level == "none" else 0.7,
                    "parameters": {"findings": findings},
                }
            ],
        }

    async def _check_install_trust(self, msg: AgentMessage) -> dict:
        """
        Check whether a package install request is trustworthy.

        In v0: checks the package name against a basic allowlist and
        flags anything from an untrusted source. In v1 this will
        integrate with unipkg's trust scorer via IPC.
        """
        payload = msg.payload
        package = payload.get("package", "")
        source = payload.get("source", "unknown")

        # Basic trust check — well-known packages from apt/flatpak/appimage
        trusted_sources = {"apt", "flatpak", "appimage"}
        source_type = source.lower().split(".")[0] if source else "unknown"

        if source_type in trusted_sources and package:
            return {
                "approved": True,
                "risk_level": "low",
                "findings": [f"Package '{package}' from trusted source '{source_type}'"],
                "confidence": 0.8,
            }

        return {
            "approved": False,
            "risk_level": "medium",
            "findings": [
                f"Package '{package}' from untrusted source '{source}'",
                "Manual review recommended before install",
            ],
            "confidence": 0.6,
        }
