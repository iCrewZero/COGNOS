# Provenance des modèles — COGNOS/OS (WSL2, RTX 4090 Laptop)

Document factuel pour trancher **Qwen2.5 vs Qwen3** et savoir **quel modèle tournait où**, afin que les comparaisons perf/qualité ultérieures soient valides.

**Mesures :** 2026-07-10 (WSL), sauf latences perf llama.cpp/vLLM POC datées des sessions indiquées dans le tableau §4.

---

## Verdict (2.5 vs 3)

| Question | Réponse mesurée |
|----------|-----------------|
| Le GGUF `qwen3-7b-instruct-q4_k_m.gguf` est-il Qwen3 ? | **Non.** Métadonnées GGUF + logs `llama-server` : architecture `qwen2`, nom **`Qwen2.5 7B Instruct`**. |
| Le POC vLLM utilisait-il Qwen3 ? | **Non.** Modèle chargé : **`Qwen/Qwen2.5-7B-Instruct-AWQ`** (`Qwen2ForCausalLM`). |
| Le repo nomme-t-il le modèle « qwen3 » ? | **Oui** — alias config/scripts (`qwen3-7b-q4_k_m`) **ne reflètent pas** l’identité réelle du fichier GGUF. |

**Les deux moteurs ont tourné la famille Qwen2.5 7B Instruct — pas Qwen3.**

**Ce ne sont toutefois PAS les mêmes poids ni le même format de quantisation :**

| Moteur | Poids | Format | Source |
|--------|-------|--------|--------|
| llama.cpp | merge **mergekit** (3 bases Qwen2.5) | GGUF **Q4_K_M** | HF `Ygz-08123/Qwen3-7B-Instruct-Q4_K_M-GGUF` (nom repo trompeur) |
| vLLM POC | checkpoint HF officiel | **AWQ 4-bit** | HF `Qwen/Qwen2.5-7B-Instruct-AWQ` |

Toute comparaison qualité llama.cpp ↔ vLLM compare donc **deux variantes Qwen2.5 7B Instruct**, pas un même artefact bit-à-bit.

---

## 1. GGUF llama.cpp (chemin production / benchmarks GPU)

### Fichier local

| Champ | Valeur mesurée |
|-------|----------------|
| Chemin | `/root/cognos-models/qwen3-7b-instruct-q4_k_m.gguf` |
| Taille disque | **4 683 074 272** octets (`stat`, 2026-07-10) |
| Alias config | `qwen3-7b-q4_k_m` (`config/intent.toml`, `intent-engine/src/config.rs`) |

### Source de téléchargement (session e2e real, 2026-07-09)

```
https://huggingface.co/Ygz-08123/Qwen3-7B-Instruct-Q4_K_M-GGUF/resolve/main/qwen3-7b-instruct-q4_k_m.gguf
```

Le dépôt HF est intitulé « Qwen3 » ; **les métadonnées embarquées dans le fichier contredisent ce libellé.**

### Métadonnées GGUF — `llama-server` au chargement

Binaire : `build/cache/llama.cpp/build-cuda/bin/llama-server`  
Build : **9940** (`259f2e2a5`), GNU 12.5.0, Linux x86_64  
Commande de capture : `scripts/llama_load_log_snippet.sh` (2026-07-10)

Extraits log (`llama_model_loader` / `print_info`) :

```
general.architecture     = qwen2
general.name             = Qwen2.5 7B Instruct
general.basename         = Qwen2.5
general.finetune         = Instruct
general.size_label       = 7B
general.file_type        = 15          → Q4_K - Medium
general.tags             = ["mergekit", "merge"]
general.base_model.0.name = Qwen2.5 Coder 7B Instruct
general.base_model.0.repo_url = https://huggingface.co/Qwen/Qwen2.5-Coder-7B-Instruct
general.base_model.1.name = Qwen2.5 7B Instruct
general.base_model.1.repo_url = https://huggingface.co/Qwen/Qwen2.5-7B-Instruct
general.base_model.2.name = Qwen2.5 Math 7B Instruct
general.base_model.2.repo_url = https://huggingface.co/Qwen/Qwen2.5-Math-7B-Instruct
qwen2.block_count        = 28
qwen2.context_length     = 32768
qwen2.embedding_length   = 3584
print_info: file format  = GGUF V3 (latest)
print_info: file type    = Q4_K - Medium
print_info: file size    = 4.36 GiB (4.91 BPW)
```

Répartition tenseurs (même log) : `f32: 141`, `q4_K: 169`, `q6_K: 29`.

### VRAM GPU (llama.cpp, mesure 2026-07-10)

Script : `scripts/llama_vram_probe_wsl.sh`  
Flags : `-ngl 99 -c 4096 --jinja --reasoning off -fa on`  
GPU : NVIDIA GeForce RTX 4090 Laptop GPU, **16 376 MiB** total

```
nvidia-smi memory.used après chargement : 4888 MiB
```

(Session GPU antérieure : ~4,9 Go VRAM occupée au repos serveur — cohérent.)

### Perf de référence (intent bénin + GBNF, session 2026-07-10)

Mesurées sur **ce même GGUF** via llama.cpp CUDA + grammaire `intent.gbnf` :

| Métrique | Valeur |
|----------|--------|
| tok/s guidé (chaud) | **19,47** |
| tok/s sans grammaire (chaud) | **~50** |
| latence guidée froid | **11 617 ms** |
| tokens guidés froid | **222** |

---

## 2. Modèle POC vLLM

### Identité

| Champ | Valeur mesurée |
|-------|----------------|
| ID Hugging Face | **`Qwen/Qwen2.5-7B-Instruct-AWQ`** |
| URL | `https://huggingface.co/Qwen/Qwen2.5-7B-Instruct-AWQ` |
| Format | **AWQ 4-bit** (`quantization="awq"` dans `scripts/poc_vllm_xgrammar_benchmark.py`) |
| Architecture résolue | **`Qwen2ForCausalLM`** (log chargement vLLM, session POC 2026-07-10 ~03:19) |
| Cache HF local | **5,2 Go** (`du` sur `~/.cache/huggingface/hub/models--Qwen--Qwen2.5-7B-Instruct-AWQ`, 2026-07-10) |

vLLM **ne charge pas** le GGUF local ; le script POC documente explicitement le choix AWQ comme « closest HF match » au GGUF (métadonnées Qwen2.5 7B Instruct).

### Stack runtime (venv `/root/cognos-vllm-venv`, mesure 2026-07-10)

| Composant | Version |
|-----------|---------|
| vLLM | **0.24.0** |
| torch | **2.11.0+cu130** |
| xgrammar | **0.2.3** (via vLLM) |
| Python | **3.12.13** |
| GPU | NVIDIA GeForce RTX 4090 Laptop GPU, CUDA OK |

Paramètres chargement POC : `max_model_len=4096`, `gpu_memory_utilization=0.85`, `trust_remote_code=True`.

### VRAM (log vLLM au chargement, session POC 2026-07-10)

Extraits du log de démarrage vLLM (fichier `/tmp/vllm-xgrammar-poc.log`, session POC ; artefact non conservé au 2026-07-10, chiffres reprise du log mesuré) :

| Poste | Valeur |
|-------|--------|
| Poids modèle (AWQ) | **~5,29 GiB** |
| Budget KV cache alloué | **~6,56 GiB** disponibles |
| GPU totale | 16 GiB (4090 Laptop) |

**Note :** llama.cpp (~4,9 GiB VRAM poids+KV minimal) et vLLM (~5,3 GiB poids seuls + pool KV séparé) n’ont **pas la même empreinte mémoire** ; les chiffres ne sont pas directement additionnables entre moteurs.

### Perf POC (intent bénin, XGrammar, même session)

| Métrique | llama.cpp GBNF | vLLM + XGrammar guidé |
|----------|----------------|------------------------|
| tok/s chaud | 19,47 | **57,52** |
| latence chaud | 10 989 ms | **1 217 ms** |
| tokens guidés chaud | 222 | **70** |

Résultats détaillés : `/tmp/vllm-xgrammar-poc.json` (généré session POC ; absent du disque au 2026-07-10).

---

## 3. Fichier secondaire (non utilisé en prod)

| Fichier | Taille | État |
|---------|--------|------|
| `/root/cognos-models/Qwen3.5-4B-Q4_K_M.gguf` | **0 octet** | placeholder / téléchargement incomplet |

Profil dégradé prévu depuis `bartowski/Qwen_Qwen3.5-4B-GGUF` — **non validé** sur cette machine.

---

## 4. Tableau — modèle par moteur et par tour

« Tour » = session de travail / objectif mesuré dans le dépôt (ordre chronologique).

| Tour | Date (UTC-7) | Moteur | Chemin / ID | Identité **réelle** (métadonnées) | Rôle | Même poids que l’autre moteur ? |
|------|--------------|--------|-------------|-----------------------------------|------|--------------------------------|
| **A** — e2e real CPU | 2026-07-09 ~03:10 | llama.cpp CPU | `qwen3-7b-instruct-q4_k_m.gguf` | **Qwen2.5 7B Instruct** merge Q4_K_M | Premier branchement llama-server ; GBNF bloquait → fallback keyword | — |
| **B** — GPU CUDA | 2026-07-09 ~09:28 | llama.cpp CUDA | idem | idem | Rebuild `GGML_CUDA=ON`, latence GPU | — |
| **C** — prompt / grammaire | 2026-07-09 ~11:07 | llama.cpp CUDA | idem | idem | Fix chat template + retrait champs injectés de la grammaire | — |
| **D** — refactor injecté | 2026-07-10 ~02:24 | llama.cpp CUDA | idem | idem | Mesures tokens/latence post-refactor compilable | — |
| **E** — diag perf GBNF | 2026-07-10 ~02:24+ | llama.cpp CUDA | idem | idem | Plafond ~19 tok/s avec GBNF vs ~50 sans | — |
| **F** — POC vLLM perf | 2026-07-10 ~03:19 | vLLM 0.24 + XGrammar | `Qwen/Qwen2.5-7B-Instruct-AWQ` | **Qwen2.5 7B Instruct AWQ** (HF officiel) | Benchmark isolé 4 intents + intent bénin | **Non** — format AWQ, poids HF ≠ merge GGUF |
| **G** — qualité golden | 2026-07-10 ~03:49+ | vLLM (prévu) | idem tour F | idem | Script `poc_vllm_golden_quality.py` ; baseline 15 golden **non finalisée** dans cette session | **Non** |

### Synthèse comparaisons

| Comparaison | Valide sémantiquement ? | Valide poids-identiques ? |
|-------------|-------------------------|---------------------------|
| Perf llama (tours B–E) vs perf vLLM (tour F) | Partiel — même **famille** Qwen2.5 7B Instruct | **Non** |
| Qualité llama vs qualité vLLM | Partiel — même famille, prompts comparables | **Non** |
| Libellé repo « Qwen3 » vs réalité | **Invalide** — le nom « Qwen3 » dans config/scripts est **trompeur** | — |

---

## 5. Méthode de reproduction

```bash
# Métadonnées GGUF via llama-server (WSL)
bash scripts/llama_load_log_snippet.sh | grep -E 'general\.|print_info|qwen2\.'

# VRAM llama.cpp après chargement
bash scripts/llama_vram_probe_wsl.sh

# Versions vLLM
/root/cognos-vllm-venv/bin/python scripts/probe_vllm_versions.py

# Taille cache HF AWQ
du -sh ~/.cache/huggingface/hub/models--Qwen--Qwen2.5-7B-Instruct-AWQ
```

Parser GGUF autonome (sans llama) : `scripts/dump_gguf_meta.py`, `scripts/parse_gguf_kv.py`.

---

## 6. Implications pour les décisions au retour

1. **Renommer** alias `qwen3-7b-q4_k_m` → libellé reflétant Qwen2.5 (ou documenter explicitement le décalage) — **DÉCISION HUMAINE**.
2. Avant d’intégrer vLLM en production, figer un **seul** artefact de référence (GGUF merge vs HF AWQ vs autre) et réévaluer les 15 golden sur **ce** poids.
3. Un test « Qwen3 » réel (ex. `Qwen3-8B-AWQ`) n’a **pas** été mené à terme sur cette machine au moment de ce document (cache HF `models--Qwen--Qwen3-8B-AWQ` : 16 Mo seulement).
