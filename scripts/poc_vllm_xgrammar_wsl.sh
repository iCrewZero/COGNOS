#!/usr/bin/env bash
# POC vLLM + XGrammar measurement (isolated, no pipeline changes)
set -euo pipefail
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
ROOT="/mnt/f/Software Engineering/COGNOS"
VENV="/root/cognos-vllm-venv"
PY="$VENV/bin/python"

if [[ ! -x "$PY" ]]; then
  echo "Run scripts/install_vllm_wsl.sh first" >&2
  exit 1
fi

fuser -k 8080/tcp 2>/dev/null || true
pkill -f 'llama-server.*8080' 2>/dev/null || true

"$PY" "$ROOT/scripts/poc_vllm_xgrammar_benchmark.py" 2>&1 | tee /tmp/vllm-xgrammar-poc.log
