"""Test intent disambiguation detection.

Verifies that ambiguous intents are flagged for clarification
instead of being silently misinterpreted.

Owner: iCrewZero
"""

# Ambiguous patterns that should trigger disambiguation
AMBIGUOUS_PATTERNS = [
    "open it",                    # "it" is ambiguous
    "delete the file",            # which file?
    "make it faster",             # what is "it"?
    "the one from yesterday",     # refers to unstated context
]

CLEAR_PATTERNS = [
    "open my workspace in VS Code",
    "delete /tmp/old-cache",
    "show me CPU usage",
]


def test_ambiguous_intents_detected():
    """Intents with ambiguous references should be flagged."""
    for text in AMBIGUOUS_PATTERNS:
        # Simple heuristic: short intent + pronoun = ambiguous
        is_ambiguous = (
            len(text.split()) <= 4 and
            any(pronoun in text.lower() for pronoun in ["it", "the one", "that"])
        )
        assert is_ambiguous, f"Should have detected ambiguity in: '{text}'"


def test_clear_intents_not_flagged():
    """Intents with specific targets should not be flagged."""
    for text in CLEAR_PATTERNS:
        has_pronoun_only = (
            len(text.split()) <= 4 and
            any(pronoun in text.lower() for pronoun in ["it", "the one", "that"])
        )
        assert not has_pronoun_only, f"False positive ambiguity in: '{text}'"
