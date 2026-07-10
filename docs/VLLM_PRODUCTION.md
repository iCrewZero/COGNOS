# vLLM production integration

**Mesuré :** 2026-07-10 (WSL, RTX 4090)

## Stack

| Composant | Valeur |
|-----------|--------|
| Backend | `HttpVllmBackend` (`intent-engine/src/backends/http_vllm.rs`) |
| API | `POST /v1/completions` + `structured_outputs.json` (schéma prod v2) |
| Modèle | `Qwen/Qwen2.5-7B-Instruct-AWQ` |
| Prompt | `intent-engine/src/prompt.rs` — **inchangé** |
| Fallback | `FallbackBackend` + `KeywordBackend` (`source=keyword_fallback`) |
| Service | `services/cognos-vllm.service` (remplace `cognos-llm.service` au rootfs) |

## Config (`config/intent.toml`)

```toml
[inference]
backend = "vllm"
endpoint = "http://127.0.0.1:8080"
model = "Qwen/Qwen2.5-7B-Instruct-AWQ"
schema = "/etc/cognos/intent-llm-output.schema.json"
```

Env : `COGNOS_INTENT_BACKEND=llama` pour revenir au legacy llama-server+GBNF.

## Démarrage dev

```bash
bash scripts/start_vllm_wsl.sh          # vLLM seul
bash scripts/dev_e2e.sh real            # pipeline complet
bash scripts/dev_e2e.sh mock            # MOCK_LLM=1 (non-régression)
```

## Goals réseau (v1)

`network_download`, `network_send` : parsés, puis `status=unsupported` avant dispatch/HAL.
Voir `intent-engine/src/unsupported_goals.rs`. **Aucune modif `hal/src/`.**

## E2E réel (mesuré)

Intent : `crée un dossier test dans /tmp` → `/tmp/test` créé, HAL granted, `source=vllm`.

```
latency:    total=2678ms parse=2438ms orchestrate=49ms execute=191ms
Tasks:
  [succeeded] create_dir → /tmp/test
           HAL: granted (risk=0.00)
```

Log intent : `source=vllm`, `cache_hit=false`, `latency_ms=2425`.
