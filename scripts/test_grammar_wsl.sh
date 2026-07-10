#!/usr/bin/env bash
# WSL-only grammar probe for llama-server (curl, no PowerShell).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

WORKDIR="/tmp/cognos-grammar-test"
mkdir -p "$WORKDIR"

LLAMA_BIN="${LLAMA_BIN:-$ROOT/build/cache/llama.cpp/build/bin/llama-server}"
MODEL="${MODEL:-/root/cognos-models/qwen3-7b-instruct-q4_k_m.gguf}"
GRAMMAR_FILE="${GRAMMAR_FILE:-$ROOT/intent-engine/grammar/intent.gbnf}"
SERVER_LOG="$WORKDIR/llama-server.log"
SERVER_PID=""

cleanup() {
  if [[ -n "${SERVER_PID}" ]] && kill -0 "${SERVER_PID}" 2>/dev/null; then
    kill "${SERVER_PID}" 2>/dev/null || true
    wait "${SERVER_PID}" 2>/dev/null || true
  fi
}
trap cleanup EXIT

ensure_server() {
  if curl -fsS "http://127.0.0.1:8080/health" >/dev/null 2>&1; then
    echo "==> llama-server already listening on :8080"
    return
  fi

  echo "==> starting llama-server"
  : >"$SERVER_LOG"
  "$LLAMA_BIN" \
    -m "$MODEL" \
    --host 127.0.0.1 \
    --port 8080 \
    -t "$(nproc)" \
    -c 4096 \
    --jinja \
    --reasoning off \
    >"$SERVER_LOG" 2>&1 &
  SERVER_PID=$!

  for _ in $(seq 1 60); do
    if curl -fsS "http://127.0.0.1:8080/health" >/dev/null 2>&1; then
      echo "  OK  /health"
      return
    fi
    sleep 1
  done
  echo "llama-server failed to start" >&2
  tail -n 40 "$SERVER_LOG" >&2 || true
  exit 1
}

write_no_grammar_payload() {
  python3 - <<'PY'
import json, pathlib
path = pathlib.Path("/tmp/cognos-grammar-test/no-grammar.json")
path.write_text(json.dumps({
    "prompt": "Say hello",
    "n_predict": 16,
    "temperature": 0.1,
    "stream": False,
}, indent=2))
print(path)
PY
}

write_grammar_payload() {
  local grammar_path="${1:-$GRAMMAR_FILE}"
  local n_predict="${2:-512}"
  python3 - "$grammar_path" "$n_predict" <<'PY'
import json, pathlib, sys
grammar_path = pathlib.Path(sys.argv[1])
n_predict = int(sys.argv[2])
grammar = grammar_path.read_text(encoding="utf-8")
path = pathlib.Path("/tmp/cognos-grammar-test/with-grammar.json")
path.write_text(json.dumps({
    "prompt": "crée un dossier test dans /tmp",
    "grammar": grammar,
    "model": "qwen3-7b-q4_k_m",
    "n_predict": n_predict,
    "temperature": 0.0,
    "top_p": 0.9,
    "repeat_penalty": 1.15,
    "stream": False,
}, ensure_ascii=False))
print(path)
PY
}

write_full_prompt_payload() {
  local user_input="${1:-crée un dossier test dans /tmp}"
  python3 - "$GRAMMAR_FILE" "$user_input" <<'PY'
import json, pathlib, subprocess, sys

grammar_path = pathlib.Path(sys.argv[1])
user_input = sys.argv[2]
grammar = grammar_path.read_text(encoding="utf-8")
root = pathlib.Path("/mnt/f/Software Engineering/COGNOS")
prompt = subprocess.check_output(
    ["cargo", "run", "--quiet", "--example", "print_prompt", "--", user_input],
    cwd=root,
    text=True,
)
path = pathlib.Path("/tmp/cognos-grammar-test/full-prompt.json")
path.write_text(json.dumps({
    "prompt": prompt,
    "grammar": grammar,
    "model": "qwen3-7b-q4_k_m",
    "n_predict": 448,
    "temperature": 0.0,
    "top_p": 0.9,
    "repeat_penalty": 1.15,
    "stream": False,
    "cache_prompt": True,
}, ensure_ascii=False))
print(path)
PY
}

step_no_grammar() {
  echo "==> STEP 1: curl without grammar"
  write_no_grammar_payload >/dev/null
  curl -sS -X POST "http://127.0.0.1:8080/completion" \
    -H "Content-Type: application/json" \
    --data-binary "@/tmp/cognos-grammar-test/no-grammar.json" \
    | tee "$WORKDIR/no-grammar-response.json"
  echo
}

step_with_grammar() {
  local grammar_path="${1:-$GRAMMAR_FILE}"
  local label="${2:-intent.gbnf}"
  echo "==> STEP 2: curl with grammar (${label})"
  write_grammar_payload "$grammar_path" 448 >/dev/null
  : >"$WORKDIR/grammar-server-tail.log"
  if [[ -n "${SERVER_PID}" ]]; then
  (
    sleep 1
    tail -n 0 -f "$SERVER_LOG"
  ) >"$WORKDIR/grammar-server-tail.log" 2>&1 &
    TAIL_PID=$!
  fi
  set +e
  curl -sS -X POST "http://127.0.0.1:8080/completion" \
    -H "Content-Type: application/json" \
    --data-binary "@/tmp/cognos-grammar-test/with-grammar.json" \
    | tee "$WORKDIR/with-grammar-response.json"
  CURL_RC=$?
  set -e
  if [[ -n "${TAIL_PID:-}" ]]; then
    kill "${TAIL_PID}" 2>/dev/null || true
  fi
  echo
  echo "--- llama-server stderr tail (grammar attempt) ---"
  if [[ -n "${SERVER_PID}" ]]; then
    tail -n 80 "$SERVER_LOG"
  else
    echo "(external server — check journal of the running llama-server process)"
  fi
  return "${CURL_RC}"
}

step_full_prompt() {
  echo "==> STEP 3: curl with build_prompt + grammar"
  write_full_prompt_payload >/dev/null
  curl -sS -X POST "http://127.0.0.1:8080/completion" \
    -H "Content-Type: application/json" \
    --data-binary "@/tmp/cognos-grammar-test/full-prompt.json" \
    | tee "$WORKDIR/full-prompt-response.json"
  echo
  python3 - <<'PY'
import json
from pathlib import Path
raw = Path("/tmp/cognos-grammar-test/full-prompt-response.json").read_text(encoding="utf-8")
resp = json.loads(raw)
content = resp.get("content", "").strip()
print("RAW_JSON_START")
print(content)
print("RAW_JSON_END")
PY
  echo "--- parse_llm_output ---"
  python3 - <<'PY' | (cd "$ROOT" && cargo run --quiet --example parse_intent_json)
import json
from pathlib import Path
raw = Path("/tmp/cognos-grammar-test/full-prompt-response.json").read_text(encoding="utf-8")
content = json.loads(raw).get("content", "").strip()
print(content)
PY
}

step_repro_broken() {
  echo "==> REPRO: broken grammars (expect failed to parse grammar)"
  for sample in \
    "$ROOT/intent-engine/grammar/intent-broken-multiline.gbnf" \
    "$ROOT/intent-engine/grammar/intent-broken-underscore.gbnf"; do
    echo "--- trying $(basename "$sample") ---"
    write_grammar_payload "$sample" 32 >/dev/null
    : >"$SERVER_LOG"
    if [[ -n "${SERVER_PID}" ]] && kill -0 "${SERVER_PID}" 2>/dev/null; then
      kill "${SERVER_PID}" 2>/dev/null || true
      wait "${SERVER_PID}" 2>/dev/null || true
    fi
    "$LLAMA_BIN" -m "$MODEL" --host 127.0.0.1 --port 8080 -t "$(nproc)" -c 4096 --jinja --reasoning off >"$SERVER_LOG" 2>&1 &
    SERVER_PID=$!
    for _ in $(seq 1 60); do
      if curl -fsS "http://127.0.0.1:8080/health" >/dev/null 2>&1; then
        break
      fi
      sleep 1
    done
    set +e
    curl -sS -X POST "http://127.0.0.1:8080/completion" \
      -H "Content-Type: application/json" \
      --data-binary "@/tmp/cognos-grammar-test/with-grammar.json" \
      >"$WORKDIR/repro-$(basename "$sample").json"
    set -e
    echo "--- llama-server stderr ---"
    rg -n "failed to parse grammar|parse: error parsing grammar|expecting" "$SERVER_LOG" 2>/dev/null || grep -nE "failed to parse grammar|parse: error parsing grammar|expecting" "$SERVER_LOG" || tail -n 20 "$SERVER_LOG"
    echo
  done
}

MODE="${1:-all}"
ensure_server
case "$MODE" in
  no-grammar) step_no_grammar ;;
  grammar) step_with_grammar ;;
  repro-broken) step_repro_broken ;;
  full) step_full_prompt ;;
  all)
    step_no_grammar
    step_repro_broken
    step_with_grammar || true
    ;;
  *) echo "usage: $0 [no-grammar|grammar|repro-broken|full|all]" >&2; exit 2 ;;
esac
