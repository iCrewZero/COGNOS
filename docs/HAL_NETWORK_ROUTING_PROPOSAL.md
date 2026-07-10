# HAL network routing — proposition (décision humaine)

**Statut :** PROPOSITION — **aucune ligne de `hal/src/` modifiée** ce tour.  
**Objectif :** préparer le mapping `network_download` / `network_send` pour décision avant câblage orchestrator/HAL.

Références lues (état repo 2026-07-10) :
- `orchestrator/src/intent_adapter.rs` — `action_to_capability` (pas de branche réseau dédiée)
- `orchestrator/src/hal_gate.rs` — `is_side_effecting`, `gate_action`
- `hal/src/risk_scorer.rs` — formule + hard floors
- `hal/src/trust_calibration.rs` — `ActionClass::NetworkChange`
- `hal/src/action_validator.rs` — `validate_capability_scope` (actions contenant `"network"` → `network.outbound`)
- `security/nftables/ai-isolation.nft` — deny-by-default egress agents IA
- `ipc/agent-ipc/src/capability.rs` — `network.outbound`, `network.inbound`
- Golden : `10_network_download_en.json` (`hal_pre_score` 0.45), `11_network_email_fr.json` (`hal_pre_score` 0.5)

---

## Tension fondamentale

Un OS dont HAL gate toute action privilégiée **ne peut pas** avoir un goal réseau **non gaté** et exécutable.

Aujourd'hui :
1. **Parser** — `network_download` / `network_send` sont dans le vocabulaire (schéma v2 + GBNF).
2. **Orchestrator** — `download_file` / `send_email` tombent en `general.execute` (fallback), pas `network.outbound`.
3. **HAL `action_validator`** — une action dont le nom contient `"network"` exige déjà la capability `network.outbound` — mais ce chemin n'est pas atteint via le mapping actuel.
4. **nftables** — même si HAL accordait `granted`, le **noyau** drop l'egress des agents IA sauf DNS (53) et loopback, tant que `allowed_api_endpoints` est vide.

**Conséquence :** soit une **route HAL explicite** est définie (votre décision), soit ces goals restent **hors du vocabulaire exécutable** de bout en bout — même s'ils sont parsables.

---

## Proposition de mapping (orchestrator, futur — non implémenté)

| Goal | Action candidate typique | Capability proposée | Op HAL gate (`SideEffect.op`) |
|------|------------------------|---------------------|-------------------------------|
| `network_download` | `download_file` | **`network.outbound`** | `net.download` (ou `network.fetch`) |
| `network_send` | `send_email` | **`network.outbound`** | `net.send` (aligné proto `cognos.proto`) |

**Changement orchestrator suggéré (hors scope ce tour) :** étendre `action_to_capability` :

```
download_file | network_download → network.outbound
send_email    | network_send     → network.outbound
```

**Changement `hal_gate::is_side_effecting` suggéré :** traiter `network.outbound` / `network.inbound` comme side-effecting (aujourd'hui `network.outbound` **n'est pas** détecté — seul `net.send` apparaît dans les marqueurs).

---

## Niveaux de risque proposés (chiffrés, modèle formel HAL)

Rappel hard floors existants (`hal/src/risk_scorer.rs`, `hal/src/score_fusion.rs`) :
| Trigger | Floor | Bande HAL typique |
|---------|-------|-------------------|
| Delete | **≥ 0.5** | Notify minimum |
| Kernel-adjacent | **≥ 0.7** | Confirm minimum |
| AI-generated unreviewed | **≥ 0.8** | Block minimum |

Rappel bandes (`RiskLevel::from_score`) :
- [0.0, 0.3) Silent — [0.3, 0.6) Notify — [0.6, 0.8) Confirm — [0.8, 1.0] Block

Rappel calibration (`ActionClass::NetworkChange`) : seuil par défaut **0.60**, floor utilisateur **0.60** minimum.

### `network_download` — téléchargement HTTP(S)

| Composant | Valeur proposée | Score composant |
|-----------|-----------------|-----------------|
| Irreversibility | `HardToReverse` (fichier distant → disque) | 0.7 |
| Scope | `SingleFileUserHome` (cible fichier unique) | 0.0 |
| TrustContext | `KnownTrusted` agent planner | 0.0 |
| TimeAnomaly | `Normal` | 0.0 |
| VibeFlag | `None` | 0.0 |
| UserHistory | `Frequent` (utilisateur télécharge souvent) | 0.7 |
| PatternMatch | `FullMatch` (URL allowlist connue) | 0.0 |

**Score formule (poids w1–w7 du modèle) :**  
`R = 0.25×0.7 + 0.20×0.0 + … − 0.10×0.7 − 0.05×0.0 ≈ **0.105**` → bande **Silent** (0.105).

**Cas conservateur (nouvel agent / URL inconnue) :** Trust `NewApp` (0.7) → `R ≈ 0.245` → **Silent** limite ; Scope `MultipleFileSingleDir` (0.3) → `R ≈ 0.305` → **Notify**.

**Alignement golden 10 :** `hal_pre_score` / `risk_estimate` fixture = **0.45** → bande **Notify** (cohérent avec un téléchargement modérément risqué sans floor delete).

**Floor réseau proposé (nouveau, à décider) :** **`NetworkOutbound ≥ 0.5`** — par analogie delete : toute egress agent IA au minimum Notify, jamais Silent.  
Avec ce floor : score plancher **0.5** → **Notify** ; gate HAL → au minimum toast + undo, souvent **`approval_required`** selon politique.

### `network_send` — envoi email / transmission

| Composant | Valeur proposée | Score composant |
|-----------|-----------------|-----------------|
| Irreversibility | `Irreversible` (email parti = non rappelable) | 1.0 |
| Scope | `MultipleFileSingleDir` (pièce jointe + destinataire) | 0.3 |
| TrustContext | `KnownTrusted` | 0.0 |
| TimeAnomaly | `Normal` | 0.0 |
| VibeFlag | `None` | 0.0 |
| UserHistory | `Occasional` (0.4) | 0.4 |
| PatternMatch | `PartialMatch` (contact connu) | 0.5 |

**Score formule :**  
`R = 0.25×1.0 + 0.20×0.3 + … − 0.10×0.4 − 0.05×0.5 ≈ **0.295**` → **Silent** (sans floor).

**Avec floor NetworkOutbound ≥ 0.5 :** **0.5** → **Notify**.  
**Avec historique faible + contact inconnu :** Trust `NewApp` → `R ≈ 0.435` ; floor 0.5 → **Notify**.

**Alignement golden 11 :** `hal_pre_score` / `risk_estimate` = **0.5** → **Notify** exact (seuil delete floor).

**Recommandation proposition :** `network_send` devrait **toujours** être ≥ **Confirm (0.6)** en politique stricte (données quittent la machine), indépendamment du score brut — soit via floor dédié **≥ 0.6** (`NetworkChange` default threshold), soit via règle gate « jamais `granted` direct ».

---

## Interaction nftables — exécutabilité réelle

Règles `security/nftables/ai-isolation.nft` (résumé mesuré dans le fichier) :

```
ip saddr @ai_agents_ipv4 ip daddr != @allowed_api_endpoints → DROP (log)
allowed_api_endpoints = { /* vide par défaut */ }
Exceptions : DNS 53 TCP/UDP, loopback 127.0.0.1
```

| Scénario | HAL `granted` | Egress noyau | Exécutable ? |
|----------|---------------|--------------|--------------|
| `allowed_api_endpoints` vide | oui | **DROP** | **Non** |
| Endpoint ajouté à la allowlist (décision utilisateur) | oui | ACCEPT vers cet IP | Oui (vers cet hôte) |
| HAL `approval_required` + allowlist | après approbation | ACCEPT si endpoint autorisé | Oui, chemin contrôlé |
| HAL `denied` | non | DROP | Non |

**Proposition opérationnelle :**

1. **Jamais `granted` silencieux** pour `network.outbound` — toujours **`approval_required`** minimum (même si score < 0.6), car nftables exige une allowlist de toute façon.
2. **Coupler** approbation HAL + ajout temporaire ou permanent de l'endpoint dans `allowed_api_endpoints` (TODO v1 déjà noté dans le fichier nft).
3. **Alternative architecture :** proxy réseau hors cgroup IA (orchestrator / service trusted) — le goal réseau ne fait pas de socket direct agent ; HAL gate le proxy, pas l'agent. Décision produit.

---

## Options de décision (choix humain)

| Option | Description | Conséquence |
|--------|-------------|-------------|
| **A — Router + gater** | Mapper goals → `network.outbound`, floor ≥ 0.5, gate toujours `approval_required`, allowlist nft | Goals exécutables après approbation + allowlist |
| **B — Parser only** | Garder vocabulaire, pas de route (état actuel) | Intent parsable, exécution échoue / fallback `general.execute` |
| **C — Retirer du vocabulaire** | Retirer `network_*` du schéma tant que non routés | Cohérent mais régresse scores golden 10/11 |
| **D — Proxy trusted** | Agent ne fait pas egress ; service COGNOS dédié | nftables inchangé ; HAL gate le proxy |

**Recommandation documentée (non imposée) :** **Option A** avec floor **0.5** (download) / politique **≥ 0.6 Confirm** (send), gate **`approval_required` systématique**, allowlist nft synchronisée sur approbation.

---

## Checklist avant implémentation (futur)

- [ ] Décision humaine sur option A/B/C/D
- [ ] `action_to_capability` : branches `download_file`, `send_email`, `network_*`
- [ ] `is_side_effecting("network.outbound")` → true
- [ ] Hard floor `NetworkOutbound` dans `risk_scorer` / `score_fusion` (si option A)
- [ ] Intégration allowlist `allowed_api_endpoints` ↔ flux approbation HAL
- [ ] Tests `hal/tests/intent_to_hal.rs` étendus pour goals réseau

**Ce tour :** zéro case cochée — proposition uniquement.
