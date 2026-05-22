"""
Cognitive Context Preloader — Restraint Model for COGNOS/OS.

Ensures the preloader stays helpful without crossing into creepy.
When in doubt, HoldBack. An OS that does nothing unexpected is more
trustworthy than one that is occasionally brilliant.
"""

from __future__ import annotations

import json
import logging
import re
from dataclasses import dataclass, field, asdict
from datetime import datetime, UTC
from enum import Enum
from pathlib import Path
from typing import NamedTuple

log = logging.getLogger("cognos.restraint_model")

COGNOS_DIR = Path.home() / ".cognos"
AUDIT_LOG = COGNOS_DIR / "audit.log"
ACCEPTANCE_FILE = COGNOS_DIR / "predictor" / "domain_acceptance.json"

# Domains that are always held back regardless of acceptance history
HIGH_SENSITIVITY_DOMAINS = {"personal", "finance", "health", "private"}

# File path substrings that trigger HoldBack regardless of domain
SENSITIVE_PATH_PATTERNS = [
    re.compile(p, re.IGNORECASE) for p in [
        r"diary", r"journal", r"private", r"personal",
        r"\.kdbx$", r"password", r"credential", r"therapy",
        r"medical", r"secret",
    ]
]

# Domains where late-night preloading is acceptable
NIGHT_OK_DOMAINS = {"coding", "work"}

# Required number of positive interactions before a domain is unlocked
ACCEPTANCE_THRESHOLD = 3


# ─── Types ───────────────────────────────────────────────────────────────────

@dataclass
class ContextPrediction:
    predicted_action: str
    confidence: float           # 0.0–1.0
    domain: str                 # e.g. "coding", "personal"
    file_paths: list[str]
    trigger_signal: str         # what pattern triggered this
    time_of_day: int            # hour 0–23


class PreloadAction(Enum):
    Preload = "preload"
    HoldBack = "holdback"


@dataclass
class PreloadDecision:
    action: PreloadAction
    reason: str                 # always populated, even for Preload
    prediction: ContextPrediction


# ─── Domain acceptance tracker ───────────────────────────────────────────────

class DomainAcceptanceTracker:
    """
    Tracks which domains the user has positively engaged with.
    New domains are locked — they must earn trust through 3 explicit
    positive interactions before predictions surface.
    """

    def __init__(self):
        self._data: dict[str, dict] = self._load()
        self._locked: set[str] = self._load_locked()

    def is_unlocked(self, domain: str) -> bool:
        """True if the domain has enough positive interactions."""
        if domain in self._locked:
            return False
        count = self._data.get(domain, {}).get("positive_count", 0)
        return count >= ACCEPTANCE_THRESHOLD

    def record_acceptance(self, domain: str) -> None:
        """Called when user positively responds to a preload in this domain."""
        if domain not in self._data:
            self._data[domain] = {"positive_count": 0, "locked": False}
        self._data[domain]["positive_count"] += 1
        self._save()
        log.info("Domain '%s' acceptance count: %d", domain,
                 self._data[domain]["positive_count"])

    def lock_domain(self, domain: str) -> None:
        """User explicitly disables predictions for a domain."""
        self._locked.add(domain)
        if domain not in self._data:
            self._data[domain] = {"positive_count": 0}
        self._data[domain]["locked"] = True
        self._save()

    def unlock_domain(self, domain: str) -> None:
        """Re-enable predictions for a domain."""
        self._locked.discard(domain)
        if domain in self._data:
            self._data[domain]["locked"] = False
        self._save()

    def _load(self) -> dict:
        if ACCEPTANCE_FILE.exists():
            try:
                return json.loads(ACCEPTANCE_FILE.read_text())
            except (json.JSONDecodeError, OSError):
                pass
        return {}

    def _load_locked(self) -> set[str]:
        return {
            d for d, v in self._data.items()
            if v.get("locked", False)
        }

    def _save(self) -> None:
        ACCEPTANCE_FILE.parent.mkdir(parents=True, exist_ok=True)
        ACCEPTANCE_FILE.write_text(json.dumps(self._data, indent=2))


# ─── Restraint model ─────────────────────────────────────────────────────────

class RestraintModel:
    """
    The gatekeeper for cognitive context preloading.

    call should_preload(prediction) → PreloadDecision
    before surfacing any preloaded context to the user.
    """

    def __init__(self):
        self._acceptance = DomainAcceptanceTracker()

    def should_preload(self, prediction: ContextPrediction) -> PreloadDecision:
        """
        Decide whether to preload a predicted context.
        HoldBack if ANY exclusion condition is true.
        """

        # 1. Confidence threshold
        if prediction.confidence < 0.85:
            return self._hold_back(
                prediction,
                f"Confidence {prediction.confidence:.2f} is below threshold 0.85"
            )

        # 2. High-sensitivity domain always blocked
        if prediction.domain in HIGH_SENSITIVITY_DOMAINS:
            return self._hold_back(
                prediction,
                f"Domain '{prediction.domain}' is high-sensitivity — never preloaded"
            )

        # 3. Sensitive file paths
        for path in prediction.file_paths:
            for pattern in SENSITIVE_PATH_PATTERNS:
                if pattern.search(path):
                    return self._hold_back(
                        prediction,
                        f"File path '{Path(path).name}' matches sensitive pattern"
                    )

        # 4. Late-night personal content
        hour = prediction.time_of_day
        if (22 <= hour <= 23 or 0 <= hour <= 6) and prediction.domain not in NIGHT_OK_DOMAINS:
            return self._hold_back(
                prediction,
                f"Late night ({hour:02d}:00) preload in non-work domain '{prediction.domain}'"
            )

        # 5. Reading file content (not just opening app)
        if "read" in prediction.predicted_action.lower():
            return self._hold_back(
                prediction,
                "Predicted action involves reading file content — only app launches allowed"
            )

        # 6. Domain not yet accepted by user
        if not self._acceptance.is_unlocked(prediction.domain):
            return self._hold_back(
                prediction,
                f"Domain '{prediction.domain}' not yet unlocked "
                f"(needs {ACCEPTANCE_THRESHOLD} positive interactions)"
            )

        # ── All checks passed ─────────────────────────────────────────────────
        decision = PreloadDecision(
            action=PreloadAction.Preload,
            reason="All restraint checks passed",
            prediction=prediction,
        )
        self._audit(decision)
        return decision

    def record_acceptance(self, domain: str) -> None:
        """User positively responded to a preload — record it."""
        self._acceptance.record_acceptance(domain)

    def lock_domain(self, domain: str) -> None:
        """cognos predict disable --scope <domain>"""
        self._acceptance.lock_domain(domain)
        log.info("Predictions disabled for domain '%s'", domain)

    def unlock_domain(self, domain: str) -> None:
        """cognos predict enable --scope <domain>"""
        self._acceptance.unlock_domain(domain)

    # ─── Private ─────────────────────────────────────────────────────────────

    def _hold_back(self, prediction: ContextPrediction, reason: str) -> PreloadDecision:
        decision = PreloadDecision(
            action=PreloadAction.HoldBack,
            reason=reason,
            prediction=prediction,
        )
        self._audit(decision)
        log.debug("HoldBack: %s [%s]", prediction.predicted_action, reason)
        return decision

    def _audit(self, decision: PreloadDecision) -> None:
        entry = {
            "ts": datetime.now(UTC).isoformat(),
            "agent": "preloader",
            "action": f"preload_{decision.action.value}",
            "target": decision.prediction.predicted_action,
            "outcome": decision.action.value,
            "reason": decision.reason,
            "domain": decision.prediction.domain,
            "confidence": decision.prediction.confidence,
        }
        try:
            AUDIT_LOG.parent.mkdir(parents=True, exist_ok=True)
            with open(AUDIT_LOG, "a") as f:
                f.write(json.dumps(entry) + "\n")
        except OSError as e:
            log.error("Audit write failed: %s", e)


# ─── Tests ───────────────────────────────────────────────────────────────────

if __name__ == "__main__":
    import unittest

    class TestRestraintModel(unittest.TestCase):
        def setUp(self):
            self.model = RestraintModel()
            # Unlock 'coding' domain by faking 3 acceptances
            for _ in range(3):
                self.model.record_acceptance("coding")

        def _pred(self, **kwargs) -> ContextPrediction:
            defaults = {
                "predicted_action": "open_workspace",
                "confidence": 0.90,
                "domain": "coding",
                "file_paths": ["~/projects/motor.py"],
                "trigger_signal": "9am pattern",
                "time_of_day": 9,
            }
            defaults.update(kwargs)
            return ContextPrediction(**defaults)

        def test_low_confidence_holds_back(self):
            d = self.model.should_preload(self._pred(confidence=0.70))
            self.assertEqual(d.action, PreloadAction.HoldBack)

        def test_personal_domain_always_holds_back(self):
            d = self.model.should_preload(self._pred(domain="personal", confidence=0.99))
            self.assertEqual(d.action, PreloadAction.HoldBack)

        def test_sensitive_file_path_holds_back(self):
            d = self.model.should_preload(self._pred(file_paths=["~/diary/entry.md"]))
            self.assertEqual(d.action, PreloadAction.HoldBack)

        def test_late_night_non_work_holds_back(self):
            d = self.model.should_preload(self._pred(domain="gaming", time_of_day=23))
            self.assertEqual(d.action, PreloadAction.HoldBack)

        def test_late_night_coding_is_ok(self):
            d = self.model.should_preload(self._pred(domain="coding", time_of_day=23))
            # coding is in NIGHT_OK_DOMAINS, and domain is unlocked, so should preload
            self.assertEqual(d.action, PreloadAction.Preload)

        def test_new_domain_holds_back(self):
            d = self.model.should_preload(self._pred(domain="research"))
            self.assertEqual(d.action, PreloadAction.HoldBack)
            self.assertIn("not yet unlocked", d.reason)

        def test_read_action_holds_back(self):
            d = self.model.should_preload(self._pred(predicted_action="read_file_content"))
            self.assertEqual(d.action, PreloadAction.HoldBack)

        def test_valid_coding_preload_passes(self):
            d = self.model.should_preload(self._pred())
            self.assertEqual(d.action, PreloadAction.Preload)

        def test_credentials_file_holds_back(self):
            d = self.model.should_preload(self._pred(file_paths=["~/credentials.env"]))
            self.assertEqual(d.action, PreloadAction.HoldBack)

    unittest.main(verbosity=2)
