#!/usr/bin/env bash
set -euo pipefail
ROOT="/mnt/f/Software Engineering/COGNOS"
LLAMA="$ROOT/build/cache/llama.cpp/build-cuda/bin/llama-server"
MODEL="/root/cognos-models/qwen3-7b-instruct-q4_k_m.gguf"
LOG="/tmp/llama-gpu-start.log"

fuser -k 8080/tcp 2>/dev/null || true
sleep 2

"$LLAMA" -m "$MODEL" --host 127.0.0.1 --port 8080 -ngl 99 -c 4096 --jinja --reasoning off -lv 1 \
  >"$LOG" 2>&1 &
SPID=$!

for _ in $(seq 1 120); do
  if curl -fsS "http://127.0.0.1:8080/health" >/dev/null 2>&1; then
    echo "HEALTH_OK pid=$SPID"
    break
  fi
  sleep 2
done

echo "==> GPU/offload lines"
grep -iE 'offload|cuda|gpu|layer|backend|device|vram|assigned' "$LOG" | head -30 || tail -20 "$LOG"

echo "==> VRAM"
nvidia-smi --query-gpu=memory.used,utilization.gpu --format=csv

echo "==> Quick completion timing"
python3 - <<'PY'
import json, pathlib, subprocess, time
root = pathlib.Path("/mnt/f/Software Engineering/COGNOS")
grammar = (root / "intent-engine/grammar/intent.gbnf").read_text(encoding="utf-8")
prompt = subprocess.check_output(
    ["cargo", "run", "--quiet", "--example", "print_prompt", "--", "crée un dossier test dans /tmp"],
    cwd=root, text=True,
)
payload = {
    "prompt": prompt, "grammar": grammar, "n_predict": 448,
    "temperature": 0.0, "stream": False, "cache_prompt": True,
}
path = pathlib.Path("/tmp/gpu-quick.json")
path.write_text(json.dumps(payload))
t0 = time.perf_counter()
import urllib.request
req = urllib.request.Request(
    "http://127.0.0.1:8080/completion",
    data=path.read_bytes(),
    headers={"Content-Type": "application/json"},
    method="POST",
)
resp = json.loads(urllib.request.urlopen(req, timeout=120).read())
wall = (time.perf_counter() - t0) * 1000
tim = resp.get("timings", {})
llm = tim.get("prompt_ms", 0) + tim.get("predicted_ms", 0)
print(f"COLD wall_ms={wall:.0f} llm_ms={llm:.0f} stop={resp.get('stop_type')}")
t0 = time.perf_counter()
resp2 = json.loads(urllib.request.urlopen(req, timeout=120).read())
wall2 = (time.perf_counter() - t0) * 1000
tim2 = resp2.get("timings", {})
llm2 = tim2.get("prompt_ms", 0) + tim2.get("predicted_ms", 0)
print(f"HOT  wall_ms={wall2:.0f} llm_ms={llm2:.0f} stop={resp2.get('stop_type')}")
PY

kill "$SPID" 2>/dev/null || true
