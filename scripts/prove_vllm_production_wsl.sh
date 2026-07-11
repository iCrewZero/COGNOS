#!/usr/bin/env bash
# Prove vLLM production pipeline end-to-end (WSL). Writes docs/PROD_VLLM_VERDICT.md
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
OUT="$ROOT/docs/PROD_VLLM_VERDICT.md"
LOG_DIR="$ROOT/build/e2e_logs"
mkdir -p "$LOG_DIR"
TS=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
VENV="/root/cognos-vllm-venv"
PY="$VENV/bin/python"

exec > >(tee "$LOG_DIR/prove_vllm_production.log") 2>&1

echo "=== PROVE vLLM PRODUCTION $TS ==="

# ── 1. État des lieux (code) ─────────────────────────────────────────────────
cat >"$OUT" <<'HDR'
# Verdict — vLLM en production (pipeline réel)

HDR
{
  echo "**Mesuré :** $TS (UTC)"
  echo ""
  echo "## 1. État des lieux (code)"
  echo ""
  echo "### Backend production"
  echo "- \`HttpVllmBackend\` (\`intent-engine/src/backends/http_vllm.rs\`) — **PAS** \`HttpLlamaBackend\` en prod par défaut."
  echo "- \`HttpLlamaBackend\` reste legacy (\`COGNOS_INTENT_BACKEND=llama\` → \`POST /completion\` + GBNF)."
  echo ""
  echo "Body requête vLLM actuel (\`complete()\`):"
  echo '```json'
  echo '{'
  echo '  "model": "<config.model>",'
  echo '  "prompt": "<build_prompt()>",'
  echo '  "temperature": 0.0,'
  echo '  "max_tokens": 448,'
  echo '  "stream": false,'
  echo '  "structured_outputs": { "json": <intent-llm-output.schema.json v2> }'
  echo '}'
  echo '```'
  echo "Endpoint: \`POST {endpoint}/v1/completions\`"
  echo ""
  echo "### intent.toml"
  echo '```toml'
  sed -n '/\[inference\]/,/^\[/p' config/intent.toml | head -n -1 || true
  echo '```'
  echo ""
  echo "### Goals réseau"
  echo "- \`unsupported_goals.rs\` + blocage \`status=unsupported\` dans \`intent_main.rs\`"
  echo "- Test: \`intent-engine/tests/network_goal_blocked.rs\`"
  echo ""
  echo "**Verdict état initial : intégration vLLM DÉJÀ FAITE** (tour précédent)."
  echo ""
} >>"$OUT"

# ── 2. Démarrer vLLM ───────────────────────────────────────────────────────
echo ""
echo "==> [2] Start vLLM + /health wait"
bash scripts/start_vllm_wsl.sh 2>&1 | tee "$LOG_DIR/vllm_start_proof.log"

HEALTH_CODE=$(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:8080/health)
MODELS_JSON=$(curl -sf http://127.0.0.1:8080/v1/models)
MODEL_ID=$(echo "$MODELS_JSON" | python3 -c 'import sys,json; d=json.load(sys.stdin); print(d["data"][0]["id"])' 2>/dev/null || echo "?")
NVIDIA_SMI=$(nvidia-smi --query-gpu=name,memory.used,memory.total --format=csv,noheader 2>/dev/null || echo "nvidia-smi unavailable")

{
  echo "## 2. vLLM démarrage"
  echo ""
  echo "| Check | Résultat |"
  echo "|-------|----------|"
  echo "| \`GET /health\` | **HTTP $HEALTH_CODE** |"
  echo "| Modèle chargé | \`$MODEL_ID\` |"
  echo "| VRAM (nvidia-smi) | \`$NVIDIA_SMI\` |"
  echo ""
} >>"$OUT"

# ── 3. E2E réel ──────────────────────────────────────────────────────────────
echo ""
echo "==> [3] dev_e2e.sh real"
E2E_LOG="$LOG_DIR/dev_e2e_real_proof.log"
bash scripts/dev_e2e.sh real 2>&1 | tee "$E2E_LOG"
E2E_RC=${PIPESTATUS[0]}

{
  echo "## 3. E2E réel (\`dev_e2e.sh real\`)"
  echo ""
  echo "Intent: \`crée un dossier test dans /tmp\`"
  echo ""
  echo "### Sortie CLI brute"
  echo '```'
  grep -v '^\[' "$E2E_LOG" | sed -n '/trace_id:/,/E2E OK/p' | head -n 20 || tail -25 "$E2E_LOG"
  echo '```'
  echo ""
} >>"$OUT"

if (( E2E_RC != 0 )); then
  echo "**E2E ÉCHEC (rc=$E2E_RC)** — voir $E2E_LOG" >>"$OUT"
  echo "VERDICT: ÉCHEC e2e real" >>"$OUT"
  exit "$E2E_RC"
fi

# IntentSchema via DispatchIntent direct (intent-engine, vLLM live)
echo ""
echo "==> [3b] Capture IntentSchema (DispatchIntent → intent-engine)"
# Restart minimal stack for schema capture (e2e killed services)
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
cargo build --workspace --bins -q
BIN="$ROOT/target/debug"
[[ -x "$BIN/cognos-ipc-server" ]] || BIN="$ROOT/target/release"

PIDS=()
cleanup() { for p in "${PIDS[@]:-}"; do kill "$p" 2>/dev/null || true; done; }
trap cleanup EXIT

"$BIN/cognos-ipc-server" >"$LOG_DIR/ipc_schema.log" 2>&1 & PIDS+=($!)
for i in $(seq 1 90); do (echo >/dev/tcp/127.0.0.1/7443) 2>/dev/null && break; sleep 1; done
"$BIN/cognos-hal" >"$LOG_DIR/hal_schema.log" 2>&1 & PIDS+=($!)
for i in $(seq 1 90); do (echo >/dev/tcp/127.0.0.1/7444) 2>/dev/null && break; sleep 1; done
export COGNOS_INTENT_SCHEMA="$ROOT/intent-engine/schema/intent-llm-output.schema.json"
RUST_LOG=info stdbuf -oL -eL "$BIN/cognos-intent" --config "$ROOT/config/intent.toml" >"$LOG_DIR/intent_schema.log" 2>&1 & PIDS+=($!)
for i in $(seq 1 90); do (echo >/dev/tcp/127.0.0.1/7445) 2>/dev/null && break; sleep 1; done

SCHEMA_OUT="$LOG_DIR/intent_schema_capture.txt"
"$PY" "$ROOT/scripts/capture_intent_schema.py" "crée un dossier test dans /tmp" 2>&1 | tee "$SCHEMA_OUT"

{
  echo "### IntentSchema produit (DispatchIntent → intent-engine → vLLM)"
  echo '```'
  sed -n '/--- IntentSchema/,$p' "$SCHEMA_OUT" | tail -n +2
  echo '```'
  echo ""
  echo "### Preuve source vLLM (intent.log)"
  echo '```'
  grep -aE 'source.*vllm|vLLM backend' "$LOG_DIR/intent_schema.log" | tail -3 || grep -aE 'source.*vllm' "$LOG_DIR/intent.log" | tail -3 || echo "(voir logs)"
  echo '```'
  echo ""
} >>"$OUT"

# ── 4. Test réseau live ──────────────────────────────────────────────────────
echo ""
echo "==> [4] Network goal live test"
NET_OUT="$LOG_DIR/network_blocked_live.txt"
"$PY" "$ROOT/scripts/capture_intent_schema.py" "télécharge le fichier depuis https://mirror.internal/archive.tar" 2>&1 | tee "$NET_OUT" || true

{
  echo "## 4. Goals réseau (live)"
  echo ""
  echo '```'
  cat "$NET_OUT"
  echo '```'
  echo ""
} >>"$OUT"

if grep -q 'status=unsupported' "$NET_OUT" && grep -qi 'non supporté' "$NET_OUT"; then
  NET_OK=1
else
  NET_OK=0
fi

# cargo test network_goal_blocked (unit)
NET_TEST=$(cargo test -p cognos-intent-engine --test network_goal_blocked -- --nocapture 2>&1 | tail -3)

# ── 5. Non-régression ───────────────────────────────────────────────────────
echo ""
echo "==> [5] Non-régression"
CARGO_OUT=$(cargo test --workspace 2>&1 | tail -5)
CARGO_COUNT=$(cargo test --workspace 2>&1 | grep -E '^test result:' | awk '{s+=$4} END {print s+0}')
PYTEST_OUT=$(.venv/bin/pytest agents/ -q 2>&1 | tail -3)
MOCK_LOG="$LOG_DIR/dev_e2e_mock_proof.log"
bash scripts/dev_e2e.sh mock 2>&1 | tee "$MOCK_LOG"
MOCK_RC=${PIPESTATUS[0]}

{
  echo "## 5. Non-régression"
  echo ""
  echo "### cargo test --workspace"
  echo '```'
  echo "$CARGO_OUT"
  echo "Total tests passed (sum of test result lines): $CARGO_COUNT"
  echo '```'
  echo ""
  echo "### pytest agents/"
  echo '```'
  echo "$PYTEST_OUT"
  echo '```'
  echo ""
  echo "### dev_e2e.sh mock"
  echo "rc=$MOCK_RC — $(grep 'E2E OK' "$MOCK_LOG" || echo FAIL)"
  echo ""
  echo "### network_goal_blocked (cargo test)"
  echo '```'
  echo "$NET_TEST"
  echo '```'
  echo ""
} >>"$OUT"

# ── Verdict ──────────────────────────────────────────────────────────────────
SOURCE_OK=0
grep -aE 'source.*vllm' "$LOG_DIR/intent_schema.log" 2>/dev/null && SOURCE_OK=1
grep -aE '"source": "vllm"|"source":"vllm"' "$SCHEMA_OUT" 2>/dev/null && SOURCE_OK=1

if (( E2E_RC == 0 && MOCK_RC == 0 && NET_OK == 1 && SOURCE_OK == 1 )); then
  VERDICT="**OUI** — l'intent manager est opérationnel en production via vLLM de bout en bout (CLI → orchestrator → intent-engine → vLLM → HAL → file_agent). source=vllm, pas keyword_fallback."
else
  VERDICT="**NON ou PARTIEL** — voir sections ci-dessus. e2e_rc=$E2E_RC mock_rc=$MOCK_RC net_ok=$NET_OK source_ok=$SOURCE_OK"
fi

{
  echo "## Verdict"
  echo ""
  echo "$VERDICT"
  echo ""
} >>"$OUT"

echo ""
echo "=== $VERDICT ==="
echo "Wrote $OUT"
