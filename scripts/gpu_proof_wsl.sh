#!/usr/bin/env bash
# One-shot GPU proof: offload log + cold/hot latency + 4 golden JSON.
set -euo pipefail
ROOT="/mnt/f/Software Engineering/COGNOS"
cd "$ROOT"
LLAMA_BIN="$ROOT/build/cache/llama.cpp/build-cuda/bin/llama-server"
MODEL="/root/cognos-models/qwen3-7b-instruct-q4_k_m.gguf"
GRAMMAR="$ROOT/intent-engine/grammar/intent.gbnf"
LOG="/tmp/llama-gpu-proof.log"

fuser -k 8080/tcp 2>/dev/null || true
sleep 2
: >"$LOG"

echo "==> nvidia-smi"
nvidia-smi --query-gpu=name,driver_version,memory.total --format=csv

echo "==> starting llama-server (-ngl 99 -fa on, from build-cuda/bin)"
cd "$ROOT/build/cache/llama.cpp/build-cuda/bin"
stdbuf -oL -eL ./llama-server -m "$MODEL" --host 127.0.0.1 --port 8080 -ngl 99 -c 4096 --jinja --reasoning off -fa on -v --log-timestamps \
  >>"$LOG" 2>&1 &
SPID=$!
for _ in $(seq 1 120); do
  curl -fsS http://127.0.0.1:8080/health >/dev/null 2>&1 && break
  sleep 2
done

echo "==> GPU offload (from log)"
grep -iE 'offloaded [0-9]+/[0-9]+ layers|assigned to device CUDA' "$LOG" | head -10 || grep -i 'offload' "$LOG" | head -5 || tail -10 "$LOG"

echo "==> VRAM after load"
nvidia-smi --query-gpu=memory.used --format=csv,noheader

python3 - <<'PY'
import json, pathlib, subprocess, time, urllib.request

root = pathlib.Path("/mnt/f/Software Engineering/COGNOS")
grammar = (root / "intent-engine/grammar/intent.gbnf").read_text(encoding="utf-8")

def prompt_for(text, domain="", files="", idle="unknown"):
    p = subprocess.check_output(
        ["cargo", "run", "--quiet", "--example", "print_prompt", "--", text],
        cwd=root, text=True,
    )
    if domain:
        p = p.replace("- active_domain: none", f"- active_domain: {domain}")
    if files:
        p = p.replace("- recent_files: none", f"- recent_files: {files}")
    if idle != "unknown":
        p = p.replace("- time_since_last_session: unknown", f"- time_since_last_session: {idle}")
    return p

def run(label, text, domain="", files="", idle="unknown"):
    payload = json.dumps({
        "prompt": prompt_for(text, domain, files, idle),
        "grammar": grammar,
        "n_predict": 448,
        "temperature": 0.0,
        "stream": False,
        "cache_prompt": True,
    }).encode()
    req = urllib.request.Request(
        "http://127.0.0.1:8080/completion",
        data=payload,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    t0 = time.perf_counter()
    resp = json.loads(urllib.request.urlopen(req, timeout=180).read())
    wall = (time.perf_counter() - t0) * 1000
    tim = resp.get("timings", {})
    llm = tim.get("prompt_ms", 0) + tim.get("predicted_ms", 0)
    content = resp.get("content", "").strip()
    print(f"\n=== {label} ===")
    print(f"wall_ms={wall:.0f} llm_ms={llm:.0f} stop={resp.get('stop_type')}")
    print("RAW_JSON_START")
    print(content)
    print("RAW_JSON_END")
    p = subprocess.run(
        ["cargo", "run", "--quiet", "--example", "parse_intent_json"],
        cwd=root, input=content, text=True, capture_output=True,
    )
    if p.returncode == 0:
        obj = json.loads(p.stdout)
        print(f"source={obj.get('source')!r} goal={obj.get('goal')!r}")
    else:
        print("parse_fail", p.stderr[:120])

# cold
run("COLD benign", "crée un dossier test dans /tmp")
# hot (same)
run("HOT benign (cached prompt)", "crée un dossier test dans /tmp")
# golden variety
run("golden-multistep", "installe ffmpeg puis convertis ma vidéo en mp4", "media", "clip.mov", "2d")
run("golden-ambiguous", "ouvre le projet robotique", "robotics", "bras.py, rover.py", "3h")
run("golden-dangerous", "supprime le dossier système /boot", "system", "", "1d")
PY

kill "$SPID" 2>/dev/null || true
echo "==> Targets: uncached <3000ms, cached <500ms"
