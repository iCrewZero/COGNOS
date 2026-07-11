#!/usr/bin/env bash
# C: measure benign intent after raw_input removed from grammar (same raw /completion path as A2a)
set -euo pipefail
ROOT="/mnt/f/Software Engineering/COGNOS"
cd "$ROOT"
LLAMA_BIN="$ROOT/build/cache/llama.cpp/build-cuda/bin/llama-server"
MODEL="/root/cognos-models/qwen3-7b-instruct-q4_k_m.gguf"
GRAMMAR="$ROOT/intent-engine/grammar/intent.gbnf"
USER_INPUT="crée un dossier test dans /tmp"
LOG="/tmp/test-c-rawinput.log"

fuser -k 8080/tcp 2>/dev/null || true
sleep 2
: >"$LOG"
cd "$ROOT/build/cache/llama.cpp/build-cuda/bin"
stdbuf -oL -eL ./llama-server -m "$MODEL" --host 127.0.0.1 --port 8080 -ngl 99 -c 4096 --jinja --reasoning off -fa on \
  >>"$LOG" 2>&1 &
SPID=$!
for _ in $(seq 1 120); do
  curl -fsS http://127.0.0.1:8080/health >/dev/null 2>&1 && break
  sleep 2
done

cargo test -p cognos-intent-engine --test grammar_schema -q

python3 - "$USER_INPUT" "$GRAMMAR" <<'PY'
import json, pathlib, subprocess, sys, time, urllib.request

user_input, grammar_path = sys.argv[1:3]
root = pathlib.Path("/mnt/f/Software Engineering/COGNOS")
grammar = pathlib.Path(grammar_path).read_text(encoding="utf-8")
for field in ("raw_input", "intent_id", "session_context", "source"):
    assert f'"{field}"' not in grammar, f"injected field still in grammar: {field}"
prompt = subprocess.check_output(
    ["cargo", "run", "--quiet", "--example", "print_prompt", "--", user_input],
    cwd=root, text=True,
)
payload = json.dumps({
    "prompt": prompt, "grammar": grammar, "n_predict": 448,
    "temperature": 0.0, "stream": False, "cache_prompt": True,
}).encode()
t0 = time.perf_counter()
resp = json.loads(urllib.request.urlopen(urllib.request.Request(
    "http://127.0.0.1:8080/completion", data=payload,
    headers={"Content-Type": "application/json"}, method="POST"), timeout=180).read())
wall = (time.perf_counter() - t0) * 1000
tim = resp["timings"]
llm = tim["prompt_ms"] + tim["predicted_ms"]
tok = resp["tokens_predicted"]
tps = tok / (tim["predicted_ms"] / 1000)
content = resp["content"].strip()
print(f"C wall_ms={wall:.0f} llm_ms={llm:.0f} tokens={tok} tok_s={tps:.1f}")
print("LLM_JSON_RAW_START")
print(content)
print("LLM_JSON_RAW_END")
p = subprocess.run(
    ["cargo", "run", "--quiet", "--example", "parse_intent_with_input"],
    cwd=root, input=f"{user_input}\n---\n{content}", text=True, capture_output=True,
)
print("PARSED_START")
print(p.stdout.strip())
print("PARSED_END")
if p.returncode != 0:
    print("PARSE_ERR", p.stderr)
PY

kill "$SPID" 2>/dev/null || true
