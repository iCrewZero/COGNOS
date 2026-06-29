"""
shared/types.py — Shared type definitions for COGNOS/OS Python agents.
Used by coordinator, coding agent, memory agent, and the IPC client.
"""

from __future__ import annotations

from dataclasses import dataclass, field, asdict
from typing import Any, Optional
from enum import Enum


class IntentGoal(str, Enum):
    """Canonical intent goal types, matching the Rust intent-engine schema."""
    OPEN_WORKSPACE = "open_workspace"
    FIND_FILES = "find_files"
    RETRIEVE_CONTEXT = "retrieve_context"
    CODING_TASK = "coding_task"
    REFACTOR = "refactor"
    IMPLEMENT = "implement"
    DEBUG = "debug"
    SECURITY_CONCERN = "security_concern"
    AUDIT_APP = "audit_app"
    CHECK_PERMISSIONS = "check_permissions"
    INSTALL_PACKAGE = "install_package"
    UNINSTALL_PACKAGE = "uninstall_package"
    SYSTEM_CONFIG = "system_config"
    MODIFY_SETTINGS = "modify_settings"
    GENERAL = "general"


class HalVerdict(str, Enum):
    """HAL gate verdict, matching the Rust HalGateResponse.status."""
    GRANTED = "granted"
    DENIED = "denied"
    APPROVAL_REQUIRED = "approval_required"
    FAILED = "failed"
    PENDING = "pending"


@dataclass
class AgentMessage:
    """Message passed between agents via the IPC bus."""
    type: str
    payload: dict
    sender: str = ""
    trace_id: str = ""
    timestamp: float = 0.0


@dataclass
class IntentSchema:
    """Parsed intent ready for dispatch. Matches the protobuf Intent message."""
    intent_id: str
    utterance: str
    action: str
    args: dict = field(default_factory=dict)
    confidence: float = 0.0
    requires: list[str] = field(default_factory=list)
    session_id: str = ""
    trace_id: str = ""


@dataclass
class HalGateResult:
    """Result from a HAL gate check. Matches HalGateResponse."""
    status: HalVerdict = HalVerdict.PENDING
    grant_token: str = ""
    risk_score: float = 0.0
    violation: Optional[dict] = None
    trace_id: str = ""


@dataclass
class MemoryHit:
    """A single memory search result."""
    object_id: str
    score: float
    payload: dict
    tags: list[str] = field(default_factory=list)


@dataclass
class MemoryResult:
    """Result of a memory query. Matches MemoryResponse."""
    hits: list[MemoryHit] = field(default_factory=list)
    total: int = 0
    elapsed_ns: int = 0
    trace_id: str = ""


@dataclass
class HeartbeatPayload:
    """Agent heartbeat, matching the protobuf Heartbeat message."""
    agent_id: str
    seq: int = 0
    load_avg: float = 0.0
    status: str = "alive"


def message_to_dict(msg: AgentMessage) -> dict:
    """Serialize an AgentMessage to a plain dict for IPC transport."""
    return asdict(msg)


def dict_to_message(data: dict) -> AgentMessage:
    """Deserialize a dict back into an AgentMessage."""
    return AgentMessage(**{k: v for k, v in data.items() if k in AgentMessage.__dataclass_fields__})