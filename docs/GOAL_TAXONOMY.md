# Goal taxonomy — COGNOS intent parser (source de vérité)

Document de référence unique pour les **goals** LLM-émis (`IntentSchema.goal`).
Aligné sur le croisement mesuré du 2026-07-10 :

| Source | Méthode |
|--------|---------|
| (a) Schéma élargi | `intent-engine/schema/intent-golden-eval.schema.json` |
| (b) Modèle (vLLM+XGrammar) | Sorties chaudes `tmp/overfit_check.json` (golden+validation, schéma élargi) |
| (c) Fixtures | `expected_intent.goal` dans `tests/golden/` (15) + `tests/validation/` (20) |

Le prompt système (`intent-engine/src/prompt.rs`) liste déjà ces goals — la contrainte prod (schéma + GBNF) était en retard.

---

## Liste canonique (13 goals)

| Goal | Catégorie | Dans expected fixtures | Produit par modèle (éval élargi) | Notes |
|------|-----------|------------------------|-----------------------------------|-------|
| `create_dir` | simple / filesystem | oui | oui | |
| `open_file` | simple / filesystem | oui | oui | |
| `open_workspace` | simple / workspace | oui | oui | |
| `search_files` | simple / search | oui (golden 03) | oui | |
| `install_package` | package | oui | oui | Forme canonique install |
| `pkg.install` | package (legacy) | non (expected utilisent `install_package`) | non en éval | Conservé : keyword fallback, agents planner |
| `package_and_convert` | multi-étapes | oui (golden 05) | oui | |
| `code_task` | multi-étapes / coding | oui (golden 04) | oui | |
| `delete_path` | dangereux | oui | oui | |
| `network_download` | réseau | oui (golden 10) | oui | |
| `network_send` | réseau | oui (golden 11) | oui | |
| `out_of_scope` | hors-scope | oui | oui | |
| `await_input` | vide / bruit | oui (golden 14–15) | oui | Court-circuit Rust si input vide |

**Union (a)∩(b)∩(c)** sur les goals métier : les 12 goals sauf `pkg.install` (legacy seul).

**Décision taxonomie :** les 13 valeurs ci-dessus sont autorisées en production (`intent-llm-output.schema.json` v2 + `intent.gbnf`). `pkg.install` reste pour compatibilité keyword/legacy ; les nouveaux intents préfèrent `install_package`.

---

## Goals exclus (non ajoutés)

Aucune valeur hallucinée hors des trois sources ci-dessus n'a été ajoutée. En particulier, pas de goal inventé absent de expected + éval + sorties modèle.

---

## Implications HAL (lecture seule — aucune modif ce tour)

Le routage production passe par `ActionGraph::from_schema` (goal ou `candidate_actions[].action`) puis `orchestrator::intent_adapter::action_to_capability`.

| Goal | Actions typiques | Capability résolue | HAL gating |
|------|------------------|--------------------|------------|
| `create_dir` | `create_dir` | `file.write` | **gated** |
| `open_file`, `open_workspace` | `open_files`, `open_file` | `file.read` | read-only |
| `search_files` | `search_files` | `file.read` (prefix search) | read-only |
| `install_package`, `pkg.install`, `package_and_convert` | `install_package`, `convert_media` | `pkg.execute` / mix | **gated** (pkg) |
| `code_task` | `refactor_code`, `run_tests` | `coding.execute`, `coding.validate` | execute **gated** |
| `delete_path` | `delete_path` | `file.write` | **gated** |
| `network_download` | `download_file` | `general.execute` (fallback) | **DÉCISION HUMAINE** — pas de route `network.outbound` explicite goal→HAL |
| `network_send` | `send_email` | `general.execute` (fallback) | **DÉCISION HUMAINE** — idem |
| `out_of_scope`, `await_input` | `[]` | n/a (pas d'exécution) | n/a |

### Goals supportés parser mais route HAL à définir

Consigné pour décision humaine (pas de changement HAL ce tour) :

1. **`network_download`** — capability effective souvent `general.execute` ; HAL a `network.outbound` côté capability lattice mais pas de mapping goal→`network.outbound` dans l'orchestrator.
2. **`network_send`** — idem pour envoi email / `send_email`.

Les autres goals ajoutés réutilisent des mappings action existants (`file.*`, `pkg.execute`, `coding.*`).

---

## Artefacts alignés (v2)

| Artefact | Version |
|----------|---------|
| `intent-engine/schema/intent-llm-output.schema.json` | **2.0.0** |
| `intent-engine/grammar/intent.gbnf` | goal-string + action-string + domain + target élargis |
| Tests | `tests/grammar_json_schema.rs`, `tests/json_schema_coverage.rs` |

Enum **goal** prod = enum **goal** de ce document (13 valeurs).
Enum **action** prod aligné sur `intent-golden-eval.schema.json` `$defs.candidate_action`.
