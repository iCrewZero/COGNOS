#!/usr/bin/env bash
set -euo pipefail
ROOT="/mnt/f/Software Engineering/COGNOS"
export PATH="$HOME/.cargo/bin:$PATH"
cd "$ROOT"
fuser -k 8080/tcp 2>/dev/null || true
sleep 2
cd "$ROOT/build/cache/llama.cpp/build-cuda/bin"
./llama-server -m /root/cognos-models/qwen3-7b-instruct-q4_k_m.gguf --host 127.0.0.1 --port 8080 -ngl 99 -c 4096 --jinja --reasoning off -fa on >/dev/null 2>&1 &
for _ in $(seq 1 90); do curl -fsS http://127.0.0.1:8080/health >/dev/null 2>&1 && break; sleep 2; done
python3 - <<'PY'
import json, subprocess, pathlib, urllib.request
root = pathlib.Path("/mnt/f/Software Engineering/COGNOS")
prompt = subprocess.check_output(["cargo","run","--quiet","--example","print_prompt","--","crée un dossier test dans /tmp"], cwd=root, text=True)
grammar = (root/"intent-engine/grammar/intent.gbnf").read_text()
for n in (192, 224, 256):
    body = {"prompt": prompt, "grammar": grammar, "n_predict": n, "temperature": 0, "stream": False, "cache_prompt": False}
    resp = json.loads(urllib.request.urlopen(urllib.request.Request("http://127.0.0.1:8080/completion", data=json.dumps(body).encode(), headers={"Content-Type":"application/json"}, method="POST"), timeout=180).read())
    tim = resp["timings"]; tok = resp["tokens_predicted"]
    tps = tok/(tim["predicted_ms"]/1000)
    content = resp["content"].strip()
    p = subprocess.run(["cargo","run","--quiet","--example","parse_intent_with_input"], cwd=root, input=f"crée un dossier test dans /tmp\n---\n{content}", text=True, capture_output=True)
    print(f"n_predict={n} tokens={tok} tok_s={tps:.2f} predicted_ms={tim['predicted_ms']:.0f} parse_ok={p.returncode==0} stop={resp.get('stop_type')}")
PY
pkill -f 'llama-server.*8080' || true
