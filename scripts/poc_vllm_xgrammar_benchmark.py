#!/usr/bin/env python3
"""POC: vLLM + XGrammar structured output vs llama.cpp GBNF baseline (measure only)."""
from __future__ import annotations

import json
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path("/mnt/f/Software Engineering/COGNOS")
SCHEMA_PATH = ROOT / "intent-engine/schema/intent-llm-output.schema.json"
VENV_PY = Path("/root/cognos-vllm-venv/bin/python")

# Closest HF match to local GGUF (metadata: Qwen2.5 7B Instruct)
MODEL = "Qwen/Qwen2.5-7B-Instruct-AWQ"

INTENTS = {
    "benign": "crée un dossier test dans /tmp",
    "multistep": "installe ffmpeg puis convertis ma vidéo en mp4",
    "ambiguous": "open the robotics project",
    "delete": "supprime le dossier système /boot",
}


def print_prompt(user_input: str) -> str:
    return subprocess.check_output(
        ["cargo", "run", "--quiet", "--example", "print_prompt", "--", user_input],
        cwd=ROOT,
        text=True,
    )


def parse_with_injection(user_input: str, llm_json: str) -> tuple[bool, str]:
    p = subprocess.run(
        ["cargo", "run", "--quiet", "--example", "parse_intent_with_input"],
        cwd=ROOT,
        input=f"{user_input}\n---\n{llm_json.strip()}",
        text=True,
        capture_output=True,
    )
    ok = p.returncode == 0
    return ok, (p.stdout.strip() if ok else p.stderr.strip())


def bench_generate(llm, prompt: str, sampling_params, label: str) -> dict:
    results = {}
    for phase, _ in [("cold", 1), ("hot", 2)]:
        t0 = time.perf_counter()
        outs = llm.generate([prompt], sampling_params, use_tqdm=False)
        wall_ms = (time.perf_counter() - t0) * 1000
        out = outs[0].outputs[0]
        tok = len(out.token_ids)
        # vLLM may expose metrics on request output
        predicted_ms = wall_ms  # wall clock for POC (comparable to llama wall_ms)
        tps = tok / (wall_ms / 1000) if wall_ms else 0
        text = out.text.strip()
        results[phase] = {
            "wall_ms": round(wall_ms),
            "tokens": tok,
            "tok_s": round(tps, 2),
            "content": text,
        }
        print(
            f"{label} {phase}: wall_ms={results[phase]['wall_ms']} "
            f"tokens={tok} tok_s={results[phase]['tok_s']:.2f}"
        )
    return results


def main() -> int:
    schema_str = SCHEMA_PATH.read_text(encoding="utf-8")
    json.loads(schema_str)  # sanity

    from vllm import LLM, SamplingParams
    from vllm.sampling_params import StructuredOutputsParams

    import torch

    print("=== ENV ===")
    import vllm

    print(f"vllm={vllm.__version__} torch={torch.__version__} cuda={torch.cuda.is_available()}")
    if torch.cuda.is_available():
        print(f"gpu={torch.cuda.get_device_name(0)}")
    print(f"model={MODEL} format=AWQ 4-bit")
    print(f"schema={SCHEMA_PATH}")

    print("=== LOADING MODEL (first run downloads weights) ===")
    t_load = time.perf_counter()
    llm = LLM(
        model=MODEL,
        quantization="awq",
        max_model_len=4096,
        gpu_memory_utilization=0.85,
        trust_remote_code=True,
    )
    print(f"load_wall_s={time.perf_counter() - t_load:.1f}")

    schema = json.loads(schema_str)
    structured = StructuredOutputsParams(json=schema)
    sp_free = SamplingParams(temperature=0.0, max_tokens=448)
    sp_guided = SamplingParams(
        temperature=0.0,
        max_tokens=448,
        structured_outputs=structured,
    )

    benign_prompt = print_prompt(INTENTS["benign"])

    print("\n=== BENIGN: unconstrained ===")
    free_benign = bench_generate(llm, benign_prompt, sp_free, "free")

    print("\n=== BENIGN: XGrammar guided JSON ===")
    guided_benign = bench_generate(llm, benign_prompt, sp_guided, "guided")

    ok, parsed = parse_with_injection(INTENTS["benign"], guided_benign["cold"]["content"])
    print(f"parse_ok={ok}")
    if parsed:
        print("PARSED:", parsed[:600])

    print("\n=== QUALITY: 4 intents (guided JSON) ===")
    quality = {}
    for name, user_input in INTENTS.items():
        prompt = print_prompt(user_input)
        t0 = time.perf_counter()
        out = llm.generate([prompt], sp_guided, use_tqdm=False)[0].outputs[0]
        wall_ms = (time.perf_counter() - t0) * 1000
        tok = len(out.token_ids)
        content = out.text.strip()
        ok, parsed = parse_with_injection(user_input, content)
        quality[name] = {
            "wall_ms": round(wall_ms),
            "tokens": tok,
            "tok_s": round(tok / (wall_ms / 1000), 2) if wall_ms else 0,
            "parse_ok": ok,
            "content": content,
        }
        print(f"\n--- {name} parse_ok={ok} tokens={tok} tok_s={quality[name]['tok_s']} ---")
        print(content[:800])

    report = {
        "env": {
            "vllm": vllm.__version__,
            "torch": torch.__version__,
            "model": MODEL,
            "format": "AWQ 4-bit",
            "schema": str(SCHEMA_PATH),
        },
        "benign_unconstrained": free_benign,
        "benign_guided": guided_benign,
        "quality": quality,
        "baseline_llama_cpp": {
            "tok_s_guided": 19.66,
            "wall_ms_cold": 11617,
            "tokens": 222,
        },
    }
    out_path = Path("/tmp/vllm-xgrammar-poc.json")
    out_path.write_text(json.dumps(report, indent=2, ensure_ascii=False), encoding="utf-8")
    print(f"\nWrote {out_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
