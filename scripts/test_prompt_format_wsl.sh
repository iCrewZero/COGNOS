#!/usr/bin/env bash
# A2: isolate prompt format — raw /completion vs Qwen3 chat template vs /chat/completions
set -euo pipefail
ROOT="/mnt/f/Software Engineering/COGNOS"
cd "$ROOT"
LLAMA="$ROOT/build/cache/llama.cpp/build-cuda/bin/llama-server"
MODEL="/root/cognos-models/qwen3-7b-instruct-q4_k_m.gguf"
GRAMMAR="$ROOT/intent-engine/grammar/intent.gbnf"
LOG="/tmp/test-prompt-format.log"
USER_INPUT="crée un dossier test dans /tmp"
WORKDIR="/tmp/test-prompt-format"
mkdir -p "$WORKDIR"

fuser -k 8080/tcp 2>/dev/null || true
sleep 2
: >"$LOG"

"$LLAMA" -m "$MODEL" --host 127.0.0.1 --port 8080 -ngl 99 -c 4096 --jinja --reasoning off \
  >>"$LOG" 2>&1 &
SPID=$!
for _ in $(seq 1 120); do
  curl -fsS http://127.0.0.1:8080/health >/dev/null 2>&1 && break
  sleep 2
done

python3 - "$USER_INPUT" "$GRAMMAR" "$WORKDIR" <<'PY'
import json, pathlib, subprocess, sys, time, urllib.request

user_input, grammar_path, workdir = sys.argv[1:4]
root = pathlib.Path("/mnt/f/Software Engineering/COGNOS")
grammar = pathlib.Path(grammar_path).read_text(encoding="utf-8")
full = subprocess.check_output(
    ["cargo", "run", "--quiet", "--example", "print_prompt", "--", user_input],
    cwd=root, text=True,
)
marker = "\n\nUSER INPUT:\n"
system_part, user_tail = full.split(marker, 1)
user_msg = user_tail.rsplit("\n\nRespond with the JSON object now.", 1)[0].strip()

# A2a: raw text (current HttpLlamaBackend path)
raw_prompt = full

# A2b: manual Qwen3 chat template on /completion
chat_prompt = (
    "<|im_start|>system\n" + system_part + "\n"
    "<|im_start|>user\n" + user_msg + "\n\nRespond with the JSON object now.\n"
    "<|im_start|>assistant\n"
)

def post_completion(name, prompt):
    payload = {
        "prompt": prompt,
        "grammar": grammar,
        "n_predict": 448,
        "temperature": 0.0,
        "stream": False,
        "cache_prompt": True,
    }
    path = pathlib.Path(workdir) / f"{name}.json"
    path.write_text(json.dumps(payload, ensure_ascii=False))
    t0 = time.perf_counter()
    req = urllib.request.Request(
        "http://127.0.0.1:8080/completion",
        data=path.read_bytes(),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    resp = json.loads(urllib.request.urlopen(req, timeout=180).read())
    wall = (time.perf_counter() - t0) * 1000
    tim = resp.get("timings", {})
    llm = tim.get("prompt_ms", 0) + tim.get("predicted_ms", 0)
    tok = resp.get("tokens_predicted", 0)
    tps = tok / (tim.get("predicted_ms", 1) / 1000) if tim.get("predicted_ms") else 0
    content = resp.get("content", "").strip()
    print(f"\n========== {name} ==========")
    print(f"wall_ms={wall:.0f} llm_ms={llm:.0f} tokens={tok} tok_s={tps:.1f}")
    print("RAW_JSON_START")
    print(content)
    print("RAW_JSON_END")
    return content

def post_chat(name):
    payload = {
        "messages": [
            {"role": "system", "content": system_part},
            {"role": "user", "content": user_msg + "\n\nRespond with the JSON object now."},
        ],
        "grammar": grammar,
        "n_predict": 448,
        "temperature": 0.0,
        "stream": False,
    }
    path = pathlib.Path(workdir) / f"{name}.json"
    path.write_text(json.dumps(payload, ensure_ascii=False))
    t0 = time.perf_counter()
    req = urllib.request.Request(
        "http://127.0.0.1:8080/v1/chat/completions",
        data=path.read_bytes(),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    resp = json.loads(urllib.request.urlopen(req, timeout=180).read())
    wall = (time.perf_counter() - t0) * 1000
    tim = resp.get("timings", {})
    llm = tim.get("prompt_ms", 0) + tim.get("predicted_ms", 0)
    usage = resp.get("usage", {})
    tok = usage.get("completion_tokens", 0)
    tps = tok / (tim.get("predicted_ms", 1) / 1000) if tim.get("predicted_ms") else 0
    content = resp["choices"][0]["message"].get("content", "").strip()
    print(f"\n========== {name} ==========")
    print(f"wall_ms={wall:.0f} llm_ms={llm:.0f} tokens={tok} tok_s={tps:.1f}")
    print("RAW_JSON_START")
    print(content)
    print("RAW_JSON_END")
    return content

print("A1: HttpLlamaBackend uses POST /completion with build_prompt() as raw prompt string (no chat template).")
post_completion("A2a_raw_completion", raw_prompt)
post_completion("A2b_manual_chat_template_completion", chat_prompt)
post_chat("A2c_chat_completions_endpoint")
PY

kill "$SPID" 2>/dev/null || true
