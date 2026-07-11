# Qualité intent — état final consolidé

**Mesuré :** 2026-07-10T16:28:16Z (UTC)

## Contexte

- Schéma production **v2.0.0** aligné (`docs/GOAL_TAXONOMY.md`)
- Prompt système **inchangé** (`intent-engine/src/prompt.rs`)
- Harnais vLLM : court-circuit `empty_input` identique à `intent-engine/src/parser.rs`
- Modèle : `Qwen/Qwen2.5-7B-Instruct-AWQ` (vLLM 0.24.0)

## Tableau de référence (jalons)

| Étape | Golden goal | Validation goal | Golden disambig | Validation disambig |
|-------|-------------|-----------------|-----------------|----------------------|
| Prod étroit (overfit check) | 8/15 | 14/20 | 15/15 | 18/20 |
| Prod aligné v2 (harnais sans short-circuit) | 14/15 | 17/20 | 15/15 | 18/20 |
| **Final (aligné + short-circuit harnais)** | **15/15** | **18/20** | 15/15 | 18/20 |

## Scores finaux (chemin production réel)

| Jeu | goal | disambiguation | candidate_actions |
|-----|------|----------------|-----------------|
| Golden | **15/15** | 15/15 | 15/15 |
| Validation | **18/20** | 18/20 | 18/20 |

## Résidus isolés (par catégorie)

### 1. Disambiguation — non corrigé ce tour (décision prompt ultérieure)

Échecs **purs** (goal OK, disambig KO) : golden **0**, validation **2**.

- `v05_ambiguous_project_en.json` : goal `open_workspace` OK — attendu disambig=True, produit=False
- `v06_ambiguous_projet_fr.json` : goal `open_workspace` OK — attendu disambig=True, produit=False

### 2. Goal sémantique résiduel (hors empty_input)

- `v13_search_files_en.json` : attendu `search_files`, produit `open_file`
- `v18_multistep_code_en.json` : attendu `code_task`, produit `network_send`

### 3. Goals réseau — route HAL en attente (décision humaine)

- `network_download` — parser OK, **pas de route orchestrator→HAL** (voir `docs/HAL_NETWORK_ROUTING_PROPOSAL.md`)
- `network_send` — idem
- **Tension** : tant que non routés, ces goals ne sont pas exécutables de bout en bout malgré le vocabulaire parser.

## Artefacts

- Taxonomie : `docs/GOAL_TAXONOMY.md`
- Proposition HAL réseau : `docs/HAL_NETWORK_ROUTING_PROPOSAL.md`
- Alignement schéma : `docs/PROD_SCHEMA_ALIGNED.md`
- JSON mesure : `tmp/quality_final.json`
