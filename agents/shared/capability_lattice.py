"""
shared/capability_lattice.py — Python-side capability enforcement for agents.
Mirrors the Rust capability lattice in agents/shared/ipc.rs.
"""
from __future__ import annotations

AGENT_CAPABILITIES: dict[str, set[str]] = {
    "planner":     {"query_memory", "send_intent_dispatch", "send_memory_query", "send_hal_gate"},
    "memory":      {"read_user_home", "read_file_meta", "read_memory_db", "write_memory_db", "send_memory_result", "send_hal_gate"},
    "security":    {"read_app_behavior_logs", "read_apparmor_logs", "static_analysis", "raise_hal_alert", "send_security_alert", "send_capability_violation"},
    "scheduler":   {"read_ebpf_telemetry", "write_sched_hints", "adjust_cgroup_weights", "switch_cpu_governor", "send_resource_hint"},
    "file":        {"read_user_home", "write_user_home", "read_file_meta", "move_file", "create_file", "list_directory", "delete_file", "open_app", "send_file_operation", "send_hal_gate"},
    "coding":      {"read_user_home", "read_file_meta", "send_hal_gate", "send_memory_query", "send_file_operation"},
    "ui":          {"render_ui", "display_notification", "read_agent_status", "send_hal_gate"},
    "coordinator": {"*"},  # coordinator routes all
}


class CapabilityViolation(Exception):
    def __init__(self, agent: str, capability: str):
        self.agent = agent
        self.capability = capability
        super().__init__(f"Capability violation: agent '{agent}' attempted '{capability}'")


class CapabilityLattice:
    def assert_allowed(self, agent: str, capability: str) -> None:
        allowed = AGENT_CAPABILITIES.get(agent, set())
        if "*" in allowed:
            return
        if capability not in allowed:
            raise CapabilityViolation(agent, capability)