#!/usr/bin/env python3
import json
import sys
from pathlib import Path

ROOT = Path("/mnt/f/Software Engineering/COGNOS/intent-engine/tests/golden")
report_path = Path(sys.argv[1]) if len(sys.argv) > 1 else Path("/tmp/vllm-golden-baseline.json")
report = json.loads(report_path.read_text())
for c in report["cases"]:
    exp = json.loads((ROOT / c["name"]).read_text())["expected_intent"]
    act = c.get("actual", {})
    s = c["scores"]
    print(
        f"{c['name']}: goal {exp['goal']} -> {act.get('goal')} ({s['goal_ok']}) "
        f"disamb exp={exp['disambiguation_required']} got={act.get('disambiguation_required')} ({s['disambig_ok']}) "
        f"cands exp={len(exp.get('candidate_actions',[]))} got={len(act.get('candidate_actions') or [])} ({s['candidates_ok']})"
    )
