#!/usr/bin/env bash
# Remeasure golden+validation with aligned production schema (v2) — one model load.
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
"$PY" "$ROOT/scripts/eval_golden_quality.py" --prod-aligned-measure \
  --aligned-json-out "$ROOT/tmp/prod_schema_aligned.json" \
  --aligned-md-out "$ROOT/docs/PROD_SCHEMA_ALIGNED.md" \
  --baseline-json "$ROOT/tmp/overfit_check.json" \
  2>&1 | tee "$ROOT/tmp/prod_aligned_measure.log"
