#!/usr/bin/env python3
"""Generate intent-enum-tight.gbnf: finite fields as literal alternations only."""
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "intent-engine" / "grammar" / "intent-enum-tight.gbnf"

# 0.00 .. 1.00 step 0.05 (21 literals) — cheaper than char-class unit rule
units = [f"{v:.2f}" for v in [i / 100 for i in range(0, 101, 5)]]
unit_rule = "unit ::= " + " | ".join(f'"{u}"' for u in units)

context_keys = [
    "recent_project",
    "pkg_manager",
    "network",
    "contact",
    "cloud_reasoning",
    "repo_state",
    "user_clarification",
]
context_key_rule = "context-key ::= " + " | ".join(f'"\\"{k}\\""' for k in context_keys)

content = f"""# Tight enum variant of intent.gbnf for GBNF throughput experiments.
# Finite fields use literal alternations only (no open char-class strings).

root ::= root-clear | root-disamb

root-clear ::= "{{" space "\\"goal\\":" space goal-string space "," space "\\"domain\\":" space domain-value space "," space "\\"confidence\\":" space unit space "," space "\\"ambiguity_score\\":" space unit space "," space "\\"risk_estimate\\":" space unit space "," space "\\"required_context\\":" space string-array space "," space "\\"candidate_actions\\":" space candidate-array space "," space "\\"disambiguation_required\\":" space "false" space "," space "\\"disambiguation_question\\":" space "null" space "," space "\\"hal_pre_score\\":" space unit space "," space "\\"escalate_to_cloud\\":" space boolean space "}}"

root-disamb ::= "{{" space "\\"goal\\":" space goal-string space "," space "\\"domain\\":" space domain-value space "," space "\\"confidence\\":" space unit space "," space "\\"ambiguity_score\\":" space unit space "," space "\\"risk_estimate\\":" space unit space "," space "\\"required_context\\":" space string-array space "," space "\\"candidate_actions\\":" space candidate-array-nonempty space "," space "\\"disambiguation_required\\":" space "true" space "," space "\\"disambiguation_question\\":" space question-literal space "," space "\\"hal_pre_score\\":" space unit space "," space "\\"escalate_to_cloud\\":" space boolean space "}}"

candidate-array ::= "[" space ( candidate ( space "," space candidate )? )? space "]"
candidate-array-nonempty ::= "[" space candidate ( space "," space candidate )+ space "]"
candidate ::= "{{" space "\\"action\\":" space action-string space "," space "\\"target\\":" space path-literal space "," space "\\"confidence\\":" space unit space "," space "\\"recency_score\\":" space unit space "}}"

string-array ::= "[" space ( context-key ( space "," space context-key )* )? space "]"

goal-string ::= "\\"create_dir\\"" | "\\"open_workspace\\"" | "\\"open_file\\"" | "\\"delete_path\\"" | "\\"pkg.install\\"" | "\\"out_of_scope\\"" | "\\"install_package\\""
action-string ::= "\\"create_dir\\"" | "\\"create_file\\"" | "\\"open_files\\"" | "\\"install_package\\"" | "\\"delete_path\\""
domain-value ::= "null" | "\\"system\\"" | "\\"robotics\\"" | "\\"finance\\""
boolean ::= "true" | "false"

{unit_rule}

{context_key_rule}

path-literal ::= "\\"/tmp\\"" | "\\"/tmp/test\\"" | "\\"/boot\\"" | "\\"/etc\\"" | "\\"~/projects/robo/motor.py\\"" | "\\"~/finance/budget.xlsx\\"" | "\\"ffmpeg\\""
question-literal ::= "\\"Motor driver or PID tuning?\\"" | "\\"Which project?\\"" | "\\"Which file?\\""

space ::= " "
"""

OUT.write_text(content, encoding="utf-8")
print(f"wrote {OUT} ({len(content)} bytes)")
