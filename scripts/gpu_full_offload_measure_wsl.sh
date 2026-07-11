#!/usr/bin/env bash
set -euo pipefail
ROOT="/mnt/f/Software Engineering/COGNOS"
BINDIR="$ROOT/build/cache/llama.cpp/build-cuda/bin"
LLAMA="$BINDIR/llama-server"
MODEL="/root/cognos-models/qwen3-7b-instruct-q4_k_m.gguf"
GRAMMAR="$ROOT/intent-engine/grammar/intent.gbnf"
USER_INPUT="crée un dossier test dans /tmp"
LOG="/tmp/llama-full-offload.log"

export PATH="$HOME/.cargo/bin:$PATH"
[ -f "$HOME/.cargo/env" ] && source "$HOME/.cargo/env"

fuser -k 8080/tcp 2>/dev/null || true
sleep 2
rm -f "$LOG"

echo "=== SCRIPTS AUDIT: -ngl before ==="
grep -h '\-ngl' "$ROOT/scripts/"*.sh 2>/dev/null | grep -v '^#' | head -8 || true

echo "=== MODEL SIZE ==="
ls -lh "$MODEL"

echo "=== VRAM before server ==="
nvidia-smi --query-gpu=memory.used,memory.total,utilization.gpu --format=csv

cd "$BINDIR"
stdbuf -oL -eL ./llama-server \
  -m "$MODEL" \
  --host 127.0.0.1 --port 8080 \
  -ngl 99 \
  -c 4096 \
  --jinja --reasoning off \
  -fa on \
  -v --log-timestamps \
  >"$LOG" 2>&1 &
SPID=$!

for _ in $(seq 1 120); do
  if curl -fsS http://127.0.0.1:8080/health >/dev/null 2>&1; then
    sleep 4
    break
  fi
  sleep 2
done

echo "=== LOG_BYTES ==="
wc -c <"$LOG"

echo "=== OFFLOAD / LAYER / DEVICE LINES ==="
grep -iE 'offload|assigned|layer|cuda|flash|device|tensor|backend|kv' "$LOG" | head -80 || true

echo "=== offloaded N/N (exact) ==="
grep -iE 'offloaded [0-9]+/[0-9]+ layers' "$LOG" || echo "NOT_FOUND"

if ! grep -qi 'offloaded' "$LOG"; then
  echo "=== LOG HEAD ==="
  head -50 "$LOG" || true
  echo "=== LOG TAIL ==="
  tail -50 "$LOG" || true
fi

echo "=== VRAM after load ==="
nvidia-smi --query-gpu=memory.used,memory.total,utilization.gpu --format=csv

python3 - "$USER_INPUT" "$GRAMMAR" <<'PY'
import json, pathlib, subprocess, sys, time, urllib.request

user_input, grammar_path = sys.argv[1:3]
root = pathlib.Path("/mnt/f/Software Engineering/COGNOS")
grammar = pathlib.Path(grammar_path).read_text(encoding="utf-8")
prompt = subprocess.check_output(
    ["cargo", "run", "--quiet", "--example", "print_prompt", "--", user_input],
    cwd=root, text=True,
)

def bench(label, grammar_text, cache_prompt, n_predict):
    body = {
        "temperature": 0.0,
        "stream": False,
        "cache_prompt": cache_prompt,
        "n_predict": n_predict,
    }
    if grammar_text is None:
        body["prompt"] = "Write the numbers 1 through 30 separated by spaces."
    else:
        body["prompt"] = prompt
        body["grammar"] = grammar_text
    payload = json.dumps(body).encode()
    t0 = time.perf_counter()
    resp = json.loads(urllib.request.urlopen(urllib.request.Request(
        "http://127.0.0.1:8080/completion", data=payload,
        headers={"Content-Type": "application/json"}, method="POST"), timeout=300).read())
    wall = (time.perf_counter() - t0) * 1000
    tim = resp["timings"]
    tok = resp["tokens_predicted"]
    tps = tok / (tim["predicted_ms"] / 1000) if tim["predicted_ms"] else 0.0
    print(f"=== {label} ===")
    print(
        f"wall_ms={wall:.0f} predicted_ms={tim['predicted_ms']:.0f} "
        f"tokens_predicted={tok} tok_s={tps:.2f} cache_prompt={cache_prompt}"
    )
    return tok, tps, wall

bench("NO_GRAMMAR_COLD", None, False, 128)
time.sleep(1)
bench("NO_GRAMMAR_HOT", None, True, 128)
time.sleep(1)
bench("INTENT_GRAMMAR_COLD", grammar, False, 448)
time.sleep(1)
bench("INTENT_GRAMMAR_HOT", grammar, True, 448)
PY

echo "=== VRAM after generation ==="
nvidia-smi --query-gpu=memory.used,memory.total,utilization.gpu --format=csv

kill "$SPID" 2>/dev/null || true
