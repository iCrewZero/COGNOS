#!/usr/bin/env bash
# B: GPU offload verification — layers, VRAM, tok/s without grammar (baseline)
set -euo pipefail
ROOT="/mnt/f/Software Engineering/COGNOS"
LLAMA="$ROOT/build/cache/llama.cpp/build-cuda/bin/llama-server"
MODEL="/root/cognos-models/qwen3-7b-instruct-q4_k_m.gguf"
LOG="/tmp/test-gpu-offload.log"

fuser -k 8080/tcp 2>/dev/null || true
sleep 2
: >"$LOG"

echo "==> Starting with -ngl 99 -lv 0 (max verbosity)"
"$LLAMA" -m "$MODEL" --host 127.0.0.1 --port 8080 -ngl 99 -c 4096 --jinja --reasoning off -lv 0 \
  >>"$LOG" 2>&1 &
SPID=$!
for _ in $(seq 1 120); do
  curl -fsS http://127.0.0.1:8080/health >/dev/null 2>&1 && break
  sleep 2
done

set +e
echo "==> Offload / GPU lines"
grep -iE 'offload|offloaded|cuda|gpu|layer|backend|device|vram' "$LOG" | head -40 || true
set -e

echo "==> VRAM"
nvidia-smi --query-gpu=memory.used,memory.total --format=csv

echo "==> tok/s baseline (no grammar, 64 tokens)"
python3 - <<'PY'
import json, time, urllib.request
payload = json.dumps({"prompt":"Count 1 to 10","n_predict":64,"temperature":0,"stream":False}).encode()
t0 = time.perf_counter()
r = json.loads(urllib.request.urlopen(urllib.request.Request(
    "http://127.0.0.1:8080/completion", data=payload,
    headers={"Content-Type":"application/json"}, method="POST"), timeout=60).read())
wall = (time.perf_counter()-t0)*1000
t = r["timings"]
tok = r["tokens_predicted"]
tps = tok/(t["predicted_ms"]/1000)
print(f"no_grammar wall_ms={wall:.0f} predicted_ms={t['predicted_ms']:.0f} tokens={tok} tok_s={tps:.1f}")
PY

kill "$SPID" 2>/dev/null || true
echo "==> Log tail (offload lines if present)"
grep -iE 'offload|offloaded|load_tensors|cuda|gpu layer' "$LOG" 2>/dev/null | head -20 || tail -15 "$LOG" 2>/dev/null || true
