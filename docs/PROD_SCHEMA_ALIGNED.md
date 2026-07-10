# Production schema aligned — remesure (prompt inchangé)

**Mesuré :** 2026-07-10T16:28:16Z (UTC)

## Contexte

L'overfit check a montré : pas d'overfit, mais le schéma production **étroit** bridait le modèle.
Ce tour aligne `intent-llm-output.schema.json` v2 + `intent.gbnf` sur `docs/GOAL_TAXONOMY.md`.
**Prompt système inchangé** (`intent-engine/src/prompt.rs`).

## Artefacts

- Schéma prod : `intent-llm-output.schema.json` **v2.0.0**
- GBNF : `intent-engine/grammar/intent.gbnf` (goal/action/domain/target alignés)
- Taxonomie : `docs/GOAL_TAXONOMY.md`
- Modèle : `Qwen/Qwen2.5-7B-Instruct-AWQ` (vLLM 0.24.0)

## Scores — schéma production ALIGNÉ

| Jeu | goal | disambiguation | candidate_actions |
|-----|------|----------------|-----------------|
| Golden | **15/15** | 15/15 | 15/15 |
| Validation | **18/20** | 18/20 | 18/20 |

## Comparaison vs baseline prod étroit (overfit check)

| Jeu | goal (étroit → aligné) | Δ goal | disambig (étroit → aligné) |
|-----|------------------------|--------|----------------------------|
| Golden | 8/15 → **15/15** | **+7** | 15/15 → 15/15 |
| Validation | 14/20 → **18/20** | **+4** | 18/20 → 18/20 |

> Référence baseline étroit : golden **8/15**, validation **14/20** (`docs/OVERFIT_CHECK.md`).
> Référence éval élargi (hors scope ce tour) : golden **14/15**, validation **17/20**.

## Gap disambiguation résiduel (goal OK, disambig KO)

Échecs **purs** disambiguation après alignement schéma — gap sémantique résiduel, **non corrigé** ce tour.

- Golden : **0/15** cas
- Validation : **2/20** cas

### Validation — disambig pur

- `v05_ambiguous_project_en.json` : goal `open_workspace` OK, attendu disambig=True, produit=False
- `v06_ambiguous_projet_fr.json` : goal `open_workspace` OK, attendu disambig=True, produit=False

### Matrice goal — golden (prod aligné)

| expected \ produced | await_input | code_task | delete_path | network_download | network_send | open_file | open_workspace | out_of_scope | package_and_convert | search_files |
|---|---|---|---|---|---|---|---|---|---|---|
| `await_input` | 2 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| `code_task` | 0 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| `delete_path` | 0 | 0 | 2 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| `network_download` | 0 | 0 | 0 | 1 | 0 | 0 | 0 | 0 | 0 | 0 |
| `network_send` | 0 | 0 | 0 | 0 | 1 | 0 | 0 | 0 | 0 | 0 |
| `open_file` | 0 | 0 | 0 | 0 | 0 | 2 | 0 | 0 | 0 | 0 |
| `open_workspace` | 0 | 0 | 0 | 0 | 0 | 0 | 2 | 0 | 0 | 0 |
| `out_of_scope` | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 2 | 0 | 0 |
| `package_and_convert` | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 1 | 0 |
| `search_files` | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 1 |

### Matrice goal — validation (prod aligné)

| expected \ produced | await_input | create_dir | delete_path | install_package | network_download | network_send | open_file | open_workspace | out_of_scope |
|---|---|---|---|---|---|---|---|---|---|
| `await_input` | 2 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| `code_task` | 0 | 0 | 0 | 0 | 0 | 1 | 0 | 0 | 0 |
| `create_dir` | 0 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| `delete_path` | 0 | 0 | 3 | 0 | 0 | 0 | 0 | 0 | 0 |
| `install_package` | 0 | 0 | 0 | 2 | 0 | 0 | 0 | 0 | 0 |
| `network_download` | 0 | 0 | 0 | 0 | 1 | 0 | 0 | 0 | 0 |
| `network_send` | 0 | 0 | 0 | 0 | 0 | 1 | 0 | 0 | 0 |
| `open_file` | 0 | 0 | 0 | 0 | 0 | 0 | 3 | 0 | 0 |
| `open_workspace` | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 3 | 0 |
| `out_of_scope` | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 2 |
| `search_files` | 0 | 0 | 0 | 0 | 0 | 0 | 1 | 0 | 0 |

### Golden — prod aligné

| Cas | Input | Exp goal | Prod goal | Exp disambig | Prod disambig | Résultat |
|-----|-------|----------|-----------|--------------|---------------|----------|
| `01_simple_file_open_en.json` | open my budget spreadsheet | `open_file` | `open_file` | False | False | ✅ |
| `02_simple_file_open_fr.json` | ouvre mon tableur de budget | `open_file` | `open_file` | False | False | ✅ |
| `03_simple_file_find_en.json` | find my meeting notes from yesterday | `search_files` | `search_files` | False | False | ✅ |
| `04_multistep_refactor_en.json` | refactor the auth module and run the tests | `code_task` | `code_task` | False | False | ✅ |
| `05_multistep_install_convert_fr.json` | installe ffmpeg puis convertis ma vidéo en mp4 | `package_and_convert` | `package_and_convert` | False | False | ✅ |
| `06_ambiguous_reference_en.json` | open the robotics project | `open_workspace` | `open_workspace` | True | True | ✅ |
| `07_ambiguous_reference_fr.json` | ouvre le projet robotique | `open_workspace` | `open_workspace` | True | True | ✅ |
| `08_out_of_scope_en.json` | what's the weather in Tokyo tomorrow | `out_of_scope` | `out_of_scope` | False | False | ✅ |
| `09_out_of_scope_fr.json` | raconte-moi une blague | `out_of_scope` | `out_of_scope` | False | False | ✅ |
| `10_network_download_en.json` | download the latest release from github | `network_download` | `network_download` | False | False | ✅ |
| `11_network_email_fr.json` | envoie ce fichier par email à Marie | `network_send` | `network_send` | False | False | ✅ |
| `12_dangerous_delete_en.json` | delete everything in /etc | `delete_path` | `delete_path` | False | False | ✅ |
| `13_dangerous_delete_fr.json` | supprime le dossier système /boot | `delete_path` | `delete_path` | False | False | ✅ |
| `14_empty_input.json` |  | `await_input` | `await_input` | False | False | ✅ |
| `15_noise_input.json` | asdf jkl ;;; qwerty | `await_input` | `await_input` | False | False | ✅ |

### Validation — prod aligné

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
| `v14_network_download_en.json` | fetch the telemetry archive from the internal mirror | `network_download` | `network_download` | False | False | ❌ |
| `v15_network_send_fr.json` | transmets le compte-rendu PDF à l'équipe comptabilité | `network_send` | `network_send` | False | False | ✅ |
| `v16_noise_input.json` | zzz ### ??? hmm | `await_input` | `await_input` | False | False | ✅ |
| `v17_empty_input.json` |  | `await_input` | `await_input` | False | False | ✅ |
| `v18_multistep_code_en.json` | sync the staging branch then execute the integration suite | `code_task` | `network_send` | False | False | ❌ |
| `v19_open_hosts_fr.json` | affiche le fichier hosts du système | `open_file` | `open_file` | False | False | ✅ |
| `v20_dangerous_purge_en.json` | purge recursively everything under ~/Downloads/archive | `delete_path` | `delete_path` | False | False | ✅ |

## JSON brut — validation (20, sortie chaude)

### Validation

#### `v01_open_report_en.json` — input: `open the quarterly report spreadsheet`

```json
{"goal":"open_file","domain":"finance","confidence":0.85,"ambiguity_score":0.1,"risk_estimate":0.05,"required_context":[],"candidate_actions":[{"action":"open_file","target":"Q1-report.xlsx","confidence":0.9,"recency_score":0.8}],"disambiguation_required":false,"disambiguation_question":null,"hal_pre_score":0.0,"escalate_to_cloud":false}
```

#### `v02_ouvre_notes_fr.json` — input: `ouvre mes notes de réunion`

```json
{"goal":"open_file","domain":"work","confidence":0.9,"ambiguity_score":0.1,"risk_estimate":0.0,"required_context":[],"candidate_actions":[{"action":"open_file","target":"reunion-lundi.md","confidence":0.9,"recency_score":0.8}],"disambiguation_required":false,"disambiguation_question":null,"hal_pre_score":0.0,"escalate_to_cloud":false}
```

#### `v03_create_dir_en.json` — input: `create a folder called experiments under /tmp`

```json
{"goal":"create_dir","domain":null,"confidence":0.95,"ambiguity_score":0.0,"risk_estimate":0.1,"required_context":[],"candidate_actions":[{"action":"create_dir","target":"/tmp/experiments","confidence":0.95,"recency_score":0.0}],"disambiguation_required":false,"disambiguation_question":null,"hal_pre_score":0.0,"escalate_to_cloud":false}
```

#### `v04_multistep_install_fr.json` — input: `installe docker puis lance le conteneur postgres`

```json
{"goal":"install_package","domain":null,"confidence":0.85,"ambiguity_score":0.0,"risk_estimate":0.1,"required_context":[],"candidate_actions":[{"action":"install_package","target":"docker","confidence":0.9,"recency_score":0.0},{"action":"run_tests","target":":/docker/postgres","confidence":0.8,"recency_score":0.0}],"disambiguation_required":false,"disambiguation_question":null,"hal_pre_score":0.0,"escalate_to_cloud":false}
```

#### `v05_ambiguous_project_en.json` — input: `open the analytics project`

```json
{"goal":"open_workspace","domain":"finance","confidence":0.85,"ambiguity_score":0.1,"risk_estimate":0.05,"required_context":[],"candidate_actions":[{"action":"open_file","target":"analytics.py","confidence":0.9,"recency_score":0.05},{"action":"open_file","target":"report.csv","confidence":0.8,"recency_score":0.1}],"disambiguation_required":false,"disambiguation_question":null,"hal_pre_score":0.0,"escalate_to_cloud":false}
```

#### `v06_ambiguous_projet_fr.json` — input: `ouvre le projet vidéo`

```json
{"goal":"open_workspace","domain":"media","confidence":0.85,"ambiguity_score":0.1,"risk_estimate":0.05,"required_context":[],"candidate_actions":[{"action":"open_file","target":"montage.prproj","confidence":0.9,"recency_score":0.8},{"action":"open_file","target":"export.mp4","confidence":0.7,"recency_score":0.6}],"disambiguation_required":false,"disambiguation_question":null,"hal_pre_score":0.0,"escalate_to_cloud":false}
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
{"goal":"install_package","domain":null,"confidence":0.9,"ambiguity_score":0.0,"risk_estimate":0.1,"required_context":[],"candidate_actions":[{"action":"install_package","target":"nginx","confidence":0.9,"recency_score":0.0}],"disambiguation_required":false,"disambiguation_question":null,"hal_pre_score":0.0,"escalate_to_cloud":false}
```

#### `v12_open_workspace_clear_en.json` — input: `open the cognos workspace repo`

```json
{"goal":"open_workspace","domain":"coding","confidence":0.85,"ambiguity_score":0.1,"risk_estimate":0.05,"required_context":[],"candidate_actions":[{"action":"open_file","target":"cognos/workspace/repo","confidence":0.9,"recency_score":0.5}],"disambiguation_required":false,"disambiguation_question":null,"hal_pre_score":0.0,"escalate_to_cloud":false}
```

#### `v13_search_files_en.json` — input: `find the backup configuration from last Tuesday`

```json
{"goal":"open_file","domain":null,"confidence":0.85,"ambiguity_score":0.1,"risk_estimate":0.05,"required_context":[],"candidate_actions":[{"action":"open_file","target":"backup.conf","confidence":0.9,"recency_score":0.8}],"disambiguation_required":false,"disambiguation_question":null,"hal_pre_score":0.0,"escalate_to_cloud":false}
```

#### `v14_network_download_en.json` — input: `fetch the telemetry archive from the internal mirror`

```json
{"goal":"network_download","domain":null,"confidence":0.9,"ambiguity_score":0.0,"risk_estimate":0.1,"required_context":[],"candidate_actions":[],"disambiguation_required":false,"disambiguation_question":null,"hal_pre_score":0.0,"escalate_to_cloud":false}
```

#### `v15_network_send_fr.json` — input: `transmets le compte-rendu PDF à l'équipe comptabilité`

```json
{"goal":"network_send","domain":null,"confidence":0.95,"ambiguity_score":0.0,"risk_estimate":0.1,"required_context":[],"candidate_actions":[{"action":"send_email","target":"comptabilite","confidence":0.95,"recency_score":0.0}],"disambiguation_required":false,"disambiguation_question":null,"hal_pre_score":0.0,"escalate_to_cloud":false}
```

#### `v16_noise_input.json` — input: `zzz ### ??? hmm`

```json
{"goal":"await_input","domain":null,"confidence":0.0,"ambiguity_score":0.0,"risk_estimate":0.0,"required_context":[],"candidate_actions":[],"disambiguation_required":false,"disambiguation_question":null,"hal_pre_score":0.0,"escalate_to_cloud":false}
```

#### `v17_empty_input.json` — input: ``

```json
{"goal": "await_input", "domain": null, "confidence": 0.1, "ambiguity_score": 0.5, "risk_estimate": 0.0, "required_context": ["user_clarification"], "candidate_actions": [], "disambiguation_required": false, "disambiguation_question": null, "hal_pre_score": 0.0, "escalate_to_cloud": true}
```

#### `v18_multistep_code_en.json` — input: `sync the staging branch then execute the integration suite`

```json
{"goal":"network_send","domain":null,"confidence":0.85,"ambiguity_score":0.0,"risk_estimate":0.0,"required_context":[],"candidate_actions":[],"disambiguation_required":false,"disambiguation_question":null,"hal_pre_score":0.0,"escalate_to_cloud":false}
```

#### `v19_open_hosts_fr.json` — input: `affiche le fichier hosts du système`

```json
{"goal":"open_file","domain":null,"confidence":0.95,"ambiguity_score":0.0,"risk_estimate":0.1,"required_context":[],"candidate_actions":[{"action":"open_file","target":"/etc/hosts","confidence":0.95,"recency_score":0.8}],"disambiguation_required":false,"disambiguation_question":null,"hal_pre_score":0.0,"escalate_to_cloud":false}
```

#### `v20_dangerous_purge_en.json` — input: `purge recursively everything under ~/Downloads/archive`

```json
{"goal":"delete_path","domain":null,"confidence":0.95,"ambiguity_score":0.0,"risk_estimate":0.95,"required_context":[],"candidate_actions":[{"action":"delete_path","target":"~/Downloads/archive","confidence":0.95,"recency_score":0.0}],"disambiguation_required":false,"disambiguation_question":null,"hal_pre_score":0.0,"escalate_to_cloud":false}
```

## Goals sans route HAL explicite

Voir `docs/GOAL_TAXONOMY.md` — `network_download`, `network_send` : parser OK, route HAL à définir (décision humaine).
