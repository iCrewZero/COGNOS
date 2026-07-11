# Verdict — vLLM en production (pipeline réel)

**Mesuré :** 2026-07-10T17:50:45Z (UTC)

## 1. État des lieux (code)

### Backend production
- `HttpVllmBackend` (`intent-engine/src/backends/http_vllm.rs`) — **PAS** `HttpLlamaBackend` en prod par défaut.
- `HttpLlamaBackend` reste legacy (`COGNOS_INTENT_BACKEND=llama` → `POST /completion` + GBNF).

Body requête vLLM actuel (`complete()`):
```json
{
  "model": "<config.model>",
  "prompt": "<build_prompt()>",
  "temperature": 0.0,
  "max_tokens": 448,
  "stream": false,
  "structured_outputs": { "json": <intent-llm-output.schema.json v2> }
}
```
Endpoint: `POST {endpoint}/v1/completions`

### intent.toml
```toml
[inference]
# Production backend: vLLM with XGrammar JSON Schema structured output.
backend = "vllm"
endpoint = "http://127.0.0.1:8080"
model = "Qwen/Qwen2.5-7B-Instruct-AWQ"
timeout_ms = 30000
schema = "/etc/cognos/intent-llm-output.schema.json"
# Legacy llama-server GBNF path (only when backend = "llama").
grammar = "/etc/cognos/intent.gbnf"

```

### Goals réseau
- `unsupported_goals.rs` + blocage `status=unsupported` dans `intent_main.rs`
- Test: `intent-engine/tests/network_goal_blocked.rs`

**Verdict état initial : intégration vLLM DÉJÀ FAITE** (tour précédent).

## 2. vLLM démarrage

| Check | Résultat |
|-------|----------|
| `GET /health` | **HTTP 200** |
| Modèle chargé | `Qwen/Qwen2.5-7B-Instruct-AWQ` |
| VRAM (nvidia-smi) | `NVIDIA GeForce RTX 4090 Laptop GPU, 15970 MiB, 16376 MiB` |

## 3. E2E réel (`dev_e2e.sh real`)

Intent: `crée un dossier test dans /tmp`

### Sortie CLI brute
```
trace_id:   24ceb396-6ce6-407b-9d78-ad3c2edd42e4
intent_id:  09c82ad3-1854-43f2-a8cc-a27981b00c54
status:     ok
message:    completed 1 task(s)
latency:    total=8967ms parse=8764ms orchestrate=42ms execute=161ms

Tasks:
  [succeeded] create_dir → /tmp/test
           HAL: granted (risk=0.00)
           Created directory /tmp/test
==> E2E latency_ms=9090
[2m2026-07-10T17:54:32.548203Z[0m [32m INFO[0m [2mcognos_intent[0m[2m:[0m pipeline stage [3mtrace_id[0m[2m=[0m8ebcb63f-3415-4d38-a57a-a66c600b832e [3mstage[0m[2m=[0m"parse_llm" [3mlatency_ms[0m[2m=[0m8753 [3mcache_hit[0m[2m=[0mfalse [3msource[0m[2m=[0mvllm
==> E2E OK (real): /tmp/test exists, HAL visible in CLI output
```

### IntentSchema produit (DispatchIntent → intent-engine → vLLM)
```
{
  "intent_id": "df39ee33-a87e-44b5-a17c-5475ad8b04b8",
  "raw_input": "crée un dossier test dans /tmp",
  "goal": "create_dir",
  "domain": null,
  "confidence": 1.0,
  "ambiguity_score": 0.0,
  "risk_estimate": 0.0,
  "required_context": [],
  "candidate_actions": [
    {
      "action": "create_dir",
      "target": "/tmp/test",
      "confidence": 1.0,
      "recency_score": 0.0
    }
  ],
  "disambiguation_required": false,
  "disambiguation_question": null,
  "session_context": {
    "last_active_domain": null,
    "last_active_files": [],
    "current_time": "17:54",
    "time_since_last_session": null
  },
  "hal_pre_score": 0.0,
  "escalate_to_cloud": false,
  "source": "vllm"
}
action_graph_nodes=1
```

### Preuve source vLLM (intent.log)
```
[2m2026-07-10T17:54:40.635739Z[0m [32m INFO[0m [2mcognos_intent[0m[2m:[0m vLLM backend: endpoint=http://127.0.0.1:8080 model=Qwen/Qwen2.5-7B-Instruct-AWQ schema=/mnt/f/Software Engineering/COGNOS/intent-engine/schema/intent-llm-output.schema.json
[2m2026-07-10T17:54:48.258553Z[0m [32m INFO[0m [2mcognos_intent[0m[2m:[0m pipeline stage [3mtrace_id[0m[2m=[0md5d55a21-8856-48d4-9b7c-039a6514b585 [3mstage[0m[2m=[0m"parse_llm" [3mlatency_ms[0m[2m=[0m6381 [3mcache_hit[0m[2m=[0mfalse [3msource[0m[2m=[0mvllm
```

## 4. Goals réseau (live)

```
status=unsupported
message=goal reconnu mais non supporté en v1: network_download
--- IntentSchema (result_json) ---
{
  "intent_id": "3828f9fd-ff1b-4d9f-8bc6-876db8a91fa8",
  "raw_input": "télécharge le fichier depuis https://mirror.internal/archive.tar",
  "goal": "network_download",
  "domain": null,
  "confidence": 1.0,
  "ambiguity_score": 0.0,
  "risk_estimate": 0.0,
  "required_context": [],
  "candidate_actions": [
    {
      "action": "download_file",
      "target": "https://mirror.internal/archive.tar",
      "confidence": 1.0,
      "recency_score": 0.0
    }
  ],
  "disambiguation_required": false,
  "disambiguation_question": null,
  "session_context": {
    "last_active_domain": null,
    "last_active_files": [],
    "current_time": "17:54",
    "time_since_last_session": null
  },
  "hal_pre_score": 0.0,
  "escalate_to_cloud": false,
  "source": "vllm"
}
```

## 5. Non-régression

### cargo test --workspace
```

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
Total tests passed (sum of test result lines): 232
```

### pytest agents/
```

-- Docs: https://docs.pytest.org/en/stable/how-to/capture-warnings.html
33 passed, 1 warning in 6.81s
```

### dev_e2e.sh mock
rc=0 — ==> E2E OK (mock): /tmp/test exists, HAL visible in CLI output

### network_goal_blocked (cargo test)
```

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.18s
```

## Verdict

**OUI** — l'intent manager est opérationnel en production via vLLM de bout en bout (CLI → orchestrator → intent-engine → vLLM → HAL → file_agent). source=vllm, pas keyword_fallback.

