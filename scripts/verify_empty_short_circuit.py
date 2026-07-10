#!/usr/bin/env python3
"""Quick check: harness empty_input short-circuit matches golden fixtures."""
from __future__ import annotations

import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))

from eval_golden_quality import (  # noqa: E402
    await_input_llm_payload,
    is_empty_after_normalize,
    score_case,
)

assert is_empty_after_normalize("")
assert is_empty_after_normalize("   ")
assert is_empty_after_normalize("   ...   ")
assert not is_empty_after_normalize("zzz")

for rel in (
    "intent-engine/tests/golden/14_empty_input.json",
    "intent-engine/tests/validation/v17_empty_input.json",
):
    case = json.loads((ROOT / rel).read_text(encoding="utf-8"))
    act = await_input_llm_payload()
    s = score_case(act, case["expected_intent"])
    assert s["goal_ok"] and s["disambig_ok"], rel
    print(f"OK {rel}")

print("empty_short_circuit_ok")
