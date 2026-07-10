# Overfit Check — prompt_v2 (inchangé)

**Mesuré :** 2026-07-10T16:02:30Z (UTC)

## Méthodologie

- Modèle : `Qwen/Qwen2.5-7B-Instruct-AWQ` (vLLM 0.24.0)
- Prompt : `intent-engine/src/prompt.rs` — **non modifié** ce tour
- Golden : 15 fixtures (`intent-engine/tests/golden/`)
- Validation : 20 fixtures **neuves** (`intent-engine/tests/validation/`)
- Validation distincte des exemples embarqués dans le prompt (pas de récitation des few-shots)

## Scores — schéma PRODUCTION (`intent-llm-output.schema.json`)

| Jeu | goal | disambiguation | candidate_actions |
|-----|------|----------------|-----------------|
| Golden | **8/15** | 15/15 | 12/15 |
| Validation | **14/20** | 18/20 | 17/20 |

## Scores — schéma ÉVAL ÉLARGI (`intent-golden-eval.schema.json`)

| Jeu | goal | disambiguation | candidate_actions |
|-----|------|----------------|-----------------|
| Golden | 14/15 | 15/15 | 15/15 |
| Validation | 17/20 | 18/20 | 18/20 |

## Écart prod vs éval élargi (goal)

- Golden : prod 8/15 vs éval 14/15 (Δ goal = 6)
- Validation : prod 14/20 vs éval 17/20 (Δ goal = 3)

> Le score **production** est celui qui compte pour l'intégration (GBNF / XGrammar étroit).

## Verdict généralisation (schéma production)

**PAS d'overfit mesuré (prod)** — validation **14/20** goal ≥ golden prod **8/15** (écart **-16.7** pts : la validation généralise *mieux* que les golden en prod). Le **14/15** annoncé en session autonome ne tient qu'au schéma **éval élargi** (14/15), pas à la prod (8/15). **Décision humaine** au retour : le combo prompt_v2 n'est pas « figé » par ce tour.

- Golden goal prod : 53.3% (8/15)
- Validation goal prod : **70.0%** (14/20)
- Golden goal éval élargi : 93.3% (14/15)
- Validation goal éval élargi : 85.0% (17/20)
- Golden disambig prod : 15/15
- Validation disambig prod : 18/20

### Matrice goal — validation (prod)

| expected \ produced | create_dir | delete_path | install_package | open_file | open_workspace | out_of_scope |
|---|---|---|---|---|---|---|
| `await_input` | 0 | 0 | 0 | 0 | 0 | 2 |
| `code_task` | 0 | 0 | 0 | 0 | 1 | 0 |
| `create_dir` | 1 | 0 | 0 | 0 | 0 | 0 |
| `delete_path` | 0 | 3 | 0 | 0 | 0 | 0 |
| `install_package` | 0 | 0 | 2 | 0 | 0 | 0 |
| `network_download` | 0 | 0 | 0 | 0 | 0 | 1 |
| `network_send` | 1 | 0 | 0 | 0 | 0 | 0 |
| `open_file` | 0 | 0 | 0 | 3 | 0 | 0 |
| `open_workspace` | 0 | 0 | 0 | 0 | 3 | 0 |
| `out_of_scope` | 0 | 0 | 0 | 0 | 0 | 2 |
| `search_files` | 0 | 0 | 0 | 1 | 0 | 0 |

### Validation — prod schéma

| Cas | Input | Exp goal | Prod goal | Exp disambig | Prod disambig | Résultat |
|-----|-------|----------|-----------|--------------|---------------|----------|
| `v01_open_report_en.json` | open the quarterly report spreadsheet | `open_file` | `open_file` | False | False | ✅ |
| `v02_ouvre_notes_fr.json` | ouvre mes notes de réunion | `open_file` | `open_file` | False | False | ✅ |
| `v03_create_dir_en.json` | create a folder called experiments under /tmp | `create_dir` | `create_dir` | False | False | ✅ |
| `v04_multistep_install_fr.json` | installe docker puis lance le conteneur postgres | `install_package` | `install_package` | False | False | ✅ |
| `v05_ambiguous_project_en.json` | open the analytics project | `open_workspace` | `open_workspace` | True | False | ❌ |
| `v06_ambiguous_projet_fr.json` | ouvre le projet vidéo | `open_workspace` | `open_workspace` | True | False | ❌ |
| `v07_dangerous_delete_en.json` | delete everything in /var/log/old | `delete_path` | `delete_path` | False | False | ✅ |
| `v08_dangerous_delete_fr.json` | supprime le dossier /etc/ssl | `delete_path` | `delete_path` | False | False | ✅ |
| `v09_out_of_scope_en.json` | what is the weather in Paris tomorrow | `out_of_scope` | `out_of_scope` | False | False | ✅ |
| `v10_out_of_scope_fr.json` | explique-moi la relativité générale en deux phrases | `out_of_scope` | `out_of_scope` | False | False | ✅ |
| `v11_install_package_en.json` | install nginx on this machine | `install_package` | `install_package` | False | False | ✅ |
| `v12_open_workspace_clear_en.json` | open the cognos workspace repo | `open_workspace` | `open_workspace` | False | False | ✅ |
| `v13_search_files_en.json` | find the backup configuration from last Tuesday | `search_files` | `open_file` | False | False | ❌ |
| `v14_network_download_en.json` | fetch the telemetry archive from the internal mirror | `network_download` | `out_of_scope` | False | False | ❌ |
| `v15_network_send_fr.json` | transmets le compte-rendu PDF à l'équipe comptabilité | `network_send` | `create_dir` | False | False | ❌ |
| `v16_noise_input.json` | zzz ### ??? hmm | `await_input` | `out_of_scope` | False | False | ❌ |
| `v17_empty_input.json` |  | `await_input` | `out_of_scope` | False | False | ❌ |
| `v18_multistep_code_en.json` | sync the staging branch then execute the integration suite | `code_task` | `open_workspace` | False | False | ❌ |
| `v19_open_hosts_fr.json` | affiche le fichier hosts du système | `open_file` | `open_file` | False | False | ✅ |
| `v20_dangerous_purge_en.json` | purge recursively everything under ~/Downloads/archive | `delete_path` | `delete_path` | False | False | ✅ |

## JSON brut — validation (20, sortie chaude, schéma production)

### Validation

#### `v01_open_report_en.json` — input: `open the quarterly report spreadsheet`

```json
{"goal":"open_file","domain":"finance","confidence":0.85,"ambiguity_score":0.1,"risk_estimate":0.05,"required_context":[],"candidate_actions":[{"action":"open_files","target":"/Q1-report.xlsx","confidence":0.9,"recency_score":0.8}],"disambiguation_required":false,"disambiguation_question":null,"hal_pre_score":0.05,"escalate_to_cloud":false}
```

#### `v02_ouvre_notes_fr.json` — input: `ouvre mes notes de réunion`

```json
{"goal":"open_file","domain":"finance","confidence":0.85,"ambiguity_score":0.1,"risk_estimate":0.05,"required_context":[],"candidate_actions":[{"action":"open_files","target":"/home/user/recent/reunion-lundi.md","confidence":0.9,"recency_score":0.8}],"disambiguation_required":false,"disambiguation_question":null,"hal_pre_score":0.0,"escalate_to_cloud":false}
```

#### `v03_create_dir_en.json` — input: `create a folder called experiments under /tmp`

```json
{"goal":"create_dir","domain":null,"confidence":0.95,"ambiguity_score":0.0,"risk_estimate":0.1,"required_context":[],"candidate_actions":[{"action":"create_dir","target":"/tmp/experiments","confidence":0.95,"recency_score":0.0}],"disambiguation_required":false,"disambiguation_question":null,"hal_pre_score":0.0,"escalate_to_cloud":false}
```

#### `v04_multistep_install_fr.json` — input: `installe docker puis lance le conteneur postgres`

```json
{"goal":"install_package","domain":null,"confidence":0.85,"ambiguity_score":0.0,"risk_estimate":0.1,"required_context":[],"candidate_actions":[{"action":"install_package","target":"/docker","confidence":0.9,"recency_score":0.0},{"action":"open_files","target":"/var/lib/docker/containers/postgres","confidence":0.8,"recency_score":0.0}],"disambiguation_required":false,"disambiguation_question":null,"hal_pre_score":0.0,"escalate_to_cloud":false}
```

#### `v05_ambiguous_project_en.json` — input: `open the analytics project`

```json
{"goal":"open_workspace","domain":"finance","confidence":0.85,"ambiguity_score":0.1,"risk_estimate":0.05,"required_context":[],"candidate_actions":[{"action":"open_files","target":"/analytics_project.py","confidence":0.9,"recency_score":0.8},{"action":"open_files","target":"/analytics_project.csv","confidence":0.8,"recency_score":0.7}],"disambiguation_required":false,"disambiguation_question":null,"hal_pre_score":0.0,"escalate_to_cloud":false}
```

#### `v06_ambiguous_projet_fr.json` — input: `ouvre le projet vidéo`

```json
{"goal":"open_workspace","domain":"finance","confidence":0.85,"ambiguity_score":0.1,"risk_estimate":0.05,"required_context":[],"candidate_actions":[],"disambiguation_required":false,"disambiguation_question":null,"hal_pre_score":0.0,"escalate_to_cloud":false}
```

#### `v07_dangerous_delete_en.json` — input: `delete everything in /var/log/old`

```json
{"goal":"delete_path","domain":null,"confidence":0.95,"ambiguity_score":0.0,"risk_estimate":0.95,"required_context":[],"candidate_actions":[{"action":"delete_path","target":"/var/log/old","confidence":0.95,"recency_score":0.0}],"disambiguation_required":false,"disambiguation_question":null,"hal_pre_score":0.0,"escalate_to_cloud":false}
```

#### `v08_dangerous_delete_fr.json` — input: `supprime le dossier /etc/ssl`

```json
{"goal":"delete_path","domain":null,"confidence":0.95,"ambiguity_score":0.0,"risk_estimate":0.95,"required_context":[],"candidate_actions":[{"action":"delete_path","target":"/etc/ssl","confidence":0.95,"recency_score":0.0}],"disambiguation_required":false,"disambiguation_question":null,"hal_pre_score":0.0,"escalate_to_cloud":false}
```

#### `v09_out_of_scope_en.json` — input: `what is the weather in Paris tomorrow`

```json
{"goal":"out_of_scope","domain":null,"confidence":0.9,"ambiguity_score":0.0,"risk_estimate":0.0,"required_context":[],"candidate_actions":[],"disambiguation_required":false,"disambiguation_question":null,"hal_pre_score":0.0,"escalate_to_cloud":false}
```

#### `v10_out_of_scope_fr.json` — input: `explique-moi la relativité générale en deux phrases`

```json
{"goal":"out_of_scope","domain":null,"confidence":0.95,"ambiguity_score":0.0,"risk_estimate":0.0,"required_context":[],"candidate_actions":[],"disambiguation_required":false,"disambiguation_question":null,"hal_pre_score":0.0,"escalate_to_cloud":false}
```

#### `v11_install_package_en.json` — input: `install nginx on this machine`

```json
{"goal":"install_package","domain":null,"confidence":0.9,"ambiguity_score":0.0,"risk_estimate":0.1,"required_context":[],"candidate_actions":[{"action":"install_package","target":"/nginx","confidence":0.9,"recency_score":0.0}],"disambiguation_required":false,"disambiguation_question":null,"hal_pre_score":0.0,"escalate_to_cloud":false}
```

#### `v12_open_workspace_clear_en.json` — input: `open the cognos workspace repo`

```json
{"goal":"open_workspace","domain":"finance","confidence":0.85,"ambiguity_score":0.1,"risk_estimate":0.05,"required_context":[],"candidate_actions":[{"action":"open_files","target":"/path/to/cognos/workspace/repo","confidence":0.9,"recency_score":0.5}],"disambiguation_required":false,"disambiguation_question":null,"hal_pre_score":0.0,"escalate_to_cloud":false}
```

#### `v13_search_files_en.json` — input: `find the backup configuration from last Tuesday`

```json
{"goal":"open_file","domain":null,"confidence":0.85,"ambiguity_score":0.1,"risk_estimate":0.05,"required_context":[],"candidate_actions":[{"action":"open_files","target":"/path/to/backup.conf","confidence":0.9,"recency_score":0.8}],"disambiguation_required":false,"disambiguation_question":null,"hal_pre_score":0.0,"escalate_to_cloud":false}
```

#### `v14_network_download_en.json` — input: `fetch the telemetry archive from the internal mirror`

```json
{"goal":"out_of_scope","domain":null,"confidence":0.8,"ambiguity_score":0.0,"risk_estimate":0.0,"required_context":[],"candidate_actions":[],"disambiguation_required":false,"disambiguation_question":null,"hal_pre_score":0.0,"escalate_to_cloud":false}
```

#### `v15_network_send_fr.json` — input: `transmets le compte-rendu PDF à l'équipe comptabilité`

```json
{"goal":"create_dir","domain":null,"confidence":0.9,"ambiguity_score":0.0,"risk_estimate":0.0,"required_context":[],"candidate_actions":[],"disambiguation_required":false,"disambiguation_question":null,"hal_pre_score":0.0,"escalate_to_cloud":false}
```

#### `v16_noise_input.json` — input: `zzz ### ??? hmm`

```json
{"goal":"out_of_scope","domain":null,"confidence":0.0,"ambiguity_score":0.0,"risk_estimate":0.0,"required_context":[],"candidate_actions":[],"disambiguation_required":false,"disambiguation_question":null,"hal_pre_score":0.0,"escalate_to_cloud":false}
```

#### `v17_empty_input.json` — input: ``

```json
{"goal":"out_of_scope","domain":null,"confidence":0.9,"ambiguity_score":0.0,"risk_estimate":0.0,"required_context":[],"candidate_actions":[],"disambiguation_required":false,"disambiguation_question":null,"hal_pre_score":0.0,"escalate_to_cloud":false}
```

#### `v18_multistep_code_en.json` — input: `sync the staging branch then execute the integration suite`

```json
{"goal":"open_workspace","domain":null,"confidence":0.85,"ambiguity_score":0.1,"risk_estimate":0.1,"required_context":[],"candidate_actions":[{"action":"open_files","target":"/path/to/staging","confidence":0.9,"recency_score":0.8},{"action":"open_files","target":"/path/to/integration.rs","confidence":0.8,"recency_score":0.7},{"action":"open_files","target":"/path/to/Cargo.toml","confidence":0.8,"recency_score":0.7},{"action":"open_files","target":"/path/to/integration_suite","confidence":0.8,"recency_score":0.7}],"disambiguation_required":false,"disambiguation_question":null,"hal_pre_score":0.1,"escalate_to_cloud":false}
```

#### `v19_open_hosts_fr.json` — input: `affiche le fichier hosts du système`

```json
{"goal":"open_file","domain":null,"confidence":0.95,"ambiguity_score":0.0,"risk_estimate":0.1,"required_context":[],"candidate_actions":[{"action":"open_files","target":"/etc/hosts","confidence":0.95,"recency_score":0.8}],"disambiguation_required":false,"disambiguation_question":null,"hal_pre_score":0.0,"escalate_to_cloud":false}
```

#### `v20_dangerous_purge_en.json` — input: `purge recursively everything under ~/Downloads/archive`

```json
{"goal":"delete_path","domain":null,"confidence":0.95,"ambiguity_score":0.0,"risk_estimate":0.95,"required_context":[],"candidate_actions":[{"action":"delete_path","target":"/home/user/Downloads/archive","confidence":0.95,"recency_score":0.0}],"disambiguation_required":false,"disambiguation_question":null,"hal_pre_score":0.0,"escalate_to_cloud":false}
```

## Court-circuit `empty_input` (Rust)

Implémenté dans `intent-engine/src/parser.rs` : entrée vide/whitespace après normalisation
→ `await_input` déterministe, **sans appel LLM** (test `empty_input_short_circuits_without_backend`).
