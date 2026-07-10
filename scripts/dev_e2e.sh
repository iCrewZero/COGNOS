#!/usr/bin/env bash
# COGNOS/OS — dev end-to-end: intent → orchestrator → intent-engine → vLLM → HAL → file_agent
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

MODE="${1:-mock}"
INTENT_TEXT="${COGNOS_E2E_INTENT:-crée un dossier test dans /tmp}"
TARGET_DIR="${COGNOS_E2E_TARGET:-/tmp/test}"
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
export CARGO_TARGET_DIR
export COGNOS_EXTRA_PATHS="/tmp"
export COGNOS_AGENTS_DIR="$ROOT/agents"
export COGNOS_PYTHON="${COGNOS_PYTHON:-python3}"
export COGNOS_IPC_ENDPOINT="${COGNOS_IPC_ENDPOINT:-http://127.0.0.1:7443}"
export COGNOS_HAL_ENDPOINT="${COGNOS_HAL_ENDPOINT:-http://127.0.0.1:7444}"
export COGNOS_INTENT_ENDPOINT="${COGNOS_INTENT_ENDPOINT:-http://127.0.0.1:7445}"
export COGNOS_ORCHESTRATOR_ENDPOINT="${COGNOS_ORCHESTRATOR_ENDPOINT:-http://127.0.0.1:7446}"
VLLM_PORT="${COGNOS_VLLM_PORT:-8080}"

PIDS=()
VLLM_PID=""
LOG_DIR="$ROOT/build/e2e_logs"
mkdir -p "$LOG_DIR"

cleanup() {
  for pid in "${PIDS[@]:-}"; do
    kill "$pid" 2>/dev/null || true
  done
  if [[ -n "${VLLM_PID:-}" ]]; then
    kill "$VLLM_PID" 2>/dev/null || true
  fi
}
trap cleanup EXIT

wait_for_tcp() {
  local host="$1" port="$2" label="$3" deadline=$((SECONDS + 90))
  while ! (echo >/dev/tcp/"$host"/"$port") 2>/dev/null; do
    if (( SECONDS >= deadline )); then
      echo "timeout waiting for $label ($host:$port)" >&2
      return 1
    fi
  done
  echo "  OK  $label ($host:$port)"
}

wait_for_vllm() {
  local deadline=$((SECONDS + 180))
  while ! curl -sf "http://127.0.0.1:${VLLM_PORT}/v1/models" >/dev/null 2>&1; do
    if (( SECONDS >= deadline )); then
      echo "vLLM not ready on :${VLLM_PORT}" >&2
      return 1
    fi
  done
  echo "  OK  vLLM (http://127.0.0.1:${VLLM_PORT}/v1/models)"
}

echo "==> Building workspace binaries"
cargo build --workspace --bins

BIN="$CARGO_TARGET_DIR/debug"
if [[ ! -d "$BIN" ]]; then
  BIN="$CARGO_TARGET_DIR/release"
fi

echo "==> Starting services (mode=$MODE)"
"$BIN/cognos-ipc-server" >"$LOG_DIR/ipc.log" 2>&1 &
PIDS+=($!)
wait_for_tcp 127.0.0.1 7443 "cognos-ipc-server"

"$BIN/cognos-scheduler" >"$LOG_DIR/scheduler.log" 2>&1 &
PIDS+=($!)
echo "  OK  cognos-scheduler (IPC client)"

"$BIN/cognos-memory" >"$LOG_DIR/memory.log" 2>&1 &
PIDS+=($!)
echo "  OK  cognos-memory (IPC client)"

"$BIN/cognos-hal" >"$LOG_DIR/hal.log" 2>&1 &
PIDS+=($!)
wait_for_tcp 127.0.0.1 7444 "cognos-hal"

if [[ "$MODE" == "mock" ]]; then
  export MOCK_LLM=1
  echo "  MOCK_LLM=1 (deterministic intent-engine backend)"
else
  unset MOCK_LLM
  export COGNOS_INTENT_LLAMA_ENDPOINT="${COGNOS_INTENT_LLAMA_ENDPOINT:-http://127.0.0.1:${VLLM_PORT}}"
  export COGNOS_INTENT_BACKEND="${COGNOS_INTENT_BACKEND:-vllm}"
  if curl -sf "http://127.0.0.1:${VLLM_PORT}/v1/models" >/dev/null 2>&1; then
    echo "  Using existing vLLM at ${COGNOS_INTENT_LLAMA_ENDPOINT}"
  else
    echo "  Starting vLLM via scripts/start_vllm_wsl.sh"
    bash "$ROOT/scripts/start_vllm_wsl.sh"
    VLLM_PID=$(pgrep -f "vllm.entrypoints.openai.api_server.*--port ${VLLM_PORT}" | head -n1 || true)
  fi
  wait_for_vllm
fi

CONFIG_PATH="$ROOT/config/intent.toml"
export COGNOS_INTENT_SCHEMA="$ROOT/intent-engine/schema/intent-llm-output.schema.json"
RUST_LOG=info stdbuf -oL -eL "$BIN/cognos-intent" --config "$CONFIG_PATH" >"$LOG_DIR/intent.log" 2>&1 &
PIDS+=($!)
wait_for_tcp 127.0.0.1 7445 "cognos-intent"

"$BIN/cognos-orchestrator" >"$LOG_DIR/orchestrator.log" 2>&1 &
PIDS+=($!)
wait_for_tcp 127.0.0.1 7446 "cognos-orchestrator"

rm -rf "$TARGET_DIR"

echo "==> Running CLI intent"
E2E_START_MS=$(python3 -c 'import time; print(int(time.time()*1000))')
set +e
CLI_OUT="$("$BIN/cognos" intent "$INTENT_TEXT" 2>&1)"
CLI_RC=$?
set -e
E2E_END_MS=$(python3 -c 'import time; print(int(time.time()*1000))')
E2E_MS=$((E2E_END_MS - E2E_START_MS))
echo "$CLI_OUT"
echo "==> E2E latency_ms=$E2E_MS"

if (( CLI_RC != 0 )); then
  echo "cognos intent failed (rc=$CLI_RC)" >&2
  exit "$CLI_RC"
fi

if [[ ! -d "$TARGET_DIR" ]]; then
  echo "expected directory missing: $TARGET_DIR" >&2
  exit 1
fi

if ! echo "$CLI_OUT" | grep -qi "HAL:"; then
  echo "CLI output missing HAL decision line" >&2
  exit 1
fi

if [[ "$MODE" == "real" ]]; then
  if echo "$CLI_OUT" | grep -qi "keyword_fallback"; then
    echo "real mode must not use keyword_fallback" >&2
    exit 1
  fi
  if ! echo "$CLI_OUT" | grep -qE 'parse=[0-9]{3,}ms'; then
    echo "real mode expected vLLM parse latency (parse>=500ms), got:" >&2
    echo "$CLI_OUT" | grep latency >&2 || true
    exit 1
  fi
  if ! grep -aE 'source.*vllm' "$LOG_DIR/intent.log" 2>/dev/null \
      && ! grep -aE 'source.*vllm' "$LOG_DIR/orchestrator.log" 2>/dev/null; then
    echo "could not confirm source=vllm in intent/orchestrator logs" >&2
    exit 1
  fi
fi

echo "==> E2E OK ($MODE): $TARGET_DIR exists, HAL visible in CLI output"
