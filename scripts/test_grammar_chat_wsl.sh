#!/usr/bin/env bash
# Quick probe: chat API + reasoning off
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
WORKDIR="/tmp/cognos-grammar-test"
mkdir -p "$WORKDIR"

LLAMA_BIN="${LLAMA_BIN:-$ROOT/build/cache/llama.cpp/build/bin/llama-server}"
MODEL="${MODEL:-/root/cognos-models/qwen3-7b-instruct-q4_k_m.gguf}"
GRAMMAR_FILE="${GRAMMAR_FILE:-$ROOT/intent-engine/grammar/intent.gbnf}"
SERVER_LOG="$WORKDIR/quick-server.log"

fuser -k 8080/tcp 2>/dev/null || true
sleep 2

python3 - <<'PY'
import json, pathlib, subprocess
root = pathlib.Path("/mnt/f/Software Engineering/COGNOS")
grammar = (root / "intent-engine/grammar/intent.gbnf").read_text(encoding="utf-8")
prompt = subprocess.check_output(
    ["cargo", "run", "--quiet", "--example", "print_prompt", "--", "crée un dossier test dans /tmp"],
    cwd=root,
    text=True,
)
marker = "\n\nUSER INPUT:\n"
system, user_tail = prompt.split(marker, 1)
payload = {
    "messages": [
        {"role": "system", "content": system},
        {"role": "user", "content": user_tail},
    ],
    "grammar": grammar,
    "temperature": 0.0,
    "top_p": 0.9,
    "repeat_penalty": 1.15,
    "n_predict": 448,
    "stream": False,
}
path = pathlib.Path("/tmp/cognos-grammar-test/chat-payload.json")
path.write_text(json.dumps(payload, ensure_ascii=False))
print("payload bytes:", path.stat().st_size)
PY

: >"$SERVER_LOG"
"$LLAMA_BIN" -m "$MODEL" --host 127.0.0.1 --port 8080 -t 8 -c 4096 --jinja --reasoning off >"$SERVER_LOG" 2>&1 &
SERVER_PID=$!
trap 'kill "$SERVER_PID" 2>/dev/null || true' EXIT

for _ in $(seq 1 60); do
  if curl -fsS "http://127.0.0.1:8080/health" >/dev/null 2>&1; then
    break
  fi
  sleep 1
done

echo "==> /v1/chat/completions"
curl -sS -X POST "http://127.0.0.1:8080/v1/chat/completions" \
  -H "Content-Type: application/json" \
  --data-binary "@$WORKDIR/chat-payload.json" \
  | tee "$WORKDIR/chat-response.json"

python3 - <<'PY'
import json
from pathlib import Path
resp = json.loads(Path("/tmp/cognos-grammar-test/chat-response.json").read_text())
content = resp["choices"][0]["message"].get("content", "").strip()
print("RAW_JSON_START")
print(content)
print("RAW_JSON_END")
try:
    json.loads(content)
    print("JSON_PARSE: OK")
except Exception as e:
    print("JSON_PARSE:", e)
PY

echo "--- parse_llm_output ---"
python3 - <<'PY' | (cd "$ROOT" && cargo run --quiet --example parse_intent_json)
import json
from pathlib import Path
resp = json.loads(Path("/tmp/cognos-grammar-test/chat-response.json").read_text())
print(resp["choices"][0]["message"].get("content", "").strip())
PY
