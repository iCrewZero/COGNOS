#!/usr/bin/env python3
"""Quality evaluation harness: vLLM + XGrammar vs golden/validation fixtures.

Measures only — does not modify prompts or repair outputs.
"""
from __future__ import annotations

import argparse
import json
import string
import subprocess
import sys
import time
from collections import Counter, defaultdict
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

ROOT = Path("/mnt/f/Software Engineering/COGNOS")
DEFAULT_GOLDEN_DIR = ROOT / "intent-engine/tests/golden"
DEFAULT_VALIDATION_DIR = ROOT / "intent-engine/tests/validation"
DEFAULT_SCHEMA = ROOT / "intent-engine/schema/intent-llm-output.schema.json"
EVAL_SCHEMA = ROOT / "intent-engine/schema/intent-golden-eval.schema.json"
PROD_SCHEMA = DEFAULT_SCHEMA
DEFAULT_MODEL = "Qwen/Qwen2.5-7B-Instruct-AWQ"
DEFAULT_MARKDOWN = ROOT / "docs/GOLDEN_BASELINE.md"
DEFAULT_OVERFIT_MD = ROOT / "docs/OVERFIT_CHECK.md"
DEFAULT_JSON = ROOT / "tmp/eval_golden_quality.json"
DEFAULT_OVERFIT_JSON = ROOT / "tmp/overfit_check.json"
DEFAULT_ALIGNED_MD = ROOT / "docs/PROD_SCHEMA_ALIGNED.md"
DEFAULT_ALIGNED_JSON = ROOT / "tmp/prod_schema_aligned.json"
DEFAULT_QUALITY_FINAL_MD = ROOT / "docs/QUALITY_FINAL.md"
DEFAULT_QUALITY_FINAL_JSON = ROOT / "tmp/quality_final.json"

LLM_KEYS = [
    "goal",
    "domain",
    "confidence",
    "ambiguity_score",
    "risk_estimate",
    "required_context",
    "candidate_actions",
    "disambiguation_required",
    "disambiguation_question",
    "hal_pre_score",
    "escalate_to_cloud",
]


def normalize_input(raw: str) -> str:
    """Mirror intent-engine/src/tokenizer.rs::normalize (deterministic)."""
    lowered = raw.strip().lower()
    stripped = "".join(
        " " if (c.isascii() and c in string.punctuation) else c for c in lowered
    )
    return " ".join(stripped.split())


def is_empty_after_normalize(raw: str) -> bool:
    return normalize_input(raw) == ""


def await_input_llm_payload() -> dict[str, Any]:
    """LLM-emitted fields for empty/whitespace input — matches parser.rs await_input_for_empty."""
    return {
        "goal": "await_input",
        "domain": None,
        "confidence": 0.1,
        "ambiguity_score": 0.5,
        "risk_estimate": 0.0,
        "required_context": ["user_clarification"],
        "candidate_actions": [],
        "disambiguation_required": False,
        "disambiguation_question": None,
        "hal_pre_score": 0.0,
        "escalate_to_cloud": True,
    }


def short_circuit_row(case: dict[str, Any], *, label: str) -> dict[str, Any]:
    """Build a result row for empty_input short-circuit (no LLM call)."""
    actual = await_input_llm_payload()
    content = json.dumps(actual, ensure_ascii=False)
    expected = llm_fields(case["expected_intent"])
    row: dict[str, Any] = {
        "name": case["_name"],
        "input": case["input"],
        "expected_goal": expected.get("goal"),
        "expected_disambig": expected.get("disambiguation_required"),
        "short_circuit": "empty_input",
        "cold": {"wall_ms": 0, "tokens": 0, "tok_s": 0.0, "content": content},
        "hot": {"wall_ms": 0, "tokens": 0, "tok_s": 0.0, "content": content},
        "actual": actual,
        "scores": score_case(actual, case["expected_intent"]),
        "produced_goal": actual.get("goal"),
        "produced_disambig": actual.get("disambiguation_required"),
        "parse_ok": True,
    }
    s = row["scores"]
    print(
        f"[{label}] {case['_name']}: SHORT_CIRCUIT empty_input goal={s['goal_ok']} "
        f"disambig={s['disambig_ok']} cands={s['candidates_ok']}"
    )
    return row


def load_cases(directory: Path, label: str, expected_count: int | None = None) -> list[dict[str, Any]]:
    paths = sorted(directory.glob("*.json"))
    if not paths:
        raise SystemExit(f"{label}: no fixtures in {directory}")
    if expected_count is not None and len(paths) != expected_count:
        raise SystemExit(f"{label}: expected {expected_count} fixtures, found {len(paths)} in {directory}")
    cases = []
    for path in paths:
        data = json.loads(path.read_text(encoding="utf-8"))
        data["_name"] = path.name
        data["_path"] = str(path)
        cases.append(data)
    return cases


def build_prompt(case: dict[str, Any]) -> str:
    return subprocess.check_output(
        ["cargo", "run", "--quiet", "--example", "print_prompt_golden", "--", case["_path"]],
        cwd=ROOT,
        text=True,
    )


def llm_fields(intent: dict[str, Any]) -> dict[str, Any]:
    return {k: intent.get(k) for k in LLM_KEYS}


def in_upper_half(value: Any) -> bool:
    return float(value or 0) >= 0.5


def score_case(actual: dict[str, Any], expected: dict[str, Any]) -> dict[str, bool]:
    exp = llm_fields(expected)
    act = actual

    goal_ok = act.get("goal") == exp.get("goal")
    disambig_ok = act.get("disambiguation_required") == exp.get("disambiguation_required")

    exp_cands = exp.get("candidate_actions") or []
    act_cands = act.get("candidate_actions") or []
    candidates_ok = (len(act_cands) == 0) if len(exp_cands) == 0 else len(act_cands) > 0

    confidence_ok = in_upper_half(act.get("confidence")) == in_upper_half(exp.get("confidence"))
    ambiguity_ok = in_upper_half(act.get("ambiguity_score")) == in_upper_half(
        exp.get("ambiguity_score")
    )
    risk_ok = in_upper_half(act.get("risk_estimate")) == in_upper_half(exp.get("risk_estimate"))

    return {
        "goal_ok": goal_ok,
        "disambig_ok": disambig_ok,
        "candidates_ok": candidates_ok,
        "confidence_ok": confidence_ok,
        "ambiguity_ok": ambiguity_ok,
        "risk_ok": risk_ok,
    }


def parse_llm_json(raw: str) -> dict[str, Any]:
    text = raw.strip()
    if text.startswith("```"):
        lines = [ln for ln in text.splitlines() if not ln.strip().startswith("```")]
        text = "\n".join(lines).strip()
    return json.loads(text)


def summarize(results: list[dict[str, Any]]) -> dict[str, Any]:
    n = len(results)

    def count(key: str) -> int:
        return sum(1 for r in results if r.get("scores", {}).get(key))

    hot_rows = [r for r in results if r.get("hot")]
    hot_walls = [r["hot"]["wall_ms"] for r in hot_rows if r["hot"].get("wall_ms")]
    hot_tok_s = [r["hot"]["tok_s"] for r in hot_rows if r["hot"].get("tok_s")]

    return {
        "cases": n,
        "goal_correct": count("goal_ok"),
        "disambig_correct": count("disambig_ok"),
        "candidates_correct": count("candidates_ok"),
        "confidence_correct": count("confidence_ok"),
        "ambiguity_correct": count("ambiguity_ok"),
        "risk_correct": count("risk_ok"),
        "avg_hot_wall_ms": round(sum(hot_walls) / len(hot_walls)) if hot_walls else None,
        "avg_hot_tok_s": round(sum(hot_tok_s) / len(hot_tok_s), 2) if hot_tok_s else None,
    }


def confusion_matrix(results: list[dict[str, Any]]) -> dict[str, dict[str, int]]:
    matrix: dict[str, dict[str, int]] = defaultdict(lambda: defaultdict(int))
    for row in results:
        exp = row.get("expected_goal", "?")
        act = (row.get("actual") or {}).get("goal", "(parse_error)")
        matrix[exp][act] += 1
    return {k: dict(v) for k, v in sorted(matrix.items())}


def run_suite(
    *,
    label: str,
    cases: list[dict[str, Any]],
    llm: Any,
    sp: Any,
    global_warmup_prompt: str | None,
) -> list[dict[str, Any]]:
    if global_warmup_prompt:
        llm.generate([global_warmup_prompt], sp, use_tqdm=False)

    results: list[dict[str, Any]] = []
    for case in cases:
        if is_empty_after_normalize(case.get("input") or ""):
            results.append(short_circuit_row(case, label=label))
            continue

        prompt = build_prompt(case)
        expected = llm_fields(case["expected_intent"])

        cold_t0 = time.perf_counter()
        cold_out = llm.generate([prompt], sp, use_tqdm=False)[0].outputs[0]
        cold_ms = round((time.perf_counter() - cold_t0) * 1000)
        cold_tokens = len(cold_out.token_ids)

        hot_t0 = time.perf_counter()
        hot_out = llm.generate([prompt], sp, use_tqdm=False)[0].outputs[0]
        hot_ms = round((time.perf_counter() - hot_t0) * 1000)
        hot_tokens = len(hot_out.token_ids)

        content = hot_out.text.strip()
        row: dict[str, Any] = {
            "name": case["_name"],
            "input": case["input"],
            "expected_goal": expected.get("goal"),
            "expected_disambig": expected.get("disambiguation_required"),
            "cold": {
                "wall_ms": cold_ms,
                "tokens": cold_tokens,
                "tok_s": round(cold_tokens / (cold_ms / 1000), 2) if cold_ms else 0,
                "content": cold_out.text.strip(),
            },
            "hot": {
                "wall_ms": hot_ms,
                "tokens": hot_tokens,
                "tok_s": round(hot_tokens / (hot_ms / 1000), 2) if hot_ms else 0,
                "content": content,
            },
        }
        try:
            actual = parse_llm_json(content)
            row["actual"] = actual
            row["scores"] = score_case(actual, case["expected_intent"])
            row["produced_goal"] = actual.get("goal")
            row["produced_disambig"] = actual.get("disambiguation_required")
            row["parse_ok"] = True
        except Exception as e:  # noqa: BLE001
            row["parse_ok"] = False
            row["error"] = str(e)
            row["produced_goal"] = None
            row["produced_disambig"] = None
            row["scores"] = {
                "goal_ok": False,
                "disambig_ok": False,
                "candidates_ok": False,
                "confidence_ok": False,
                "ambiguity_ok": False,
                "risk_ok": False,
            }
        results.append(row)
        s = row["scores"]
        print(
            f"[{label}] {case['_name']}: goal={s['goal_ok']} disambig={s['disambig_ok']} "
            f"cands={s['candidates_ok']} hot_ms={hot_ms} tok_s={row['hot']['tok_s']}"
        )
    return results


def render_confusion_md(title: str, matrix: dict[str, dict[str, int]]) -> str:
    produced = sorted({p for row in matrix.values() for p in row})
    lines = [f"### {title}", "", "| expected \\ produced | " + " | ".join(produced) + " |", "|---|" + "|".join(["---"] * len(produced)) + "|"]
    for exp, row in matrix.items():
        cells = [str(row.get(p, 0)) for p in produced]
        lines.append(f"| `{exp}` | " + " | ".join(cells) + " |")
    lines.append("")
    return "\n".join(lines)


def render_case_table(title: str, results: list[dict[str, Any]]) -> str:
    lines = [
        f"### {title}",
        "",
        "| Cas | Input | Exp goal | Prod goal | Exp disambig | Prod disambig | Résultat |",
        "|-----|-------|----------|-----------|--------------|---------------|----------|",
    ]
    for r in results:
        s = r["scores"]
        ok = s["goal_ok"] and s["disambig_ok"] and s["candidates_ok"]
        mark = "✅" if ok else "❌"
        inp = (r["input"] or "").replace("|", "\\|")[:60]
        lines.append(
            f"| `{r['name']}` | {inp} | `{r['expected_goal']}` | `{r['produced_goal']}` | "
            f"{r['expected_disambig']} | {r['produced_disambig']} | {mark} |"
        )
    lines.append("")
    return "\n".join(lines)


def render_raw_json_block(title: str, results: list[dict[str, Any]]) -> str:
    lines = [f"### {title}", ""]
    for r in results:
        lines.append(f"#### `{r['name']}` — input: `{r['input']}`")
        lines.append("")
        lines.append("```json")
        lines.append(r["hot"]["content"] if r.get("hot") else "{}")
        lines.append("```")
        lines.append("")
    return "\n".join(lines)


def render_markdown(report: dict[str, Any]) -> str:
    g = report["golden"]["summary"]
    v = report["validation"]["summary"]
    ts = report["meta"]["timestamp"]
    lines = [
        "# Golden Quality Baseline",
        "",
        f"**Mesuré :** {ts} (UTC)",
        "",
        "## Méthodologie",
        "",
        "- Moteur : vLLM + XGrammar (`StructuredOutputsParams`)",
        f"- Modèle : `{report['meta']['model']}`",
        f"- Schéma : `{report['meta']['schema']}` (tâche 4 — production)",
        f"- Prompt système : **inchangé** (`intent-engine/src/prompt.rs`, via `print_prompt_golden`)",
        "- Scoring : goal exact ; disambiguation_required exact ; candidate_actions présent/absent ;",
        "  confidence / ambiguity_score / risk_estimate = même moitié [<0.5 vs ≥0.5] (tolérant)",
        "- Latence : **froid** = 1er passage du cas ; **chaud** = 2e passage immédiat (score sur sortie chaude)",
        "",
        "## Scores agrégés",
        "",
        "| Jeu | n | goal | disambiguation | candidate_actions | conf. moitié | ambig. moitié | risk moitié | hot tok/s moy. |",
        "|-----|---|------|----------------|-----------------|--------------|---------------|-------------|----------------|",
        f"| Golden | {g['cases']} | {g['goal_correct']}/{g['cases']} | {g['disambig_correct']}/{g['cases']} | "
        f"{g['candidates_correct']}/{g['cases']} | {g['confidence_correct']}/{g['cases']} | "
        f"{g['ambiguity_correct']}/{g['cases']} | {g['risk_correct']}/{g['cases']} | {g['avg_hot_tok_s']} |",
        f"| Validation | {v['cases']} | {v['goal_correct']}/{v['cases']} | {v['disambig_correct']}/{v['cases']} | "
        f"{v['candidates_correct']}/{v['cases']} | {v['confidence_correct']}/{v['cases']} | "
        f"{v['ambiguity_correct']}/{v['cases']} | {v['risk_correct']}/{v['cases']} | {v['avg_hot_tok_s']} |",
        "",
        "> **Overfit instrument :** comparer golden vs validation au fil du temps. Si golden monte sans validation, suspect.",
        "",
        render_confusion_md("Matrice de confusion — goal (golden)", report["golden"]["confusion"]),
        render_confusion_md("Matrice de confusion — goal (validation)", report["validation"]["confusion"]),
        "## Latence par cas (ms)",
        "",
        "| Cas | Froid | Chaud | tok/s chaud |",
        "|-----|-------|-------|-------------|",
    ]
    for section, title in [("golden", "Golden"), ("validation", "Validation")]:
        for r in report[section]["cases"]:
            lines.append(
                f"| `{r['name']}` ({title}) | {r['cold']['wall_ms']} | {r['hot']['wall_ms']} | {r['hot']['tok_s']} |"
            )
    lines.extend(
        [
            "",
            render_case_table("Tableau par cas — golden", report["golden"]["cases"]),
            render_case_table("Tableau par cas — validation", report["validation"]["cases"]),
            "## JSON brut produit (sortie chaude)",
            "",
            render_raw_json_block("Golden (15)", report["golden"]["cases"]),
            render_raw_json_block("Validation", report["validation"]["cases"]),
            "## Notes de contrainte schéma",
            "",
            "Le schéma production (`docs/GOAL_TAXONOMY.md`, v2) autorise les 13 goals canoniques ; "
            "les scores goal reflètent la capacité sémantique du modèle sous cette contrainte alignée.",
            "",
        ]
    )
    return "\n".join(lines)


def make_sampling_params(schema_path: Path) -> Any:
    from vllm.sampling_params import SamplingParams, StructuredOutputsParams

    schema = json.loads(schema_path.read_text(encoding="utf-8"))
    structured = StructuredOutputsParams(json=schema)
    return SamplingParams(
        temperature=0.0,
        max_tokens=448,
        structured_outputs=structured,
    )


def suite_report(
    *,
    label: str,
    cases: list[dict[str, Any]],
    llm: Any,
    sp: Any,
    schema_path: Path,
    warmup_prompt: str | None = None,
) -> dict[str, Any]:
    results = run_suite(
        label=label,
        cases=cases,
        llm=llm,
        sp=sp,
        global_warmup_prompt=warmup_prompt,
    )
    return {
        "label": label,
        "schema": str(schema_path),
        "summary": summarize(results),
        "confusion": confusion_matrix(results),
        "cases": results,
    }


def count_pure_disambig_failures(results: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """Cases where goal matches but disambiguation_required does not."""
    out = []
    for r in results:
        s = r.get("scores") or {}
        if s.get("goal_ok") and not s.get("disambig_ok"):
            out.append(r)
    return out


def load_baseline_overfit(json_path: Path) -> dict[str, Any] | None:
    if not json_path.is_file():
        return None
    data = json.loads(json_path.read_text(encoding="utf-8"))
    return {
        "golden_prod": data.get("golden_prod", {}).get("summary"),
        "validation_prod": data.get("validation_prod", {}).get("summary"),
    }


def render_prod_aligned_markdown(report: dict[str, Any], baseline: dict[str, Any] | None) -> str:
    g = report["golden"]["summary"]
    v = report["validation"]["summary"]
    ts = report["meta"]["timestamp"]
    schema_ver = report["meta"].get("schema_version", "?")

    bg = (baseline or {}).get("golden_prod") or {}
    bv = (baseline or {}).get("validation_prod") or {}
    bg_goal = bg.get("goal_correct")
    bv_goal = bv.get("goal_correct")

    golden_disambig_pure = count_pure_disambig_failures(report["golden"]["cases"])
    validation_disambig_pure = count_pure_disambig_failures(report["validation"]["cases"])

    lines = [
        "# Production schema aligned — remesure (prompt inchangé)",
        "",
        f"**Mesuré :** {ts} (UTC)",
        "",
        "## Contexte",
        "",
        "L'overfit check a montré : pas d'overfit, mais le schéma production **étroit** bridait le modèle.",
        "Ce tour aligne `intent-llm-output.schema.json` v2 + `intent.gbnf` sur `docs/GOAL_TAXONOMY.md`.",
        "**Prompt système inchangé** (`intent-engine/src/prompt.rs`).",
        "",
        "## Artefacts",
        "",
        f"- Schéma prod : `intent-llm-output.schema.json` **v{schema_ver}**",
        "- GBNF : `intent-engine/grammar/intent.gbnf` (goal/action/domain/target alignés)",
        "- Taxonomie : `docs/GOAL_TAXONOMY.md`",
        f"- Modèle : `{report['meta']['model']}` (vLLM {report['meta']['vllm']})",
        "",
        "## Scores — schéma production ALIGNÉ",
        "",
        "| Jeu | goal | disambiguation | candidate_actions |",
        "|-----|------|----------------|-----------------|",
        f"| Golden | **{g['goal_correct']}/{g['cases']}** | {g['disambig_correct']}/{g['cases']} | {g['candidates_correct']}/{g['cases']} |",
        f"| Validation | **{v['goal_correct']}/{v['cases']}** | {v['disambig_correct']}/{v['cases']} | {v['candidates_correct']}/{v['cases']} |",
        "",
        "## Comparaison vs baseline prod étroit (overfit check)",
        "",
        "| Jeu | goal (étroit → aligné) | Δ goal | disambig (étroit → aligné) |",
        "|-----|------------------------|--------|----------------------------|",
    ]
    if bg_goal is not None:
        lines.append(
            f"| Golden | {bg_goal}/{g['cases']} → **{g['goal_correct']}/{g['cases']}** | "
            f"**+{g['goal_correct'] - bg_goal}** | {bg.get('disambig_correct')}/{g['cases']} → {g['disambig_correct']}/{g['cases']} |"
        )
    else:
        lines.append(f"| Golden | (baseline absent) → **{g['goal_correct']}/{g['cases']}** | — | — |")
    if bv_goal is not None:
        lines.append(
            f"| Validation | {bv_goal}/{v['cases']} → **{v['goal_correct']}/{v['cases']}** | "
            f"**+{v['goal_correct'] - bv_goal}** | {bv.get('disambig_correct')}/{v['cases']} → {v['disambig_correct']}/{v['cases']} |"
        )
    else:
        lines.append(f"| Validation | (baseline absent) → **{v['goal_correct']}/{v['cases']}** | — | — |")

    lines.extend(
        [
            "",
            f"> Référence baseline étroit : golden **8/15**, validation **14/20** (`docs/OVERFIT_CHECK.md`).",
            f"> Référence éval élargi (hors scope ce tour) : golden **14/15**, validation **17/20**.",
            "",
            "## Gap disambiguation résiduel (goal OK, disambig KO)",
            "",
            "Échecs **purs** disambiguation après alignement schéma — gap sémantique résiduel, **non corrigé** ce tour.",
            "",
            f"- Golden : **{len(golden_disambig_pure)}/{g['cases']}** cas",
            f"- Validation : **{len(validation_disambig_pure)}/{v['cases']}** cas",
            "",
        ]
    )
    if golden_disambig_pure:
        lines.append("### Golden — disambig pur")
        lines.append("")
        for r in golden_disambig_pure:
            lines.append(
                f"- `{r['name']}` : goal `{r['produced_goal']}` OK, "
                f"attendu disambig={r['expected_disambig']}, produit={r['produced_disambig']}"
            )
        lines.append("")
    if validation_disambig_pure:
        lines.append("### Validation — disambig pur")
        lines.append("")
        for r in validation_disambig_pure:
            lines.append(
                f"- `{r['name']}` : goal `{r['produced_goal']}` OK, "
                f"attendu disambig={r['expected_disambig']}, produit={r['produced_disambig']}"
            )
        lines.append("")

    lines.extend(
        [
            render_confusion_md("Matrice goal — golden (prod aligné)", report["golden"]["confusion"]),
            render_confusion_md("Matrice goal — validation (prod aligné)", report["validation"]["confusion"]),
            render_case_table("Golden — prod aligné", report["golden"]["cases"]),
            render_case_table("Validation — prod aligné", report["validation"]["cases"]),
            "## JSON brut — validation (20, sortie chaude)",
            "",
            render_raw_json_block("Validation", report["validation"]["cases"]),
            "## Goals sans route HAL explicite",
            "",
            "Voir `docs/GOAL_TAXONOMY.md` — `network_download`, `network_send` : parser OK, route HAL à définir (décision humaine).",
            "",
        ]
    )
    return "\n".join(lines)


def render_quality_final_markdown(report: dict[str, Any]) -> str:
    g = report["golden"]["summary"]
    v = report["validation"]["summary"]
    ts = report["meta"]["timestamp"]
    schema_ver = report["meta"].get("schema_version", "?")

    golden_disambig_pure = count_pure_disambig_failures(report["golden"]["cases"])
    validation_disambig_pure = count_pure_disambig_failures(report["validation"]["cases"])

    goal_fail_golden = [r for r in report["golden"]["cases"] if not r["scores"]["goal_ok"]]
    goal_fail_validation = [r for r in report["validation"]["cases"] if not r["scores"]["goal_ok"]]

    lines = [
        "# Qualité intent — état final consolidé",
        "",
        f"**Mesuré :** {ts} (UTC)",
        "",
        "## Contexte",
        "",
        "- Schéma production **v{schema_ver}** aligné (`docs/GOAL_TAXONOMY.md`)".replace(
            "{schema_ver}", schema_ver
        ),
        "- Prompt système **inchangé** (`intent-engine/src/prompt.rs`)",
        "- Harnais vLLM : court-circuit `empty_input` identique à `intent-engine/src/parser.rs`",
        f"- Modèle : `{report['meta']['model']}` (vLLM {report['meta']['vllm']})",
        "",
        "## Tableau de référence (jalons)",
        "",
        "| Étape | Golden goal | Validation goal | Golden disambig | Validation disambig |",
        "|-------|-------------|-----------------|-----------------|----------------------|",
        "| Prod étroit (overfit check) | 8/15 | 14/20 | 15/15 | 18/20 |",
        "| Prod aligné v2 (harnais sans short-circuit) | 14/15 | 17/20 | 15/15 | 18/20 |",
        f"| **Final (aligné + short-circuit harnais)** | **{g['goal_correct']}/{g['cases']}** | "
        f"**{v['goal_correct']}/{v['cases']}** | {g['disambig_correct']}/{g['cases']} | "
        f"{v['disambig_correct']}/{v['cases']} |",
        "",
        "## Scores finaux (chemin production réel)",
        "",
        "| Jeu | goal | disambiguation | candidate_actions |",
        "|-----|------|----------------|-----------------|",
        f"| Golden | **{g['goal_correct']}/{g['cases']}** | {g['disambig_correct']}/{g['cases']} | "
        f"{g['candidates_correct']}/{g['cases']} |",
        f"| Validation | **{v['goal_correct']}/{v['cases']}** | {v['disambig_correct']}/{v['cases']} | "
        f"{v['candidates_correct']}/{v['cases']} |",
        "",
        "## Résidus isolés (par catégorie)",
        "",
        "### 1. Disambiguation — non corrigé ce tour (décision prompt ultérieure)",
        "",
        f"Échecs **purs** (goal OK, disambig KO) : golden **{len(golden_disambig_pure)}**, "
        f"validation **{len(validation_disambig_pure)}**.",
        "",
    ]
    for r in golden_disambig_pure + validation_disambig_pure:
        lines.append(
            f"- `{r['name']}` : goal `{r['produced_goal']}` OK — attendu disambig={r['expected_disambig']}, "
            f"produit={r['produced_disambig']}"
        )
    if not golden_disambig_pure and not validation_disambig_pure:
        lines.append("- (aucun)")
    lines.extend(
        [
            "",
            "### 2. Goal sémantique résiduel (hors empty_input)",
            "",
        ]
    )
    for r in goal_fail_golden + goal_fail_validation:
        lines.append(
            f"- `{r['name']}` : attendu `{r['expected_goal']}`, produit `{r['produced_goal']}`"
        )
    if not goal_fail_golden and not goal_fail_validation:
        lines.append("- (aucun)")
    lines.extend(
        [
            "",
            "### 3. Goals réseau — route HAL en attente (décision humaine)",
            "",
            "- `network_download` — parser OK, **pas de route orchestrator→HAL** (voir `docs/HAL_NETWORK_ROUTING_PROPOSAL.md`)",
            "- `network_send` — idem",
            "- **Tension** : tant que non routés, ces goals ne sont pas exécutables de bout en bout malgré le vocabulaire parser.",
            "",
            "## Artefacts",
            "",
            "- Taxonomie : `docs/GOAL_TAXONOMY.md`",
            "- Proposition HAL réseau : `docs/HAL_NETWORK_ROUTING_PROPOSAL.md`",
            "- Alignement schéma : `docs/PROD_SCHEMA_ALIGNED.md`",
            f"- JSON mesure : `tmp/quality_final.json`",
            "",
        ]
    )
    return "\n".join(lines)


def run_prod_aligned_measure(
    *,
    model: str,
    golden_dir: Path,
    validation_dir: Path,
    json_out: Path,
    markdown_out: Path,
    baseline_json: Path,
    quality_json_out: Path | None = None,
    quality_md_out: Path | None = None,
) -> dict[str, Any]:
    from vllm import LLM
    import torch
    import vllm as vllm_mod

    schema_path = PROD_SCHEMA
    schema = json.loads(schema_path.read_text(encoding="utf-8"))
    schema_version = schema.get("x-cognos-schema-version", "?")

    goldens = load_cases(golden_dir, "golden", expected_count=15)
    validations = load_cases(validation_dir, "validation", expected_count=20)

    print(f"=== PROD ALIGNED MEASURE load model={model} schema=v{schema_version} ===")
    t_load = time.perf_counter()
    llm = LLM(
        model=model,
        quantization="awq",
        max_model_len=4096,
        gpu_memory_utilization=0.85,
        trust_remote_code=True,
    )
    load_s = round(time.perf_counter() - t_load, 1)
    print(f"load_wall_s={load_s}")

    sp = make_sampling_params(schema_path)
    warmup = build_prompt(goldens[0])

    golden = suite_report(
        label="golden-aligned",
        cases=goldens,
        llm=llm,
        sp=sp,
        schema_path=schema_path,
        warmup_prompt=warmup,
    )
    validation = suite_report(
        label="validation-aligned",
        cases=validations,
        llm=llm,
        sp=sp,
        schema_path=schema_path,
    )

    report = {
        "meta": {
            "timestamp": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
            "model": model,
            "schema": str(schema_path),
            "schema_version": schema_version,
            "vllm": vllm_mod.__version__,
            "torch": torch.__version__,
            "gpu": torch.cuda.get_device_name(0) if torch.cuda.is_available() else None,
            "load_wall_s": load_s,
            "prompt": "intent-engine/src/prompt.rs (unchanged)",
            "harness_empty_short_circuit": True,
        },
        "golden": golden,
        "validation": validation,
        "baseline": load_baseline_overfit(baseline_json),
    }

    baseline = report["baseline"]
    json_out.parent.mkdir(parents=True, exist_ok=True)
    json_out.write_text(json.dumps(report, indent=2, ensure_ascii=False), encoding="utf-8")
    markdown_out.parent.mkdir(parents=True, exist_ok=True)
    markdown_out.write_text(render_prod_aligned_markdown(report, baseline), encoding="utf-8")

    q_json = quality_json_out or DEFAULT_QUALITY_FINAL_JSON
    q_md = quality_md_out or DEFAULT_QUALITY_FINAL_MD
    q_json.parent.mkdir(parents=True, exist_ok=True)
    q_json.write_text(json.dumps(report, indent=2, ensure_ascii=False), encoding="utf-8")
    q_md.parent.mkdir(parents=True, exist_ok=True)
    q_md.write_text(render_quality_final_markdown(report), encoding="utf-8")
    return report


def render_overfit_markdown(report: dict[str, Any]) -> str:
    gp = report["golden_prod"]["summary"]
    ge = report["golden_eval"]["summary"]
    vp = report["validation_prod"]["summary"]
    ve = report["validation_eval"]["summary"]
    ts = report["meta"]["timestamp"]

    g_goal_pct = round(100 * gp["goal_correct"] / gp["cases"], 1)
    v_goal_pct = round(100 * vp["goal_correct"] / vp["cases"], 1)
    ge_goal_pct = round(100 * ge["goal_correct"] / ge["cases"], 1)
    ve_goal_pct = round(100 * ve["goal_correct"] / ve["cases"], 1)
    gap_prod = round(g_goal_pct - v_goal_pct, 1)
    gap_eval = round(ge_goal_pct - ve_goal_pct, 1)

    if gap_prod >= 15:
        verdict = (
            f"**OVERFIT SUSPECT (prod)** — golden prod {gp['goal_correct']}/{gp['cases']} "
            f"vs validation {vp['goal_correct']}/{vp['cases']} (écart goal {gap_prod} pts). "
            "Le prompt semble mémoriser les golden sans généraliser."
        )
    elif gap_eval >= 15 and ve_goal_pct + 10 < ge_goal_pct:
        verdict = (
            f"**OVERFIT SUSPECT (éval élargi)** — golden éval {ge['goal_correct']}/{ge['cases']} "
            f"vs validation {ve['goal_correct']}/{ve['cases']} (écart {gap_eval} pts)."
        )
    elif v_goal_pct >= g_goal_pct - 10:
        verdict = (
            f"**PAS d'overfit mesuré (prod)** — validation {vp['goal_correct']}/{vp['cases']} "
            f"goal ≥ golden prod {gp['goal_correct']}/{gp['cases']} (écart {gap_prod} pts). "
            f"Le 14/15 historique ne s'applique qu'au schéma **éval élargi** "
            f"({ge['goal_correct']}/{ge['cases']}), pas à la prod ({gp['goal_correct']}/{gp['cases']})."
        )
    else:
        verdict = (
            f"**ZONE GRISE** — prod golden {gp['goal_correct']}/{gp['cases']}, "
            f"validation {vp['goal_correct']}/{vp['cases']} (écart {gap_prod} pts)."
        )

    lines = [
        "# Overfit Check — prompt_v2 (inchangé)",
        "",
        f"**Mesuré :** {ts} (UTC)",
        "",
        "## Méthodologie",
        "",
        f"- Modèle : `{report['meta']['model']}` (vLLM {report['meta']['vllm']})",
        "- Prompt : `intent-engine/src/prompt.rs` — **non modifié** ce tour",
        f"- Golden : {gp['cases']} fixtures (`intent-engine/tests/golden/`)",
        f"- Validation : {vp['cases']} fixtures **neuves** (`intent-engine/tests/validation/`)",
        "- Validation distincte des exemples embarqués dans le prompt (pas de récitation des few-shots)",
        "",
        "## Scores — schéma PRODUCTION (`intent-llm-output.schema.json`)",
        "",
        "| Jeu | goal | disambiguation | candidate_actions |",
        "|-----|------|----------------|-----------------|",
        f"| Golden | **{gp['goal_correct']}/{gp['cases']}** | {gp['disambig_correct']}/{gp['cases']} | {gp['candidates_correct']}/{gp['cases']} |",
        f"| Validation | **{vp['goal_correct']}/{vp['cases']}** | {vp['disambig_correct']}/{vp['cases']} | {vp['candidates_correct']}/{vp['cases']} |",
        "",
        "## Scores — schéma ÉVAL ÉLARGI (`intent-golden-eval.schema.json`)",
        "",
        "| Jeu | goal | disambiguation | candidate_actions |",
        "|-----|------|----------------|-----------------|",
        f"| Golden | {ge['goal_correct']}/{ge['cases']} | {ge['disambig_correct']}/{ge['cases']} | {ge['candidates_correct']}/{ge['cases']} |",
        f"| Validation | {ve['goal_correct']}/{ve['cases']} | {ve['disambig_correct']}/{ve['cases']} | {ve['candidates_correct']}/{ve['cases']} |",
        "",
        "## Écart prod vs éval élargi (goal)",
        "",
        f"- Golden : prod {gp['goal_correct']}/{gp['cases']} vs éval {ge['goal_correct']}/{ge['cases']} "
        f"(Δ goal = {ge['goal_correct'] - gp['goal_correct']})",
        f"- Validation : prod {vp['goal_correct']}/{vp['cases']} vs éval {ve['goal_correct']}/{ve['cases']} "
        f"(Δ goal = {ve['goal_correct'] - vp['goal_correct']})",
        "",
        "> Le score **production** est celui qui compte pour l'intégration (GBNF / XGrammar étroit).",
        "",
        "## Verdict généralisation (schéma production)",
        "",
        verdict,
        "",
        f"- Golden goal : {g_goal_pct}%",
        f"- Validation goal : {v_goal_pct}%",
        f"- Golden disambig : {gp['disambig_correct']}/{gp['cases']}",
        f"- Validation disambig : {vp['disambig_correct']}/{vp['cases']}",
        "",
        render_confusion_md("Matrice goal — validation (prod)", report["validation_prod"]["confusion"]),
        render_case_table("Validation — prod schéma", report["validation_prod"]["cases"]),
        "## JSON brut — validation (20, sortie chaude, schéma production)",
        "",
        render_raw_json_block("Validation", report["validation_prod"]["cases"]),
        "## Court-circuit `empty_input` (Rust)",
        "",
        "Implémenté dans `intent-engine/src/parser.rs` : entrée vide/whitespace après normalisation",
        "→ `await_input` déterministe, **sans appel LLM** (test `empty_input_short_circuits_without_backend`).",
        "",
    ]
    return "\n".join(lines)


def run_overfit_check(
    *,
    model: str,
    golden_dir: Path,
    validation_dir: Path,
    json_out: Path,
    markdown_out: Path,
) -> dict[str, Any]:
    from vllm import LLM
    import torch
    import vllm as vllm_mod

    goldens = load_cases(golden_dir, "golden", expected_count=15)
    validations = load_cases(validation_dir, "validation", expected_count=20)

    print(f"=== OVERFIT CHECK load model={model} ===")
    t_load = time.perf_counter()
    llm = LLM(
        model=model,
        quantization="awq",
        max_model_len=4096,
        gpu_memory_utilization=0.85,
        trust_remote_code=True,
    )
    load_s = round(time.perf_counter() - t_load, 1)
    print(f"load_wall_s={load_s}")

    warmup = build_prompt(goldens[0])
    sp_prod = make_sampling_params(PROD_SCHEMA)
    sp_eval = make_sampling_params(EVAL_SCHEMA)

    golden_prod = suite_report(
        label="golden-prod",
        cases=goldens,
        llm=llm,
        sp=sp_prod,
        schema_path=PROD_SCHEMA,
        warmup_prompt=warmup,
    )
    golden_eval = suite_report(
        label="golden-eval",
        cases=goldens,
        llm=llm,
        sp=sp_eval,
        schema_path=EVAL_SCHEMA,
    )
    validation_prod = suite_report(
        label="validation-prod",
        cases=validations,
        llm=llm,
        sp=sp_prod,
        schema_path=PROD_SCHEMA,
    )
    validation_eval = suite_report(
        label="validation-eval",
        cases=validations,
        llm=llm,
        sp=sp_eval,
        schema_path=EVAL_SCHEMA,
    )

    report = {
        "meta": {
            "timestamp": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
            "model": model,
            "prod_schema": str(PROD_SCHEMA),
            "eval_schema": str(EVAL_SCHEMA),
            "vllm": vllm_mod.__version__,
            "torch": torch.__version__,
            "gpu": torch.cuda.get_device_name(0) if torch.cuda.is_available() else None,
            "load_wall_s": load_s,
        },
        "golden_prod": golden_prod,
        "golden_eval": golden_eval,
        "validation_prod": validation_prod,
        "validation_eval": validation_eval,
    }

    json_out.parent.mkdir(parents=True, exist_ok=True)
    json_out.write_text(json.dumps(report, indent=2, ensure_ascii=False), encoding="utf-8")
    markdown_out.parent.mkdir(parents=True, exist_ok=True)
    markdown_out.write_text(render_overfit_markdown(report), encoding="utf-8")
    return report


def main() -> int:
    ap = argparse.ArgumentParser(description="Evaluate intent quality on golden/validation fixtures")
    ap.add_argument("--model", default=DEFAULT_MODEL)
    ap.add_argument("--schema", default=str(DEFAULT_SCHEMA))
    ap.add_argument("--golden-dir", default=str(DEFAULT_GOLDEN_DIR))
    ap.add_argument("--validation-dir", default=str(DEFAULT_VALIDATION_DIR))
    ap.add_argument("--markdown-out", default=str(DEFAULT_MARKDOWN))
    ap.add_argument("--json-out", default=str(DEFAULT_JSON))
    ap.add_argument("--skip-validation", action="store_true")
    ap.add_argument(
        "--overfit-check",
        action="store_true",
        help="Run golden+validation under prod and eval schemas; write OVERFIT_CHECK.md",
    )
    ap.add_argument("--overfit-json-out", default=str(DEFAULT_OVERFIT_JSON))
    ap.add_argument("--overfit-md-out", default=str(DEFAULT_OVERFIT_MD))
    ap.add_argument(
        "--prod-aligned-measure",
        action="store_true",
        help="Remeasure golden+validation with aligned prod schema only; write PROD_SCHEMA_ALIGNED.md",
    )
    ap.add_argument("--aligned-json-out", default=str(DEFAULT_ALIGNED_JSON))
    ap.add_argument("--aligned-md-out", default=str(DEFAULT_ALIGNED_MD))
    ap.add_argument(
        "--baseline-json",
        default=str(DEFAULT_OVERFIT_JSON),
        help="Prior overfit JSON for narrow-prod baseline comparison",
    )
    ap.add_argument("--quality-json-out", default=str(DEFAULT_QUALITY_FINAL_JSON))
    ap.add_argument("--quality-md-out", default=str(DEFAULT_QUALITY_FINAL_MD))
    args = ap.parse_args()

    if args.prod_aligned_measure:
        report = run_prod_aligned_measure(
            model=args.model,
            golden_dir=Path(args.golden_dir),
            validation_dir=Path(args.validation_dir),
            json_out=Path(args.aligned_json_out),
            markdown_out=Path(args.aligned_md_out),
            baseline_json=Path(args.baseline_json),
            quality_json_out=Path(args.quality_json_out),
            quality_md_out=Path(args.quality_md_out),
        )
        g = report["golden"]["summary"]
        v = report["validation"]["summary"]
        g_pure = len(count_pure_disambig_failures(report["golden"]["cases"]))
        v_pure = len(count_pure_disambig_failures(report["validation"]["cases"]))
        print(
            f"\n=== ALIGNED DONE === golden goal {g['goal_correct']}/{g['cases']} "
            f"disambig {g['disambig_correct']}/{g['cases']} (disambig pur {g_pure}) | "
            f"validation goal {v['goal_correct']}/{v['cases']} "
            f"disambig {v['disambig_correct']}/{v['cases']} (disambig pur {v_pure})"
        )
        print(f"Wrote {args.aligned_json_out}")
        print(f"Wrote {args.aligned_md_out}")
        print(f"Wrote {args.quality_json_out}")
        print(f"Wrote {args.quality_md_out}")
        return 0

    if args.overfit_check:
        report = run_overfit_check(
            model=args.model,
            golden_dir=Path(args.golden_dir),
            validation_dir=Path(args.validation_dir),
            json_out=Path(args.overfit_json_out),
            markdown_out=Path(args.overfit_md_out),
        )
        gp = report["golden_prod"]["summary"]
        vp = report["validation_prod"]["summary"]
        print(
            f"\n=== OVERFIT DONE === golden prod goal {gp['goal_correct']}/{gp['cases']} "
            f"disambig {gp['disambig_correct']}/{gp['cases']} | "
            f"validation prod goal {vp['goal_correct']}/{vp['cases']} "
            f"disambig {vp['disambig_correct']}/{vp['cases']}"
        )
        print(f"Wrote {args.overfit_json_out}")
        print(f"Wrote {args.overfit_md_out}")
        return 0

    schema_path = Path(args.schema)
    golden_dir = Path(args.golden_dir)
    validation_dir = Path(args.validation_dir)

    from vllm import LLM, SamplingParams
    from vllm.sampling_params import StructuredOutputsParams
    import torch
    import vllm as vllm_mod

    schema = json.loads(schema_path.read_text(encoding="utf-8"))
    structured = StructuredOutputsParams(json=schema)
    sp = SamplingParams(temperature=0.0, max_tokens=448, structured_outputs=structured)

    goldens = load_cases(golden_dir, "golden")
    validations = [] if args.skip_validation else load_cases(validation_dir, "validation")

    print(f"=== LOAD model={args.model} schema={schema_path.name} ===")
    t_load = time.perf_counter()
    llm = LLM(
        model=args.model,
        quantization="awq",
        max_model_len=4096,
        gpu_memory_utilization=0.85,
        trust_remote_code=True,
    )
    load_s = round(time.perf_counter() - t_load, 1)
    print(f"load_wall_s={load_s}")

    warmup_prompt = build_prompt(goldens[0]) if goldens else None

    golden_results = run_suite(
        label="golden",
        cases=goldens,
        llm=llm,
        sp=sp,
        global_warmup_prompt=warmup_prompt,
    )
    validation_results = (
        run_suite(
            label="validation",
            cases=validations,
            llm=llm,
            sp=sp,
            global_warmup_prompt=None,
        )
        if validations
        else []
    )

    report = {
        "meta": {
            "timestamp": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
            "model": args.model,
            "schema": str(schema_path),
            "vllm": vllm_mod.__version__,
            "torch": torch.__version__,
            "gpu": torch.cuda.get_device_name(0) if torch.cuda.is_available() else None,
            "load_wall_s": load_s,
        },
        "golden": {
            "summary": summarize(golden_results),
            "confusion": confusion_matrix(golden_results),
            "cases": golden_results,
        },
        "validation": {
            "summary": summarize(validation_results),
            "confusion": confusion_matrix(validation_results),
            "cases": validation_results,
        },
    }

    json_out = Path(args.json_out)
    json_out.parent.mkdir(parents=True, exist_ok=True)
    json_out.write_text(json.dumps(report, indent=2, ensure_ascii=False), encoding="utf-8")

    md_out = Path(args.markdown_out)
    md_out.parent.mkdir(parents=True, exist_ok=True)
    md_out.write_text(render_markdown(report), encoding="utf-8")

    g = report["golden"]["summary"]
    v = report["validation"]["summary"]
    print(
        f"\n=== DONE === golden goal {g['goal_correct']}/{g['cases']} "
        f"disambig {g['disambig_correct']}/{g['cases']} "
        f"candidates {g['candidates_correct']}/{g['cases']}"
    )
    if validation_results:
        print(
            f"validation goal {v['goal_correct']}/{v['cases']} "
            f"disambig {v['disambig_correct']}/{v['cases']} "
            f"candidates {v['candidates_correct']}/{v['cases']}"
        )
    print(f"Wrote {json_out}")
    print(f"Wrote {md_out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
