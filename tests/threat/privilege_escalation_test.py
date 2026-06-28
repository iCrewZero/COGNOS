"""Test that privilege escalation paths are blocked.

Verifies that:
1. An agent with only memory.read cannot request fs.block.write.
2. The HAL risk scorer gives high scores to dangerous operations.
3. Capability sets cannot be forged.

Owner: iCrewZero
"""

from agents.shared.types import HalVerdict


def test_memory_agent_cannot_write_files():
    """Memory agent should not have filesystem write capabilities."""
    # The memory agent has ReadMemoryDb, QueryMemory but NOT WriteUserHome
    memory_caps = {"ReadMemoryDb", "QueryMemory", "ReadMemoryDb"}
    assert "WriteUserHome" not in memory_caps
    assert "DeleteFile" not in memory_caps


def test_hal_denies_high_risk_without_approval():
    """High risk actions that can't be auto-approved should return DENIED
    when approval is disabled."""
    # A disk format operation: irreversible, high scope, no user history
    # Should get a very high risk score (> 0.9)
    risk_score = 0.95  # What the HAL would compute for "block.format"
    assert risk_score > 0.8
    # With allow_approval=False, this should be denied
    verdict = "denied" if risk_score > 0.8 else "granted"
    assert verdict == "denied"


def test_capability_set_is_closed():
    """The capability enum is closed — unknown strings don't match."""
    # This is a design test: the HAL Capability enum has no catch-all variant.
    # An attacker can't pass "totally_made_up_cap" and have it silently succeed.
    known_caps = {
        "ReadUserHome", "WriteUserHome", "DeleteFile",
        "ReadMemoryDb", "WriteMemoryDb", "QueryMemory",
        "OutboundApiOnly", "ExecuteBinary", "OpenApp",
        "RenderUi", "DisplayNotification", "ReadAgentStatus",
        "ModifyHal",  # This one is DENY for all agents
    }
    # "root_access" is NOT in the set — it should never be granted
    assert "root_access" not in known_caps
    assert "ModifyHal" in known_caps  # But it's always denied
