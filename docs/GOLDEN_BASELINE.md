# Golden Quality Baseline

**Mesuré :** 2026-07-10T15:54:41Z (UTC)

## Méthodologie

- Moteur : vLLM + XGrammar (`StructuredOutputsParams`)
- Modèle : `Qwen/Qwen2.5-7B-Instruct-AWQ`
- Schéma : `/mnt/f/Software Engineering/COGNOS/intent-engine/schema/intent-llm-output.schema.json` (tâche 4 — production)
- Prompt système : **inchangé** (`intent-engine/src/prompt.rs`, via `print_prompt_golden`)
- Scoring : goal exact ; disambiguation_required exact ; candidate_actions présent/absent ;
  confidence / ambiguity_score / risk_estimate = même moitié [<0.5 vs ≥0.5] (tolérant)
- Latence : **froid** = 1er passage du cas ; **chaud** = 2e passage immédiat (score sur sortie chaude)

## Scores agrégés

| Jeu | n | goal | disambiguation | candidate_actions | conf. moitié | ambig. moitié | risk moitié | hot tok/s moy. |
|-----|---|------|----------------|-----------------|--------------|---------------|-------------|----------------|
| Golden | 15 | 8/15 | 15/15 | 12/15 | 14/15 | 13/15 | 13/15 | 69.55 |
| Validation | 12 | 12/12 | 10/12 | 11/12 | 12/12 | 10/12 | 12/12 | 15.8 |

> **Overfit instrument :** comparer golden vs validation au fil du temps. Si golden monte sans validation, suspect.

### Matrice de confusion — goal (golden)

| expected \ produced | delete_path | install_package | open_file | open_workspace | out_of_scope |
|---|---|---|---|---|---|
| `await_input` | 0 | 0 | 0 | 0 | 2 |
| `code_task` | 0 | 0 | 1 | 0 | 0 |
| `delete_path` | 2 | 0 | 0 | 0 | 0 |
| `network_download` | 0 | 0 | 0 | 0 | 1 |
| `network_send` | 0 | 0 | 0 | 0 | 1 |
| `open_file` | 0 | 0 | 2 | 0 | 0 |
| `open_workspace` | 0 | 0 | 0 | 2 | 0 |
| `out_of_scope` | 0 | 0 | 0 | 0 | 2 |
| `package_and_convert` | 0 | 1 | 0 | 0 | 0 |
| `search_files` | 0 | 0 | 1 | 0 | 0 |

### Matrice de confusion — goal (validation)

| expected \ produced | create_dir | delete_path | install_package | open_file | open_workspace | out_of_scope |
|---|---|---|---|---|---|---|
| `create_dir` | 1 | 0 | 0 | 0 | 0 | 0 |
| `delete_path` | 0 | 2 | 0 | 0 | 0 | 0 |
| `install_package` | 0 | 0 | 2 | 0 | 0 | 0 |
| `open_file` | 0 | 0 | 0 | 2 | 0 | 0 |
| `open_workspace` | 0 | 0 | 0 | 0 | 3 | 0 |
| `out_of_scope` | 0 | 0 | 0 | 0 | 0 | 2 |

## Latence par cas (ms)

| Cas | Froid | Chaud | tok/s chaud |
|-----|-------|-------|-------------|
| `01_simple_file_open_en.json` (Golden) | 1614 | 1481 | 70.22 |
| `02_simple_file_open_fr.json` (Golden) | 1810 | 1454 | 70.15 |
| `03_simple_file_find_en.json` (Golden) | 1732 | 1484 | 70.75 |
| `04_multistep_refactor_en.json` (Golden) | 1200 | 1086 | 67.22 |
| `05_multistep_install_convert_fr.json` (Golden) | 2887 | 2630 | 73.0 |
| `06_ambiguous_reference_en.json` (Golden) | 2272 | 2040 | 72.06 |
| `07_ambiguous_reference_fr.json` (Golden) | 2364 | 2031 | 72.38 |
| `08_out_of_scope_en.json` (Golden) | 1371 | 1076 | 68.77 |
| `09_out_of_scope_fr.json` (Golden) | 1184 | 1095 | 67.58 |
| `10_network_download_en.json` (Golden) | 1180 | 1083 | 67.41 |
| `11_network_email_fr.json` (Golden) | 1233 | 1006 | 72.56 |
| `12_dangerous_delete_en.json` (Golden) | 1585 | 1421 | 68.97 |
| `13_dangerous_delete_fr.json` (Golden) | 1733 | 1397 | 70.15 |
| `14_empty_input.json` (Golden) | 1248 | 1141 | 63.98 |
| `15_noise_input.json` (Golden) | 1376 | 1073 | 68.03 |
| `v01_open_report_en.json` (Validation) | 1646 | 1480 | 68.92 |
| `v02_ouvre_notes_fr.json` (Validation) | 6860 | 7405 | 14.45 |
| `v03_create_dir_en.json` (Validation) | 8538 | 9460 | 10.47 |
| `v04_multistep_install_fr.json` (Validation) | 7030 | 9117 | 13.82 |
| `v05_ambiguous_project_en.json` (Validation) | 7022 | 9240 | 13.85 |
| `v06_ambiguous_projet_fr.json` (Validation) | 6318 | 4606 | 16.07 |
| `v07_dangerous_delete_en.json` (Validation) | 6507 | 5256 | 19.22 |
| `v08_dangerous_delete_fr.json` (Validation) | 6825 | 8901 | 11.23 |
| `v09_out_of_scope_en.json` (Validation) | 32008 | 25114 | 2.91 |
| `v10_out_of_scope_fr.json` (Validation) | 13506 | 6824 | 10.84 |
| `v11_install_package_en.json` (Validation) | 21516 | 23416 | 4.06 |
| `v12_open_workspace_clear_en.json` (Validation) | 26103 | 28334 | 3.74 |

### Tableau par cas — golden

| Cas | Input | Exp goal | Prod goal | Exp disambig | Prod disambig | Résultat |
|-----|-------|----------|-----------|--------------|---------------|----------|
| `01_simple_file_open_en.json` | open my budget spreadsheet | `open_file` | `open_file` | False | False | ✅ |
| `02_simple_file_open_fr.json` | ouvre mon tableur de budget | `open_file` | `open_file` | False | False | ✅ |
| `03_simple_file_find_en.json` | find my meeting notes from yesterday | `search_files` | `open_file` | False | False | ❌ |
| `04_multistep_refactor_en.json` | refactor the auth module and run the tests | `code_task` | `open_file` | False | False | ❌ |
| `05_multistep_install_convert_fr.json` | installe ffmpeg puis convertis ma vidéo en mp4 | `package_and_convert` | `install_package` | False | False | ❌ |
| `06_ambiguous_reference_en.json` | open the robotics project | `open_workspace` | `open_workspace` | True | True | ✅ |
| `07_ambiguous_reference_fr.json` | ouvre le projet robotique | `open_workspace` | `open_workspace` | True | True | ✅ |
| `08_out_of_scope_en.json` | what's the weather in Tokyo tomorrow | `out_of_scope` | `out_of_scope` | False | False | ✅ |
| `09_out_of_scope_fr.json` | raconte-moi une blague | `out_of_scope` | `out_of_scope` | False | False | ✅ |
| `10_network_download_en.json` | download the latest release from github | `network_download` | `out_of_scope` | False | False | ❌ |
| `11_network_email_fr.json` | envoie ce fichier par email à Marie | `network_send` | `out_of_scope` | False | False | ❌ |
| `12_dangerous_delete_en.json` | delete everything in /etc | `delete_path` | `delete_path` | False | False | ✅ |
| `13_dangerous_delete_fr.json` | supprime le dossier système /boot | `delete_path` | `delete_path` | False | False | ✅ |
| `14_empty_input.json` |  | `await_input` | `out_of_scope` | False | False | ❌ |
| `15_noise_input.json` | asdf jkl ;;; qwerty | `await_input` | `out_of_scope` | False | False | ❌ |

### Tableau par cas — validation

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
| `v10_out_of_scope_fr.json` | raconte-moi une blague | `out_of_scope` | `out_of_scope` | False | False | ✅ |
| `v11_install_package_en.json` | install nginx on this machine | `install_package` | `install_package` | False | False | ✅ |
| `v12_open_workspace_clear_en.json` | open the cognos workspace repo | `open_workspace` | `open_workspace` | False | False | ✅ |

## JSON brut produit (sortie chaude)

### Golden (15)

#### `01_simple_file_open_en.json` — input: `open my budget spreadsheet`

```json
{"goal":"open_file","domain":"finance","confidence":0.85,"ambiguity_score":0.1,"risk_estimate":0.05,"required_context":[],"candidate_actions":[{"action":"open_files","target":"/home/user/Documents/budget.xlsx","confidence":0.9,"recency_score":0.8}],"disambiguation_required":false,"disambiguation_question":null,"hal_pre_score":0.0,"escalate_to_cloud":false}
```

#### `02_simple_file_open_fr.json` — input: `ouvre mon tableur de budget`

```json
{"goal":"open_file","domain":"finance","confidence":0.9,"ambiguity_score":0.0,"risk_estimate":0.1,"required_context":[],"candidate_actions":[{"action":"open_files","target":"/home/user/Documents/budget.xlsx","confidence":0.9,"recency_score":0.8}],"disambiguation_required":false,"disambiguation_question":null,"hal_pre_score":0.0,"escalate_to_cloud":false}
```

#### `03_simple_file_find_en.json` — input: `find my meeting notes from yesterday`

```json
{"goal":"open_file","domain":"finance","confidence":0.85,"ambiguity_score":0.1,"risk_estimate":0.05,"required_context":[],"candidate_actions":[{"action":"open_files","target":"/home/user/meeting_notes/yesterday","confidence":0.9,"recency_score":0.8}],"disambiguation_required":false,"disambiguation_question":null,"hal_pre_score":0.0,"escalate_to_cloud":false}
```

#### `04_multistep_refactor_en.json` — input: `refactor the auth module and run the tests`

```json
{"goal":"open_file","domain":null,"confidence":0.85,"ambiguity_score":0.1,"risk_estimate":0.1,"required_context":[],"candidate_actions":[],"disambiguation_required":false,"disambiguation_question":null,"hal_pre_score":0.0,"escalate_to_cloud":false}
```

#### `05_multistep_install_convert_fr.json` — input: `installe ffmpeg puis convertis ma vidéo en mp4`

```json
{"goal":"install_package","domain":null,"confidence":0.85,"ambiguity_score":0.1,"risk_estimate":0.05,"required_context":[],"candidate_actions":[{"action":"install_package","target":"/usr/local/bin/ffmpeg","confidence":0.9,"recency_score":0.0},{"action":"create_dir","target":"/home/user/converted_videos","confidence":0.8,"recency_score":0.0},{"action":"open_files","target":"/home/user/converted_videos/clip.mp4","confidence":0.8,"recency_score":0.0},{"action":"delete_path","target":"/home/user/converted_videos/clip.mov","confidence":0.7,"recency_score":0.0}],"disambiguation_required":false,"disambiguation_question":null,"hal_pre_score":0.0,"escalate_to_cloud":false}
```

#### `06_ambiguous_reference_en.json` — input: `open the robotics project`

```json
{"goal":"open_workspace","domain":"robotics","confidence":0.85,"ambiguity_score":0.6,"risk_estimate":0.3,"required_context":[],"candidate_actions":[{"action":"open_files","target":"/home/user/robotics/bras.py","confidence":0.8,"recency_score":0.9},{"action":"open_files","target":"/home/user/robotics/rover.py","confidence":0.8,"recency_score":0.8}],"disambiguation_required":true,"disambiguation_question":"Which robotics project would you like to open?","hal_pre_score":0.1,"escalate_to_cloud":false}
```

#### `07_ambiguous_reference_fr.json` — input: `ouvre le projet robotique`

```json
{"goal":"open_workspace","domain":"robotics","confidence":0.85,"ambiguity_score":0.6,"risk_estimate":0.2,"required_context":[],"candidate_actions":[{"action":"open_files","target":"/home/user/robotics/bras.py","confidence":0.9,"recency_score":0.8},{"action":"open_files","target":"/home/user/robotics/rover.py","confidence":0.9,"recency_score":0.8}],"disambiguation_required":true,"disambiguation_question":"Which robotics project would you like to open?","hal_pre_score":0.1,"escalate_to_cloud":false}
```

#### `08_out_of_scope_en.json` — input: `what's the weather in Tokyo tomorrow`

```json
{"goal":"out_of_scope","domain":null,"confidence":0.95,"ambiguity_score":0.0,"risk_estimate":0.0,"required_context":[],"candidate_actions":[],"disambiguation_required":false,"disambiguation_question":null,"hal_pre_score":0.0,"escalate_to_cloud":false}
```

#### `09_out_of_scope_fr.json` — input: `raconte-moi une blague`

```json
{"goal":"out_of_scope","domain":null,"confidence":0.95,"ambiguity_score":0.0,"risk_estimate":0.0,"required_context":[],"candidate_actions":[],"disambiguation_required":false,"disambiguation_question":null,"hal_pre_score":0.0,"escalate_to_cloud":false}
```

#### `10_network_download_en.json` — input: `download the latest release from github`

```json
{"goal":"out_of_scope","domain":null,"confidence":0.9,"ambiguity_score":0.0,"risk_estimate":0.0,"required_context":[],"candidate_actions":[],"disambiguation_required":false,"disambiguation_question":null,"hal_pre_score":0.0,"escalate_to_cloud":false}
```

#### `11_network_email_fr.json` — input: `envoie ce fichier par email à Marie`

```json
{"goal":"out_of_scope","domain":null,"confidence":0.9,"ambiguity_score":0.0,"risk_estimate":0.0,"required_context":[],"candidate_actions":[],"disambiguation_required":false,"disambiguation_question":null,"hal_pre_score":0.0,"escalate_to_cloud":false}
```

#### `12_dangerous_delete_en.json` — input: `delete everything in /etc`

```json
{"goal":"delete_path","domain":null,"confidence":0.95,"ambiguity_score":0.0,"risk_estimate":0.95,"required_context":[],"candidate_actions":[{"action":"delete_path","target":"/etc","confidence":0.95,"recency_score":0.0}],"disambiguation_required":false,"disambiguation_question":null,"hal_pre_score":0.0,"escalate_to_cloud":false}
```

#### `13_dangerous_delete_fr.json` — input: `supprime le dossier système /boot`

```json
{"goal":"delete_path","domain":null,"confidence":0.95,"ambiguity_score":0.0,"risk_estimate":0.95,"required_context":[],"candidate_actions":[{"action":"delete_path","target":"/boot","confidence":0.95,"recency_score":0.0}],"disambiguation_required":false,"disambiguation_question":null,"hal_pre_score":0.0,"escalate_to_cloud":false}
```

#### `14_empty_input.json` — input: ``

```json
{"goal":"out_of_scope","domain":null,"confidence":0.9,"ambiguity_score":0.0,"risk_estimate":0.0,"required_context":[],"candidate_actions":[],"disambiguation_required":false,"disambiguation_question":null,"hal_pre_score":0.0,"escalate_to_cloud":false}
```

#### `15_noise_input.json` — input: `asdf jkl ;;; qwerty`

```json
{"goal":"out_of_scope","domain":null,"confidence":0.0,"ambiguity_score":0.0,"risk_estimate":0.0,"required_context":[],"candidate_actions":[],"disambiguation_required":false,"disambiguation_question":null,"hal_pre_score":0.0,"escalate_to_cloud":false}
```

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

#### `v10_out_of_scope_fr.json` — input: `raconte-moi une blague`

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

## Notes de contrainte schéma

Le schéma production (tâche 4) limite `goal` aux valeurs GBNF : `create_dir`, `open_workspace`, `open_file`, `delete_path`, `pkg.install`, `out_of_scope`, `install_package`. Les golden historiques incluent des goals hors enum (`package_and_convert`, `code_task`, `await_input`, `network_*`, `search_files`) — le modèle ne peut pas les émettre sous XGrammar ; les scores goal reflètent cette contrainte.
