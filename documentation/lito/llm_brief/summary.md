# Résumé de Session — Projet Lito

*Document de référence technique · À utiliser comme prompt d'initialisation pour un LLM*

> **Ce document a été réécrit** pour refléter l'état actuel du dépôt : une seule implémentation
> (Rust + Slint, binaire unique), à la racine du dépôt. L'ancienne architecture SwiftUI + sidecar Rust
> décrite dans les versions précédentes de ce fichier a été **entièrement supprimée** (voir
> `history.md` pour le déroulé historique complet, y compris cette bascule).

-----

## 1. Contexte & Philosophie

**Objectif** : Application desktop macOS d'automatisation de fichiers en langage naturel, ciblant des
utilisateurs non-développeurs.

**Nom du projet** : **Lito** (du grec *Litos* : simple, sobre, épuré). Reflète le minimalisme de l'UI,
la légèreté de l'architecture et l'obsession de l'économie de tokens.

**Principes non négociables :**

- **Local-first** : le traitement IA se fait sur la machine, pas dans le cloud.
- **Privacy-first** : aucune donnée fichier ne quitte l'appareil. `lito-exec` reconstruit
  l'environnement des sous-processus au lieu de l'hériter, pour qu'aucune clé d'API du parent ne
  fuie vers une commande.
- **Sécurité par validation** : l'agent ne touche jamais au disque sans qu'un plan ait été généré et
  affiché au préalable. Pour `RUN_COMMAND` (commande terminal, sans inverse, non analysable), c'est
  renforcé : un système de **modes d'approbation** (`validation` / `auto-restrictif` /
  `auto-unsafe`), une **liste noire dure** incontournable même en `auto-unsafe`, un confinement du
  `cwd` à des racines autorisées (`~/Desktop`, `~/Documents`, `~/Downloads`, `~/Pictures`), et une
  carte d'approbation par commande (`crates/policy`).
- **Undo systématique** : toute opération fichier exécutée est journalisée (undo log en mémoire,
  groupé par session) et peut être annulée. **Exception assumée** : une commande `RUN_COMMAND` n'a
  pas d'inverse ; elle est marquée « non annulable » dans l'UI et `handle_undo` signale combien de
  commandes d'un lot n'ont pas pu être défaites.

**Inspiration** : Goose (Block/Apache 2.0), Raycast, Zed Editor.

-----

## 2. Stack Technique

**Un seul binaire Rust (`lito-app`), un seul process, pas d'IPC.** L'UI Slint et la logique cœur
tournent dans le même process, sur le même runtime Tokio, communiquant par `tokio::sync::mpsc`.

| Crate | Rôle |
| :--- | :--- |
| `crates/app` | Point d'entrée binaire (`main.rs`), câblage des services + boucle `run_core_agent` |
| `crates/ui` | UI Slint (`crates/ui/ui/app-window.slint`) + `ViewModel`/`ViewModelHandle` |
| `crates/protocol` | `Operation`/`PlanStep` (étapes de plan, mutantes, journalisées) et `QueryOp`/`FsQuery`/`FileMetadata`/`QueryResult` (requêtes LIST/STAT, lecture seule, jamais journalisées) ; types hérités de l'ancienne architecture NDJSON non utilisés en pratique |
| `crates/agent` | `Agent::respond()` : prompt + contexte + historique → `AgentResponse` (chat ou `Vec<PlanStep>`) via Ollama, avec un aller-retour LIST/STAT possible en interne (dépend de `lito-fs` pour ça, pas seulement `lito-llm`/`lito-protocol`). Émet des `AgentEvent` (`Token`/`Thinking`/`Querying`/`PlanSteps`) au fil du stream via un callback unique |
| `crates/fs` | `FsOps` : exécution réelle des opérations fichiers + undo log en mémoire, **et** requêtes LIST/STAT en lecture seule (`run_query`, non journalisées). `Operation::RunCommand` n'est **pas** géré ici (son bras `execute_step` renvoie une `Err` explicite) — voir `crates/policy`/`crates/exec` |
| `crates/policy` | `lito-policy` : `CommandPolicy::judge(&CommandSpec) -> Verdict` (`AutoAllow`/`NeedsApproval`/`Denied`) pour les commandes terminal. Ordre : liste noire dure (`denylist.rs`) d'abord (donc `auto-unsafe` ne la contourne pas), puis `cwd` hors racines autorisées, puis le mode. `readonly.rs` = petite allowlist de motifs argv, consultée **seulement** par les commandes de reconnaissance. Aucune UI, aucune écriture disque, entièrement testé. Dimensionné pour accueillir le `PathPolicy` de P1.1 |
| `crates/exec` | `lito-exec` : `run(&CommandSpec) -> CommandOutcome`. Suppose la politique déjà passée ; ne fait qu'exécuter proprement — `stdin` fermé, **environnement reconstruit** (`PATH`/`HOME`/`USER`/`LANG`/`TERM=dumb`), sortie plafonnée à 64 Kio/flux, timeout 120 s puis `kill`. Un code de sortie non nul est un `Ok`, pas un `Err` |
| `crates/llm` | `OllamaClient` : health check, complétion, streaming HTTP vers Ollama (`/api/generate` non-conversationnel et `/api/chat` avec historique `ChatMessage`). `diagnose()` sonde réellement le modèle sélectionné (une requête `/api/chat` bornée à 20 s, `think` off) et renvoie un `OllamaDiagnosis` : `Ok`, `DaemonUnreachable`, `ModelNotFound`, `CloudSignInRequired` (modèle `:cloud`, **erreur d'auth explicite** 401/403/corps), `CloudUnresponsive` (modèle `:cloud` qui ne renvoie rien avant la limite — **ambigu** : démarrage à froid côté hébergé *ou* session non authentifiée ; le message couvre les deux), `ServerError`, `ModelUnresponsive` (modèle **local** bloqué). `is_cloud_model()` détecte le suffixe `:cloud`. `health_check()` ne teste que `/api/tags`, qui réussit même quand le modèle est inutilisable — d'où `diagnose()` |
| `crates/memory` | `MemoryStore` : persistance SQLite des sessions (conversations), des plans et de l'historique de conversation, tous rattachés à une `session_id` (connexion derrière un `Mutex` pour rester `Sync`) |

**Dépendances notables du workspace** : `tokio` (runtime multi-thread), `serde`/`serde_json`,
`slint`/`slint-build` (UI), `rusqlite` (+ `bundled`), `reqwest` (client Ollama), `uuid`, `anyhow`,
`tracing`. Présentes mais **pas encore branchées** : `notify`/`walkdir` (surveillance de dossiers),
`keyring` (stockage de clés API), `fastembed`/`sqlite-vec` (RAG local), `lopdf`/`docx-rs`/`calamine`
(parsing de documents).

### Modèle LLM

`OllamaClient::new(endpoint, "qwen3.5:9b")` — un seul modèle local, utilisé à la fois pour la
planification et le ping de santé. Le routage multi-modèles (petit modèle pour l'autocomplétion,
gros modèle pour la planification, API cloud pour la synthèse de gros volumes) évoqué dans les
premières itérations du projet n'est pas implémenté : il n'y a qu'un `OllamaClient` unique.
L'endpoint lui-même n'est plus codé en dur : il vaut `lito_llm::DEFAULT_BASE_URL`
(`http://localhost:11434`, l'adresse par défaut d'Ollama) sauf si `settings.ollama_endpoint` a été
persisté (panneau « Réglages » de la titlebar — voir §4 et §5), pour le cas où Ollama est exposé sur
une autre machine/port.

-----

## 3. Architecture de fichiers réelle

```
lito/                          # racine du dépôt = racine du workspace Cargo
├── Cargo.toml                  # workspace + dépendances partagées
├── Makefile                    # build / run / check / clippy / test / package / dmg
├── Info.plist                  # métadonnées macOS pour le bundle .app
├── assets/                     # icônes, ressources du bundle
├── crates/
│   ├── app/src/main.rs         # entry point, run_core_agent, handlers
│   ├── ui/
│   │   ├── build.rs            # slint_build::compile("ui/app-window.slint")
│   │   ├── ui/app-window.slint # markup Slint : MainWindow, ChatMessageData, PlanStepData
│   │   ├── ui/icons/*.svg      # icônes d'opération (OpIcon) + chrome générique Lucide (Icon), teintées par Image.colorize
│   │   ├── ui/fixtures/*.json  # données de rendu pour make ui-shot
│   │   ├── ui/pxsample.py      # lecture de pixels exacts dans un rendu PNG
│   │   └── src/
│   │       ├── lib.rs          # mod generated { include_modules! } + re-exports
│   │       ├── app_window.rs   # AppWindow (wrapper Rust du composant Slint)
│   │       └── view_model.rs   # ViewModel / ViewModelHandle / UiState
│   ├── protocol/src/lib.rs     # Operation (dont RunCommand) / PlanStep (dont argv/shell/cwd) + QueryOp/FsQuery/FileMetadata/QueryResult
│   ├── agent/src/lib.rs        # Agent::respond() (chat / query LIST-STAT / commande recon / plan) + trait ReconRunner
│   ├── fs/src/lib.rs           # FsOps::execute_step()/undo() + run_query()/list_dir()/stat()
│   ├── policy/src/{lib,denylist,readonly}.rs  # CommandPolicy::judge, liste noire dure, allowlist de lecture
│   ├── exec/src/lib.rs         # lito_exec::run (tokio::process, env reconstruit, timeout, cap sortie)
│   ├── llm/src/lib.rs          # OllamaClient
│   └── memory/src/lib.rs       # MemoryStore (SQLite)
└── documentation/
    ├── learn/                  # tutoriels (overview, Rust, Slint UI, + 04-10: tour de fichiers pour débutant complet)
    ├── llm_brief/               # ce fichier + history.md
    ├── design.md                # système de design de l'UI (palette, règle de couleur, layout, hors-scope)
    └── slint-docs/              # doc officielle Slint vendored (référence, non maintenue ici)
```

-----

## 4. Communication interne (remplace l'ancien protocole NDJSON)

Plus de socket Unix, plus de pipes stdin/stdout, plus de JSON sur le fil : deux canaux
`tokio::sync::mpsc::unbounded_channel` typés, définis dans `crates/ui/src/view_model.rs`.

### UI → Core (`UiToCoreMsg`)

```rust
enum UiToCoreMsg {
    SendMessage { prompt: String, context: String },
    ExecutePlan { plan_id: String },
    Undo { session_id: String }, // id de session d'undo *filesystem* (FsOps), pas une conversation
    RejectPlan { plan_id: String },
    NewSession,
    SwitchSession { session_id: String },
    SetModel { model: String },
    SetTheme { dark: bool },
    SetEndpoint { url: String }, // endpoint Ollama, depuis le panneau « Réglages »
    Cancel, // arrête la génération en cours (bouton « Arrêter »)
    ApprovalResponse { request_id: String, decision: ApprovalDecision }, // réponse à la carte d'approbation (Deny/Allow/AllowAndRemember)
    SetApprovalMode { mode: String }, // "validation" | "auto-restricted" | "auto-unsafe"
    ForgetAllowlistEntry { key: String }, // révoque une commande mémorisée
    Ping,
    DiagnoseOllama, // lance OllamaClient::diagnose (essaie réellement le modèle, cloud compris) → DiagnosisResult
}
```

### Core → UI (`CoreToUiMsg`)

```rust
enum CoreToUiMsg {
    SessionSwitched { session_id: String, messages: Vec<(String, String, Option<String>)>, plan: Option<RestoredPlan> }, // démarrage + tout switch/nouvelle session
    SessionList { sessions: Vec<SessionListItem> }, // sidebar, envoyé après toute mutation pertinente
    Token { content: String },
    Thinking { content: String }, // trace de raisonnement d'un modèle "thinking" (qwen3, deepseek-r1, ...), séparée de Token
    Querying { description: String }, // l'agent lance une requête LIST/STAT avant de pouvoir répondre (ex. "Listing ~/Desktop")
    PlanStreaming { steps: Vec<PlanStep> }, // les étapes déjà écrites par le modèle, liste complète à chaque émission
    MessageDone,
    Cancelled, // génération interrompue : le tour n'a PAS été persisté
    PlanReady { plan_id: String, steps: Vec<PlanStep> },
    PlanRejected { plan_id: String },
    StepStart { step: usize, total: usize, operation: Operation },
    StepDone { step: usize, total: usize },
    StepFailed { step: usize, total: usize, message: String }, // refus de politique, approbation refusée, code non nul, ou erreur fs → le plan s'arrête, statut "failed" en base
    CommandOutput { step, exit_code: Option<i32>, stdout, stderr, truncated }, // sortie d'un RUN_COMMAND, affichée repliée sous l'étape
    ApprovalRequest { request_id, argv, shell, cwd, resolved_program, origin, rememberable }, // une commande attend la décision de l'utilisateur (carte au-dessus du composer)
    PolicyState { mode: String, allowlist: Vec<AllowlistEntryView> }, // mode + commandes mémorisées, pour le panneau Réglages
    PlanDone { undo_id: String },
    UndoDone { irreversible_commands: usize }, // combien de commandes du lot n'ont pas pu être annulées
    FileCreated { path: String },
    FileMoved { from: String, to: String },
    Pong { ollama: bool, model: String, available_models: Vec<String>, endpoint: String },
    Error { code: ErrorCode, message: String },
    DiagnosisResult { ok: bool, needs_signin: bool, title: String, detail: String }, // résultat du diagnostic Ollama, affiché dans une modale ; needs_signin → bloc « ollama signin »
}
```

`current_session_id` a été renommé `current_undo_id` côté `UiState`/Slint (`current-undo-id`) pour ne
plus être confondu avec une session de conversation, une fois que "session" a pris ce second sens.

Le sens Core → UI passe par une tâche `tokio::spawn`ée (le "pump", démarrée dans
`ViewModelHandle::set_channels`) qui appelle `apply_core_message` pour chaque message reçu, puis
pousse l'état résultant vers la fenêtre Slint via `Weak::upgrade_in_event_loop` — nécessaire parce que
cette tâche ne tourne pas sur le thread UI (voir `documentation/lito/learn/03-slint-ui.md` pour le détail).

Appliquer l'état et le pousser vers l'UI sont deux choses séparées : `apply_core_message` ne fait que
muter `UiState`, et le pump vide tout ce qui est déjà en file dans le même lot avant d'appeler
`sync_to_ui()` **une seule fois**, au plus tous les `MIN_SYNC_INTERVAL` (33 ms). `sync_to_ui` ne
réassigne jamais un modèle sans raison : `messages` est diffé ligne à ligne et muté sur place dans le
`VecModel` posé au premier sync, `plan_steps`/`sessions` ne sont remplacés que si leur empreinte a
changé. Un repeater Slint compare le `ModelRc` entrant **par pointeur** et jette, à toute nouvelle
instance, ses lignes instanciées *et* son `RepeaterLayoutState` (ancre de scroll, hauteur moyenne,
position précédente) — réassigner à chaque token détruisait donc l'ancrage du scroll et les
`TextInput` sous-jacents, avec la sélection de texte en cours.

### Boucle de requêtes filesystem (LIST/STAT) dans `Agent::respond`

`Agent::respond` ne se contente plus de classifier la réponse du modèle en "chat" ou "plan" : le
system prompt lui offre une troisième forme, un objet JSON `{"query": "LIST"|"STAT", "path": "..."}`.
Quand le modèle répond par ça, `respond()` :

1. émet `AgentEvent::Querying` (→ `CoreToUiMsg::Querying`) pour que l'UI affiche ce qui est en train
   d'être inspecté ;
2. exécute la requête via `FsOps::run_query` (LIST = enfants directs d'un dossier, non récursif,
   plafonné à `MAX_LIST_ENTRIES` = 500 ; STAT = métadonnées d'un chemin précis) ;
3. réinjecte le résultat (JSON brut, préfixé `[FILESYSTEM QUERY RESULT]`) comme message "user" dans
   la conversation *interne* à cet appel de `respond()` — jamais dans `history`/SQLite, qui ne voient
   que la réponse finale ;
4. relance une complétion Ollama avec cet historique augmenté.

Ça boucle jusqu'à `MAX_QUERY_ROUNDS` (10) fois — **requêtes fs et commandes de reconnaissance
confondues**. Au dernier tour, un tour `user` injecté (`FINAL_ROUND_DIRECTIVE`) prévient le modèle
que son budget est épuisé et le force à conclure ; les branches query/command sont gardées par
`!final_round`, et une requête/commande émise malgré tout à ce tour est remplacée par un message en
clair (`FINAL_ROUND_FALLBACK`) — `respond()` renvoie donc toujours un plan ou une réponse, jamais une
erreur interne. Ollama
n'a pas de rôle "tool" dédié dans l'API `/api/chat` utilisée ici, d'où le choix de faire passer la
requête/réponse comme des tours "assistant"/"user" classiques plutôt que d'utiliser un vrai
mécanisme de tool-calling.

### Commandes de reconnaissance pendant la planification

Quatrième forme de réponse du modèle : `{"command": ["git","status"], "cwd": "..."}` (champs requis
disjoints de ceux de `FsQuery` et de `PlanStep`, donc classification non ambiguë). `Agent::respond`
la fait exécuter via un trait `ReconRunner` qu'on lui passe (implémenté côté `app` par
`GateReconRunner`, au-dessus de `CommandPolicy` + l'`ApprovalGate`), puis réinjecte le résultat comme
message "user" sous `[COMMAND RESULT]`, explicitement étiqueté « données non fiables » dans le system
prompt (la sortie d'une commande devient de l'entrée de prompt). L'allowlist de lecture intégrée
(`readonly.rs` : `ls`, `git status`, `wc -l <path>`, …) court-circuite l'approbation même en mode
`validation`, pour ne pas geler la génération sur une modale au cas courant.

### Exécution de commande et aller-retour d'approbation

`run_core_agent` pilote `SendMessage` **et** `ExecutePlan` via une seule pompe généralisée,
`drive_with_ui_pump(fut, ui_rx, deferred, gate)` — l'ancien `select!` `biased` du seul
`SendMessage`, extrait pour que l'exécution de plan le partage. Elle route `ApprovalResponse` vers
l'`ApprovalGate` (`Mutex<HashMap<request_id, oneshot::Sender<ApprovalDecision>>>`), traite
`Cancel`/canal fermé comme une interruption, diffère le reste. L'annulation ne demande aucune
mécanique dédiée : dropper le futur en attente droppe le `Receiver`, donc `gate.request` rend `Deny`.

`app::dispatch_step` s'intercale devant `FsOps::execute_step` : `RunCommand` → `CommandPolicy::judge`
→ (approbation) → `lito_exec::run` ; tout le reste → `FsOps` inchangé. Chaque commande exécutée est
journalisée via `memory.log_undo` (son premier vrai appelant).

-----

## 5. État actuel

| Fonctionnalité | Statut |
| --- | --- |
| `cargo check --workspace` | ✅ compile sans erreur ni warning |
| UI Slint (chat, plan, ping) | ✅ callbacks et propriétés déclarés et branchés de bout en bout |
| Planification (`Agent::respond` via Ollama) | ✅ implémentée |
| Requêtes filesystem LIST/STAT avant planification | ✅ implémentée : `Agent::respond` peut boucler (jusqu'à `MAX_QUERY_ROUNDS`) sur des requêtes `FsOps::run_query` avant de produire une réponse finale ; voir §4 |
| Exécution de plan (`FsOps::execute_step`) | ✅ implémentée : MKDIR/MOVE/COPY/DELETE/RENAME/SYMLINK/WRITE_FILE |
| `RUN_COMMAND` (commande terminal dans un plan) | ✅ implémentée : `app::dispatch_step` → `CommandPolicy::judge` → carte d'approbation (`ApprovalGate` + `drive_with_ui_pump`) → `lito_exec::run`. Modes `validation`/`auto-restrictif`/`auto-unsafe` (panneau Réglages, persistés), liste noire dure (`crates/policy/src/denylist.rs`, testée exhaustivement) incontournable même en `auto-unsafe`, `cwd` confiné aux racines autorisées dans tous les modes, allowlist `(argv, racine)` mémorisable. Sortie affichée repliée sous l'étape + réinjectée dans l'historique sous `[COMMAND RESULTS]`. Un échec (refus, code non nul, erreur) arrête le plan en statut `"failed"` |
| Commandes de reconnaissance pendant la planification | ✅ 4ᵉ forme de réponse `{"command":[...],"cwd":"..."}`, exécutée via le trait `ReconRunner`, résultat réinjecté sous `[COMMAND RESULT]` étiqueté « non fiable » ; allowlist de lecture (`readonly.rs`) court-circuite l'approbation |
| Undo par session (`FsOps::undo`) | ✅ implémentée, groupée par `session_id`. Les `RUN_COMMAND` ne sont pas réversibles : `handle_undo` lit la table `undo_log` de SQLite et renvoie `UndoDone { irreversible_commands }` |
| Persistance SQLite (`MemoryStore`) | ✅ sauvegarde/relit les sessions, les plans **et** l'historique de conversation (table `messages`), tous rattachés à une `session_id`. Le message **utilisateur** est écrit *avant* d'appeler le modèle (`handle_send_message`) : une génération qui échoue / expire / est annulée ne fait plus disparaître le tour au redémarrage suivant ; seuls la réponse de l'assistant et le push dans `history` en mémoire sont réservés au succès (`history` est de toute façon rechargé depuis la base au redémarrage) |
| Sessions multiples | ✅ créer/lister/switcher entre conversations (sidebar groupée par date + recherche client-side), titre dérivé du premier message, panneau de plan restauré à la reprise d'une session |
| Historique de conversation multi-tour | ✅ `run_core_agent` charge les `MAX_HISTORY_MESSAGES` derniers tours de la session active au démarrage/switch, les tient à jour en mémoire, les persiste en SQLite, et les envoie au modèle via `OllamaClient::stream_chat` (`/api/chat`) à chaque `SendMessage` |
| Thème sombre | ✅ toggle dans la titlebar, persisté (`settings.dark_mode`), palette dérivée via `Palette.dark` dans `app-window.slint` |
| Endpoint Ollama configurable | ✅ panneau « Réglages » dans la titlebar (badge + popup, même style que le sélecteur de modèle) : champ éditable, « Réinitialiser » (remplit le champ avec `lito_llm::DEFAULT_BASE_URL` sans sauvegarder), « Enregistrer » → `UiToCoreMsg::SetEndpoint`. `handle_set_endpoint` (miroir de `handle_set_model`) met à jour l'`OllamaClient` vivant, persiste dans `settings.ollama_endpoint`, puis relance un ping — pas de mise à jour optimiste de `UiState::ollama_endpoint`, qui n'avance qu'au `Pong` suivant, pour qu'une URL invalide se traduise par « hors ligne » plutôt que par un changement silencieusement accepté |
| Diagnostic de connexion Ollama | ✅ bouton « Diagnostiquer la connexion » dans le panneau « Réglages » → `UiToCoreMsg::DiagnoseOllama` → `handle_diagnose` appelle `OllamaClient::diagnose` (sonde réelle du modèle, ~20 s max) et renvoie `CoreToUiMsg::DiagnosisResult` affiché dans une **modale** (enfant direct de la `Window`, au-dessus d'un scrim). Cas modèle `:cloud` non authentifié → titre « Connexion à Ollama requise » + bloc d'instructions `ollama signin`. Rejoué **automatiquement** (`auto = true`, résultat sain silencieux) après une génération qui échoue, pour que le scénario « aucune réponse, modèle cloud » ouvre la modale sans intervention |
| Rejeter un plan proposé | ✅ `RejectPlan`/`PlanRejected`, distinct de l'undo post-exécution (rien n'a touché le disque) |
| Streaming de tokens (`OllamaClient::stream`/`stream_chat`) | ✅ branché dans le core agent : `handle_send_message` traduit chaque `AgentEvent` en `CoreToUiMsg` et l'UI l'append au dernier message assistant en cours |
| Streaming des **plans**, étape par étape | ✅ `StreamingPlanParser` (`crates/agent`) découpe le tableau JSON retenu au fil de l'eau et émet `AgentEvent::PlanSteps` → `CoreToUiMsg::PlanStreaming` ; le panneau se remplit pendant l'écriture, en statut `"streaming"` (ni `plan_id` ni actions, puisque rien n'est encore persisté). Le JSON brut n'est jamais montré dans le chat. Ne traite que les tableaux de premier niveau : un `{` seul est ambigu (plan d'une étape vs. requête LIST/STAT), le parseur se désarme |
| Indicateur de génération | ✅ deux composants pilotés par `Timer` : `StreamingCaret` (bloc clignotant) sur un flux de tokens commencé puis en pause ; `Spinner` (arc `loader.svg` qui tourne) là où **rien ne stream** — message `system` `streaming` d'une commande de reconnaissance / requête disque en cours (« Exécution en cours… »), message assistant encore vide (« Génération… »), étape de plan `running` (à côté du badge « En cours »). Avant, la ligne `Commande : …` restait figée sans indicateur pendant tout l'aller-retour. Côté état : `CoreToUiMsg::Querying` pousse le message `system` avec `streaming: true`, éteint par `settle_query_spinners` (reprise du modèle) ou `finish_streaming` (fin de tour). Panneau de plan en « PLAN EN COURS » |
| Interruption de la génération | ✅ bouton « Arrêter » → `UiToCoreMsg::Cancel`. La boucle `run_core_agent` attend `handle_send_message` dans un `tokio::select!` (`biased`) à côté du canal, sinon le `Cancel` resterait en file jusqu'à la fin de la génération. Dropper le futur ferme réellement le stream HTTP vers Ollama (le `Receiver` de `stream_chat` disparaît → la tâche émettrice sort → `reqwest` est droppé). Les autres messages reçus pendant la génération sont mis en file (`deferred`) et rejoués. Rien du tour annulé n'est persisté |
| Trace de raisonnement ("thinking") | ✅ `stream_chat` envoie `think: true` à Ollama et distingue `ChatChunk::Thinking`/`ChatChunk::Content` ; `Agent::respond` relaie les morceaux `Thinking` via `AgentEvent::Thinking` vers `CoreToUiMsg::Thinking`, affichés dans un panneau repliable séparé du chat (`crates/ui/ui/app-window.slint`). Sans effet sur les modèles non-"thinking" (le champ est simplement absent). La trace complète du tour est aussi persistée par tour (colonne `thinking` de la table `messages`, `MemoryStore::save_message`/`get_recent_messages`) et rechargée au démarrage via `CoreToUiMsg::SessionSwitched`, donc elle survit à un redémarrage exactement comme le reste du tour |
| Surveillance de dossiers | ❌ non implémentée (dépendances présentes) |
| RAG local / embeddings | ❌ non implémenté (dépendances présentes) |
| Stockage clé API (keyring) | ❌ non implémenté (dépendance présente) |
| Packaging `.app` / DMG | ✅ `make package` / `make dmg` |
| Vérification visuelle de l'UI | ✅ `make ui-check` / `make ui-shot` — rend `app-window.slint` en PNG sans lancer l'app, à partir des fixtures `crates/ui/ui/fixtures/{demo,stress,streaming}.json` (voir entrées 23 et 24 de `history.md`). Serveur MCP Slint (inspection de l'app en cours d'exécution) disponible mais non activé |
| Rendu markdown des réponses | ✅ chaque réponse `assistant` est découpée en blocs de premier niveau côté Rust (`crates/ui/src/markdown.rs::split_blocks`, `pulldown-cmark`) et rendue bloc par bloc : paragraphes et listes via l'élément natif `StyledText` de Slint 1.17, blocs de code / titres / tableaux / citations via `CopiableText` — donc sélectionnables au clic-glisser, ce que `StyledText` ne permet pas. Nécessaire parce que `StyledText::from_markdown` échoue sur **tout** le message dès qu'une construction non supportée apparaît : un seul bloc de code faisait retomber la réponse entière en markdown brut. Actions par message : « Copier » (texte rendu, marqueurs retirés), « Copier le markdown » (source), « Texte brut »/« Rendu » ; plus un « Copier » par bloc de code. Voir `design.md`, section « Rendu markdown » |
| Coloration syntaxique (code + commandes) | ✅ tokenizer maison `crates/ui/src/highlight.rs` (pas de dépendance externe — shell/rust/python/js-ts/json/toml/yaml/C-like + repli générique), classe chaque token en `plain`/`keyword`/`string`/`comment`/`number`/`function`. Colore les blocs de code d'une réponse (`MessageBlock`, kind `"code"`) et la ligne `$ <commande>` d'une étape `RUN_COMMAND` dans le panneau de plan. `Palette.syntax-*` (4 teintes neuves, espacées de `accent`/`success`/`warn`/`danger`) + `global Syntax` (fonctions pures `color`/`italic`/`weight`) dans `app-window.slint`. Nécessaire parce que `TextInput`/`Text` n'ont qu'une seule couleur uniforme et que `StyledText::from_markdown` — malgré la doc amont — ne supporte **pas** `<span style="color:…">` sur Slint 1.17.1 (vérifié, erreur "HTML tag <span> is not supported") : un bloc coloré est donc une rangée de petits `CodeSpan` (`TextInput` mono-ligne) plutôt qu'un seul élément de texte, ce qui sacrifie la sélection clic-glisser continue sur toute la ligne (le bouton « Copier » du bloc copie toujours tout). Voir entrée 42 de `history.md` pour le bug `ListView` + `ScrollView` imbriqué rencontré en route |
| Design system de l'UI | ✅ `global Theme` dans `app-window.slint` : 4 rôles typographiques, rampe d'espacement de 4px, durées d'animation, constantes structurelles partagées ; composants `Label` (accents sur majuscules), `HDivider`/`VDivider` (un seul trait par séparation), `OpIcon`/`Icon` (icônes SVG, opération + chrome générique Lucide), `TextButton`/`IconButton` (action texte ou icône seule avec vraie zone de clic). Voir `design.md` |
| États d'interaction | ✅ survol/enfoncé sur tous les contrôles (rampe `surface-hover`/`surface-press` monotone et mesurée), `mouse-cursor: pointer` sur toute cible cliquable, liseré de focus sur `Composer`/`SearchField`, transitions `Theme.anim-fast`/`anim-med`. Boutons d'action en ghost (sans bordure au repos) ; 8 → 4 `HDivider`. Voir entrée 26 de `history.md`. ⚠️ Pas de focus clavier sur les boutons (aucun `FocusScope`) |

-----

## 6. Prochaines étapes logiques

1. Brancher `notify` dans `lito-fs` pour émettre de vrais `CoreToUiMsg::FileCreated`/`FileMoved`.
2. Ajouter un mock HTTP pour le test `crates/llm::test_ollama_client`, qui requiert aujourd'hui un
   serveur Ollama local pour passer.
3. Design UI : la fenêtre Slint actuelle est fonctionnelle mais minimale (pas de palette de couleurs
   dédiée, pas de layout multi-colonnes) — un travail de direction artistique reste à faire si
   l'objectif produit final le demande.
4. Éventuellement rendre `MAX_HISTORY_MESSAGES` (actuellement une constante en dur dans `main.rs`)
   configurable, et/ou remplacer la troncature "N derniers messages" par un vrai budget de tokens si
   les conversations longues deviennent un problème de latence.

-----

*Fin du résumé — Version v2 (Rust + Slint uniquement)*
