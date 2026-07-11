#!/usr/bin/env bash
# GPU intent benchmark: cold/hot latency + golden JSON quality probe.
#
# Usage:
#   bash scripts/build_llama_cuda_wsl.sh          # once
#   bash scripts/benchmark_gpu_intent_wsl.sh      # measure
#
# Targets: cached <500ms, uncached <3000ms (wall clock on /completion).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

WORKDIR="/tmp/cognos-gpu-bench"
mkdir -p "$WORKDIR"

LLAMA_BIN="${LLAMA_BIN:-$ROOT/build/cache/llama.cpp/build-cuda/bin/llama-server}"
MODEL="${MODEL:-/root/cognos-models/qwen3-7b-instruct-q4_k_m.gguf}"
GRAMMAR_FILE="${GRAMMAR_FILE:-$ROOT/intent-engine/grammar/intent.gbnf}"
SERVER_LOG="$WORKDIR/llama-server.log"
INTENT_LOG="$WORKDIR/intent.log"
SERVER_PID=""
INTENT_PID=""
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
export CARGO_TARGET_DIR
BIN="$CARGO_TARGET_DIR/debug"

cleanup() {
  [[ -n "${INTENT_PID}" ]] && kill "${INTENT_PID}" 2>/dev/null || true
  [[ -n "${SERVER_PID}" ]] && kill "${SERVER_PID}" 2>/dev/null || true
}
trap cleanup EXIT

start_llama() {
  fuser -k 8080/tcp 2>/dev/null || true
  sleep 2
  : >"$SERVER_LOG"
  echo "==> llama-server GPU (-ngl 99)"
  "$LLAMA_BIN" \
    -m "$MODEL" \
    --host 127.0.0.1 \
    --port 8080 \
    -ngl 99 \
    -c 4096 \
    --jinja \
    --reasoning off \
    >"$SERVER_LOG" 2>&1 &
  SERVER_PID=$!
  for _ in $(seq 1 120); do
    curl -fsS "http://127.0.0.1:8080/health" >/dev/null 2>&1 && return 0
    sleep 1
  done
  tail -n 40 "$SERVER_LOG" >&2
  exit 1
}

show_gpu_offload() {
  echo "==> GPU offload log"
  grep -iE 'offload|cuda|gpu|device|vulkan' "$SERVER_LOG" | head -25 || tail -n 25 "$SERVER_LOG"
}

start_intent_engine() {
  fuser -k 7445/tcp 2>/dev/null || true
  sleep 1
  cargo build -p cognos-intent-engine --bin cognos-intent -q
  unset MOCK_LLM
  export COGNOS_INTENT_GRAMMAR_PATH="$GRAMMAR_FILE"
  : >"$INTENT_LOG"
  "$BIN/cognos-intent" >"$INTENT_LOG" 2>&1 &
  INTENT_PID=$!
  for _ in $(seq 1 60); do
    (echo >/dev/tcp/127.0.0.1/7445) 2>/dev/null && return 0
    sleep 1
  done
  tail -n 20 "$INTENT_LOG" >&2
  exit 1
}

write_payload() {
  local name="$1" user_input="$2" domain="${3:-}" files="${4:-}" idle="${5:-unknown}"
  python3 - "$name" "$user_input" "$domain" "$files" "$idle" "$GRAMMAR_FILE" <<'PY'
import json, pathlib, subprocess, sys
name, user_input, domain, files, idle, grammar_path = sys.argv[1:7]
root = pathlib.Path("/mnt/f/Software Engineering/COGNOS")
grammar = pathlib.Path(grammar_path).read_text(encoding="utf-8")
prompt = subprocess.check_output(
    ["cargo", "run", "--quiet", "--example", "print_prompt", "--", user_input],
    cwd=root, text=True,
)
if domain:
    prompt = prompt.replace("- active_domain: none", f"- active_domain: {domain}")
if files and files != "none":
    prompt = prompt.replace("- recent_files: none", f"- recent_files: {files}")
if idle != "unknown":
    prompt = prompt.replace("- time_since_last_session: unknown", f"- time_since_last_session: {idle}")
path = pathlib.Path(f"/tmp/cognos-gpu-bench/{name}.json")
path.write_text(json.dumps({
    "prompt": prompt, "grammar": grammar, "model": "qwen3-7b-q4_k_m",
    "n_predict": 448, "temperature": 0.0, "top_p": 0.9, "repeat_penalty": 1.15,
    "stream": False, "cache_prompt": True,
}, ensure_ascii=False))
PY
}

run_completion() {
  local name="$1"
  local payload="/tmp/cognos-gpu-bench/${name}.json"
  local out="/tmp/cognos-gpu-bench/${name}-response.json"
  local start end ms
  start=$(date +%s%3N)
  curl -sS -X POST "http://127.0.0.1:8080/completion" \
    -H "Content-Type: application/json" \
    --data-binary "@$payload" >"$out"
  end=$(date +%s%3N)
  ms=$((end - start))
  python3 - "$out" "$ms" "$name" <<'PY'
import json, subprocess, sys
from pathlib import Path
out, wall_ms, name = Path(sys.argv[1]), int(sys.argv[2]), sys.argv[3]
resp = json.loads(out.read_text())
content = resp.get("content", "").strip()
t = resp.get("timings", {})
llm_ms = t.get("prompt_ms", 0) + t.get("predicted_ms", 0)
print(f"[{name}] wall_ms={wall_ms} llm_ms={llm_ms:.0f} stop={resp.get('stop_type')}")
print("RAW_JSON_START")
print(content)
print("RAW_JSON_END")
p = subprocess.run(
    ["cargo", "run", "--quiet", "--example", "parse_intent_json"],
    cwd="/mnt/f/Software Engineering/COGNOS",
    input=content, text=True, capture_output=True,
)
if p.returncode == 0:
    obj = json.loads(p.stdout)
    print(f"parse_llm_output=OK goal={obj.get('goal')!r} source={obj.get('source')!r}")
else:
    print("parse_llm_output=FAIL", p.stderr.strip()[:200])
PY
}

probe_intent_log_source() {
  local label="$1"
  if [[ -f "$INTENT_LOG" ]]; then
    grep -E "source=|latency_ms=|keyword_fallback" "$INTENT_LOG" | tail -3 || true
  fi
}

echo "==> nvidia-smi"
nvidia-smi

[[ -x "$LLAMA_BIN" ]] || { echo "Build GPU binary first: bash scripts/build_llama_cuda_wsl.sh" >&2; exit 1; }

start_llama
show_gpu_offload

echo "==> COLD (first completion after load)"
write_payload "cold-benign" "crée un dossier test dans /tmp" >/dev/null
run_completion "cold-benign"

echo "==> HOT (same payload, warm KV)"
run_completion "cold-benign"

echo "==> Golden quality probes (4 intents)"
write_payload "golden-benign" "crée un dossier test dans /tmp" >/dev/null
write_payload "golden-multistep" "installe ffmpeg puis convertis ma vidéo en mp4" "media" "clip.mov" "2d" >/dev/null
write_payload "golden-ambiguous" "ouvre le projet robotique" "robotics" "bras.py, rover.py" "3h" >/dev/null
write_payload "golden-dangerous" "supprime le dossier système /boot" "system" "" "1d" >/dev/null

for c in golden-benign golden-multistep golden-ambiguous golden-dangerous; do
  echo "--- $c ---"
  run_completion "$c"
  echo
done

echo "==> Intent-engine source check (optional — starts cognos-intent)"
start_intent_engine
echo "  OK cognos-intent :7445"
# cold via engine: restart llama for true cold engine path omitted; log shows source on DispatchIntent
probe_intent_log_source "after start"

echo "==> Targets: cached wall <500ms, uncached wall <3000ms"
