#!/usr/bin/env bash
set -euo pipefail
BIN="/mnt/f/Software Engineering/COGNOS/build/cache/llama.cpp/build-cuda/bin/llama-server"
MODEL="/root/cognos-models/qwen3-7b-instruct-q4_k_m.gguf"
LOG="/tmp/llama-vram-probe.log"

fuser -k 18080/tcp 2>/dev/null || true
sleep 1
rm -f "$LOG"

"$BIN" -m "$MODEL" --host 127.0.0.1 --port 18080 -ngl 99 -c 4096 --jinja --reasoning off -fa on \
  >"$LOG" 2>&1 &
SPID=$!

for _ in $(seq 1 60); do
  if curl -fsS http://127.0.0.1:18080/health >/dev/null 2>&1; then
    sleep 3
    break
  fi
  sleep 2
done

echo "=== nvidia-smi after load ==="
nvidia-smi --query-gpu=memory.used,memory.total --format=csv,noheader
echo "=== metadata ==="
grep -iE 'print_info|file type|file size|general\.name|general\.architecture' "$LOG" | head -10 || true
kill "$SPID" 2>/dev/null || true
