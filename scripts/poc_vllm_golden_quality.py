#!/usr/bin/env python3
"""Score vLLM+XGrammar output against the 15 golden fixtures (measure only)."""
from __future__ import annotations

import argparse
import json
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

ROOT = Path("/mnt/f/Software Engineering/COGNOS")
GOLDEN_DIR = ROOT / "intent-engine/tests/golden"
VENV_PY = Path("/root/cognos-vllm-venv/bin/python")
DEFAULT_SCHEMA = ROOT / "intent-engine/schema/intent-golden-eval.schema.json"
DEFAULT_MODEL = "Qwen/Qwen2.5-7B-Instruct-AWQ"


def load_goldens() -> list[dict[str, Any]]:
    cases = []
    for path in sorted(GOLDEN_DIR.glob("*.json")):
        data = json.loads(path.read_text(encoding="utf-8"))
        data["_name"] = path.name
        data["_path"] = str(path)
        cases.append(data)
    if len(cases) != 15:
        raise SystemExit(f"expected 15 golden files, found {len(cases)}")
    return cases


def build_prompt(case: dict[str, Any]) -> str:
    return subprocess.check_output(
        ["cargo", "run", "--quiet", "--example", "print_prompt_golden", "--", case["_path"]],
        cwd=ROOT,
        text=True,
    )


def llm_fields(intent: dict[str, Any]) -> dict[str, Any]:
    """Strip injected / session fields — compare LLM-emitted semantics only."""
    keys = [
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
    return {k: intent.get(k) for k in keys}


def score_case(actual: dict[str, Any], expected: dict[str, Any]) -> dict[str, bool]:
    exp = llm_fields(expected)
    act = actual

    goal_ok = act.get("goal") == exp.get("goal")

    disambig_ok = act.get("disambiguation_required") == exp.get("disambiguation_required")

    exp_amb_high = float(exp.get("ambiguity_score", 0)) > 0.6
    act_amb_high = float(act.get("ambiguity_score", 0)) > 0.6
    ambiguity_ok = exp_amb_high == act_amb_high

    exp_cands = exp.get("candidate_actions") or []
    act_cands = act.get("candidate_actions") or []
    if len(exp_cands) == 0:
        candidates_ok = len(act_cands) == 0
    elif len(exp_cands) >= 2:
        candidates_ok = len(act_cands) >= 2
    else:
        candidates_ok = len(act_cands) >= 1

    return {
        "goal_ok": goal_ok,
        "disambig_ok": disambig_ok,
        "ambiguity_ok": ambiguity_ok,
        "candidates_ok": candidates_ok,
    }


def parse_llm_json(raw: str) -> dict[str, Any]:
    text = raw.strip()
    if text.startswith("```"):
        # tolerate markdown fences from unconstrained runs
        lines = [ln for ln in text.splitlines() if not ln.strip().startswith("```")]
        text = "\n".join(lines).strip()
    return json.loads(text)


def summarize(results: list[dict[str, Any]]) -> dict[str, Any]:
    n = len(results)
    goal = sum(1 for r in results if r["scores"]["goal_ok"])
    disambig = sum(1 for r in results if r["scores"]["disambig_ok"])
    ambiguity = sum(1 for r in results if r["scores"]["ambiguity_ok"])
    candidates = sum(1 for r in results if r["scores"]["candidates_ok"])
    walls = [r["wall_ms"] for r in results if r.get("wall_ms") is not None]
    toks = [r["tokens"] for r in results if r.get("tokens")]
    return {
        "cases": n,
        "goal_correct": goal,
        "disambig_correct": disambig,
        "ambiguity_correct": ambiguity,
        "candidates_correct": candidates,
        "avg_wall_ms": round(sum(walls) / len(walls)) if walls else None,
        "avg_tok_s": round(
            sum(r["tokens"] / (r["wall_ms"] / 1000) for r in results if r.get("wall_ms"))
            / len(walls),
            2,
        )
        if walls
        else None,
        "total_tokens": sum(toks),
    }


def run_benchmark(
    *,
    label: str,
    model: str,
    schema_path: Path,
    warmup: bool,
    fast_start: bool = False,
) -> dict[str, Any]:
    from vllm import LLM, SamplingParams
    from vllm.sampling_params import StructuredOutputsParams
    import torch
    import vllm as vllm_mod

    schema = json.loads(schema_path.read_text(encoding="utf-8"))
    structured = StructuredOutputsParams(json=schema)
    sp = SamplingParams(
        temperature=0.0,
        max_tokens=448,
        structured_outputs=structured,
    )

    print(f"=== LOAD {label} model={model} ===")
    t_load = time.perf_counter()
    llm = LLM(
        model=model,
        quantization="awq" if "AWQ" in model.upper() or "awq" in model.lower() else None,
        max_model_len=4096,
        gpu_memory_utilization=0.85,
        trust_remote_code=True,
        enforce_eager=fast_start,
    )
    load_s = round(time.perf_counter() - t_load, 1)
    print(f"load_wall_s={load_s}")

    goldens = load_goldens()
    if warmup:
        wp = build_prompt(goldens[0])
        llm.generate([wp], sp, use_tqdm=False)

    results: list[dict[str, Any]] = []
    for case in goldens:
        prompt = build_prompt(case)
        t0 = time.perf_counter()
        out = llm.generate([prompt], sp, use_tqdm=False)[0].outputs[0]
        wall_ms = round((time.perf_counter() - t0) * 1000)
        tokens = len(out.token_ids)
        content = out.text.strip()
        row: dict[str, Any] = {
            "name": case["_name"],
            "input": case["input"],
            "wall_ms": wall_ms,
            "tokens": tokens,
            "tok_s": round(tokens / (wall_ms / 1000), 2) if wall_ms else 0,
            "content": content,
        }
        try:
            actual = parse_llm_json(content)
            row["actual"] = actual
            row["scores"] = score_case(actual, case["expected_intent"])
            row["parse_ok"] = True
        except Exception as e:  # noqa: BLE001
            row["parse_ok"] = False
            row["error"] = str(e)
            row["scores"] = {
                "goal_ok": False,
                "disambig_ok": False,
                "ambiguity_ok": False,
                "candidates_ok": False,
            }
        results.append(row)
        s = row["scores"]
        print(
            f"{case['_name']}: goal={s['goal_ok']} disambig={s['disambig_ok']} "
            f"amb={s['ambiguity_ok']} cands={s['candidates_ok']} "
            f"wall_ms={wall_ms} tok_s={row['tok_s']}"
        )

    summary = summarize(results)
    report = {
        "label": label,
        "env": {
            "vllm": vllm_mod.__version__,
            "torch": torch.__version__,
            "model": model,
            "schema": str(schema_path),
            "load_wall_s": load_s,
            "gpu": torch.cuda.get_device_name(0) if torch.cuda.is_available() else None,
        },
        "summary": summary,
        "cases": results,
    }
    return report


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--label", required=True)
    ap.add_argument("--model", default=DEFAULT_MODEL)
    ap.add_argument("--schema", default=str(DEFAULT_SCHEMA))
    ap.add_argument("--out", default="/tmp/vllm-golden-quality.json")
    ap.add_argument("--no-warmup", action="store_true")
    ap.add_argument(
        "--fast-start",
        action="store_true",
        help="enforce_eager=True to skip long CUDA graph capture (benchmark iterations)",
    )
    args = ap.parse_args()

    report = run_benchmark(
        label=args.label,
        model=args.model,
        schema_path=Path(args.schema),
        warmup=not args.no_warmup,
        fast_start=args.fast_start,
    )
    out = Path(args.out)
    out.write_text(json.dumps(report, indent=2, ensure_ascii=False), encoding="utf-8")
    s = report["summary"]
    print(
        f"\n=== {args.label} === goal {s['goal_correct']}/{s['cases']} "
        f"disambig {s['disambig_correct']}/{s['cases']} "
        f"avg_wall_ms={s['avg_wall_ms']} avg_tok_s={s['avg_tok_s']}"
    )
    print(f"Wrote {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
