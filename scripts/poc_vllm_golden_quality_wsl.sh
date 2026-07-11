#!/usr/bin/env bash
# Run golden quality benchmark via vLLM+XGrammar (WSL, measure only).
set -euo pipefail
export PATH="$HOME/.local/bin:$PATH"
ROOT="/mnt/f/Software Engineering/COGNOS"
VENV="/root/cognos-vllm-venv"
PY="$VENV/bin/python"
LABEL="${1:-baseline}"
MODEL="${2:-Qwen/Qwen2.5-7B-Instruct-AWQ}"
OUT="/tmp/vllm-golden-${LABEL}.json"

if [[ ! -x "$PY" ]]; then
  echo "missing venv at $VENV — run scripts/install_vllm_wsl.sh first" >&2
  exit 1
fi

cd "$ROOT/intent-engine"
cargo build --quiet --example print_prompt_golden

exec "$PY" "$ROOT/scripts/poc_vllm_golden_quality.py" \
  --label "$LABEL" \
  --model "$MODEL" \
  --out "$OUT" \
  2>&1 | tee "/tmp/vllm-golden-${LABEL}.log"
