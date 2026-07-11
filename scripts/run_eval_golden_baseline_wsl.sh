#!/usr/bin/env bash
# Run golden + validation quality baseline (vLLM+XGrammar, prompt unchanged).
set -euo pipefail
export PATH="$HOME/.cargo/bin:$PATH"
ROOT="/mnt/f/Software Engineering/COGNOS"
VENV="/root/cognos-vllm-venv"
PY="$VENV/bin/python"

if [[ ! -x "$PY" ]]; then
  echo "Run scripts/install_vllm_wsl.sh first" >&2
  exit 1
fi

fuser -k 8080/tcp 2>/dev/null || true
pkill -f 'llama-server.*8080' 2>/dev/null || true

mkdir -p "$ROOT/tmp"
"$PY" "$ROOT/scripts/eval_golden_quality.py" \
  --markdown-out "$ROOT/docs/GOLDEN_BASELINE.md" \
  --json-out "$ROOT/tmp/eval_golden_quality.json" \
  2>&1 | tee "$ROOT/tmp/eval_golden_quality.log"
