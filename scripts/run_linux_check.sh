#!/usr/bin/env bash
set -euo pipefail
export PATH="${HOME}/.cargo/bin:${PATH}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

if [[ -x .venv/bin/python3 ]]; then
  PYTHON=.venv/bin/python3
else
  PYTHON=python3
fi

echo "==> proto ($PYTHON)"
make -C build proto PYTHON="$PYTHON" || "$PYTHON" agents/generate_proto.py

echo "==> cargo check --workspace --all-targets"
cargo check --workspace --all-targets

echo "==> cargo test --workspace"
cargo test --workspace

echo "CHECK_OK"
