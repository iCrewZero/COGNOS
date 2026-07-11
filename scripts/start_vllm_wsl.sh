#!/usr/bin/env bash
# Start vLLM and wait until /health returns 200 (no fixed sleep).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VENV="${VLLM_VENV:-/root/cognos-vllm-venv}"
PY="$VENV/bin/python"
PORT="${COGNOS_VLLM_PORT:-8080}"
MODEL="${COGNOS_VLLM_MODEL:-Qwen/Qwen2.5-7B-Instruct-AWQ}"
LOG="${COGNOS_VLLM_LOG:-$ROOT/build/e2e_logs/vllm.log}"
HEALTH_URL="http://127.0.0.1:${PORT}/health"
MODELS_URL="http://127.0.0.1:${PORT}/v1/models"
WAIT_SECS="${VLLM_STARTUP_WAIT_SECS:-900}"

vllm_ready() {
  local health models
  health=$(curl -s -o /dev/null -w '%{http_code}' "$HEALTH_URL" 2>/dev/null || echo "000")
  models=$(curl -s -o /dev/null -w '%{http_code}' "$MODELS_URL" 2>/dev/null || echo "000")
  [[ "$health" == "200" || "$models" == "200" ]]
}

if [[ ! -x "$PY" ]]; then
  echo "Run scripts/install_vllm_wsl.sh first (venv at $VENV)" >&2
  exit 1
fi

mkdir -p "$(dirname "$LOG")"

if vllm_ready; then
  echo "==> vLLM already healthy on :$PORT"
else
  fuser -k "${PORT}/tcp" 2>/dev/null || true
  pkill -f "vllm.entrypoints.openai.api_server.*--port ${PORT}" 2>/dev/null || true
  pkill -f "vllm serve.*--port ${PORT}" 2>/dev/null || true
  pkill -f 'llama-server.*8080' 2>/dev/null || true

  echo "==> Starting vLLM model=$MODEL port=$PORT log=$LOG"
  nohup "$PY" -m vllm.entrypoints.openai.api_server \
    --model "$MODEL" \
    --host 127.0.0.1 \
    --port "$PORT" \
    --quantization awq \
    --max-model-len 4096 \
    --gpu-memory-utilization 0.85 \
    --trust-remote-code \
    >"$LOG" 2>&1 &

  deadline=$((SECONDS + WAIT_SECS))
  echo "==> Waiting for vLLM ready (/health or /v1/models, max ${WAIT_SECS}s)..."
  while true; do
    if vllm_ready; then
      break
    fi
    if (( SECONDS >= deadline )); then
      echo "vLLM not ready — tail $LOG" >&2
      tail -40 "$LOG" >&2 || true
      exit 1
    fi
    if (( (SECONDS % 30) == 0 )); then
      tail -1 "$LOG" 2>/dev/null || true
    fi
    sleep 3
  done
fi

HEALTH_CODE=$(curl -s -o /dev/null -w '%{http_code}' "$HEALTH_URL" 2>/dev/null || echo "000")
echo "==> vLLM ready on :$PORT (/health=$HEALTH_CODE)"
models_json=$(curl -sf "$MODELS_URL")
echo "==> /v1/models: $(echo "$models_json" | python3 -c 'import sys,json; d=json.load(sys.stdin); print(d["data"][0]["id"] if d.get("data") else "?")' 2>/dev/null || echo "$MODELS_URL")"

if command -v nvidia-smi >/dev/null 2>&1; then
  echo "==> nvidia-smi (VRAM):"
  nvidia-smi --query-gpu=name,memory.used,memory.total,utilization.gpu --format=csv,noheader
else
  echo "==> nvidia-smi not available"
fi
