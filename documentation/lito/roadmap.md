# Roadmap Lito → production

*Document de travail · rédigé le 2026-08-15 contre l'état réel du dépôt · chaque item est
accompagné d'un prompt prêt à coller dans Claude Code*

> Ce fichier décrit **ce qui reste à faire**. Pour ce qui existe déjà, voir
> `llm_brief/summary.md` (état technique), `llm_brief/history.md` (déroulé) et `design.md`
> (système de design).

---

## 1. Constat de départ

Lito fait aujourd'hui, de bout en bout : chat multi-tour avec historique persisté, planification
via Ollama, inspection filesystem LIST/STAT avant plan, exécution des 7 opérations, undo groupé,
sessions multiples, thème sombre, streaming + trace de raisonnement, packaging `.app`. C'est un
**prototype complet**, pas un produit : la boucle marche, mais rien ne protège l'utilisateur quand
elle rate.

Les trous vérifiés dans le code (pas supposés) :

| Constat | Où | Conséquence |
| --- | --- | --- |
| **0 commit git**, pas de `.gitignore`, `target/` non ignoré | racine | aucun filet de sécurité pour refactorer, aucun historique |
| **Aucune validation de chemin** | `crates/fs/src/lib.rs` | le modèle peut émettre `DELETE ~/Library`, `MOVE ../..`, un symlink vers `/System` — c'est exécuté tel quel |
| **Undo en mémoire uniquement** | `FsOps.undo_log: Vec<UndoEntry>` | quitter l'app = perdre l'undo. `MemoryStore::log_undo`/`get_undo_actions` existent mais **ne sont jamais appelées** : la table `undo_log` est vide |
| **Pas de rollback sur échec partiel** | `handle_execute_plan`, `main.rs` | le `?` sur `execute_step` laisse les étapes déjà faites appliquées et le plan figé en statut `executing` |
| **DELETE = renommage `.bak.<uuid>` à côté du fichier** | `FsOps::delete` | `with_extension()` *remplace* l'extension (`photo.jpg` → `photo.bak.<uuid>`), les backups ne sont jamais purgés, et ça pollue les dossiers de l'utilisateur |
| **Pas d'annulation d'une génération en cours** | `view_model.rs` | `abort()` n'existe qu'au shutdown ; un modèle qui part en boucle bloque l'UI jusqu'au bout |
| **Tool-calling artisanal** (JSON dans le texte) | `crates/agent` | pas de lecture de contenu, pas de recherche récursive, 6 tours max, parsing fragile |
| **Toutes les erreurs remontent en `ErrorCode::Internal`** | `run_core_agent` | l'UI ne peut pas proposer de résolution ciblée |
| **Pas de CI, pas de test d'intégration** | — | `crates/llm::test_ollama_client` exige un Ollama vivant ; `execute_step`/`undo` ne sont pas testés |
| **`assets/` est vide** | `make package` fait `cp -r assets/`, `Info.plist` référence `AppIcon` | l'app bundle n'a pas d'icône |
| **7 dépendances déclarées jamais utilisées** | `notify`, `walkdir`, `keyring`, `fastembed`, `lopdf`, `docx-rs`, `calamine` | build alourdi (dont ONNX), surface d'audit inutile |
| **`Info.plist` : `NSAllowsArbitraryLoads: true`**, aucune usage description TCC | racine | refus probable à la notarisation/review, et prompts macOS non expliqués |
| **Chaînes UI en dur en français** | `app-window.slint`, `main.rs` | pas d'i18n possible sans refonte |

---

## 2. Ce que « production ready » veut dire ici

Lito n'est pas un chatbot : **c'est un agent qui écrit sur le disque de l'utilisateur.** La barre du
marché (Goose, Claude Desktop, Cursor, Warp, Raycast AI) se joue sur six axes, dans cet ordre
d'importance pour ce produit :

1. **Réversibilité et confinement** — l'agent ne peut pas sortir d'un périmètre consenti ; tout ce
   qu'il fait est annulable après redémarrage ; un échec à mi-plan ne laisse jamais un état bâtard.
2. **Boucle agentique solide** — tool-calling typé, erreurs réinjectées, interruption, budget de
   contexte.
3. **Qualité mesurée** — des évals qui disent si un changement de prompt ou de modèle améliore ou
   dégrade, en CI.
4. **Observabilité** — chaque run est traçable, exportable, rejouable.
5. **Onboarding et états d'erreur** — Ollama absent, modèle non téléchargé, dossier refusé : chaque
   cas a un écran et une action, pas un toast rouge.
6. **Distribution** — signature Developer ID, notarisation, mises à jour, rapport de bug.

---

## 3. Les phases

Six phases, ~30 items. Tailles : **S** ≤ 1 jour · **M** 1–3 jours · **L** ≈ 1 semaine (équivalent
temps plein). Chaque prompt suppose que Claude Code démarre à la racine du dépôt.

---

### Phase 0 — Fondations d'ingénierie

*Objectif : pouvoir changer le code sans peur. Rien d'autre ne devrait être entrepris avant.*

#### P0.1 — Mettre le projet sous git (S)

**Pourquoi** — `git log` échoue : `main` n'a aucun commit. Tout le travail décrit dans
`history.md` (25 entrées) existe uniquement dans le working tree. Un `rm -rf` malheureux, ou un
refactor de la Phase 1, et il n'y a rien à restaurer.

**Critères d'acceptation**
- `.gitignore` couvrant `target/`, `.idea/`, `.DS_Store`, `target/ui-shots/`, les artefacts de bundle
- premier commit contenant tout le code source, la doc et `Cargo.lock`
- `documentation/slint-docs/` : décider explicitement de le versionner ou non (c'est du vendored)
- un `CONTRIBUTING.md` court : conventions de commit, comment lancer les checks

```
Le dépôt Lito n'a aucun commit git (`main` est vide) et pas de `.gitignore` — `target/` et `.idea/`
apparaissent dans `git status`. Mets le projet sous contrôle de version proprement :

1. Écris un `.gitignore` adapté à un workspace Rust + Slint sur macOS : `/target`, `.DS_Store`,
   `.idea/`, plus les sorties de `make ui-shot` et de `make package`/`make dmg` si elles ne sont pas
   déjà sous `target/`. `Cargo.lock` DOIT être versionné (c'est un binaire, et la doc note un skew
   Slint 1.6→1.17.1 qu'on ne veut surtout pas perdre).
2. `documentation/slint-docs/` est une copie vendored de la doc officielle Slint (~lourde). Regarde
   sa taille réelle et recommande-moi de la versionner ou de l'ignorer, avec l'argument, puis
   applique ta recommandation.
3. Fais le premier commit avec un message qui décrit l'état atteint (binaire unique Rust + Slint,
   agent de planification Ollama, exécution + undo, sessions SQLite), pas juste "initial commit".
4. Ajoute un `CONTRIBUTING.md` court : les cibles du Makefile à lancer avant de committer
   (`make fmt`, `make clippy`, `make test`, `make ui-check`), la convention de message de commit, et
   la règle du CLAUDE.md sur la mise à jour de `documentation/lito/` dans le même changement.

Ne committe rien d'autre que le contenu du dépôt actuel, et montre-moi `git status` avant et après.
```

#### P0.2 — CI GitHub Actions (S)

**Pourquoi** — Aucune vérification automatique n'existe. Les cibles `make` sont bonnes mais
dépendent de la discipline manuelle.

**Critères d'acceptation**
- workflow sur `push` et `pull_request` : `cargo fmt --check`, `cargo clippy -D warnings`,
  `cargo test --workspace`, `cargo build --release`
- job séparé qui installe `slint-viewer` et lance `make ui-check` + `make ui-shot`, avec les PNG
  publiés en artefacts de build (relecture visuelle des PR)
- cache `~/.cargo` + `target/` ; runner `macos-latest`
- les tests exigeant Ollama sont exclus par défaut (voir P0.4)

```
Ajoute une CI GitHub Actions à Lito (workspace Rust + Slint, cible macOS, voir le Makefile pour les
commandes existantes).

Workflow `.github/workflows/ci.yml`, sur push et pull_request :
- job `check` sur `macos-latest` : `cargo fmt --all -- --check`, `cargo clippy --workspace -- -D
  warnings`, `cargo test --workspace`, `cargo build --release`. Cache cargo registry + `target/`
  avec `Swatinem/rust-cache`.
- job `ui` : installe slint-viewer épinglé (`cargo install slint-viewer --version "^1.17" --locked`,
  comme `make install-viewer`), lance `make ui-check` puis `make ui-shot`, et uploade
  `target/ui-shots/*.png` en artefacts pour qu'on puisse relire les rendus depuis la PR.

Attention : `crates/llm`'s `test_ollama_client` exige un serveur Ollama vivant sur localhost:11434 et
échouera en CI. Marque-le `#[ignore]` avec un commentaire expliquant pourquoi, jusqu'à ce qu'il soit
mocké, et vérifie que `cargo test --workspace` passe sans lui.

Vérifie la syntaxe du workflow avant de finir (`actionlint` si disponible, sinon relecture attentive)
et mets à jour le Makefile avec une cible `make ci` qui reproduit localement le job `check`.
```

#### P0.3 — Purger les dépendances fantômes + audit (S)

**Pourquoi** — `notify`, `walkdir`, `keyring`, `fastembed` (avec ONNX), `lopdf`, `docx-rs`,
`calamine` sont déclarées dans `Cargo.toml` et **utilisées nulle part**. Elles allongent le build,
gonflent le graphe d'audit, et donnent une fausse impression de fonctionnalités présentes.

**Critères d'acceptation**
- dépendances non utilisées retirées du `[workspace.dependencies]` **et** des crates qui les
  listent sans les utiliser (`lito-fs` liste `notify`/`walkdir`)
- `cargo-udeps` ou `cargo machete` lancé pour confirmer qu'il n'en reste pas
- `cargo deny check` (ou `cargo audit`) ajouté à la CI avec un `deny.toml` de départ
- temps de `cargo build --release` mesuré avant/après, noté dans le commit

```
`Cargo.toml` de Lito déclare des dépendances qui ne sont utilisées nulle part dans `crates/*/src` :
notify, walkdir, keyring, fastembed (qui tire ONNX), lopdf, docx-rs, calamine. `crates/fs/Cargo.toml`
liste notify et walkdir sans jamais les importer.

1. Confirme la liste avec `cargo machete` ou `cargo +nightly udeps` (installe ce qui manque), ne te
   fie pas seulement à un grep.
2. Retire-les du `[workspace.dependencies]` et des manifestes de crate. Ces dépendances correspondent
   à des fonctionnalités prévues (surveillance de dossiers, keyring, RAG, parsing de documents) : ne
   les supprime pas silencieusement, note dans le commit qu'elles seront réintroduites au moment où
   la fonctionnalité arrive.
3. Mesure `cargo build --release` en propre avant et après, et donne-moi les deux chiffres.
4. Ajoute `cargo-deny` avec un `deny.toml` de départ (advisories + licences) et branche
   `cargo deny check` dans la CI.
5. Mets à jour le tableau des dépendances dans `documentation/lito/llm_brief/summary.md` §2, qui
   liste aujourd'hui ces crates comme « présentes mais pas encore branchées ».
```

#### P0.4 — Harnais de test : Ollama mocké + intégration FS (M)

**Pourquoi** — `FsOps::execute_step` et `undo` sont le cœur mutant de l'app et n'ont **aucun test**
(seuls `list_dir`/`stat`/`expand_tilde` en ont). Le test LLM exige un serveur réel. Impossible de
refactorer la Phase 1 sereinement dans ces conditions.

**Critères d'acceptation**
- `wiremock` (ou équivalent) pour `OllamaClient` : health check, `complete`, `stream_chat` avec
  chunks NDJSON, y compris un cas `thinking`
- tests d'intégration `FsOps` sur `tempfile::tempdir` : chaque opération + son undo, undo groupé
  multi-étapes, undo dans le désordre inverse, undo d'un WRITE_FILE écrasant un fichier existant
- un test qui reproduit l'échec partiel (étape 2 sur 3 impossible) et documente le comportement
  actuel — il servira de test de non-régression pour P1.4

```
Lito n'a aucun test sur son cœur mutant : `crates/fs/src/lib.rs` teste `list_dir`, `stat` et
`expand_tilde`, mais ni `execute_step` ni `undo`. Et `crates/llm`'s test exige un Ollama vivant.

1. Ajoute `wiremock` en dev-dependency de `crates/llm` et écris des tests qui couvrent
   `health_check`, `complete`, `list_models` et surtout `stream_chat` : le stream NDJSON de
   `/api/chat`, avec un cas contenant un champ `thinking` et un cas sans, plus un cas de stream
   coupé en plein milieu (le client doit remonter une erreur propre, pas paniquer). Retire le
   `#[ignore]` posé en CI sur l'ancien test ou remplace-le par ces tests mockés.

2. Ajoute des tests d'intégration pour `FsOps` (dans `crates/fs/tests/`), sur des `tempfile::tempdir`,
   couvrant pour chaque opération (MKDIR, MOVE, COPY, DELETE, RENAME, SYMLINK, WRITE_FILE) :
   l'effet attendu sur le disque, puis `undo(session_id)` qui restaure exactement l'état d'avant.
   Ajoute aussi : un plan de 3 étapes annulé en une fois et vérifié dans l'ordre inverse ; un
   WRITE_FILE qui écrase un fichier existant puis est annulé (le contenu d'origine doit revenir) ;
   deux sessions d'undo entrelacées où annuler l'une ne touche pas l'autre.

3. Écris un test qui exécute un plan de 3 étapes dont la 2e est impossible (chemin source
   inexistant). Documente par des assertions le comportement ACTUEL — étape 1 appliquée et non
   compensée. Nomme-le explicitement comme un test de constat (`partial_failure_currently_leaves_
   state_applied`) avec un commentaire disant qu'il sera inversé quand l'exécution transactionnelle
   arrivera.

Lance `make test` et montre-moi la sortie complète.
```

---

### Phase 1 — Sécurité, confinement, réversibilité

*C'est LE bloc qui sépare le prototype du produit. Un agent qui peut supprimer `~/Documents` sans
garde-fou n'est pas livrable, quelle que soit la qualité du reste.*

#### P1.1 — Politique de chemins et confinement (L) ⚠️ bloquant

> **Partiellement amorcé le 2026-09-06** par le lot « Exécution de commandes terminal » (entrée 34
> de `history.md`) : le crate `crates/policy` (`lito-policy`) existe, avec la liste des **racines
> autorisées** (`~/Desktop`, `~/Documents`, `~/Downloads`, `~/Pictures`, `lito_policy::default_roots`)
> et la **liste noire dure** (`denylist.rs`) — mais **appliquées uniquement aux commandes
> `RUN_COMMAND`** (`CommandPolicy`/`CommandSpec`), pas encore aux opérations fichier. `ErrorCode::
> PermissionDenied` est désormais réellement construit (refus de politique de commande). Reste à
> faire pour clore P1.1 : le `PathPolicy` + `SafePath` pour `MKDIR`/`MOVE`/`COPY`/`DELETE`/`RENAME`/
> `SYMLINK`/`WRITE_FILE`, câblé dans `FsOps` et à la validation du plan. Le crate est dimensionné
> pour l'accueillir à côté de `CommandPolicy`. **À enchaîner immédiatement** : sans lui, `DELETE`
> peut toujours effacer `~/Documents` alors qu'une commande y est confinée — asymétrie à résorber.

**Pourquoi** — Aujourd'hui `execute_step` fait `expand_tilde` puis exécute. Rien n'empêche
`DELETE /`, `MOVE ~/Library/Keychains`, un `..` de traversée, ou un symlink qui pointe hors
périmètre. Le modèle est local et faillible ; l'utilisateur peut aussi être manipulé par un nom de
fichier piégé lu pendant un LIST.

**Critères d'acceptation**
- nouveau crate `lito-policy` (ou module `fs::policy`) : `PathPolicy::check(op, path) ->
  Result<SafePath, PolicyViolation>`
- canonicalisation avant contrôle (résolution des `..` et des symlinks du parent), refus si le
  chemin résolu sort du périmètre
- périmètre = liste de racines autorisées, persistée en réglage, par défaut `~/Desktop`,
  `~/Documents`, `~/Downloads`, `~/Pictures`
- liste noire dure, non contournable même si l'utilisateur ajoute la racine : `/System`, `/Library`,
  `~/Library`, `/private`, `/bin`, `/usr`, `/etc`, `.git`, `.ssh`, le dossier de données de Lito
  lui-même
- refus des opérations sur un symlink existant (on n'écrit jamais « à travers »)
- appliqué **au moment de la validation du plan** (avant affichage à l'utilisateur) *et* à
  l'exécution — jamais uniquement dans le prompt
- toute violation produit un `ErrorCode::PermissionDenied` avec le chemin fautif, affiché sur
  l'étape concernée dans le panneau de plan
- tests exhaustifs : traversal, symlink pointant dehors, chemin relatif, `~` mal formé, chemin
  Unicode/NFD, casse (APFS insensible à la casse par défaut)

```
Lito exécute aujourd'hui n'importe quel chemin produit par le modèle : `FsOps::execute_step`
(crates/fs/src/lib.rs) fait un `expand_tilde` puis appelle directement tokio::fs. Rien n'empêche
`DELETE ~/Library`, une traversée `../..`, ou un symlink qui sort du périmètre. C'est le blocage
n°1 avant toute mise en production.

Implémente une politique de chemins :

1. Crée `crates/policy` (`lito-policy`) exposant `PathPolicy` avec
   `check(&self, op: Operation, path: &Path) -> Result<SafePath, PolicyViolation>`. `SafePath` est un
   newtype qui ne peut être construit que par la politique — fais en sorte que `FsOps` n'accepte
   plus que des `SafePath`, pour que le confinement soit garanti par le typage et pas par la
   discipline d'appel.
2. Règles : canonicalise le chemin (résous `..` et les symlinks du parent) AVANT tout contrôle, puis
   vérifie qu'il est sous une des racines autorisées. Racines par défaut : ~/Desktop, ~/Documents,
   ~/Downloads, ~/Pictures, persistées via `MemoryStore::set_setting`. Liste noire dure et non
   contournable même si l'utilisateur ajoute la racine : /System, /Library, ~/Library, /private,
   /bin, /usr, /etc, tout composant `.git` ou `.ssh`, et le répertoire de données de Lito lui-même.
   Refuse toute opération dont le chemin final EST un symlink.
3. Branche la vérification à deux endroits : à la validation du plan dans `handle_send_message`
   (avant d'envoyer `PlanReady` à l'UI — un plan qui viole la politique doit être présenté comme
   invalide, pas exécuté puis rejeté) et dans `execute_step`. Ne compte jamais sur le system prompt
   pour faire respecter ça.
4. Une violation doit produire `ErrorCode::PermissionDenied` avec le chemin fautif et la règle
   violée, remonté jusqu'à l'étape concernée du panneau de plan (charge à toi de voir avec les
   messages `CoreToUiMsg` existants ; si l'UI doit changer, charge d'abord la skill `slint` et
   vérifie le rendu avec `make ui-shot`).
5. Tests : traversal, symlink pointant hors périmètre, chemin relatif, `~` mal formé, Unicode NFD vs
   NFC (macOS normalise), et sensibilité à la casse sur APFS.

Mets à jour `documentation/lito/llm_brief/summary.md` (nouvelle crate + nouveau flux de validation)
et le CLAUDE.md (layout des crates).

Prompts de test à me proposer une fois fini : un qui doit passer (« range mes captures d'écran du
bureau dans un dossier Captures ») et un qui doit être refusé proprement (« supprime le dossier
Library de mon dossier utilisateur »).
```

#### P1.2 — Niveaux de risque et confirmation graduée (M)

> **Partiellement couvert le 2026-09-06** pour `RUN_COMMAND` uniquement : le lot « Exécution de
> commandes terminal » introduit un système de **modes d'approbation** (`validation` /
> `auto-restrictif` / `auto-unsafe`, panneau Réglages, persistés), une **confirmation par commande**
> avec case « mémoriser », et un marquage visuel « non annulable » distinct de l'accent. Le
> `RiskLevel` par `Operation` pour les opérations fichier, le chiffrage réel de l'impact
> (nb fichiers / volume / dossiers) et le « mode confiance » pour les non-destructives restent à
> faire.

**Pourquoi** — Toutes les étapes ont le même poids dans l'UI : créer un dossier et supprimer 400
fichiers demandent le même clic « Exécuter ». Le standard du marché (Cursor, Claude Code, Warp)
c'est un tiering : lecture < création < mutation < destruction.

**Critères d'acceptation**
- chaque `Operation` porte un `RiskLevel` (Safe / Reversible / Destructive)
- l'UI marque visuellement les étapes destructives (règle « accent = CTA uniquement » de
  `design.md` : utiliser une couleur de danger dédiée, pas l'accent)
- un plan contenant du destructif exige une confirmation distincte (bouton différent, ou case à
  cocher « je comprends que N fichiers seront supprimés »)
- le récapitulatif chiffre l'impact : nombre de fichiers, volume total, dossiers touchés — calculé
  réellement, pas estimé par le modèle
- réglage « mode confiance » qui abaisse la confirmation pour les opérations non destructives,
  explicitement opt-in

```
Dans Lito, toutes les étapes d'un plan se valident avec le même bouton « Exécuter », qu'il s'agisse
d'un MKDIR ou de la suppression de 400 fichiers. Introduis une graduation du risque.

1. Dans `crates/protocol`, ajoute un `RiskLevel { Safe, Reversible, Destructive }` et une méthode
   `Operation::risk()`. DELETE et un WRITE_FILE qui écrase un fichier existant sont Destructive ;
   MOVE/RENAME/COPY/SYMLINK sont Reversible ; MKDIR est Safe. Note que le risque de WRITE_FILE
   dépend de l'existence de la cible, donc il se calcule sur le plan résolu, pas sur l'opération
   seule.
2. Avant d'envoyer `PlanReady`, calcule un vrai récapitulatif d'impact en interrogeant le
   filesystem : nombre de fichiers touchés, volume total en octets, dossiers concernés, et combien
   d'entre eux seront supprimés ou écrasés. Ne demande pas ces chiffres au modèle.
3. Côté UI (charge la skill `slint` d'abord) : affiche ce récapitulatif en tête du panneau de plan,
   marque les étapes destructives avec une couleur de danger dédiée — PAS la couleur d'accent, qui
   est réservée au CTA d'après `documentation/lito/design.md`, que tu dois lire et mettre à jour
   avec le nouveau token. Un plan contenant au moins une étape destructive doit exiger une
   confirmation explicite supplémentaire avant que « Exécuter » ne s'active.
4. Ajoute un réglage persisté « ne plus demander pour les opérations réversibles », opt-in,
   désactivé par défaut.

Vérifie le rendu avec `make ui-shot` en clair ET en sombre, et regarde les PNG avant de conclure.
Ajoute une fixture avec un plan destructif dans `crates/ui/ui/fixtures/` et pense à garder les
fixtures synchronisées avec les nouvelles propriétés.

Prompts de test : « supprime tous les fichiers .zip de mes téléchargements » (doit exiger la
confirmation renforcée) et « crée un dossier Factures 2026 dans Documents » (ne doit pas).
```

#### P1.3 — Corbeille macOS au lieu des backups `.bak` (S)

**Pourquoi** — `FsOps::delete` renomme le fichier en sibling `.bak.<uuid>` via `with_extension()`,
ce qui **remplace l'extension** (`photo.jpg` → `photo.bak.<uuid>`, illisible), pollue le dossier de
l'utilisateur, et n'est jamais purgé. Une suppression doit aller dans la Corbeille : c'est
l'affordance que l'utilisateur connaît, et c'est réversible en dehors de Lito.

**Critères d'acceptation**
- suppression via la Corbeille macOS (crate `trash`, ou `NSFileManager.trashItem` via `objc2`)
- l'undo restaure depuis la Corbeille quand c'est possible, sinon explique clairement que le fichier
  y est et donne le chemin
- les backups temporaires restants (WRITE_FILE écrasant un fichier) vont dans un répertoire de
  travail de Lito, **pas** à côté du fichier d'origine, et sont purgés selon une politique
  d'ancienneté
- migration : détecter et proposer le nettoyage des `.bak.<uuid>` laissés par les versions
  précédentes

```
`FsOps::delete` (crates/fs/src/lib.rs) implémente la suppression par un renommage en sibling :
`path.with_extension(format!("bak.{uuid}"))`. Trois problèmes : `with_extension` REMPLACE l'extension
(donc `photo.jpg` devient `photo.bak.<uuid>`, non identifiable), les backups sont créés dans le
dossier de l'utilisateur, et ils ne sont jamais purgés.

1. Remplace la suppression par un envoi vers la Corbeille macOS. Évalue la crate `trash` vs un appel
   direct à `NSFileManager.trashItem` via `objc2`, et dis-moi ce que tu choisis et pourquoi
   (contrainte : ça doit fonctionner dans une app sandboxée si on va vers le sandbox App Store plus
   tard). L'entrée d'undo doit stocker de quoi retrouver l'élément.
2. Pour les backups qui restent nécessaires (WRITE_FILE écrasant un fichier existant), écris-les
   dans un répertoire de travail de Lito sous `dirs::data_dir()`, avec le nom d'origine préservé et
   un index, jamais à côté du fichier source. Ajoute une purge par ancienneté (par ex. 30 jours) au
   démarrage, avec la durée en constante documentée.
3. Écris une migration one-shot au démarrage : détecte les fichiers `*.bak.<uuid>` laissés par les
   versions précédentes dans les racines autorisées, et propose leur nettoyage à l'utilisateur
   plutôt que de les supprimer d'office.
4. Adapte les tests d'intégration de `crates/fs` : un DELETE doit être vérifiable comme « plus à
   l'emplacement d'origine » sans supposer un sibling `.bak`.

Attention à ne pas régresser l'undo : `UndoOperation::Delete` et `WriteFile` stockent aujourd'hui un
chemin de backup, ces variantes vont devoir changer de forme.
```

#### P1.4 — Exécution transactionnelle : pré-vol + rollback (L)

> **Partiellement amorcé le 2026-09-06** : `handle_execute_plan` ne fait plus un `?` nu sur l'échec
> d'une étape — un refus de politique, une approbation refusée, un code de sortie non nul ou une
> erreur fs émettent `CoreToUiMsg::StepFailed { step, total, message }`, arrêtent la boucle, et
> passent le plan en statut `"failed"` en base (plus de blocage en `"executing"`). Reste à faire :
> le **pré-vol** (validation en bloc avant la 1ʳᵉ étape) et le **rollback automatique** des étapes
> déjà appliquées — sachant qu'un `RUN_COMMAND` déjà exécuté ne peut pas être rollbacké (il est
> journalisé dans `undo_log` et compté comme irréversible par `handle_undo`).

**Pourquoi** — `handle_execute_plan` boucle sur les étapes avec un `?`. Une étape 3/5 qui échoue
laisse 1 et 2 appliquées, le plan en statut `executing` pour toujours, et l'utilisateur devant un
état qu'il ne comprend pas. C'est le pire scénario possible pour un outil de fichiers.

**Critères d'acceptation**
- **pré-vol** : avant d'exécuter la moindre étape, valider tout le plan (politique de chemins,
  existence des sources, collisions de cibles, espace disque, permissions) et refuser en bloc si
  quelque chose ne va pas
- **rollback** : en cas d'échec malgré le pré-vol, annuler automatiquement les étapes déjà
  appliquées et remonter un état final cohérent
- si le rollback lui-même échoue, l'état est nommé explicitement dans l'UI avec la liste précise de
  ce qui a été fait et pas défait
- statut de plan `failed` / `rolled_back` en base, plus jamais bloqué en `executing`
- reprise au démarrage : un plan trouvé en `executing` (crash pendant l'exécution) est détecté et
  proposé au rollback

```
`handle_execute_plan` dans crates/app/src/main.rs exécute les étapes d'un plan en boucle avec un `?`
sur `fs_ops.execute_step`. Si l'étape 3 sur 5 échoue, les deux premières restent appliquées, le plan
reste en statut "executing" à vie, et l'utilisateur se retrouve avec un état partiel. Rends
l'exécution transactionnelle.

1. Pré-vol : avant d'exécuter quoi que ce soit, valide le plan entier — politique de chemins
   (lito-policy), existence de chaque source, collisions sur chaque cible, espace disque suffisant
   pour les COPY, permissions en écriture sur les dossiers parents. Si une étape échoue au pré-vol,
   RIEN n'est exécuté et l'utilisateur voit précisément quelle étape et pourquoi.
2. Rollback : si une étape échoue malgré le pré-vol (course, disque plein, fichier verrouillé),
   annule automatiquement les étapes déjà appliquées via le mécanisme d'undo existant, et remonte à
   l'UI le fait que le plan a été entièrement annulé.
3. Si le rollback échoue lui-même, ne masque pas : remonte un état "partiellement appliqué" nommé,
   avec la liste exacte des étapes appliquées et des étapes non défaites, et le chemin du journal.
4. Ajoute les statuts `failed` et `rolled_back` dans `MemoryStore::update_plan_status` et fais en
   sorte qu'aucun chemin de code ne puisse laisser un plan en `executing`.
5. Au démarrage de `run_core_agent`, détecte les plans laissés en `executing` par un crash et
   propose leur rollback à l'utilisateur.

Inverse le test `partial_failure_currently_leaves_state_applied` ajouté en P0.4 : il doit maintenant
vérifier que l'état est intégralement restauré. Ajoute des tests pour le pré-vol (chaque cause de
refus) et pour un rollback qui échoue à mi-parcours.
```

#### P1.5 — Undo persistant et historique des opérations (M)

**Pourquoi** — L'undo vit dans `FsOps.undo_log: Vec<UndoEntry>`, en RAM. Fermer Lito = perdre la
possibilité d'annuler. `MemoryStore::log_undo` et `get_undo_actions` existent, sérialisent une
action en JSON… et **ne sont appelées nulle part**. La table `undo_log` est vide depuis le premier
jour.

**Critères d'acceptation**
- `UndoOperation` sérialisable, écrite en SQLite à chaque opération, dans la même transaction que le
  changement de statut du plan
- `FsOps` reconstruit son journal depuis SQLite au démarrage
- écran « historique des opérations » : chaque run passé, ce qu'il a fait, bouton annuler, avec
  invalidation si le disque a changé depuis (vérification d'empreinte : taille + mtime)
- rétention configurable, purge des entrées dont les backups ont expiré (cohérent avec P1.3)

```
L'undo de Lito ne survit pas à un redémarrage : `FsOps` garde son journal dans un
`Vec<UndoEntry>` en mémoire. Et `MemoryStore::log_undo` / `get_undo_actions` (crates/memory) sont du
code mort — ils ne sont appelés depuis nulle part, la table `undo_log` est vide.

1. Rends `UndoOperation` (crates/fs) sérialisable et persiste chaque entrée en SQLite au moment où
   elle est journalisée. L'écriture de l'entrée d'undo et la mise à jour du statut du plan doivent
   être atomiques — un crash entre les deux ne doit pas produire un plan "done" sans undo.
2. Au démarrage, `FsOps` reconstruit son journal depuis la base pour les runs encore annulables.
   Décide et documente une politique de rétention.
3. Ajoute un écran "historique des opérations" listant les runs passés (date, session d'origine,
   nombre d'étapes, statut) avec un bouton annuler par run. Avant d'annuler, vérifie que le disque
   n'a pas divergé depuis (compare au minimum taille + mtime des chemins concernés) et refuse
   proprement en expliquant, plutôt que d'écraser un changement fait par l'utilisateur entre-temps.
4. Purge les entrées dont les backups ont expiré, en cohérence avec la politique de P1.3.

Charge la skill `slint` avant de toucher à l'UI, et vérifie le rendu avec `make ui-shot`. L'écran
d'historique est une nouvelle surface : lis `documentation/lito/design.md` d'abord et mets-le à jour,
il documente explicitement ce qui a été délibérément écarté et pourquoi.

Prompt de test : exécuter un plan, quitter l'app, la relancer, et pouvoir toujours annuler ce plan.
```

---

### Phase 2 — Boucle agentique au niveau du marché

*Objectif : que l'agent soit réellement capable, pas seulement capable de proposer.*

#### P2.1 — Tool-calling natif Ollama (L)

**Pourquoi** — `Agent::respond` demande au modèle de renvoyer du JSON brut et devine ensuite s'il
s'agit d'un chat, d'une requête ou d'un plan, avec une heuristique sur le premier caractère
(`looks_like_plan`), un dé-fençage markdown, et un splitter de balises `<think>`. Ollama supporte
maintenant le tool-calling natif dans `/api/chat` : schémas typés, appels structurés, plus de
devinette. C'est aussi ce qui débloque le multi-outil de P2.2.

**Critères d'acceptation**
- `OllamaClient::stream_chat` accepte des `tools` et remonte des `ChatChunk::ToolCall`
- outils déclarés avec un schéma JSON : `list_dir`, `stat`, `propose_plan`
- détection de capacité : si le modèle sélectionné ne supporte pas les tools, repli automatique et
  documenté sur le mode JSON-in-text actuel (à garder, pas à supprimer)
- le buffering « on retient les tokens tant qu'on ne sait pas si c'est un plan » disparaît pour les
  modèles à tools : le texte est du texte
- tests avec un mock qui renvoie des tool calls

```
`Agent::respond` (crates/agent/src/lib.rs) implémente un tool-calling artisanal : le system prompt
demande au modèle de répondre soit en texte, soit par un objet JSON `{"query": ...}`, soit par un
tableau JSON de PlanStep, et le code devine lequel avec `looks_like_plan()` sur le premier caractère,
un dé-fençage markdown, et un ThinkTagSplitter. Ollama supporte le tool-calling natif dans
/api/chat — migre dessus.

1. Étends `OllamaClient::stream_chat` (crates/llm) pour accepter une liste d'outils au format
   attendu par Ollama et pour émettre une variante `ChatChunk::ToolCall { name, arguments }` en plus
   de Content et Thinking. Vérifie la forme exacte de la réponse streamée sur la doc Ollama actuelle
   avant d'implémenter.
2. Déclare trois outils avec leur schéma JSON : `list_dir(path)`, `stat(path)`, et `propose_plan(steps)`.
   `propose_plan` est ce qui remplace « le modèle crache un tableau JSON » : le plan devient un
   appel d'outil structuré.
3. Tous les modèles Ollama ne supportent pas les tools. Ajoute une détection de capacité (interroge
   /api/show pour le modèle sélectionné) et garde le chemin actuel JSON-in-text comme repli
   automatique. Les deux chemins doivent coexister proprement, pas par des `if` éparpillés —
   isole-les derrière une abstraction dans `crates/agent`.
4. Sur le chemin tool-calling, supprime le buffering « je retiens les tokens tant que je ne sais pas
   si c'est un plan » : le texte redevient du texte, streamé immédiatement. Le `StreamingPlanParser`
   (ajouté depuis, voir entrée 27 de `history.md`) disparaît avec lui sur ce chemin — un tool call
   arrive déjà structuré, il n'y a plus de tableau JSON à découper à la main — mais il reste
   nécessaire sur le chemin de repli JSON-in-text, sans quoi les modèles sans support des outils
   perdraient l'affichage progressif du plan. Garde aussi le ThinkTagSplitter, il répond à un autre
   problème (les modèles de raisonnement sans support de template Ollama).
5. Tests avec le mock wiremock de P0.4 : une réponse avec tool call, une avec plusieurs tool calls,
   une avec du texte + tool call, une avec des arguments malformés.

Documente la bascule dans `documentation/lito/llm_brief/summary.md` §4, qui décrit aujourd'hui en
détail la boucle LIST/STAT en JSON-in-text.

Prompts de test : « qu'est-ce qu'il y a dans mon dossier Téléchargements ? » (doit passer par
list_dir) et « range les PDF de Téléchargements dans Documents/PDF » (doit passer par list_dir puis
propose_plan).
```

#### P2.2 — Élargir la boîte à outils : lire, chercher, comprendre (L)

**Pourquoi** — L'agent voit des noms de fichiers et des dates, jamais un contenu. Il ne peut pas
chercher récursivement. Résultat : il ne sait pas faire « classe mes factures par fournisseur » ni
« retrouve le contrat qui parle de résiliation », qui sont exactement les cas d'usage qui
justifient un agent plutôt qu'un Finder.

**Critères d'acceptation**
- `read_file(path, max_bytes)` avec plafond strict et détection binaire
- `find(root, glob, max_depth, max_results)` récursif borné (réintroduire `walkdir`)
- extraction de texte pour PDF/DOCX/XLSX (réintroduire `lopdf`/`docx-rs`/`calamine`), plafonnée
- toute lecture passe par `PathPolicy` au même titre qu'une écriture
- budget global : nombre d'outils par tour, octets injectés dans le contexte, avec troncature
  explicite signalée au modèle
- `MAX_QUERY_ROUNDS` remplacé par un vrai budget (tours **et** tokens)

```
L'agent Lito ne peut inspecter que des métadonnées (LIST/STAT). Il ne lit jamais un contenu et ne
sait pas chercher récursivement, donc il ne peut pas traiter « classe mes factures par fournisseur »
ou « retrouve le PDF qui parle de résiliation ». Élargis la boîte à outils (à faire après P2.1, sur
le tool-calling natif).

1. `read_file(path, max_bytes)` : lecture plafonnée, détection de contenu binaire (ne jamais injecter
   des octets bruts dans le contexte), et signalement explicite au modèle quand le contenu est
   tronqué.
2. `find(root, glob, max_depth, max_results)` : parcours récursif borné avec `walkdir` (à
   réintroduire dans crates/fs, il avait été retiré en P0.3), respectant les exclusions de la
   politique de chemins.
3. Extraction de texte pour les formats bureautiques via `lopdf`, `docx-rs`, `calamine` (mêmes
   remarques de réintroduction). Isole ça dans un module `extract` avec une interface unique
   `extract_text(path, max_chars)` et un repli propre pour les formats non supportés ou les PDF
   scannés (pas d'OCR à ce stade — dis-le explicitement au modèle plutôt que de renvoyer une chaîne
   vide).
4. Toute lecture passe par `PathPolicy` exactement comme une écriture : lire hors périmètre est
   aussi un problème de confidentialité, pas seulement écrire.
5. Remplace `MAX_QUERY_ROUNDS` (constante à 10 dans crates/agent) par un budget explicite : nombre
   d'appels d'outils par tour ET volume total d'octets injectés dans le contexte. Le dernier tour
   informe déjà le modèle que son budget est épuisé (`FINAL_ROUND_DIRECTIVE`) et retombe sur un
   message en clair (`FINAL_ROUND_FALLBACK`) au lieu d'une erreur sèche ; ce mécanisme est à
   généraliser au budget octets, pas à réinventer.

Prompts de test : « trouve tous les PDF de mon disque qui datent de plus de deux ans » et « regroupe
les factures de Documents par nom de fournisseur, en te basant sur leur contenu ».
```

#### P2.3 — Timeouts et budget de contexte (M) — *interruption : faite*

**Pourquoi** — `MAX_HISTORY_MESSAGES = 20` est une troncature naïve qui ne tient pas compte de la
taille réelle des messages (un LIST de 500 entrées mange le budget à lui seul), et rien ne borne la
durée d'une génération.

> **Point 1 livré le 2026-09-02** (entrée 27 de `llm_brief/history.md`) : le bouton « Arrêter »
> existe, `UiToCoreMsg::Cancel` est traité par un `tokio::select!` dans `run_core_agent`, et le
> stream HTTP se ferme réellement — sans `CancellationToken` : dropper le futur suffit, puisque ça
> drop le `Receiver` de `stream_chat`, ce qui fait sortir la tâche émettrice et droppe la réponse
> `reqwest`. Reste le point 2 : un tour interrompu n'est **pas** persisté du tout aujourd'hui
> (`handle_send_message` n'écrit qu'à sa toute fin), donc il disparaît au redémarrage au lieu d'être
> enregistré comme interrompu.

**Critères d'acceptation**
- ~~bouton « Arrêter » visible pendant la génération, qui coupe réellement le stream HTTP~~ ✅
- un run interrompu est enregistré comme tel, et l'historique reste cohérent
- timeouts : connexion, premier token, durée totale — chacun avec un message d'erreur distinct
- budget de contexte en tokens (estimation par un tokenizer approché) plutôt qu'en nombre de
  messages, avec éviction des plus anciens et résumé optionnel

```
`MAX_HISTORY_MESSAGES = 20` (crates/app/src/main.rs) tronque l'historique par nombre de messages sans
regarder leur taille, et aucune génération n'est bornée dans le temps. L'interruption elle-même est
déjà implémentée (bouton « Arrêter » → `UiToCoreMsg::Cancel` → `tokio::select!` dans
`run_core_agent`).

1. Un run interrompu doit laisser un état cohérent et *persistant* : aujourd'hui rien n'est écrit,
   parce que `handle_send_message` ne sauvegarde qu'à sa toute fin et n'y arrive jamais. Enregistre
   le tour partiel comme interrompu dans l'historique SQLite et affiche-le comme tel après un
   redémarrage — sans message fantôme ni session bloquée.
2. Ajoute trois timeouts distincts avec des messages d'erreur différents : connexion à Ollama,
   attente du premier token, durée totale de la génération. Rends-les configurables via les settings
   avec des valeurs par défaut raisonnables.
3. Remplace la troncature par nombre de messages par un budget en tokens. Une estimation
   caractères/4 suffit dans un premier temps si elle est documentée comme telle. Le budget doit
   tenir compte des résultats d'outils réinjectés, qui peuvent à eux seuls saturer le contexte (un
   LIST plafonné à 500 entrées, par exemple).

Si tu touches à l'UI, charge la skill `slint` et vérifie le rendu (`make ui-shot`) en clair et en
sombre.
```

#### P2.4 — Auto-réparation sur erreur d'exécution (M)

**Pourquoi** — Quand une étape échoue, l'erreur va au log et à un toast. Un agent de niveau marché
renvoie l'erreur au modèle et lui laisse une tentative de correction — c'est ce qui fait la
différence entre « ça a raté » et « il s'est débrouillé ».

**Critères d'acceptation**
- après un rollback (P1.4), l'erreur exacte est réinjectée avec le plan d'origine
- une seule tentative de replanification, présentée à l'utilisateur comme un nouveau plan à valider
  (jamais exécutée automatiquement)
- boucle de réparation bornée et traçable dans le run

```
Quand une étape de plan échoue dans Lito, l'erreur finit dans un log et dans un message d'erreur
générique. Ajoute une boucle d'auto-réparation (à faire après l'exécution transactionnelle de P1.4).

1. Après un rollback, réinjecte au modèle : le plan d'origine, l'étape qui a échoué, l'erreur exacte
   du système de fichiers, et l'état actuel du disque aux chemins concernés. Demande-lui un plan
   corrigé.
2. UNE seule tentative de replanification, et le résultat est présenté comme un nouveau plan à
   valider par l'utilisateur — jamais exécuté automatiquement. C'est un point non négociable : la
   règle « rien ne touche le disque sans plan validé » ne souffre pas d'exception pour la réparation.
3. Trace la tentative dans le run (voir P3.2 si déjà fait) : plan initial, erreur, plan corrigé,
   décision de l'utilisateur.
4. Si la réparation échoue aussi, arrête et affiche l'erreur d'origine plus ce qui a été tenté.

Prompt de test : demander un déplacement vers un dossier qui n'existe pas encore — l'agent doit
proposer un plan corrigé qui crée le dossier d'abord.
```

#### P2.5 — Registre de modèles et prompts versionnés (M)

**Pourquoi** — Le modèle par défaut est une chaîne en dur dans `main.rs` (`"qwen3.5:9b"` — à
vérifier, un tag inexistant échoue au premier message sans explication). Rien ne décrit les
capacités par modèle (tools ? thinking ? taille de contexte ?), ni ne versionne le system prompt,
ce qui rendra les évals de la Phase 3 ininterprétables.

**Critères d'acceptation**
- registre déclaratif : par modèle, taille de contexte, support tools/thinking, RAM recommandée,
  paramètres d'échantillonnage
- system prompt sorti du code dans un fichier versionné, avec un identifiant de version enregistré
  sur chaque run
- vérification au démarrage que le modèle sélectionné est bien téléchargé ; sinon état d'erreur
  actionnable (voir P4.1)
- modèle par défaut choisi selon la RAM de la machine

```
Dans crates/app/src/main.rs, le modèle par défaut est la chaîne en dur "qwen3.5:9b". Vérifie d'abord
si ce tag existe réellement dans la bibliothèque Ollama — si non, c'est un échec au premier message
sans explication pour tout nouvel utilisateur. Puis structure la gestion des modèles.

1. Crée un registre déclaratif des modèles supportés (fichier TOML ou constante Rust bien
   documentée) : pour chacun, taille de contexte, support du tool-calling, support du thinking, RAM
   recommandée, paramètres d'échantillonnage. `OllamaClient` doit lire ses paramètres depuis ce
   registre plutôt que d'utiliser des valeurs implicites.
2. Sors le SYSTEM_PROMPT de crates/agent/src/lib.rs dans un fichier versionné (`prompts/system.v1.md`
   ou équivalent, inclus à la compilation), avec un identifiant de version. Cet identifiant doit être
   enregistré sur chaque run : sans ça, les évals de la Phase 3 ne seront pas interprétables.
3. Au démarrage et à chaque changement de modèle, vérifie via /api/tags que le modèle sélectionné est
   effectivement téléchargé. Si non, remonte une erreur typée dédiée (pas ErrorCode::Internal) que
   l'UI pourra transformer en action « télécharger ce modèle ».
4. Choisis le modèle par défaut en fonction de la RAM de la machine, avec un repli documenté.

Mets à jour `documentation/lito/llm_brief/summary.md` §2 (« Modèle LLM »), qui décrit aujourd'hui un
OllamaClient unique avec un modèle en dur.
```

---

### Phase 3 — Qualité mesurable

*C'est l'axe le plus souvent négligé et le plus discriminant. Sans évals, chaque changement de
prompt est une superstition.*

#### P3.1 — Harnais d'évals filesystem (L)

**Pourquoi** — Il n'existe aucun moyen de répondre à « est-ce que ce changement de prompt améliore
Lito ? ». Pour un agent qui produit des plans vérifiables, les évals sont faciles : on part d'une
arborescence connue, on donne une consigne, on compare l'arborescence obtenue à l'attendue.

**Critères d'acceptation**
- format de scénario déclaratif : arborescence de départ, prompt utilisateur, assertions sur l'état
  final (fichiers présents/absents, contenus, chemins), et assertions négatives (rien touché hors
  périmètre)
- runner `make eval` qui monte chaque scénario dans un tmpdir, exécute la boucle complète
  (planification + exécution), compare, et produit un rapport
- au moins 20 scénarios couvrant : rangement par type, par date, renommage en lot, déduplication,
  cas ambigus (l'agent doit demander, pas deviner), cas dangereux (l'agent doit refuser)
- score par modèle et par version de prompt, comparable entre runs, stocké en JSON

```
Lito n'a aucune façon de mesurer si un changement de prompt ou de modèle améliore ou dégrade la
qualité des plans. Construis un harnais d'évals — c'est faisable proprement ici parce qu'un plan
produit un état de filesystem vérifiable.

1. Définis un format de scénario déclaratif (TOML ou YAML, un fichier par scénario dans `evals/`) :
   arborescence de départ à créer, prompt utilisateur, assertions sur l'état final (chemins qui
   doivent exister, chemins qui ne doivent plus exister, contenus attendus) et assertions négatives
   (aucun chemin hors du tmpdir n'a été touché).
2. Écris un runner (`crates/evals` ou un binaire dans crates/app) qui, pour chaque scénario : monte
   l'arborescence dans un tempdir, lance la boucle complète planification + exécution contre un
   Ollama réel, compare l'état final aux assertions, et produit un rapport JSON avec le taux de
   réussite, la latence, le nombre de tours d'outils et le nombre de tokens. Ajoute `make eval`.
3. Écris au moins 20 scénarios répartis sur : rangement par type de fichier, rangement par date,
   renommage en lot avec motif, déduplication, création d'arborescence, et surtout deux catégories
   critiques — les cas AMBIGUS où le bon comportement est de demander une clarification plutôt que
   de deviner, et les cas DANGEREUX où le bon comportement est de refuser (suppression hors
   périmètre, chemin système).
4. Le rapport doit être indexé par modèle ET par version de system prompt (voir P2.5), et
   comparable d'un run à l'autre. Prévois `make eval-compare A B` qui diffe deux rapports.

Documente le tout dans `documentation/lito/evals.md` : comment ajouter un scénario, comment lire un
rapport, et quel score est considéré comme acceptable pour une release.
```

#### P3.2 — Traces de run et export (M)

**Pourquoi** — Quand un plan est bizarre, il n'y a rien à regarder : les logs `tracing` partent sur
stdout et disparaissent. Un agent de niveau marché sait montrer ce qui s'est passé.

**Critères d'acceptation**
- table `runs` : prompt, modèle, version de prompt, appels d'outils avec arguments et résultats
  tronqués, plan produit, décision utilisateur, résultat d'exécution, durée, tokens estimés
- panneau « détails du run » dans l'UI, replié par défaut
- export d'un run en JSON depuis l'UI (base d'un rapport de bug — voir P5.4)
- logs `tracing` écrits aussi dans un fichier rotatif

```
Quand un plan de Lito est mauvais, il n'y a rien à inspecter après coup : les logs tracing vont sur
stdout et sont perdus. Ajoute une traçabilité des runs.

1. Nouvelle table `runs` dans crates/memory : identifiant, session, prompt, modèle, version du system
   prompt, liste des appels d'outils (nom, arguments, résultat tronqué), plan produit, décision de
   l'utilisateur (exécuté / rejeté / interrompu), résultat d'exécution, durée, estimation de tokens.
   Écris-la depuis `run_core_agent`.
2. Ajoute un panneau « détails du run » dans l'UI, replié par défaut, accessible depuis un message
   de l'assistant : la chronologie des appels d'outils et le plan brut. Réutilise le motif du
   panneau de raisonnement existant plutôt que d'inventer une nouvelle interaction.
3. Ajoute un export du run en JSON (bouton dans ce panneau) — ce sera la base du rapport de bug.
4. Configure `tracing-subscriber` pour écrire aussi dans un fichier rotatif sous `dirs::data_dir()`,
   en plus de stdout, avec une rotation quotidienne et une rétention bornée.

Charge la skill `slint` avant de toucher à l'UI, mets à jour les fixtures, et vérifie le rendu.
```

#### P3.3 — Durcir le parsing et fuzzer (S)

**Pourquoi** — `extract_plan`/`extract_query`/`ThinkTagSplitter` traitent une entrée hostile par
nature (sortie de LLM). Ils ont de bons tests unitaires, mais aucun test de robustesse sur des
entrées dégénérées.

**Critères d'acceptation**
- `proptest` ou `cargo-fuzz` sur `ThinkTagSplitter::feed` (découpages arbitraires : la concaténation
  des sorties doit être invariante par découpage)
- fuzz sur `extract_plan`/`extract_query` : jamais de panique, jamais de plan accepté avec des
  champs manquants
- test explicite de troncature UTF-8 dans le streaming (un caractère multi-octets coupé entre deux
  chunks)

```
Le parsing des réponses du modèle dans crates/agent (`extract_plan`, `extract_query`,
`ThinkTagSplitter`, `strip_code_fence`) traite une entrée hostile par nature. Les tests unitaires
existants couvrent les cas nominaux ; ajoute de la robustesse.

1. Ajoute `proptest` : pour `ThinkTagSplitter`, la propriété clé est que le résultat ne dépend pas
   du découpage en chunks — génère un texte avec des balises <think>, découpe-le aléatoirement, et
   vérifie que la concaténation des sorties est identique quel que soit le découpage.
2. Fuzze `extract_plan` et `extract_query` (proptest ou cargo-fuzz) : aucune entrée ne doit
   paniquer, et aucun plan ne doit être accepté avec des champs requis manquants ou une opération
   inconnue.
3. Ajoute un test explicite pour la troncature UTF-8 dans le streaming : un caractère multi-octets
   (accent, emoji) coupé entre deux chunks HTTP. Vérifie le comportement réel de
   `OllamaClient::stream_chat` là-dessus — s'il y a un bug, corrige-le, c'est exactement le genre de
   chose qui casse sur du texte français en production.

Lance `make test` et montre-moi la sortie.
```

#### P3.4 — Évals en CI (S)

**Pourquoi** — Des évals qu'on ne lance pas ne servent à rien.

```
Branche les évals de P3.1 en CI. Contrainte : GitHub Actions n'a pas d'Ollama et les runners macOS
sont lents et chers.

Propose-moi deux options chiffrées et implémente celle que je choisis :
(a) un sous-ensemble de scénarios exécutés sur un petit modèle installé dans le job CI,
(b) des évals lancées uniquement sur un runner self-hosted ou manuellement avant release, avec une
    cible `make eval-ci` et un seuil de régression bloquant.

Dans les deux cas, le résultat doit être comparé au dernier rapport de référence versionné et
échouer si le taux de réussite baisse de plus d'un seuil défini. Documente le seuil et sa
justification dans `documentation/lito/evals.md`.
```

---

### Phase 4 — Produit et expérience

#### P4.1 — Onboarding premier lancement (L)

**Pourquoi** — Aujourd'hui, un utilisateur sans Ollama installé ou sans le modèle téléchargé voit un
message d'erreur. C'est le mur d'entrée d'un produit local-first : il faut le franchir pour lui.

**Critères d'acceptation**
- détection : Ollama installé ? lancé ? modèle présent ?
- installation ou lancement guidés (avec instructions claires si Lito ne peut pas le faire lui-même)
- téléchargement du modèle avec barre de progression réelle (Ollama streame la progression du pull)
- **écran de consentement des dossiers** : l'utilisateur choisit explicitement les racines
  autorisées (P1.1) — c'est l'écran qui rend le confinement compréhensible, pas juste implémenté
- exemples de premiers prompts, adaptés aux dossiers choisis

```
Un nouvel utilisateur de Lito sans Ollama installé, ou sans le modèle téléchargé, se prend un message
d'erreur générique. Construis un onboarding de premier lancement.

Étapes, chacune avec un état visuel clair et une action :
1. Détection : Ollama est-il installé ? lancé ? le modèle sélectionné est-il téléchargé ? Utilise
   /api/tags et la détection du binaire. Chaque cas d'échec a son propre écran avec l'action de
   résolution — jamais un message générique.
2. Si Ollama manque : instructions d'installation, et propose de l'ouvrir/de le lancer si c'est
   faisable depuis l'app. Ne tente pas d'installer silencieusement quelque chose.
3. Téléchargement du modèle avec une vraie barre de progression : /api/pull streame la progression,
   branche-la. Choisis le modèle recommandé selon la RAM détectée (voir le registre de P2.5).
4. Écran de consentement des dossiers : l'utilisateur choisit explicitement les racines auxquelles
   Lito aura accès (les racines de la politique de P1.1). Explique en une phrase que Lito ne pourra
   rien faire en dehors. C'est cet écran qui rend le confinement compréhensible plutôt que
   seulement implémenté — soigne-le.
5. Écran final avec 3 exemples de prompts cliquables, adaptés aux dossiers effectivement choisis.

Charge la skill `slint` d'abord, lis `documentation/lito/design.md` et mets-le à jour (c'est une
nouvelle surface majeure). Rends chaque écran avec `make ui-shot` en clair et en sombre et
regarde-les avant de conclure — ajoute des fixtures dédiées.

Un moyen de rejouer l'onboarding est nécessaire pour le tester : ajoute un flag ou un réglage caché.
```

#### P4.2 — États d'erreur de première classe (M)

**Pourquoi** — `run_core_agent` transforme toute erreur en `ErrorCode::Internal` avec un
`e.to_string()`. `ErrorCode` a pourtant six variantes prévues, inutilisées.

**Critères d'acceptation**
- chaque erreur du core mappée sur le bon `ErrorCode`
- chaque `ErrorCode` a dans l'UI un titre, une explication en français courant, et une action
  (relancer Ollama, télécharger le modèle, ouvrir les réglages de dossiers, réessayer)
- les erreurs ne sont pas des toasts éphémères : elles restent dans le fil de conversation
- aucun message d'erreur ne montre un `Debug` de type Rust à l'utilisateur

```
Dans `run_core_agent` (crates/app/src/main.rs), toutes les erreurs remontent en
`ErrorCode::Internal` avec un `e.to_string()`, alors que `ErrorCode` (crates/protocol) définit six
variantes : OllamaOffline, PlanFailed, FsError, ValidationFailed, PermissionDenied, Internal.

1. Introduis un type d'erreur typé dans le core (thiserror) et mappe chaque cas d'échec réel sur le
   bon ErrorCode. Internal doit devenir le cas rare, pas le cas par défaut.
2. Pour chaque ErrorCode, définis côté UI un titre, une explication en français courant (pas de
   jargon, pas de message technique brut) et une action concrète : relancer Ollama, télécharger le
   modèle manquant, ouvrir les réglages de dossiers autorisés, réessayer.
3. Les erreurs doivent rester dans le fil de conversation, pas disparaître comme un toast :
   l'utilisateur doit pouvoir relire ce qui a raté. Prévois quand même un moyen de voir le détail
   technique (repliable), pour les rapports de bug.
4. Vérifie qu'aucun `Debug` de type Rust ne peut atteindre l'utilisateur.

Charge la skill `slint`, ajoute une fixture avec chaque état d'erreur, et vérifie les rendus.
```

#### P4.3 — Clavier, accessibilité, internationalisation (M)

**Pourquoi** — Toutes les chaînes sont en dur en français, dans le `.slint` **et** dans `main.rs`
(« Nouvelle tâche », « Aujourd'hui », « N étapes », les noms de mois). Une app macOS de niveau
marché a aussi des raccourcis clavier et un support VoiceOver.

**Critères d'acceptation**
- raccourcis : ⌘N nouvelle session, ⌘K recherche de sessions, ⌘↵ exécuter, ⌘Z annuler, Échap arrêter
- propriétés d'accessibilité Slint sur tous les contrôles interactifs, navigation au clavier
  complète
- extraction des chaînes vers un mécanisme d'i18n (Slint supporte gettext), français et anglais
- les dates et pluriels passent par une bibliothèque de locale plutôt que par les constantes en dur
  de `main.rs`

```
Lito a toutes ses chaînes en dur en français, à deux endroits : `crates/ui/ui/app-window.slint` et
`crates/app/src/main.rs` (« Nouvelle tâche », « Aujourd'hui », « Hier », le tableau FRENCH_MONTHS,
« N étapes »). Il n'a ni raccourcis clavier ni support d'accessibilité.

1. Raccourcis clavier : ⌘N nouvelle session, ⌘K recherche de sessions, ⌘↵ exécuter le plan affiché,
   ⌘Z annuler le dernier run, Échap arrêter la génération. Ajoute une aide contextuelle qui les liste.
2. Accessibilité : ajoute les propriétés `accessible-*` de Slint sur tous les contrôles
   interactifs, et vérifie que toute l'app est utilisable au clavier seul (focus visible, ordre de
   tabulation cohérent). Teste avec VoiceOver et dis-moi ce qui ne marche pas.
3. Internationalisation : extrais toutes les chaînes vers le mécanisme gettext supporté par Slint,
   plus un équivalent côté Rust pour les chaînes de main.rs. Fournis fr et en. Remplace le tableau
   FRENCH_MONTHS et le formatage de dates par une bibliothèque de locale, et gère les pluriels
   correctement (« 1 étape » / « 2 étapes »).
4. La langue suit le réglage système, avec possibilité de forcer dans les réglages.

Charge la skill `slint` avant de commencer. Vérifie les rendus dans les deux langues avec
`make ui-shot` — l'anglais est plus court, le français plus long, et les deux doivent tenir sans
troncature ni débordement. Ajoute une fixture avec les chaînes les plus longues.
```

#### P4.4 — Aperçu du plan : diff avant/après (M)

**Pourquoi** — Le panneau de plan liste des étapes ; l'utilisateur doit se représenter mentalement
le résultat. Montrer l'arborescence obtenue est ce qui transforme « je fais confiance » en « je
vois ».

```
Le panneau de plan de Lito liste des étapes textuelles. Pour valider, l'utilisateur doit se
représenter le résultat dans sa tête. Montre-lui plutôt l'état obtenu.

1. Calcule, à partir du plan validé par le pré-vol (P1.4), l'arborescence résultante des dossiers
   touchés, et affiche un diff avant/après : nouveaux fichiers, fichiers déplacés (avec leur origine),
   fichiers supprimés, dossiers créés. Uniquement les dossiers concernés, pas tout le disque.
2. Limite la profondeur et le nombre d'entrées affichées, avec un repli « + N autres » — un plan qui
   touche 500 fichiers ne doit pas produire une liste de 500 lignes.
3. Le diff doit être calculé par le code à partir des chemins du plan, JAMAIS demandé au modèle.

Charge la skill `slint`, lis `documentation/lito/design.md` (l'arbre est une nouvelle famille de
composants — décide s'il rejoint le système de design et documente-le), ajoute une fixture avec un
plan volumineux, et vérifie les rendus en clair et en sombre.
```

#### P4.5 — Surveillance de dossiers et règles (L)

**Pourquoi** — C'est la fonctionnalité qui fait passer Lito d'« assistant qu'on ouvre » à « service
qui travaille ». Elle était prévue dès le départ (`notify` était déjà dans les dépendances). Elle
n'a de sens **qu'après** la Phase 1 : une automatisation qui s'exécute sans surveillance humaine
exige que le confinement et la réversibilité soient irréprochables.

**Critères d'acceptation**
- une règle = un dossier surveillé + une consigne en langage naturel + un déclencheur
- l'exécution automatique n'est possible que pour les opérations non destructives, et seulement si
  l'utilisateur a activé la règle explicitement
- toute action automatique est notifiée, journalisée, et annulable depuis l'historique (P1.5)
- anti-rafale : temporisation, ignorer les fichiers en cours d'écriture, ne jamais réagir à ses
  propres écritures
- les règles se testent à blanc avant activation

```
Réintroduis `notify` dans crates/fs pour implémenter la surveillance de dossiers et les règles
automatiques. À NE FAIRE QU'APRÈS la Phase 1 (politique de chemins, exécution transactionnelle,
undo persistant) : une automatisation qui tourne sans supervision humaine exige que le confinement
et la réversibilité soient irréprochables.

1. Une règle = un dossier surveillé + une consigne en langage naturel + un déclencheur (nouveau
   fichier, fichier modifié). Persiste-les en SQLite, avec une UI de gestion.
2. Sécurité : l'exécution automatique n'est autorisée que pour les opérations non destructives
   (RiskLevel::Safe et Reversible de P1.2). Une règle dont le plan contient du destructif s'arrête et
   demande une validation. Pas d'exception, pas de réglage pour la contourner.
3. Robustesse du watcher : temporisation anti-rafale, ignorer les fichiers encore en cours d'écriture
   (taille instable), et surtout ne jamais réagir aux écritures faites par Lito lui-même — sinon
   boucle infinie.
4. Chaque action automatique produit une notification macOS, une entrée dans l'historique des
   opérations (P1.5), et reste annulable.
5. Mode « test à blanc » : l'utilisateur lance la règle sur le contenu actuel du dossier et voit ce
   qu'elle FERAIT, sans rien exécuter. C'est le seul moyen raisonnable d'avoir confiance avant
   d'activer.

Émets enfin de vrais `CoreToUiMsg::FileCreated`/`FileMoved` — ces variantes existent déjà mais ne
sont jamais envoyées.

Prompt de test : une règle sur ~/Downloads avec la consigne « range les images dans un sous-dossier
Images et les PDF dans un sous-dossier Documents ».
```

---

### Phase 5 — Distribution

#### P5.1 — Identité visuelle et bundle correct (S)

**Pourquoi** — `assets/` est **vide**, alors que `make package` fait `cp -r assets/` et que
`Info.plist` déclare `CFBundleIconFile: AppIcon`. L'app livrée n'a pas d'icône.

```
`assets/` est vide dans le dépôt Lito, alors que `make package` fait `cp -r assets/
target/release/Lito.app/Contents/Resources/` et que `Info.plist` déclare `CFBundleIconFile: AppIcon`.
L'app bundle n'a donc pas d'icône.

1. Produis un `.icns` complet (toutes les tailles requises jusqu'à 1024px, @1x et @2x) à partir d'une
   icône cohérente avec l'identité du produit : Lito vient du grec « litos » (simple, sobre, épuré),
   voir `documentation/lito/design.md` pour la palette. Propose-moi 2 ou 3 directions avant de
   produire l'asset final.
2. Corrige `make package` : il doit échouer bruyamment si un asset attendu manque, plutôt que de
   produire silencieusement un bundle incomplet. Vérifie que le bundle produit se lance vraiment
   depuis le Finder et affiche la bonne icône dans le Dock.
3. Vérifie la cohérence de `Info.plist` : `CFBundleShortVersionString` doit être dérivé de la version
   du crate, pas figé à 0.1.0 en dur dans deux endroits différents.
```

#### P5.2 — Signature, notarisation, permissions macOS (M)

**Pourquoi** — `make package` fait un `codesign --sign -` ad hoc : l'app affiche un avertissement
Gatekeeper à l'ouverture chez quiconque n'est pas toi. `Info.plist` a
`NSAllowsArbitraryLoads: true` (blanket, alors que seul localhost est nécessaire) et aucune usage
description pour les dossiers protégés par TCC — ce qui donne des prompts macOS sans explication.

```
`make package` signe Lito en ad-hoc (`codesign --sign -`), donc Gatekeeper avertit tout utilisateur
autre que moi. Et `Info.plist` a des problèmes : `NSAllowsArbitraryLoads: true` en global alors que
seul localhost:11434 est nécessaire, et aucune usage description pour les dossiers protégés par TCC
(Bureau, Documents, Téléchargements) — donc macOS affiche des demandes d'accès sans explication.

1. Corrige `Info.plist` : remplace `NSAllowsArbitraryLoads` par une exception limitée à localhost
   (`NSExceptionDomains`), et ajoute `NSDesktopFolderUsageDescription`,
   `NSDocumentsFolderUsageDescription`, `NSDownloadsFolderUsageDescription` avec des textes en
   français qui expliquent vraiment pourquoi Lito demande l'accès.
2. Ajoute le hardened runtime et les entitlements nécessaires. Explique-moi les implications :
   notamment si le sandbox App Store est visé un jour, l'accès arbitraire au filesystem demandera des
   security-scoped bookmarks — dis-moi si ça change quelque chose à l'architecture de la politique
   de chemins de P1.1, pour qu'on ne se ferme pas cette porte.
3. Écris un script de release qui signe avec un certificat Developer ID, notarise via `notarytool`,
   agrafe le ticket, et vérifie le résultat avec `spctl -a -vvv`. Les identifiants doivent venir de
   variables d'environnement ou du keychain, jamais du dépôt. Le script doit fonctionner sans
   certificat en mode « dry-run » pour que je puisse le tester avant d'en acheter un.
4. Documente la procédure complète dans `documentation/lito/release.md`, y compris ce que je dois
   obtenir chez Apple et dans quel ordre.

Je n'ai pas encore de compte Apple Developer payant : dis-moi précisément ce qui est bloqué sans, et
ce qui peut être préparé en attendant.
```

#### P5.3 — Mises à jour automatiques (M)

```
Lito n'a aucun mécanisme de mise à jour : un utilisateur qui installe le DMG reste sur sa version.

Compare pour moi Sparkle (standard macOS, mais intégration Objective-C depuis Rust) et une solution
maison basée sur un flux JSON + téléchargement + remplacement du bundle. Recommande-en une avec ton
argument principal, puis implémente-la.

Contraintes non négociables, cohérentes avec la philosophie local-first et privacy-first du produit
(voir `documentation/lito/llm_brief/summary.md` §1) :
- la vérification de mise à jour doit être la SEULE requête réseau sortante de l'app en dehors de
  localhost, et elle doit être désactivable dans les réglages ;
- aucune donnée d'usage, aucun identifiant machine, aucune télémétrie ne part avec ;
- les mises à jour sont vérifiées par signature avant installation ;
- l'utilisateur voit ce qui change (notes de version) avant d'accepter.

Ajoute une entrée dans les réglages pour la fréquence de vérification, dont « jamais ».
```

#### P5.4 — Rapport de bug et diagnostic (S)

```
Quand Lito rate, je n'ai aucun moyen de savoir pourquoi : les logs partent sur stdout et le profil
release a `panic = "abort"`, donc un panic tue l'app silencieusement.

1. Ajoute un hook de panic qui écrit un rapport de crash dans un fichier sous `dirs::data_dir()`
   avant de mourir, et vérifie qu'il fonctionne bien avec `panic = "abort"` (teste-le réellement en
   provoquant un panic dans une build release). Si ce profil empêche le hook de tourner, dis-le-moi
   et propose l'alternative.
2. Au démarrage, si un rapport de crash du run précédent est trouvé, propose à l'utilisateur de le
   consulter ou de l'envoyer — sans jamais rien envoyer automatiquement.
3. Ajoute une commande « Exporter un rapport de diagnostic » qui produit UN fichier contenant :
   version, modèle, version du system prompt, logs récents, dernier run tracé (P3.2), et le rapport
   de crash s'il y en a un. Anonymise les chemins par défaut (remplace le nom d'utilisateur) et
   montre à l'utilisateur le contenu exact avant qu'il ne partage quoi que ce soit.
4. Aucune télémétrie automatique. Le rapport est un fichier que l'utilisateur décide d'envoyer, ou
   pas.
```

#### P5.5 — Processus de release (S)

```
Formalise la release de Lito.

1. Versioning sémantique dérivé d'une source unique (le Cargo.toml du workspace), propagé
   automatiquement dans `Info.plist` et dans l'UI — aujourd'hui la version est écrite en dur à
   plusieurs endroits.
2. `CHANGELOG.md` en Keep a Changelog, avec une cible `make changelog` qui prépare la section à
   partir des commits depuis le dernier tag.
3. Workflow GitHub Actions déclenché sur tag `v*` : build release, package, signature + notarisation
   (P5.2), DMG, création de la GitHub Release avec les notes du changelog et le DMG en asset.
4. Une checklist de release dans `documentation/lito/release.md` : évals passées au-dessus du seuil
   (P3.4), `make ci` vert, rendus UI relus, DMG testé sur une machine propre (ou un compte
   utilisateur neuf), notes de version rédigées.
```

---

## 4. Séquencement recommandé

| Ordre | Bloc | Items | Effort cumulé |
| --- | --- | --- | --- |
| 1 | Fondations | P0.1 → P0.4 | ~1 semaine |
| 2 | **Sécurité et réversibilité** | P1.1 → P1.5 | ~3 semaines |
| 3 | Boucle agentique | P2.1 → P2.5 | ~3 semaines |
| 4 | Qualité mesurable | P3.1 → P3.4 | ~2 semaines |
| 5 | Produit | P4.1 → P4.4 | ~3 semaines |
| 6 | Distribution | P5.1 → P5.5 | ~1,5 semaine |
| 7 | Automatisation | P4.5 | ~1 semaine |

**≈ 15 semaines équivalent temps plein** pour la totalité. P4.5 (surveillance de dossiers) est
volontairement placé en dernier malgré son attrait : c'est la fonctionnalité qui exige le plus de
confiance dans tout ce qui précède.

### Si tu ne fais que cinq choses

1. **P0.1 — git.** Tout le reste suppose de pouvoir revenir en arrière.
2. **P1.1 — politique de chemins.** Sans ça, Lito ne peut pas être installé chez quelqu'un d'autre,
   point.
3. **P1.4 — exécution transactionnelle.** L'échec partiel silencieux est le bug qui fera désinstaller.
4. **P1.5 — undo persistant.** « Annulable » est la promesse centrale du produit, et elle est
   aujourd'hui fausse après un redémarrage.
5. **P3.1 — évals.** Sans elles, chaque amélioration de prompt est une croyance.

---

## 5. Hors périmètre, volontairement

Cohérent avec la démarche de `design.md`, qui documente aussi ce qui a été écarté :

- **Modèles cloud par défaut.** Le local-first est la proposition de valeur. Une option cloud
  explicite, opt-in, pourrait se discuter — mais pas avant que la boucle locale soit excellente.
- **RAG et embeddings** (`fastembed`, `sqlite-vec`). Tentant, mais l'agent ne sait même pas encore
  lire un fichier (P2.2). Indexer sémantiquement avant de savoir lire, c'est l'ordre inverse.
- **Windows / Linux.** Le code a des `#[cfg(windows)]` épars, mais le produit, la distribution et
  les permissions sont pensés macOS. Choisir explicitement plutôt que de porter à moitié.
- **Multi-agent, sous-agents.** Le problème de Lito n'est pas la profondeur de raisonnement, c'est
  la sûreté d'exécution.
- **Plugins / MCP.** Une fois la Phase 2 solide, exposer les outils de Lito via MCP serait cohérent —
  mais chaque outil supplémentaire élargit la surface de risque couverte par P1.1.
