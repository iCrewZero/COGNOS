#!/usr/bin/env bash
# GBNF lever benchmark — one isolated change per row (WSL)
set -euo pipefail
ROOT="/mnt/f/Software Engineering/COGNOS"
cd "$ROOT"
export PATH="$HOME/.cargo/bin:$PATH"
[ -f "$HOME/.cargo/env" ] && source "$HOME/.cargo/env"

BINDIR="$ROOT/build/cache/llama.cpp/build-cuda/bin"
MODEL="/root/cognos-models/qwen3-7b-instruct-q4_k_m.gguf"
GRAMMAR_BASE="$ROOT/intent-engine/grammar/intent.gbnf"
GRAMMAR_ENUM="$ROOT/intent-engine/grammar/intent-enum-tight.gbnf"
USER_INPUT="crée un dossier test dans /tmp"
RESULTS="/tmp/gbnf-lever-results.txt"

python3 "$ROOT/scripts/gen_intent_enum_tight_gbnf.py"

echo "=== LLAMA BUILD ==="
cd "$ROOT/build/cache/llama.cpp"
echo "commit: $(git log -1 --format='%h %ci %s')"
echo "upstream: $(git log origin/master -1 --format='%h %ci %s' 2>/dev/null || echo n/a)"
echo "grammar commits ahead: $(git log HEAD..origin/master --oneline -- 'common/*grammar*' '**/grammar*.cpp' '**/sampler*.cpp' 2>/dev/null | wc -l)"

fuser -k 8080/tcp 2>/dev/null || true
sleep 2
cd "$BINDIR"
stdbuf -oL -eL ./llama-server -m "$MODEL" --host 127.0.0.1 --port 8080 -ngl 99 -c 4096 --jinja --reasoning off -fa on \
  >/tmp/gbnf-lever-server.log 2>&1 &
SPID=$!
for _ in $(seq 1 120); do
  curl -fsS http://127.0.0.1:8080/health >/dev/null 2>&1 && break
  sleep 2
done

python3 - "$USER_INPUT" "$GRAMMAR_BASE" "$GRAMMAR_ENUM" "$RESULTS" <<'PY'
import json, pathlib, subprocess, sys, time, urllib.request

user_input, grammar_base, grammar_enum, results_path = sys.argv[1:5]
root = pathlib.Path("/mnt/f/Software Engineering/COGNOS")
prompt = subprocess.check_output(
    ["cargo", "run", "--quiet", "--example", "print_prompt", "--", user_input],
    cwd=root, text=True,
)

rows = []

def bench(label, grammar_text, n_predict=448, extra=None):
    body = {
        "prompt": prompt,
        "grammar": grammar_text,
        "n_predict": n_predict,
        "temperature": 0.0,
        "stream": False,
        "cache_prompt": True,
    }
    if extra:
        body.update(extra)
    out = {}
    for phase, cache in [("cold", False), ("hot", True)]:
        body["cache_prompt"] = cache
        pl = json.dumps(body).encode()
        try:
            t0 = time.perf_counter()
            resp = json.loads(urllib.request.urlopen(urllib.request.Request(
                "http://127.0.0.1:8080/completion", data=pl,
                headers={"Content-Type": "application/json"}, method="POST"), timeout=300).read())
            wall = (time.perf_counter() - t0) * 1000
            tim = resp["timings"]
            tok = resp["tokens_predicted"]
            tps = tok / (tim["predicted_ms"] / 1000) if tim["predicted_ms"] else 0.0
            out[phase] = {
                "wall_ms": round(wall),
                "predicted_ms": round(tim["predicted_ms"]),
                "tokens": tok,
                "tok_s": round(tps, 2),
                "content": resp["content"].strip(),
            }
        except Exception as e:
            err = str(e)
            if hasattr(e, "read"):
                err += " " + e.read().decode(errors="replace")[:200]
            out[phase] = {"error": err}
    rows.append((label, out))
    print(f"\n=== {label} ===")
    for phase in ("cold", "hot"):
        o = out[phase]
        if "error" in o:
            print(f"{phase}: ERROR {o['error']}")
        else:
            print(
                f"{phase}: wall_ms={o['wall_ms']} predicted_ms={o['predicted_ms']} "
                f"tokens={o['tokens']} tok_s={o['tok_s']}"
            )
    return out

grammar = pathlib.Path(grammar_base).read_text(encoding="utf-8")
grammar_tight = pathlib.Path(grammar_enum).read_text(encoding="utf-8")

bench("1_baseline", grammar)
bench("2_lazy_pattern_brace", grammar, extra={
    "grammar_lazy": True,
    "grammar_triggers": [{"type": 2, "value": "^\\{"}],
})
bench("2b_lazy_pattern_full", grammar, extra={
    "grammar_lazy": True,
    "grammar_triggers": [{"type": 3, "value": "^\\{"}],
})
bench("3_enum_tight_grammar", grammar_tight)
bench("4_n_predict_128", grammar, n_predict=128)
bench("5_enum_tight_n_predict_128", grammar_tight, n_predict=128)

# validate JSON from best-looking grammar outputs
print("\n=== PARSE CHECK (baseline cold) ===")
cold_content = rows[0][1].get("cold", {}).get("content", "")
if cold_content:
    p = subprocess.run(
        ["cargo", "run", "--quiet", "--example", "parse_intent_with_input"],
        cwd=root, input=f"{user_input}\n---\n{cold_content}", text=True, capture_output=True,
    )
    print(p.stdout.strip()[:500])
    if p.returncode != 0:
        print("PARSE_ERR", p.stderr[:300])

pathlib.Path(results_path).write_text(json.dumps(rows, indent=2), encoding="utf-8")
print(f"\nWrote {results_path}")
PY

kill "$SPID" 2>/dev/null || true

echo "=== grammar_schema (baseline grammar in repo) ==="
cargo test -p cognos-intent-engine --test grammar_schema -q
