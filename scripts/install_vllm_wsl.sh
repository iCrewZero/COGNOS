#!/usr/bin/env bash
# Dedicated vLLM POC venv (not agents venv)
set -euo pipefail
export PATH="$HOME/.local/bin:$PATH"
VENV="/root/cognos-vllm-venv"
PY="3.12"
if [[ ! -d "$VENV" ]]; then
  uv venv "$VENV" --python "$PY"
fi
PIP="$VENV/bin/python -m pip"
PYTHON="$VENV/bin/python"
"$PYTHON" --version
"$PYTHON" -m ensurepip --upgrade 2>/dev/null || true
$PIP install -U pip wheel 2>&1 | tail -3
echo "==> Installing vLLM (may take several minutes)..."
$PIP install vllm 2>&1 | tail -25
"$PYTHON" -c "import vllm; import torch; print('vllm', vllm.__version__); print('torch', torch.__version__); print('cuda', torch.cuda.is_available()); print('gpu', torch.cuda.get_device_name(0) if torch.cuda.is_available() else 'none')"
nvcc --version 2>/dev/null | head -3 || true
