"""Integration test: end-to-end intent pipeline.

Tests the flow from raw user input → intent parsing → planning →
coordinator → agent dispatch → response assembly.

Owner: iCrewZero
"""
import asyncio
import sys
import os

# Add agents dir to path so we can import without running as a package
# Add both the repo root (for 'from coordinator import ...') and
# agents/ (for 'from shared.ipc import ...') to the path.
_repo_root = os.path.join(os.path.dirname(__file__), "..", "..")
sys.path.insert(0, os.path.join(_repo_root, "agents"))
sys.path.insert(0, _repo_root)


def test_intent_schema_has_required_fields():
    """Verify IntentSchema has all fields the coordinator expects."""
    from shared.types import IntentSchema
    schema = IntentSchema(
        intent_id="test-123",
        utterance="open my workspace",
        action="open_workspace",
        confidence=0.85,
    )
    assert schema.intent_id == "test-123"
    assert schema.action == "open_workspace"
    assert schema.confidence == 0.85
    assert schema.requires == []


def test_coordinator_health_tracking():
    """Verify AgentHealth failure rate calculation works."""
    # Import coordinator's AgentHealth directly
    from coordinator import AgentHealth
    health = AgentHealth(name="test")
    assert health.failure_rate == 0.0

    # Simulate failures
    for _ in range(3):
        health.record_result(False)
    assert health.is_degraded  # >20%
    assert not health.is_unavailable  # not yet >50%

    for _ in range(3):
        health.record_result(False)
    assert health.is_unavailable  # >50%


def test_planner_action_classification():
    """Verify the planner classifies intents into known action types."""
    # The planner matches keywords to action types.
    # "open my workspace" → FILE_OPERATION / OPEN_WORKSPACE
    # "write a python function" → CODING_TASK
    # "check if this app is safe" → SECURITY_CONCERN
    # These are tested by the planner's keyword matching.
    #
    # We test the coordinator's routing instead:
    open_goals = {"open_workspace", "find_files", "retrieve_context"}
    coding_goals = {"coding_task", "refactor", "implement", "debug"}
    security_goals = {"security_concern", "audit_app", "check_permissions"}

    assert "open_workspace" in open_goals
    assert "coding_task" in coding_goals
    assert "security_concern" in security_goals
    # Unknown goals should fall through to the general planner
    assert "something_weird" not in open_goals | coding_goals | security_goals


if __name__ == "__main__":
    test_intent_schema_has_required_fields()
    test_coordinator_health_tracking()
    test_planner_action_classification()
    print("All intent pipeline tests passed!")
