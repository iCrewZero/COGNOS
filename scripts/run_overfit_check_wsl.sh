#!/usr/bin/env bash
# Overfit check: golden 15 + validation 20, prod vs eval schemas, one model load.
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
"$PY" "$ROOT/scripts/eval_golden_quality.py" --overfit-check \
  --overfit-json-out "$ROOT/tmp/overfit_check.json" \
  --overfit-md-out "$ROOT/docs/OVERFIT_CHECK.md" \
  2>&1 | tee "$ROOT/tmp/overfit_check.log"
