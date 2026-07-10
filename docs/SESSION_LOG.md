# SESSION_LOG — COGNOS/OS (session non supervisée)

Journal horodaté des tâches exécutées automatiquement. Les décisions de jugement
sont laissées vides ou marquées **DÉCISION HUMAINE** pour revue au retour.

---

## Tâche 1 — Protection immédiate des données

**Horodatage :** 2026-07-10T11:25:00Z (WSL)

### Actions

1. Mise à jour `.gitignore` :
   - `*.gguf`, `*.safetensors`, `*.bin`, `*.pt`, `*.pth`, `*.ckpt` (poids ML)
   - `/build/artifacts/`, `/build/rootfs/`, `*.iso`
   - `/build/cache/` (déjà présent, conservé)
   - fixtures lourdes : `tmp/`, `**/vllm-golden-*.json|log`, `**/vllm-xgrammar-*.json|log`
2. Audit `git ls-files` extensions lourdes : **aucun fichier tracké**
3. Audit secrets dans l’index git (voir ci-dessous)

### Gros binaires déjà trackés (`git ls-files | grep -E '\.(gguf|safetensors|bin|iso)$'`)

```
(aucun)
```

**DÉCISION HUMAINE :** rien à retirer de l’historique pour ces extensions.

### Fichiers trackés > 1 MiB

Non mesuré automatiquement (script audit bloqué par quoting PowerShell→WSL).
Extensions ciblées : **0 fichier tracké** dans la liste complète `git ls-files`
( revue manuelle de la sortie — pas de `.gguf`, `.safetensors`, `.bin`, `.iso` ).

### Secrets trackés

`git grep -n COGNOS_IPC_SECRET` — références **nom de variable / génération runtime** uniquement :

| Fichier | Nature |
|---------|--------|
| `agents/shared/ipc.py` | commentaire (lecture env) |
| `cli/src/runtime.rs` | `std::env::var("COGNOS_IPC_SECRET")` |
| `orchestrator/src/bin/orchestrator_main.rs` | idem |
| `scripts/rootfs_builder.sh` | génération `secrets.token_hex(32)` au premier boot |

Aucune valeur littérale `COGNOS_IPC_SECRET=<hex>` trackée.
Aucun fichier `.env` tracké.
Aucun fichier `.pem` tracké.

**DÉCISION HUMAINE :** confirmer que l’absence de secret en clair est suffisante.

### Vérification

- [x] `.gitignore` mis à jour
- [x] État gros fichiers / secrets consigné
- [x] Commit : `575aad7` — `chore: ignore ML weights, build artifacts, and heavy POC fixtures`
  (fichiers : `.gitignore`, `docs/SESSION_LOG.md`)

### Échecs / blocages

- `scripts/audit_git_hygiene.sh` : CRLF Windows → `set: pipefail\r` sous WSL ; **non utilisé**
  pour les conclusions ci-dessus (commandes `wsl --cd` directes à la place).

---

## Tâche 2 — CI Linux verte

**Horodatage :** 2026-07-10T11:44:00Z (WSL)

### Corrections code (avant CI)

1. **`memory/Cargo.toml`** : `cognos-anfs` / `reqwest` / `libc` sous `[dependencies]` optional
   (HEAD les avait sous `[dev-dependencies] optional = true` → invalide Cargo).
2. **Warnings `cargo build --workspace --bins`** : imports inutilisés nettoyés
   (scheduler, orchestrator, hal — pas de logique HAL modifiée).
   `ipc/grpc` : `client.rs` sans `base64::Engine` ; `server.rs` : `Arc` utilisé.
   CLI v0 stubs : `#![allow(dead_code)]` sur commands/runtime/tui.
3. **Scan final** : `cargo build --workspace --bins 2>&1 | grep warning` → **(none)**

### Prérequis session (blocages résolus)

| Problème | Action |
|----------|--------|
| Disque `F:\` 100% plein | `cargo clean` → **14,6 GiB** libérés ; `CARGO_TARGET_DIR=/tmp/cognos-cargo-target` |
| `grpcio-tools` absent | `.venv` + `pip install -r agents/requirements.txt pytest` |
| CRLF scripts | `sed` sur `dev_e2e.sh`, `run_ci_task2_wsl.sh` |

### Séquence CI locale — résultats bruts

| Étape | Commande | Exit | Résumé |
|-------|----------|------|--------|
| 1 | `make -C build proto` | **0** | Proto stubs générés via `.venv/bin/python` |
| 2 | `cargo build --workspace --bins` | **0** | Finished dev profile |
| 3 | `cargo check --workspace --all-targets` | **0** | Finished en 41s |
| 4 | `cargo test --workspace` | **0** | Tous crates verts (intent-engine 79 tests, hal 50…) |
| 5 | `.venv/bin/pytest agents/` | **0** | **33 passed**, 1 PytestConfigWarning |
| 6 | `bash scripts/dev_e2e.sh mock` | **0** | `/tmp/test` + `HAL: granted` |

**E2E extrait :**
```
[succeeded] create_dir → /tmp/test
         HAL: granted (risk=0.00)
==> E2E OK (mock): /tmp/test exists, HAL visible in CLI output
```

### Preuve `cfg(unix)` — `HalDaemon::run()`

`cargo check -p cognos-hal --all-targets` → exit **0**

`cargo rustc -p cognos-hal --bin cognos-hal -- --print cfg` :
```
target_family="unix"
target_os="linux"
unix
```

### GitHub Actions

`gh` non installé dans WSL → **déclenchement distant en attente**.

### Scripts reproductibilité

- `scripts/run_ci_task2_wsl.sh`
- `scripts/setup_ci_venv_wsl.sh`

### Échecs intermédiaires

1. Disque plein → `cargo test` exit 101
2. proto/pytest/CRLF → résolus avant run final exit 0

---

## Tâche 3 — Clarification provenance modèle

**Horodatage :** 2026-07-10T12:00:00Z (WSL)

### Mesures effectuées

1. **GGUF llama.cpp** — `scripts/llama_load_log_snippet.sh` sur
   `/root/cognos-models/qwen3-7b-instruct-q4_k_m.gguf` :
   - `general.architecture = qwen2`
   - `general.name = Qwen2.5 7B Instruct`
   - `general.file_type = 15` → **Q4_K_M**, taille fichier **4,36 GiB**
   - tags **mergekit/merge**, 3 `base_model` Qwen2.5 (Coder, Instruct, Math)
2. **VRAM llama** — `scripts/llama_vram_probe_wsl.sh` : **4888 MiB** / 16376 MiB
3. **vLLM POC** — venv `/root/cognos-vllm-venv` : vLLM **0.24.0**, torch **2.11.0+cu130**
   ; modèle **`Qwen/Qwen2.5-7B-Instruct-AWQ`** ; cache HF **5,2 Go**
4. **Verdict** : nom repo/config « qwen3 » **trompeur** ; les deux moteurs = **famille Qwen2.5 7B
   Instruct**, mais **poids différents** (merge GGUF Q4_K_M vs HF AWQ)

### Livrable

- `docs/MODEL_PROVENANCE.md` — métadonnées citées, tableau par tour, alerte comparaisons

### Scripts ajoutés (mesure)

- `scripts/llama_load_log_snippet.sh`
- `scripts/llama_vram_probe_wsl.sh`
- `scripts/dump_gguf_meta.py`, `scripts/read_gguf_gguflib.py`, `scripts/probe_vllm_versions.py`

---

## Tâche 4 — JSON Schema consolidé et testé

**Horodatage :** 2026-07-10T15:35:00Z (WSL)

### Livrables

1. **`intent-engine/schema/intent-llm-output.schema.json`** — version **1.0.0**,
   métadonnées `x-cognos-*` (champs LLM vs injectés documentés).
2. **`intent-engine/src/llm_output_schema.rs`** — struct `LlmEmittedIntent`,
   dérivation dynamique des noms de champs via serde.
3. **Tests** :
   - `tests/json_schema_coverage.rs` — couverture exacte struct ↔ schéma JSON
   - `tests/grammar_json_schema.rs` — cohérence GBNF ↔ JSON Schema (champs + enums)

### Vérification

- `scripts/run_overfit_check_wsl.sh`

```bash
cargo test -p cognos-intent-engine --test json_schema_coverage --test grammar_json_schema
```

---

## Overfit check — prompt_v2 (mesure, prompt inchangé)

**Horodatage :** 2026-07-10T16:02:30Z (UTC)

### Validation

- 20 fixtures neuves : `intent-engine/tests/validation/` (v01–v20)
- Harnais : `scripts/eval_golden_quality.py --overfit-check`
- Rapport : `docs/OVERFIT_CHECK.md`, JSON : `tmp/overfit_check.json`

### Scores mesurés (Qwen2.5-7B-AWQ, vLLM 0.24.0)

| Schéma | Golden goal | Golden disambig | Validation goal | Validation disambig |
|--------|-------------|---------------|-----------------|---------------------|
| **Production** | **8/15** | 15/15 | **14/20** | 18/20 |
| Éval élargi | 14/15 | 15/15 | 17/20 | 18/20 |

### Verdict

Pas d'overfit : validation prod **supérieure** à golden prod. Le 14/15 session = schéma éval élargi uniquement.

### Court-circuit `empty_input`

`intent-engine/src/parser.rs` : whitespace → `await_input` sans LLM ; test `empty_input_short_circuits_without_backend` vert.

---

## Alignement schéma production — goal taxonomy v2

**Horodatage :** 2026-07-10T16:16:29Z (UTC, mesuré)

### Changements (schéma + GBNF uniquement)

1. **`docs/GOAL_TAXONOMY.md`** — source de vérité (13 goals canoniques).
2. **`intent-engine/schema/intent-llm-output.schema.json`** — **v2.0.0** : goals, actions, domaines, pattern target élargis.
3. **`intent-engine/grammar/intent.gbnf`** — `goal-string`, `action-string`, `domain-value`, `target-string` alignés.
4. **`intent-engine/src/llm_output_schema.rs`** — `SCHEMA_VERSION = "2.0.0"`.
5. Harnais : `scripts/eval_golden_quality.py --prod-aligned-measure`.

### Goals supportés parser mais route HAL à définir (décision humaine)

- **`network_download`** — pas de mapping goal→`network.outbound` dans l'orchestrator.
- **`network_send`** — idem pour envoi réseau / `send_email`.

Aucune modification HAL ce tour.

### Vérification cohérence

```bash
cargo test -p cognos-intent-engine --test json_schema_coverage --test grammar_json_schema
```

**Résultat :** 12 tests verts (6 + 6).

### Remesure (prompt inchangé, Qwen2.5-7B-AWQ, vLLM 0.24.0)

| Jeu | goal étroit → aligné | Δ | disambig | disambig pur |
|-----|---------------------|---|----------|--------------|
| Golden | 8/15 → **14/15** | **+6** | 15/15 → 15/15 | **0** |
| Validation | 14/20 → **17/20** | **+3** | 18/20 → 18/20 | **2** (v05, v06) |

Scores alignés prod = scores éval élargi (gap schéma refermé). Rapport : `docs/PROD_SCHEMA_ALIGNED.md`, JSON : `tmp/prod_schema_aligned.json`.

**Gap sémantique résiduel (disambig pur) :** 2 cas validation (`v05`, `v06`) — goal OK, `disambiguation_required` attendu `true`, produit `false`. Non corrigé ce tour.

**Échecs goal résiduels (aligné) :** golden `14_empty_input` (LLM `out_of_scope` vs `await_input` — court-circuit Rust hors harnais) ; validation `v13`, `v17`, `v18`.

---

## Harnais empty_input + proposition HAL réseau

**Horodatage :** 2026-07-10T16:28:16Z (UTC, mesuré)

### Correction artefact mesure

- `scripts/eval_golden_quality.py` : `normalize_input` + court-circuit `await_input` (miroir `parser.rs`), sans appel LLM.
- Vérif : `scripts/verify_empty_short_circuit.py` + tests `tokenizer::` verts.

### Scores finaux (prod v2 + short-circuit harnais, prompt inchangé)

| Jeu | goal | disambig |
|-----|------|----------|
| Golden | **15/15** | 15/15 |
| Validation | **18/20** | 18/20 |

`14_empty_input` et `v17_empty_input` : **SHORT_CIRCUIT** → passent.

### Résidus (non traités ce tour)

- Disambig pur : **2** (`v05`, `v06`) — prompt ultérieur.
- Goal sémantique : `v13` (search_files), `v18` (code_task).
- HAL réseau : `network_download`, `network_send` — proposition dans `docs/HAL_NETWORK_ROUTING_PROPOSAL.md` (zéro ligne `hal/src/`).

### Livrables

- `docs/QUALITY_FINAL.md`, `tmp/quality_final.json`
- `docs/HAL_NETWORK_ROUTING_PROPOSAL.md`

---

## vLLM production — pipeline bout en bout

**Horodatage :** 2026-07-10T16:51:43Z (UTC, mesuré)

### Changements

1. **`HttpVllmBackend`** — `POST /v1/completions` + JSON Schema structured output (prod v2).
2. **`config/intent.toml`** — `[inference] backend=vllm`, modèle AWQ, schéma prod.
3. **`services/cognos-vllm.service`** — remplace llama-server au rootfs (note dans unité).
4. **Goals réseau** — `unsupported_goals.rs` + blocage `status=unsupported` dans `intent_main.rs` (zéro modif `hal/src/`).
5. **`scripts/dev_e2e.sh real`** — vLLM + assertions `source=vllm`, `parse>=500ms`.

### E2E réel (vLLM)

```
latency: total=2678ms parse=2438ms orchestrate=49ms execute=191ms
Tasks: create_dir → /tmp/test, HAL: granted (risk=0.00)
intent.log: source=vllm cache_hit=false
```

### Non-régression

- `cargo test --workspace` : vert
- `pytest agents/` : 33 passed
- `dev_e2e.sh mock` : vert

### HAL audit

Aucune modification `hal/src/` ce tour.

---

## Preuve pipeline production vLLM (audit indépendant)

**Horodatage :** 2026-07-10T17:57:10Z (UTC, mesuré)

### État initial

Intégration **déjà faite** : `HttpVllmBackend` (`POST /v1/completions` + `structured_outputs.json`), pas `HttpLlamaBackend` en prod. Goals réseau câblés (`unsupported_goals.rs` + test `network_goal_blocked.rs`).

### vLLM démarrage

| Check | Résultat |
|-------|----------|
| `/health` | HTTP **200** |
| Modèle | `Qwen/Qwen2.5-7B-Instruct-AWQ` |
| VRAM | **15970 / 16376 MiB** (RTX 4090 Laptop) |

Note : premier cold start peut dépasser 600s (CUDA graphs) — `start_vllm_wsl.sh` attend `/health` ou `/v1/models` jusqu'à 900s (override `VLLM_STARTUP_WAIT_SECS`).

### E2E réel (`dev_e2e.sh real`)

```
status:     ok
latency:    total=8967ms parse=8764ms orchestrate=42ms execute=161ms
Tasks: create_dir → /tmp/test, HAL: granted (risk=0.00)
source=vllm (intent.log, cache_hit=false)
```

IntentSchema vLLM : `goal=create_dir`, `source=vllm`, `candidate_actions=[create_dir→/tmp/test]`.

### Réseau live

`status=unsupported`, `message=goal reconnu mais non supporté en v1: network_download` — pas d'ActionGraph, pas d'exécution.

### Non-régression

- `cargo test --workspace` : **232** tests OK
- `pytest agents/` : **33** passed
- `dev_e2e.sh mock` : OK

### Verdict

**OUI** — intent manager opérationnel en prod via vLLM bout en bout. Rapport : `docs/PROD_VLLM_VERDICT.md`.

---
