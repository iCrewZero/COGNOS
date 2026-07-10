#!/usr/bin/env bash
set -euo pipefail
source "${HOME}/.cargo/env"
cd "/mnt/f/Software Engineering/COGNOS"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/tmp/cognos-cargo-target}"
export PATH="/mnt/f/Software Engineering/COGNOS/.venv/bin:${PATH}"
LOG_DIR="/tmp/cognos-ci-task2"
mkdir -p "$LOG_DIR"

run_step() {
  local name="$1"
  shift
  echo "=== RUNNING: $name ===" | tee "$LOG_DIR/${name}.log"
  set +e
  "$@" 2>&1 | tee -a "$LOG_DIR/${name}.log"
  local ec=${PIPESTATUS[0]}
  set -e
  echo "=== EXIT: $name code=$ec ===" | tee -a "$LOG_DIR/${name}.log"
  return "$ec"
}

FAIL=0

run_step "01_proto" make -C build proto PYTHON="$(pwd)/.venv/bin/python" || FAIL=1
run_step "02_build_bins" cargo build --workspace --bins || FAIL=1
run_step "03_check_all_targets" cargo check --workspace --all-targets || FAIL=1
run_step "04_test_workspace" cargo test --workspace || FAIL=1

if [[ -x ".venv/bin/pytest" ]]; then
  run_step "05_pytest_agents" .venv/bin/pytest agents/ || FAIL=1
elif command -v pytest >/dev/null 2>&1; then
  run_step "05_pytest_agents" pytest agents/ || FAIL=1
else
  echo "pytest not found" | tee "$LOG_DIR/05_pytest_agents.log"
  FAIL=1
fi

run_step "06_dev_e2e_mock" bash scripts/dev_e2e.sh mock || FAIL=1

echo "=== WARNINGS SCAN ===" | tee "$LOG_DIR/warnings.log"
cargo build --workspace --bins 2>&1 | grep warning: | tee -a "$LOG_DIR/warnings.log" || echo "(none)" | tee -a "$LOG_DIR/warnings.log"

echo "=== HAL UNIX PROOF ===" | tee "$LOG_DIR/hal_unix_proof.log"
cargo check -p cognos-hal --all-targets 2>&1 | tee -a "$LOG_DIR/hal_unix_proof.log"
cargo rustc -p cognos-hal --bin cognos-hal -- --print cfg 2>&1 | tee -a "$LOG_DIR/hal_unix_proof.log"

exit "$FAIL"
