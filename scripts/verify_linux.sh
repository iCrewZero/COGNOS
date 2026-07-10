#!/usr/bin/env bash
# COGNOS/OS — reproduce a green Linux dev workspace (see docs/DEV_LINUX.md).
set -euo pipefail

export PATH="${HOME}/.cargo/bin:${PATH}"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

if ! command -v rustc >/dev/null 2>&1; then
  echo "ERROR: rustc not found — install Rust per docs/DEV_LINUX.md" >&2
  exit 1
fi

if [[ -d .venv ]]; then
  # shellcheck disable=SC1091
  source .venv/bin/activate
elif command -v python3 >/dev/null 2>&1; then
  python3 -m venv .venv
  # shellcheck disable=SC1091
  source .venv/bin/activate
else
  echo "ERROR: python3 not found; create .venv per docs/DEV_LINUX.md" >&2
  exit 1
fi
pip install -q -U 'pip<25' || pip install -q 'pip==24.3.1' || true
pip install -q -r agents/requirements.txt pytest || {
  echo "WARN: full requirements install failed — installing minimal proto/test deps" >&2
  pip install -q grpcio-tools grpcio pytest
}

echo "==> make proto"
make -C build proto

echo "==> cargo check --workspace --all-targets"
cargo check --workspace --all-targets

echo "==> cargo test --workspace"
cargo test --workspace

echo "==> pytest agents/"
pytest agents/ -v

echo "All checks passed."
