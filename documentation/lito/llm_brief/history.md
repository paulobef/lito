# 📜 Historique de Session — Projet Lito

Ce document résume de manière concise toutes les étapes franchies depuis le début de la Phase 1. Il est destiné à donner un historique immédiat à un autre agent LLM lors de la reprise du projet.

> **Note de 2026-07-25** : les Phases 1 à 2 ci-dessous décrivent l'architecture SwiftUI + sidecar Rust
> (`LitoApp/`, `lito-rust/`, `protocol/messages.json`) telle qu'elle a existé et été développée. Cette
> architecture a été **entièrement supprimée** à la Phase 3 (voir en bas de ce document) au profit de
> l'architecture Rust + Slint qui vivait jusque-là dans `lito-v2/`. Les Phases 1-2 restent ici comme
> historique de ce qui a été construit puis abandonné — ne t'appuie pas dessus pour comprendre l'état
> actuel du code, seule la Phase 3 (et `documentation/lito/learn/`, `documentation/lito/llm_brief/summary.md`)
> décrit l'état courant.

---

## 🛠️ Phase 1 : Fondations (Complétée)

### 1. Structuration et Workspace Cargo
- **Architecture Multi-Crates** : Configuration du workspace Rust dans `lito-rust/Cargo.toml` regroupant les crates `server`, `socket`, `agent`, `fs`, `llm`, et `memory`.
- **Squelettes des Crates** : Création de fichiers d'entrée (`src/lib.rs` et `src/main.rs`) et de fichiers `Cargo.toml` minimaux pour toutes les crates afin de valider la compilation.
- **Moteur Server (Smoke Test)** : Implémentation du point d'entrée `lito-rust/crates/server/src/main.rs` simulant un démon d'arrière-plan (`std::thread::park()`).

### 2. Automatisation du Build (`Makefile`)
- Création du `Makefile` à la racine de `lito/` gérant :
  - `make build-rust` : Compilation release du moteur.
  - `make copy-binary` : Copie sécurisée du binaire `server` vers `LitoApp/LitoApp/Resources/lito-rust`.
  - `make clean` : Nettoyage des artefacts de compilation.

### 3. Application Hôte SwiftUI & Cycle de vie
- **Création du Projet** : Initialisation d'un projet SwiftUI macOS natif nommé `LitoApp` dans le répertoire `lito/LitoApp`.
- **Orchestration AppKit AppDelegate** :
  - Création de `AppDelegate.swift` gérant le cycle de vie du processus sidecar via `Process()`.
  - Lancement transparent du binaire `lito-rust` à l'initialisation (`applicationDidFinishLaunching`) avec passage d'arguments IPC (`--socket /tmp/lito.sock`).
  - Capture asynchrone des flux d'erreur/sortie standard dans le terminal pour le débogage.
  - Arrêt propre immédiat (`SIGTERM`) lors de la fermeture de l'application (`applicationWillTerminate`).
- **Liaison App-Delegate** : Modification de `LitoApp.swift` pour lier le cycle de vie de SwiftUI à l'AppKit `AppDelegate` via `@NSApplicationDelegateAdaptor`.
- **Vue Initiale** : Création d'une `ContentView.swift` de démarrage de base.

### 4. Validation & Intégration
- Compilation Rust réussie.
- Copie du binaire réussie dans l'App Bundle.
- Intégration manuelle du binaire dans les ressources de la cible Xcode.
- L'application SwiftUI lance et contrôle avec succès le sidecar Rust en tâche de fond.

---

## 🛡️ Phase 1.5 : Résolution Sandbox & Build Xcode (Complétée)

### 5. Problème de longueur du chemin socket (SUN_LEN)
- **Erreur** : `Error { kind: InvalidInput, message: "path must be shorter than SUN_LEN" }` — le chemin `/Users/.../Application Support/com.lito.app/lito.sock` dépassait la limite de 108 octets des sockets Unix sur macOS.
- **Correctif `AppDelegate.swift`** : Chemin raccourci vers le dossier `Data` du container sandbox : `~/Library/Containers/paulobef.LitoApp/Data/lito.sock` (~70 octets).
- **Log de debug** : Ajout de la longueur du chemin dans les logs pour vérification.

### 6. Build Rust automatisé dans Xcode (Run Script Phase)
- **Makefile mis à jour** :
  - Variable `CARGO` surchargeable depuis l'environnement (chemin complet vers `~/.cargo/bin/cargo`).
  - Cible `copy-binary` écrit dans `BUILT_PRODUCTS_DIR/GeneratedResources/` (autorisé par le sandbox Xcode).
- **Phase de script d'exécution Xcode** (avant *Copy Bundle Resources*) :
  ```bash
  export CARGO="/Users/paulobef/.cargo/bin/cargo"
  make -C "${PROJECT_DIR}/.." copy-binary BUILT_PRODUCTS_DIR="${BUILT_PRODUCTS_DIR}"
  ```
  - **Output Files** : `$(BUILT_PRODUCTS_DIR)/GeneratedResources/lito-rust` (donne la permission sandbox).
  - **"Based on dependency analysis"** : décoché.
- **Référence de fichier** : `lito-rust` ajouté une fois dans le navigateur de projet (dossier Resources), **sans** copie (« Copy items if needed » décoché), exclu de *Copy Bundle Resources*.
- **Résultat** : Build complet depuis Xcode (⌘B), sidecar Rust compilé et embarqué automatiquement.

### 7. Nettoyage code Rust
- `crates/socket/src/server.rs` : Import inutilisé `Path` supprimé.

---

## 🔌 Phase 2 : Communication Socket Swift ↔ Rust (Complétée)

### 8. Schéma de protocole unifié (`protocol/messages.json`)
- **JSON Schema draft-07** définissant tous les messages `ClientMessage` (Swift→Rust) et `ServerMessage` (Rust→Swift) avec `oneOf` + discriminants `action` / `type`.
- **Définitions réutilisables** : `PlanStep` référencé par `$ref`.
- **Enums typés** : `Operation` (MKDIR, MOVE, COPY, DELETE, RENAME, SYMLINK) et `ErrorCode` (ollama_offline, plan_failed, fs_error, validation_failed, permission_denied, internal).
- **Champs requis** : `model` obligatoire dans `pong`, `code` enum dans `error`.

### 9. Côté Rust — `crates/socket` (Protocol + Server)
**`protocol.rs`** — Types partagés avec sérialisation Serde :
- `Operation` enum avec `strum` (SCREAMING_SNAKE_CASE) pour JSON `"MKDIR"` / `"MOVE"` etc.
- `PlanStep` avec `operation: Operation` typé, `path`/`target` `Option<PathBuf>`.
- `ClientMessage` externally tagged (`tag = "action"`, `rename_all = "snake_case"`).
- `ServerMessage` externally tagged (`tag = "type"`, `rename_all = "snake_case"`).
- `ErrorCode` avec `thiserror::Error` + `strum::EnumString` (snake_case).
- Helpers NDJSON : `ClientMessage::parse_line()` / `ServerMessage::to_line()`.

**`server.rs`** — Serveur Tokio Unix complet :
- `start_server(socket_path: PathBuf)` : bind, cleanup, `accept()` loop, `tokio::spawn` par connexion.
- `handle_connection` : `BufReader::read_line` NDJSON, parse via `parse_line()`, dispatch vers `handle_message`.
- `handle_message` : pattern matching exhaustif sur `ClientMessage` :
  - `Ping` → `Pong { ollama: true, model: "qwen3.5:9b" }`
  - Tous les autres → `Error { code: ErrorCode::*, message: "not yet implemented" }`
- Gestion d'erreurs typée (`Box<dyn Error + Send + Sync>`), logging `tracing`.

**Dépendances ajoutées** : `strum` (derive), `thiserror`.

### 10. Côté Swift — Module `Socket/`
**`SocketMessage.swift`** — Types `Codable` + `Sendable` miroir exact du Rust :
- `Operation` / `ErrorCode` enums `String`-backed (validation compile-time).
- `PlanStep` avec `operation: Operation`, `path`/`target: String?`.
- `ClientMessage` / `ServerMessage` avec `encode(to:)` / `init(from:)` manuels pour matcher le format *externally tagged* de Serde (`action` / `type` + `snake_case`).
- Constructeurs de convenance : `ClientMessage.plan()`, `.execute()`, `.ping()`, etc.

**`UnixSocketClient.swift`** — Client asynchrone NDJSON :
- `connect()` : ouverture `FileHandle` écriture, queues séparées lecture/écriture.
- `waitForSocket(timeout:)` : polling actif pour attendre création fichier socket par Rust.
- `send(_ message:)` : `message.toLine()` → `Data` → `writeQueue.async` → `FileHandle.write`.
- `messages()` → `AsyncThrowingStream<ServerMessage, Error>` : `FileHandle.readInBackgroundAndNotify` + buffer `NSMutableData` + split `\n` + `JSONDecoder` → `yield`.
- `disconnect()` : cleanup file handle.

**`EventBus.swift`** — Pub/Sub thread-safe `@Observable` :
- `publish(_ message: ServerMessage)` : broadcast à toutes les continuations.
- `subscribe()` → `AsyncStream<ServerMessage>`.
- Streams de convenance pré-filtrés : `tokenStream`, `planReadyStream`, `stepStartStream`, `stepDoneStream`, `planDoneStream`, `errorStream`, `pongStream`, `fileCreatedStream`, `fileMovedStream`, `undoDoneStream`.
- `NSLock` interne, continuations indexées par `ObjectIdentifier` + `UUID`.

### 11. Intégration `AppDelegate.swift`
- Nouveaux champs : `socketClient: UnixSocketClient?`, `messageTask: Task<Void, Never>?`.
- `setupSocketClient(socketPath:)` : instancie client, `waitForSocket(10s)`, `connect()`, lance `startMessageListener`.
- `startMessageListener` : `for try await message in client.messages() { EventBus.shared.publish(message) }`.
- `stopRustSidecar()` : `messageTask?.cancel()`, `socketClient?.disconnect()`, puis `process.terminate()`.
- Chemin socket court validé (`~/Library/Containers/paulobef.LitoApp/Data/lito.sock`).

### 12. Validation compilation
- **Rust** : `cargo check -p socket` ✅ (workspace complet ✅)
- **Swift** : Fichiers créés, syntaxe validée (build Xcode à faire)

---

## 📋 Prochaines étapes identifiées à l'époque (jamais réalisées — architecture abandonnée)

Cette table faisait suite à la Phase 2 et planifiait la suite de l'architecture SwiftUI + sidecar.
Elle n'a jamais été exécutée : le projet a basculé sur l'architecture Rust + Slint (Phase 3
ci-dessous) avant que ces étapes soient entreprises. Conservée ici pour mémoire uniquement.

| Priorité | Tâche | Fichiers concernés |
|----------|-------|-------------------|
| 1 | **Rust `server/main.rs`** : remplacer `thread::park()` par `#[tokio::main] async fn main() { socket::start_server(...).await }` + parser arg `--socket` | `crates/server/src/main.rs`, `crates/server/Cargo.toml` |
| 2 | **Swift UI Shell** : `NavigationSplitView` 3 colonnes, translucidité, Sidebar/Chat/Inspector | `LitoApp/Views/`, `LitoApp/ViewModels/` |
| 3 | **Swift Chat ViewModel** : wrapper `@Observable` autour de `EventBus.tokenStream`, `planReadyStream`, `errorStream` | `LitoApp/ViewModels/ChatViewModel.swift` |
| 4 | **Swift Chat Console** : `List` messages, `TextField` input, envoi `ClientMessage.plan()`, streaming tokens | `LitoApp/Views/ChatConsole.swift` |
| 5 | **Rust Agent** : implémenter `agent::plan(prompt, context) -> Vec<PlanStep>` via LLM (Ollama) | `crates/agent/src/lib.rs`, `crates/llm/src/lib.rs` |
| 6 | **Rust FS/Memory** : opérations fichiers + undo-log SQLite | `crates/fs/src/lib.rs`, `crates/memory/src/lib.rs` |

---

## 🦀 Phase 3 : Bascule définitive vers Rust + Slint, suppression de l'architecture Swift (2026-07-25)

### 13. Décision produit

Le projet abandonne l'architecture SwiftUI + sidecar Rust (Phases 1-2 ci-dessus) au profit exclusif
de la réécriture Rust + Slint qui se développait en parallèle dans `lito-v2/`. Raison : un seul
binaire, pas d'IPC, une seule stack à maintenir.

### 14. Suppression de l'ancienne architecture

- Suppression complète de `LitoApp/` (projet Xcode, SwiftUI, AppKit, `Socket/` Swift).
- Suppression complète de `lito-rust/` (workspace Cargo du sidecar : `server`, `socket`, `agent`,
  `fs`, `llm`, `memory` — tous stubs sauf `server`/`socket`).
- Suppression de `protocol/messages.json` (schéma JSON Schema du protocole NDJSON, jamais
  code-généré).
- Suppression de `test-entitlements/` (expérimentation sandbox macOS autonome, sans lien avec les
  builds de l'app).
- Suppression du `Makefile` racine historique (`build-rust`/`copy-binary`/`sign-binary`/`prepare`,
  pensé pour le build Xcode du sidecar).

### 15. Aplatissement de `lito-v2/` à la racine du dépôt

`lito-v2/{Cargo.toml, Cargo.lock, crates/, Makefile, Info.plist, assets/, README.md}` déplacés
directement à la racine de `lito/`. Le dépôt ne contient plus qu'un seul workspace Cargo, à la racine.

### 16. Correction du crate `crates/ui` (ne compilait pas)

Root causes identifiées :
- Le composant Slint généré s'appelait `AppWindow`, entrant en collision avec le wrapper Rust
  `AppWindow` du même nom dans `app_window.rs`.
- `app_window.rs`/`view_model.rs` référençaient un chemin `crate::ui::AppWindow` inexistant (le
  module généré par `slint::include_modules!()` vivait à la racine du crate, pas dans un sous-module
  `ui`).
- `ViewModel` était importé (`lib.rs`, `app_window.rs`) mais jamais défini — seul `ViewModelHandle`
  existait.
- `crates/ui/ui/app-window.slint` ne déclarait **aucune** propriété ni callback : tous les
  `window.on_send_message(...)`, `window.set_is_processing(...)`, etc. côté Rust visaient des méthodes
  qui n'existaient pas.

Correctifs appliqués :
- Renommage du composant Slint en `MainWindow`, isolé dans `mod generated { slint::include_modules!(); }`
  côté Rust (voir `documentation/lito/learn/03-slint-ui.md` pour le détail du pattern).
- `app-window.slint` réécrit avec les propriétés (`is-processing`, `ollama-connected`, `ollama-model`,
  `current-plan-id`, `current-session-id`, `show-plan`, `messages: [ChatMessageData]`,
  `plan-steps: [PlanStepData]`) et callbacks (`send-message`, `execute-plan`, `undo`, `ping`) attendus
  par le code Rust, avec un layout fonctionnel (liste de chat, liste d'étapes de plan, champ de
  saisie).
- Ajout d'un vrai type `ViewModel` (possède le `JoinHandle` de la tâche core agent, `.handle()` rend
  un `ViewModelHandle` clonable, `.shutdown()` abort la tâche).
- `ViewModelHandle::send_message`/`execute_plan`/`undo`/`ping` rendues synchrones (elles ne faisaient
  qu'un `mpsc::UnboundedSender::send`, jamais bloquant) — supprime le besoin de `slint::spawn_local`
  pour les callbacks UI.
- `set_channels` démarre maintenant réellement une tâche (`tokio::spawn`) qui consomme
  `core_to_ui_rx` en boucle et appelle `handle_core_message` — auparavant le receiver était stocké
  mais jamais lu, donc aucune mise à jour de l'UI depuis le core n'arrivait jamais.
- `sync_to_ui` utilise `Weak::upgrade_in_event_loop` plutôt que `.upgrade()` direct, car cette
  fonction est maintenant appelée depuis la tâche `tokio::spawn`ée ci-dessus (pas le thread UI).

### 17. Corrections dans les autres crates, nécessaires pour que `crates/app` compile

- `lito-agent::Agent::plan` : `&mut self` / `llm: &mut OllamaClient` → `&self` / `&OllamaClient`
  (aucun des deux ne détient d'état mutable ; le code appelant passait déjà des références non-`mut`).
- `lito-fs::FsOps` : ajout de `execute_step(step, session_id)` (dispatch par `Operation`) — la
  fonction n'existait pas, `main.rs` l'appelait déjà en anticipation. **Bug corrigé au passage** :
  l'ancien `log_undo` générait un `id` aléatoire par entrée et `undo(session_id)` filtrait sur
  `id == session_id`, qui ne correspondait donc jamais à rien — l'undo était silencieusement un
  no-op. Chaque `UndoEntry` porte maintenant un vrai `session_id` passé explicitement par l'appelant,
  distinct de l'identité de l'entrée.
- `lito-app/main.rs` : `handle_execute_plan` chargeait deux `PlanStep` factices codés en dur au lieu
  de charger le vrai plan sauvegardé — remplacé par `memory.get_plan(&plan_id)` (la méthode existait
  déjà côté `lito-memory` mais n'était pas appelée). `Agent::execute`/`Agent::undo` (jamais définies)
  supprimées des call sites : l'exécution/l'undo passent directement par `FsOps`, `Agent` reste
  cantonné à la planification.
- `lito-memory::MemoryStore` : `conn: Connection` → `conn: Mutex<Connection>`. `rusqlite::Connection`
  est `Send` mais pas `Sync` ; `run_core_agent` garde des `&MemoryStore` à travers des `.await` dans
  une tâche `tokio::spawn`ée, ce qui requiert que le type traversé soit `Sync`. Sans ça,
  `tokio::spawn(run_core_agent(...))` ne compilait pas (`future cannot be sent between threads
  safely`).

### 18. Résultat

`cargo check --workspace` passe sans erreur ni warning depuis la racine du dépôt. Voir
`documentation/lito/llm_brief/summary.md` (réécrit) et `documentation/lito/learn/*.md` (réécrits/renommés :
`03-swift.md` → `03-slint-ui.md`) pour l'état courant détaillé.

### 19. Historique de conversation multi-tour

Avant ce changement, chaque message passait par `OllamaClient::stream` (`/api/generate`), un
endpoint Ollama sans état : un seul `prompt` + `system`, aucune notion de tours précédents. Le
modèle "oubliait" donc tout d'un message au suivant.

- `lito-llm` : ajout de `ChatMessage { role, content }` et `OllamaClient::stream_chat(messages)`, qui
  poste sur `/api/chat` (l'endpoint conversationnel d'Ollama) au lieu de `/api/generate`.
- `lito-agent::Agent::respond` : signature étendue avec `history: &[ChatMessage]` ; construit
  désormais `[system, ...history, user]` et appelle `stream_chat` plutôt que `stream`.
- `lito-memory::MemoryStore` : nouvelle table `messages (id, role, content, created_at)` +
  `save_message`/`get_recent_messages(limit)`.
- `lito-app/main.rs` : `run_core_agent` charge les `MAX_HISTORY_MESSAGES` (20) derniers tours au
  démarrage dans un `Vec<ChatMessage>` local, l'envoie à l'UI une fois via un nouveau
  `CoreToUiMsg::History` (pour que le chat affiché reflète ce dont le modèle se souvient réellement),
  puis à chaque `SendMessage` : persiste le tour user+assistant en SQLite, met à jour le `Vec` en
  mémoire, et le tronque aux `MAX_HISTORY_MESSAGES` derniers messages. Les plans (qui n'ont pas de
  texte brut) sont résumés en une phrase ("Proposed a plan: ...") pour rester référençables dans les
  tours suivants.

### 20. Persistance de la trace de raisonnement ("thinking") par tour

Avant ce changement, `CoreToUiMsg::Thinking` (voir §19 ci-dessus et `07-agent.md`) affichait la trace
de raisonnement du tour en cours, mais rien ne la conservait au-delà de ce tour : ni en mémoire, ni en
SQLite, ni au rechargement d'un historique après redémarrage.

- `lito-memory::MemoryStore` : colonne `thinking TEXT` (nullable) ajoutée à la table `messages`, via un
  `ALTER TABLE ... ADD COLUMN` séparé du `CREATE TABLE IF NOT EXISTS` (qui ne modifie pas une table
  existante) — l'erreur "duplicate column name" qu'il renvoie sur une base déjà à jour est ignorée
  explicitement plutôt que traitée comme une vraie erreur. `save_message` prend désormais
  `thinking: Option<&str>` et `get_recent_messages` renvoie des triplets `(role, content, thinking)`
  au lieu de paires `(role, content)`.
- `lito-ui::view_model` : `ChatMessage` (état UI) gagne un champ `thinking: Option<String>` — remplace
  l'ancien design où `UiState` gardait `thinking`/`is_thinking` comme accumulateur global du tour en
  cours (effacé au tour suivant). `CoreToUiMsg::Thinking` accumule maintenant dans le `thinking` du
  dernier message assistant en streaming, au lieu d'un état séparé — le même patron que `Token` avec
  `content`. `CoreToUiMsg::History { messages: Vec<(String, String, Option<String>)> }` transporte le
  triplet complet pour que les tours rechargés au démarrage affichent aussi leur trace de raisonnement.
- `lito-app/main.rs` : `handle_send_message` accumule les morceaux `Thinking` reçus dans une `String`
  locale au fil du stream, la convertit en `Option<&str>` (`None` si vide) une fois le tour terminé, et
  la passe à `memory.save_message`. La copie en mémoire (`Vec<ChatMessage> history`, envoyée au modèle)
  ne garde que `role`/`content` — `thinking` ne sert qu'à l'affichage et à la persistance, jamais
  renvoyé à Ollama.
- Vérifié par un test dédié dans `lito-memory` (`save_message`/`get_recent_messages` avec `thinking:
  Some(...)`, relu après une fermeture/réouverture de `MemoryStore` pour simuler un redémarrage réel).

### 21. Refonte visuelle de `crates/ui` (phase 1 : design pur, sans nouvelle donnée)

Avant ce changement, `app-window.slint` était une UI purement fonctionnelle héritée de la première
version qui compilait (une seule colonne empilée, widgets `std-widgets` par défaut, couleurs codées en
dur). Un exercice de design séparé (mockup HTML/artifact, non versionné) a convergé vers un style
"Carbon" (IBM) : angles droits, densité forte, palette froide quasi neutre, couleur d'accent réservée
au seul vrai CTA (envoi de message), statuts de plan sur les couleurs sémantiques standard
(vert/jaune/rouge). Cette entrée documente le portage de ce design en Slint — **aucune propriété ni
callback exposé à Rust n'a changé** (`is-processing`, `ollama-connected`, `ollama-model`,
`available-models`, `current-model-index`, `current-plan-id`, `current-session-id`, `show-plan`,
`messages`, `plan-steps`, et les callbacks `send-message`/`execute-plan`/`undo`/`select-model`/`ping`
sont inchangés) — donc aucun changement côté `view_model.rs`/`app_window.rs`.

- **`global Palette`** ajouté en tête de `app-window.slint` : tokens de couleur (`bg`, `surface`,
  `sidebar`, `border`, `text`/`text-muted`/`text-faint`, `bubble-bg`, `accent`/`accent-strong`,
  `success`/`warn`/`danger` + leurs variantes `-soft`) au lieu de couleurs hex éparpillées dans les
  éléments. Thème clair uniquement pour l'instant — un thème sombre est un suivi possible mais n'a pas
  été fait ici (pas de détection d'apparence système câblée).
- **`Button`, `LineEdit` de `std-widgets` retirés** (il ne reste que `ComboBox`/`ListView` de
  `std-widgets`) et remplacés par des composants maison : `Composer` (champ de saisie + bouton d'envoi
  comme un seul groupe bordé, enveloppant un `TextInput` brut plutôt qu'un `LineEdit` — nécessaire pour
  contrôler bordure/fond/angles, que `LineEdit` n'expose pas) et `ActionButton` (bouton bordé
  icône+libellé, teinte `success`/`danger`/`neutral` via `enum ActionTone`, utilisé pour
  Exécuter/Annuler). Raison : le chrome natif de `std-widgets` (coins arrondis, style de plateforme)
  ne peut pas être resurfacé en angles droits via les propriétés exposées — voir la note "std-widgets
  constraints" dans les échanges de design qui ont précédé ce changement.
- **`ChatMessageRow`** restructuré : les messages `role == "user"` restent dans une bulle
  (`Palette.bubble-bg`) alignée à droite ; tout le reste (`assistant`/`system`) s'affiche en texte nu
  sans bulle ni bordure, aligné à gauche (registre "transcript" façon Claude Desktop plutôt que
  bulles bidirectionnelles). `CopiableText` (le `TextInput` en lecture seule existant, pour permettre
  la sélection/copie) est réutilisé tel quel dans les deux cas.
- **`StatusBadge`** ajouté : mappe la chaîne `PlanStepData.status` (déjà `"pending"`/`"running"`/
  `"done"`/`"error"` côté `view_model.rs`, aucun changement de vocabulaire nécessaire) vers
  `Palette.success`/`warn`/`danger` — remplace l'ancien `Text { text: step.status }` brut.
- **Layout** passé d'un empilement vertical unique à une disposition à deux colonnes quand
  `show-plan` est vrai : colonne chat (messages + zone de saisie) à gauche, colonne "inspecteur" de
  plan (étapes + actions Exécuter/Annuler) à droite — les deux alimentées par les mêmes propriétés
  qu'avant (`messages`, `plan-steps`, `current-plan-id`, `current-session-id`).
- Hors scope, car nécessiterait une nouvelle donnée exposée par `view_model.rs` : historique des
  sessions passées (pas de liste de sessions dans l'état actuel, seulement `current-session-id`), un
  vrai callback "rejeter le plan" (seuls `execute-plan`/`undo` existent), et le thème sombre.

### 22. Sessions multiples (2026-08-05)

Les trois points listés comme hors scope à l'entrée 21 ont été implémentés dans le même changement,
sur la base d'une maquette cible fournie par l'utilisateur (sidebar de sessions groupées par date +
recherche, toggle thème sombre, "Rejeter" un plan proposé, chip de plan cliquable dans le chat,
badges d'icône par opération sur les étapes).

- **`lito-memory`** : nouvelle table `sessions` (`id`/`title`/`created_at`/`updated_at`) ; colonne
  `session_id` ajoutée à `messages` et `plans` via le même pattern idempotent `ALTER TABLE` que
  `thinking` ; migration de secours qui regroupe les lignes pré-existantes sans `session_id` dans une
  session "Conversation précédente" plutôt que de les perdre (`MemoryStore::backfill_default_session`,
  vérifié en conditions réelles : la base de dev existante a bien été migrée au premier lancement
  après ce changement). Nouvelles méthodes : `create_session`/`touch_session`/`set_session_title`/
  `list_sessions`/`latest_plan_for_session`/`get_plan_session_id`. `save_plan`/`save_message`/
  `get_recent_messages_for_session` prennent maintenant un `session_id` réel (le paramètre
  `_conversation_id` de `save_plan`, ignoré jusque-là, est devenu le vrai `session_id`).
- **Renommage important** : `UiState.current_session_id`/`current-session-id` (Slint) désignaient en
  réalité l'id de *session d'undo filesystem* (un uuid par exécution de plan, sans rapport avec une
  conversation) — renommés en `current_undo_id`/`current-undo-id` pour ne plus entrer en collision de
  sens avec les nouvelles "sessions" de conversation.
- **Protocole UI↔core** (`crates/ui/src/view_model.rs`) : `CoreToUiMsg::History` remplacé par
  `SessionSwitched { session_id, messages, plan }` (réutilisé au démarrage et à chaque switch, `plan`
  restaure le panneau inspecteur là où la session en était) ; nouveaux `SessionList`/`PlanRejected` ;
  nouveaux `UiToCoreMsg::NewSession`/`SwitchSession`/`RejectPlan`/`SetTheme`. `ChatMessageData`/
  `ChatMessage` portent maintenant `plan_id`/`plan_step_count`, posés uniquement sur le message qui
  vient de proposer un plan dans le run courant (pas reconstruit depuis l'historique persisté) - c'est
  ce qui alimente le chip "Plan proposé" dans le chat.
- **`crates/app/src/main.rs`** : `run_core_agent` garde désormais un `active_session_id` mutable ;
  `derive_title` tronque le premier message utilisateur en titre de session (déclenché quand
  `history.is_empty()` juste avant ce tour - signal gratuit pour "c'est le premier tour de cette
  session", pas de requête DB supplémentaire) ; `format_group_label` calcule "Aujourd'hui"/"Hier"/
  "3 juin" (table de noms de mois français en dur, `chrono` était déjà une dépendance inutilisée du
  crate) ; `session_list_msg` reconstruit la liste triée+groupée après toute mutation pertinente
  (nouveau message, exécution de plan, nouvelle session).
- **`app-window.slint`** : `global Palette` a maintenant une propriété `dark` et chaque token de
  couleur est un ternaire clair/sombre (palette sombre choisie par l'agent, aucune maquette sombre
  fournie - voir le token `dark ? ... : ...` de chaque couleur pour les valeurs). Nouvelle colonne
  sidebar (bouton "+ Nouvelle tâche", `SearchField`, `ListView` de `SessionRow` groupée par
  `group-label`, pied de sidebar avec "Annuler la dernière opération"/"Tout s'exécute en local" -
  remplace le texte d'aide qui était sous le `Composer`). Panneau plan : bouton fermeture, et les
  boutons d'action deviennent conditionnels à `current-plan-status` (`"pending"` → Rejeter/Exécuter,
  `"done"` → Annuler, `"rejected"` → texte "Plan rejeté" sans bouton). Chaque carte d'étape gagne un
  badge carré à gauche (glyphe par `operation`, fond `*-soft` par `status`) en plus du `StatusBadge`
  déjà existant à droite. `show-plan` reste piloté par Rust (`in property`, pas `in-out`) ; l'ouverture
  depuis le chip et la fermeture depuis le panneau passent par `ViewModelHandle::open_plan`/
  `close_plan`, deux mutations d'état purement locales (pas d'aller-retour vers le core) plutôt que par
  une propriété Slint bidirectionnelle - pour éviter qu'un `sync_to_ui` déclenché par un événement sans
  rapport (ex. un token qui arrive) ne réécrase un état que Rust ne connaît pas encore.

### 22bis. Le sélecteur de modèle `ComboBox` remplacé par un badge texte (2026-08-05)

Après l'entrée 22, l'utilisateur a signalé (captures d'écran à l'appui, thème clair) que le
contrôle fusionné statut+sélecteur de la titlebar s'affichait comme une case vide et
anormalement large. Deux causes cumulées, toutes deux dans le `ComboBox` de `std-widgets` :

1. `ComboBox` définit `min-width: max(160px, layout.min-width)` en interne (voir
   `i-slint-compiler::widgets::cupertino::combobox.slint` dans les sources vendues du crate
   `slint`) - largement plus large que ce dont ce titlebar dense a besoin.
2. `sync_to_ui` (`crates/ui/src/view_model.rs`) reconstruisait un `ModelRc`/`VecModel` tout neuf
   pour `available-models` à **chaque** appel - c'est-à-dire à chaque mise à jour d'état, y
   compris chaque token streamé - alors que la liste de modèles ne change qu'au `Pong`/
   `SetModel`. `ComboBoxBase` (dans `widgets/common/combobox-base.slint`) a un handler `changed
   model => { root.reset-current(); }` qui recalcule `current-value` à partir de `current-index`
   à chaque nouvelle instance de modèle assignée ; avec des dizaines de réassignations par
   seconde pendant un stream, la valeur affichée pouvait se retrouver vide/désynchronisée
   (`current-index` et `available-models` n'étant pas garantis appliqués de façon atomique côté
   Slint - `set_available_models` puis `set_current_model_index` sont deux appels séparés dans le
   même closure `upgrade_in_event_loop`).

Correctifs :
- `ViewModelHandle` garde un `model_selector_cache: Arc<Mutex<(String, Vec<String>)>>` et
  `sync_to_ui` ne touche plus `ollama-model`/`available-models`/`current-model-index` que quand ce
  cache montre un changement réel - élimine la cause 2 pour de bon, quel que soit le widget
  utilisé côté Slint.
- Le `ComboBox` a quand même été retiré et remplacé par un badge texte cliquable maison (voir
  "Composants custom" dans `documentation/lito/design.md`) : plus simple, entièrement dans notre
  palette/style, et ça évite complètement la classe de bug côté `ComboBoxBase` (y compris le
  problème connu documenté dans son propre commentaire de code, renvoyant vers
  https://github.com/slint-ui/slint/issues/11970). Le clic fait défiler
  `root.available-models` au suivant plutôt que d'ouvrir un vrai dropdown - acceptable tant qu'il
  n'y a typiquement qu'une poignée de modèles locaux.
- À cette occasion, deux séparateurs (`Rectangle { height: 1px; background: Palette.border; }`,
  même motif que la ligne au-dessus du `Composer`) ont été ajoutés dans la sidebar de sessions,
  entre le bouton "+ Nouvelle tâche" et le champ de recherche, et entre le champ de recherche et
  la liste - la sidebar n'avait auparavant aucune séparation visuelle entre ces trois zones.

### 22ter. Alignement titlebar/sidebar, hauteur du pied de sidebar, et sélecteur en dropdown (2026-08-05)

Trois tours d'itération supplémentaires avec l'utilisateur (captures à l'appui) après 22bis :

- **Titlebar/sidebar** : "Lito" flottait à sa largeur naturelle suivi d'un spacer `stretch:1`, donc
  le bord droit du badge modèle s'alignait *par coïncidence* (dépendant de la longueur du nom du
  modèle affiché) avec le bord droit de la sidebar (220px) en dessous - un quasi-alignement qui
  looked comme un bug plutôt qu'un design intentionnel. Corrigé en donnant à "Lito" une vraie
  cellule de 220px (`Rectangle { width: 220px; ... }`), la même valeur que `width: 220px;` de la
  sidebar - alignement garanti par construction, pas par coïncidence. Une bordure/divider ajoutée à
  cette frontière dans un premier temps a été retirée sur retour utilisateur ("remove this in
  between rectangle") : la largeur de cellule identique suffit, pas besoin d'une ligne visible en plus.
- **Pied de sidebar** : la hauteur calculée à partir du padding+contenu (`### 22bis`) restait
  décalée par rapport au bloc composer du chat, parce qu'elle dépend des métriques de police réelles
  au rendu (non déterministe depuis le code source seul). Remplacé par une `height: 59px;` explicite
  (58px = hauteur exacte du bloc composer, +1px car la bordure du pied de sidebar fait office de
  séparateur alors que le chat a une ligne de 1px séparée) avec `clip: true` comme garde-fou.
- **`Text` sans `vertical-alignment: center`** : plusieurs `Text` ajoutés dans cette session (bouton
  "+ Nouvelle tâche", lignes du pied de sidebar) n'avaient pas ce réglage explicite. Le défaut Slint
  de `TextVerticalAlignment` est `top` (premier variant de l'enum), pas `center` - le texte collait
  donc en haut de sa ligne au lieu d'être centré. Ajouté partout où c'était manquant.
- **Sélecteur de modèle redevenu un vrai sélecteur** : le badge "clic = modèle suivant" de l'entrée
  22bis permettait de *cycler* mais pas de choisir un modèle précis. Remplacé par un petit
  `PopupWindow` maison (une ligne par `available-models`, clic = `select-model`, ligne du modèle
  courant surlignée) ancré sous le badge - toujours pas de `ComboBox` de `std-widgets` (raisons
  inchangées, voir 22bis), mais un vrai menu déroulant plutôt qu'un simple cycle.
- **Mise à jour de la skill `slint`** (`.claude/skills/slint/SKILL.md`) et de `CLAUDE.md` (section
  "UI architecture") avec une checklist "Pixel-perfect layout gotchas" listant ces bugs concrets
  (alignement vertical par défaut, layout imbriqué qui ne remplit pas son parent, `vertical-stretch`
  implicite, tailles internes cachées des `std-widgets`, modèles Slint réassignés sans changement
  réel) et un rappel explicite : cet environnement n'a aucun moyen de capturer une fenêtre macOS
  native, donc toute affirmation visuelle doit rester non confirmée tant que l'utilisateur n'a pas
  revu une vraie capture d'écran.

### 23. Boucle de rendu Slint : on peut enfin voir les pixels (2026-08-05)

Point de départ : `CLAUDE.md` et la skill `slint` affirmaient qu'aucune capture de la fenêtre macOS
n'était possible depuis cet environnement, donc que toute affirmation visuelle devait rester non
vérifiée jusqu'à ce que l'utilisateur regarde. **C'était faux**, et c'était la cause racine des
entrées 22/22bis/22ter : à défaut de pouvoir regarder, on validait le code par lecture, or c'est
précisément ce que la lecture ne sait pas attraper en Slint.

- **`slint-viewer --screenshot`** rend un `.slint` en PNG sans fenêtre, sans app, sans Ollama, sans
  SQLite. **`--load-data <json>`** peuple les propriétés `in`/`in-out` de `MainWindow`, y compris les
  **tableaux de structs** (`messages`, `plan-steps`, `sessions`), et une clé au nom d'un `global`
  règle ce global — `{"Palette": {"dark": true}}` produit la capture en thème sombre.
  `--check` type-check le fichier en ~0,05 s avec les warnings (`padding only has effect on layout
  elements`), bien plus vite que `cargo check`.
- **Ajouté** : `crates/ui/ui/fixtures/{demo,stress}.json` + `README.md`, et les cibles
  `make ui-check` / `make ui-shot` / `make ui-watch` / `make install-viewer`. `ui-shot` écrit
  `target/ui-shots/{demo-light,demo-dark,stress}.png`.
- **Version** : `Cargo.toml` déclare `slint = "1.6"` mais `Cargo.lock` résout **1.17.1** — c'est
  1.17.1 qui compile, et le viewer est épinglé `^1.17` pour parler le même dialecte.
- **Bug trouvé immédiatement** (invisible en revue de code, visible à la première capture) :
  **4 des 7 glyphes d'opération ne s'affichent pas du tout** avec la police système — `▢` MKDIR,
  `⧉` COPY, `✎` RENAME, `▤` WRITE_FILE sont vides (pas même un carré « tofu »), seuls `→`, `×` et
  `↗` sortent. Documenté dans `design.md`, **non corrigé** (choix de design à faire : glyphes de
  substitution ou vraies icônes SVG).
- **Deux gotchas de la skill étaient fausses** et ont été testées puis corrigées :
  - `border-width` **ne décale pas** les enfants — il ne réserve aucune place dans la mise en page.
    Mesuré : enfant nu, enfant-layout et enfant d'un parent sans bordure atterrissent tous à `x=0`
    avec la largeur pleine du parent. Le vrai piège est inverse : la bordure est un trait *intérieur*
    et **un enfant qui remplit le parent avec un `background` la recouvre entièrement** (vérifié :
    bordure rouge 4px + enfant bleu → tout bleu).
  - Un layout imbriqué dans un `Rectangle` **remplit bien** son parent (mesuré `width: 300` sur 300,
    y compris avec un `TouchArea` frère). Si un `alignment: center` ne centre pas, le coupable est un
    *ancêtre* dimensionné par son contenu — remonter l'arbre avec `debug()`.
  - Précision sur le `Text` top-aligné (entrée 22ter) : le piège n'apparaît que quand la *boîte* du
    `Text` est plus haute que son contenu, c'est-à-dire dans un layout ou avec une `height` explicite
    (mesuré `h=90, y=0`, encre en lignes 0–13). Enfant libre d'un `Rectangle`, le même `Text` fait la
    hauteur de son contenu et est centré (mesuré `h=14, y=38`) — d'où la difficulté à le repérer :
    le même extrait est correct à un endroit et faux à l'autre.
- **Aussi ajouté à la skill** : une section « Why your React instincts misfire here » (pourquoi ce
  modèle est bon en React et moins bon en Slint : absence de boucle de retour, a priori CSS
  confiants et faux, pas de reset ni d'échelle de tokens, données de test côté Rust et non dans le
  markup, pas de bibliothèque de composants), un tableau « CSS → Slint false friends », et une
  section tokens s'appuyant sur `demos/energy-monitor/ui/theme.slint` de Slint (tokens typés par
  structs). Constat associé, non corrigé : `app-window.slint` contient **8 tailles de police**
  distinctes (dont `12.8px`, `11.5px`, `9.5px`) et **17 valeurs d'espacement** (dont `0.4px`,
  `0.5px`, `0.6px`) — ces sous-pixels sont des compensations à la main d'un décalage qu'on ne
  pouvait pas voir.
- **Existe aussi, non activé** : Slint 1.17+ embarque un serveur MCP
  (`SLINT_EMIT_DEBUG_INFO=1 SLINT_MCP_PORT=8080 cargo run -p lito-app --features slint/mcp`) qui
  expose l'arbre des éléments, la lecture de propriétés, des captures et la simulation d'entrées sur
  l'app *en cours d'exécution*. `slint/mcp` n'est pas dans `crates/ui/Cargo.toml` — à activer le jour
  où un bug dépend de l'état runtime réel.

### 24. Passe de correction visuelle : alignements, séparateurs, accents, icônes (2026-08-06)

Première session à *utiliser* la boucle de rendu de l'entrée 23 pour corriger plutôt que
constater. Toutes les affirmations ci-dessous sont vérifiées au rendu, plusieurs par
échantillonnage de pixels (`crates/ui/ui/pxsample.py`) ou par `debug()` sur la géométrie calculée,
pas à l'œil sur une image redimensionnée.

Défauts corrigés dans `crates/ui/ui/app-window.slint` :

- **Sélecteur de modèle étiré sur toute la titlebar.** Un `height` posé sur un enfant direct de
  `HorizontalLayout` est respecté mais **aligné en haut**, et `max-height` y est **ignoré**
  (mesuré : `height: 22px` → `y=0`; `max-height: 22px` → `h=50`). Le seul moyen de centrer un
  enfant à hauteur fixe sur l'axe transverse est de l'envelopper dans un
  `VerticalLayout { alignment: center }` (mesuré : centré). Même correctif pour le bouton d'envoi
  du `Composer`, qui était calé trop haut dans son champ (bouton de 26px dans une boîte de 24px).
- **Séparateurs doublés.** La sidebar et le panneau de plan portaient un `border-width: 1px` sur
  le conteneur : la bordure se dessine sur les 4 côtés, donc elle doublait le `Rectangle` de 1px
  de la section voisine (trait de 2px sous la titlebar, côté sidebar — mesuré `#d2d7dc` en y=38
  *et* y=39) et peignait un cadre parasite sur les bords gauche/bas de la fenêtre. Remplacé par
  des composants `HDivider`/`VDivider` : un trait est dessiné une fois et une seule. Un scan des
  trois captures ne trouve plus aucune paire de lignes adjacentes de la couleur de bordure.
- **Accents sur majuscules rognés.** "PLAN PROPOSÉ" s'affichait "PLAN PROPOSE", "Échec" "Echec".
  Cause : la boîte d'un `Text` fait exactement sa `font-size` et les glyphes y sont rognés, donc
  tout signe au-dessus de la hauteur de capitale disparaît — silencieusement, sous femtovg *et*
  sous skia. Corrigé par un composant `Label` (= `Text` + `min-height: font-size * 1.45` +
  centrage vertical) utilisé pour tout le chrome, et par `height: preferred-height + 4px` sur
  `CopiableText` (dont seule la première ligne, calée en `y=0`, était touchée). Slint refuse de
  lier `height` et `min-height` sur le même élément — d'où `min-height`, qui marche aussi pour
  les enfants de layout.
- **Icônes d'opération invisibles** (le bug laissé ouvert par l'entrée 23) : remplacées par de
  vraies icônes SVG dans `crates/ui/ui/icons/`, teintées par `Image { colorize: … }` via le
  composant `OpIcon`. La feature `svg` d'`i-slint-core` est déjà active dans le graphe de
  dépendances de `lito-ui` (`cargo tree -e features -i resvg`), donc rien à ajouter au
  `Cargo.toml`. Deux passes de test de couverture de police (60 + 40 glyphes candidats rendus en
  grille) ont confirmé qu'il n'existait pas de jeu cohérent de substituts : `⧉ ✎ ▤ ⊞ ⟳ ☰ ⧗ ⌂ ✚`…
  sont tous absents de la police système.
- **`StatusBadge` en bannière.** Simple enfant de layout, il s'étirait sur toute la largeur
  restante ; il est maintenant dimensionné sur son propre texte.
- **Pied de sidebar faussement centré.** `HorizontalLayout` sans `alignment` distribue la largeur
  restante aux enfants, et chaque `Text` centre alors son texte dans une cellule surdimensionnée :
  une ligne censée être à gauche apparaissait indentée au milieu. `alignment: start` explicite.
- **"Lito" invisible en thème sombre** : le `Text` de la titlebar n'avait pas de `color` et
  gardait donc le noir par défaut. `Label` porte `color: Palette.text`.
- **Tokens** (le constat non corrigé de l'entrée 23) : un `global Theme` remplace les 8 tailles de
  police par 4 rôles (`font-xs/sm/md/lg` = 10/11/12/13px), les 17 valeurs d'espacement par une
  rampe de 4px (`space-1`…`space-5`), et expose les constantes structurelles partagées entre
  colonnes (`titlebar-h`, `sidebar-w`, `plan-w`, `bottom-bar-h`). Le pied de sidebar et le bloc
  composer lisent tous les deux `bottom-bar-h`, donc leurs `HDivider` tombent sur la même ligne
  (mesuré : y=575 des deux côtés) au lieu de « presque » la même.

Rien n'a changé côté Rust : mêmes propriétés, mêmes callbacks, mêmes structs, donc mêmes
fixtures. `make check` et `slint-viewer --check` passent sans warning.

### 25. Centrage optique des libellés de bouton (2026-08-15)

Signalé par l'utilisateur : le texte du bouton "+ Nouvelle tâche" restait perçu comme "un peu trop
haut" malgré le passage à `Label` (entrée 24). Vérifié par échantillonnage de pixels plutôt qu'à
l'œil, avec une centroïde pondérée par luminosité de l'encre du texte (pas juste son étendue) :

- **"+ Nouvelle tâche"** : centroïde à 0.6-1px au-dessus du centre géométrique du bouton, mesuré de
  façon identique en isolation (fichier de test autonome) et dans le rendu réel — donc pas un
  artefact du contexte, un vrai biais reproductible. Cause : `Text`/`TextInput` avec
  `vertical-alignment: center` centre la *boîte de ligne* du texte (métriques ascent+descent de la
  police), pas l'encre effectivement dessinée. Une chaîne sans descendante (aucune lettre
  g/j/p/q/y — le cas de la quasi-totalité du texte d'interface en français) a son encre confinée à
  la moitié supérieure de cette boîte de ligne, donc son encre paraît décalée vers le haut même
  quand la boîte elle-même est parfaitement centrée. Ce n'est pas un bug de layout : c'est la
  différence entre centrage métrique et centrage optique, un phénomène connu en typographie.
- **"Rejeter"/"Exécuter"/"Annuler"** (`ActionButton`) : même cause, en pire — la centroïde était
  2 à 2.7px trop haute. `ActionButton` empilait un `height: 30px` fixe sur le `Rectangle` avec un
  `padding: Theme.space-2` (8px) sur le `HorizontalLayout` interne, qui étirait ses enfants sur
  l'axe transverse à `30 - 2*8 = 14px` — plus petit que le `min-height` des `Label` à ces tailles de
  police (~15-17px). Le `min-height` ne l'emportait pas sur cet étirement forcé par le layout
  parent, donc le texte était en plus *écrasé* dans une boîte trop petite avant même le biais
  optique. Corrigé structurellement en enveloppant la ligne icône+libellé dans un
  `VerticalLayout { alignment: center }` (comme déjà fait pour le sélecteur de modèle et le bouton
  d'envoi à l'entrée 24), qui laisse la ligne prendre sa hauteur naturelle puis la centre — ça seul
  a ramené le biais au même ordre de grandeur que "Nouvelle tâche" (~1.2px).
- **Correctif final** : `Theme.optical-center-y(parent-height, self-height)`, une fonction du
  `global Theme` qui retourne `(parent-height - self-height) / 2 + 1px` — le centrage géométrique
  habituel, plus un nudge de 1px vers le bas. 1px (pas une valeur sub-pixel) a été choisi parce que
  c'est la valeur entière qui minimise l'erreur mesurée dans les deux sens testés (0px : ~0.6-2.7px
  trop haut ; 1px : ~0.3-0.4px trop bas ; 2px aurait été pire). Appliqué via un binding `y:` sur
  l'*instance* du `Label`, pas sur la définition du composant : `parent` n'est pas accessible à la
  racine d'un composant (même restriction que pour `HDivider`/`VDivider`, voir entrée 24 et la
  skill `slint`), donc la fonction ne peut être invoquée qu'au site d'appel, à l'intérieur du
  `HorizontalLayout` qui définit ce `parent`.
- Après coup, tous les biais mesurés sont sous 0.4px (contre 0.6 à 2.7px avant), sur les deux
  boutons, en clair et en sombre. Ne pas appliquer `optical-center-y` partout par réflexe : la
  correction suppose l'absence de descendante dans le texte réel affiché, donc elle est documentée
  pour un usage ciblé (libellés de bouton/pastille courts et centrés), pas pour le corps de texte.

Note en passant : `Theme.radius` (4px, sur les boutons/champs/puces/bulles, pas sur les panneaux
structurels ni les dividers) et le `border-radius: Theme.radius` associé sur la plupart des
`Rectangle` bordés du fichier ont été introduits entre l'entrée 24 et celle-ci, hors du fil de cette
conversation — non documenté plus en détail ici faute de contexte sur cette itération-là, mais
cohérent avec la règle "accent réservé au CTA" et le reste du système de tokens décrit à l'entrée
24 ; voir `documentation/lito/design.md` pour l'état actuel.

### 26. États d'interaction (survol / enfoncé / focus) et passe de minimalisme (2026-09-02)

Demande de l'utilisateur : rendre l'UI plus agréable et plus réactive via des interactions de
survol et de clic, et profiter de ces interactions pour alléger l'interface (supprimer des
séparateurs). Options proposées avant implémentation ; arbitrages retenus : **tous les
`ActionButton` en ghost** (pas seulement les secondaires) et **lot minimalisme complet**.

État de départ, mesuré et non supposé — `grep` sur `crates/ui/ui/app-window.slint` :
`has-hover` : **0 occurrence**. `animate` : **0 occurrence**. `mouse-cursor` : **0 occurrence**.
`pressed` : 4 occurrences. Autrement dit rien ne réagissait au survol, rien ne s'animait, et aucun
élément cliquable n'affichait le curseur main. Le focus des champs était invisible : `has-focus`
n'était lu que pour masquer le placeholder, donc un champ focalisé et un champ au repos étaient
strictement identiques au pixel près.

**La rampe d'interaction, corrigée par la mesure.** Premier jet : `surface-hover: #f2f4f7`,
`surface-press: #e4e8ed`. Rendu dans un fichier-sonde autonome (matrice repos/survol/enfoncé/actif
côte à côte, puisqu'un `--screenshot` ne peut pas simuler un survol) puis échantillonné avec
`pxsample.py` :

    repos #fbfcfd  →  survol #f2f4f7  →  actif #eef1f4  →  enfoncé #e4e8ed

Deux défauts que la lecture du code ne révélait pas : le survol n'était qu'à 9 unités du fond de
sidebar (à peine perceptible), et surtout la rampe **n'était pas monotone** — le survol tombait
plus clair que la session active, donc pointer une ligne signalait moins d'emphase que la ligne
déjà sélectionnée, et l'enfoncer sautait par-dessus l'état qu'elle allait prendre. Corrigé en
`repos → survol #eef1f4 → enfoncé/sélectionné #e4e8ed`, ≥ 10 unités par palier, monotone sur blanc
comme sur le quasi-blanc de la sidebar. Effet de bord assumé : le survol par défaut vaut exactement
le remplissage standard d'un bloc, donc un contrôle posé *sur* un bloc rempli (le toggle "Afficher"
du bloc de raisonnement) doit décaler ses deux états d'un cran — d'où `hover-bg`/`press-bg` sur
`TextButton`.

**`TextButton`, et pourquoi la zone de clic était le vrai problème.** Six actions texte (`Copier`,
`Texte brut`, `Afficher`, `Sombre`, `Ping`, le `×` du panneau de plan) étaient un `Label` nu avec un
`TouchArea` *à l'intérieur* : la région cliquable valait exactement la boîte du glyphe, quelques
pixels de haut, et le `×` faisait 14px de large. Y coller un fond de survol tel quel aurait produit
un aplat à la taille exacte du texte, qui se lit comme un bug de rendu et pas comme un bouton. D'où
un composant unique avec padding, curseur, fond de survol et transition — septième fois que ce
fichier gagne à envelopper une primitive une fois plutôt qu'à la redécliner (cf. `Label`,
`CopiableText`, `HDivider`, `OpIcon`).

Conséquence d'alignement : le padding interne du bouton décale son texte vers la droite par rapport
au corps de message au-dessus. Résolu par un `padding-left` **négatif** sur la ligne qui les
contient, qui annule exactement le padding interne — la boîte survolable déborde du texte des deux
côtés, mais l'encre reste alignée. Testé au préalable dans une sonde autonome (`padding-left: -8px`
sur un enfant de layout : `abs-x` mesuré à 12 pour une origine à 20 — donc les paddings négatifs
fonctionnent en Slint 1.17.1). Même mécanisme sur la ligne d'actions du panneau de plan, où le
premier essai à `-space-2` laissait un décalage résiduel **mesuré à +4px** contre le titre
"PLAN PROPOSÉ" (l'`ActionButton` a un padding interne de `space-3`, pas `space-2`) ; à `-space-3`,
les trois éléments s'alignent sur x=793.

**Boutons ghost.** `ActionButton` perd bordure et fond au repos, révèle `surface-hover` au survol et
`surface-press` à l'enfoncement. Il passe aussi en `horizontal-stretch: 0` avec une largeur lue sur
sa ligne interne : étiré comme avant, un ghost aurait allumé la moitié de la colonne au survol, ce
qui se lit comme une région sélectionnée et non comme un bouton.

**Minimalisme.** 8 `HDivider` → 4. Retirés : les deux hairlines internes de la sidebar (les deux
contrôles fusionnent en un bloc d'en-tête de 92px, hauteur explicite et non dérivée du contenu, pour
la même raison que le pied de sidebar), et celui entre le titre du panneau de plan et sa ligne
d'actions. Conservés : ceux qui bornent une zone scrollable (sous la titlebar, au-dessus du composer
et du pied de sidebar, au-dessus de la liste d'étapes) — sans eux le contenu défile visuellement
dans le composer. `SearchField` passe d'un contour à un aplat, bordure conservée à 1px mais
transparente au repos pour que le focus anime une couleur et non une géométrie. Cartes d'étape :
contour → aplat `bubble-bg`, parce que `surface` sur le fond `sidebar` du panneau ne fait que
~2 unités d'écart et qu'une carte sans bordure y aurait été invisible.

Vérifié : `make ui-check` sans erreur ni avertissement, `cargo check --workspace` OK, les trois
rendus (`demo-light`, `demo-dark`, `stress`) relus, et les alignements confirmés par balayage de
l'encre la plus à gauche plutôt qu'à l'œil. Limite honnête de cette passe : `slint-viewer
--screenshot` ne peut pas placer le pointeur sur la fenêtre, donc les états de survol n'ont été
validés que via une sonde qui les reproduit à l'identique en forçant la couleur — le vrai survol
n'est observable que dans l'app lancée (`make run`).

---

### 27. Streaming des plans, indicateur de génération, interruption (2026-09-02)

**Constat de départ.** Le streaming token par token existait déjà de bout en bout (`stream_chat` →
`Agent::respond` → `CoreToUiMsg::Token` → UI) et était marqué ✅ dans `summary.md`. Ce qui manquait
était ailleurs : `Agent::respond` **retenait tous les tokens** dès que la réponse ressemblait à un
plan (`looks_like_plan` sur le premier caractère non-blanc) et ne les relâchait jamais si c'en était
un. Conséquence : pendant toute la génération d'un plan — c'est-à-dire pendant le cas d'usage
principal de l'app, souvent plusieurs dizaines de secondes sur un modèle local — l'écran ne montrait
strictement rien.

**`StreamingPlanParser` (`crates/agent`).** `serde_json` n'a pas de mode incrémental : il parse un
document complet ou échoue. Le parseur fait donc uniquement le découpage — trouver où un `{`
s'ouvre et où le `}` correspondant se referme, en suivant l'état chaîne/échappement pour que les
accolades d'un `content` de WRITE_FILE ne comptent pas — puis confie chaque objet entier à
`serde_json`. Il se désarme (`disabled`) dans trois cas : réponse commençant par `{` et non `[` (un
objet seul est ambigu : plan d'une étape ou requête `{"query": …}`, et streamer une requête dans le
panneau de plan serait pire que rien), objet complet qui ne désérialise pas en `PlanStep`, ou `]` de
fin atteint. C'est un agrément d'affichage, jamais une source de vérité : `extract_plan(&buffer)` en
fin de flux reste seul juge, et si les deux divergent l'UI efface ce qui était affiché
(`discard_streaming_plan`).

Les tests l'alimentent **un caractère à la fois** (`feed_char_by_char`) : c'est le pire découpage
possible, donc le seul qui prouve qu'aucune supposition du genre « une frontière de morceau HTTP
coïncide avec une frontière JSON » ne s'est glissée dedans.

**`AgentEvent` remplace trois closures.** `respond` prenait `on_token`/`on_thinking`/`on_query`,
trois paramètres génériques et un `#[allow(clippy::too_many_arguments)]`. Le quatrième événement
aurait donné quatre blocs `|x| { … }` anonymes empilés au site d'appel. Un `enum AgentEvent<'a>`
(variantes empruntées — `&'a str`, `&'a [PlanStep]` — pour ne pas allouer une `String` par token) et
un seul `FnMut(AgentEvent)` remplacent le tout ; `main.rs` n'est plus qu'une table de correspondance
`AgentEvent` → `CoreToUiMsg`. `AgentEvent::PlanSteps` porte **toute** la liste à chaque émission, pas
la nouvelle étape seule : le récepteur remplace au lieu d'accumuler, donc pas de message « remets à
zéro » à prévoir et aucune dérive possible.

**Interruption réelle.** La boucle `run_core_agent` était un `while let … = ui_rx.recv().await` :
tant qu'elle attendait `handle_send_message`, elle ne lisait plus le canal, donc un `Cancel` serait
resté en file jusqu'à la fin de la génération — le bouton n'aurait rien annulé. La branche
`SendMessage` attend maintenant son handler dans un `tokio::select!` à côté du canal, avec `biased;`
pour que la réponse soit toujours poll-ée en premier (sans ça, un `Cancel` arrivant en même temps
qu'une réponse déjà terminée pouvait gagner le tirage aléatoire et annoncer une interruption pour un
travail fini). Aucun `CancellationToken` : **dropper le futur est l'annulation** — le `Receiver` du
stream disparaît, la tâche émettrice de `stream_chat` échoue à envoyer et sort, `reqwest` est droppé,
la connexion HTTP se ferme. Les messages arrivés pendant la génération (la sidebar reste cliquable)
vont dans une `VecDeque deferred` et sont rejoués au tour suivant. Rien du tour annulé n'est
persisté, puisque `handle_send_message` n'écrit `history` et SQLite qu'à sa toute fin.

**UI.** `StreamingCaret` : un bloc de 7×`font-lg` clignotant à 520 ms, piloté par un `Timer` qui
bascule un booléen — pas par une `animate` infinie, qui n'a rien à interpoler sans propriété qui
change. Posé sur sa propre ligne sous le texte (Slint n'a pas de flux inline : placer un élément
après du texte coupé en lignes demanderait de réimplémenter la coupure), avec un libellé
« Génération… » tant que le message est vide — un bloc clignotant seul, sans rien au-dessus, se lit
comme un artefact de rendu. Nouveau statut `"streaming"` pour `current-plan-status` : le panneau
affiche « PLAN EN COURS » et « Le modèle écrit le plan… », et masque la ligne `Plan <id>` et les
boutons Exécuter/Rejeter, qui n'auraient rien à désigner (le plan n'a ni identifiant ni existence en
base tant qu'il n'est pas complet). Le bouton « Arrêter » apparaît **à côté** du composer grisé
plutôt qu'à sa place, pour que la géométrie de la barre du bas ne saute pas à chaque envoi.

Un caret avait aussi été mis dans la ligne d'actions du panneau de plan ; rendu, il faisait doublon
avec la ligne italique juste au-dessus et avec les étapes qui apparaissent une à une — retiré.

**Un bug introduit puis corrigé dans la même passe.** `send_message` pousse un message assistant
vide marqué `streaming` avant même l'envoi. Si le tour commence par une requête LIST/STAT, le message
système « Listing … » se glisse derrière lui, et le premier `Token` crée donc un *nouveau* message
assistant — laissant le premier orphelin, toujours `streaming: true`, que `MessageDone` ne touchait
pas (il ne regardait que `messages.back_mut()`). Invisible jusqu'ici (contenu vide = rien à
afficher) ; avec un caret conditionné à `streaming`, l'orphelin clignotait « Génération… »
indéfiniment. D'où `finish_streaming(state)` : il supprime les messages qui n'ont jamais rien reçu et
retire `streaming` de **tous** les autres, appelé par `MessageDone`, `Cancelled`, `PlanReady` et
`Error`. Leçon générale : rendre visible un champ jusque-là sans effet visuel transforme chacune de
ses valeurs incohérentes en bug.

**Vérifications.** Nouvelle fixture `crates/ui/ui/fixtures/streaming.json` (message en cours sans
contenu, plan en statut `streaming`, `is-processing: true`) ajoutée à `make ui-shot`, qui rend
maintenant quatre PNG. `make ui-check` propre, `cargo clippy --workspace -- -D warnings` propre, 20
tests dans `lito-agent`. Les deux chemins ont aussi été vérifiés **en vrai** contre l'Ollama local
(`qwen3.5:4b`) via une sonde temporaire : un prompt de plan produit deux `PlanSteps` successifs et
aucun `Token` (le JSON ne fuit pas dans le chat), un prompt conversationnel produit bien des `Token`
mot à mot.

**Docs mises à jour dans le même changement** : `learn/01-overview.md` et `learn/02-rust.md`
(réécrits — ils décrivaient encore `Agent::plan`, `complete()` sans streaming, `ClientMessage`/
`ServerMessage` supprimés depuis, et `Operation` sans `WriteFile`), `learn/04-view-model.md`,
`learn/07-agent.md`, `learn/09-main.md`, `design.md`, `summary.md`, et le README des fixtures.

### 28. Fluidité de la conversation : diff de modèle, lot de synchronisation, ScrollView (2026-09-02)

**Constat de départ.** Défilement saccadé dans la conversation et sélection de texte qui « lâche ».
Trois causes distinctes, dont une seule dans ce code.

**La cause principale : `sync_to_ui` réassignait tous les modèles à chaque token.**
`window.set_messages(ModelRc::new(VecModel::from(messages)))` crée une *nouvelle* instance. Or un
repeater Slint compare le `ModelRc` entrant **par pointeur** (`Repeater::model` dans `i-slint-core` :
`if old_model != m { *self.data().inner.borrow_mut() = RepeaterInner::default(); }`) et jette alors
toutes ses lignes instanciées *et* son `RepeaterLayoutState` — ancre de scroll, hauteur moyenne des
lignes, position précédente du viewport. Comme `CoreToUiMsg::Token` passait par `update_ui_state`,
qui synchronise inconditionnellement, cela arrivait **à chaque token** : la conversation perdait son
ancrage des dizaines de fois par seconde, et les `TextInput` sous-jacents étaient recréés, emportant
toute sélection en cours. Chaque token payait en plus un clone complet de `UiState` et un
`StyledText::from_markdown` sur **tous** les messages assistant de la conversation, pas seulement
celui en train de s'écrire.

Corrigé en trois temps. (1) `apply_core_message` (ex-`handle_core_message`) ne mute plus que
`UiState` ; le pump vide tout ce qui est déjà en file dans le même lot puis appelle `sync_to_ui` une
seule fois, au plus tous les `MIN_SYNC_INTERVAL` (33 ms) — le premier message d'une rafale part
immédiatement, donc une réponse isolée n'attend pas. (2) `sync_to_ui` installe une fois son propre
`VecModel` puis le mute sur place, en décidant quelles lignes ont changé par empreinte
(`message_fingerprint`) ; `plan_steps` et `sessions` gardent un cache d'empreinte et ne sont
remplacés que s'ils ont bougé. (3) Ne construire `ChatMessageData` que pour les lignes changées fait
tomber le re-parse markdown de toute la conversation à un seul message.

**Deuxième cause : la virtualisation du `ListView` ne servait à rien ici.** `main.rs` charge une
session avec `get_recent_messages_for_session(session_id, MAX_HISTORY_MESSAGES)` — la conversation
est déjà plafonnée à 20 messages. À cette taille `ListView` n'apportait rien et coûtait deux défauts
visibles : la hauteur de son viewport est extrapolée de `cached_item_height`, une moyenne glissante
des lignes instanciées, donc avec des messages d'une à quarante lignes l'ascenseur changeait de
taille et la position dérivait ; et son `viewport-y` appartient au layout runtime (la passe flickable
du compilateur saute délibérément de le lier pour `ListView`), donc rien côté Slint ne pouvait le
piloter. Remplacé par `ScrollView` + `VerticalLayout { alignment: start }`. La sidebar et les étapes
de plan restent des `ListView`.

**Troisième point, et une hypothèse fausse corrigée en route.** Le diagnostic initial attribuait la
sélection de texte cassée aux seuils du `Flickable` (`FORWARD_DELAY` de 100 ms, interception du drag
au-delà de 8 px). C'est faux ici : `ScrollView` — dont `ListView` hérite — pose `interactive: false`
sur son `Flickable` interne, et `flickable.rs:155` renvoie alors `ForwardAndIgnore` pour tout ce qui
n'est pas un événement de molette. Ces seuils ne s'appliquent qu'à un `Flickable` nu. La sélection
cassait à cause de la reconstruction du modèle (cause 1) et parce que `StyledText` n'a pas de
sélection du tout. Mécanisme plausible inventé pour expliquer un symptôme réel — exactement ce
contre quoi la fin de `.claude/skills/slint/SKILL.md` met en garde.

**Suivi automatique du bas.** `viewport-y` étant maintenant pilotable, un handler
`changed viewport-height` sur `chat-scroll` épingle la vue au bas tant que `stick-to-bottom` est
armé ; `scrolled` le désarme dès qu'on remonte de plus de `Theme.stick-slack` (24 px) et le réarme si
on redescend. Armé par l'envoi d'un message, l'ouverture d'une session et la création d'une session.

**Vérifications.** `make ui-check` propre, `clippy --workspace -- -D warnings` propre, tous les tests
passent. Le rendu a été vérifié sur les quatre fixtures plus une fixture jetable de 20 messages
débordant la vue : la capture montre bien la conversation épinglée en bas au premier rendu. Une sonde
`debug()` temporaire a confirmé les valeurs (`viewport-y = -560.5` = exactement
`visible-height - viewport-height`, métrique de slack à 0). Le chemin Rust, impossible à exercer avec
`slint-viewer`, a été validé en lançant l'app contre un `HOME` temporaire avec une base amorcée (pour
ne pas toucher aux données réelles) et une sonde injectant 121 tokens à ~125/s : le `downcast_ref`
vers `VecModel` réussit, la branche `push` s'exécute à l'arrivée du message, puis **29 syncs** en
1,00 s (≈30 Hz, conforme au throttle) touchant **1 seule ligne chacune**, et zéro reconstruction de
modèle. Les deux sondes ont été retirées ensuite.

**Non fait, délibérément.** La pagination vers le haut (charger les messages plus anciens quand on
remonte) a été conçue et validée techniquement — `ScrollView.viewport-y` est `in-out`, `scrolled` se
déclenche à la molette, `changed` permet de compenser la hauteur insérée après layout — mais pas
implémentée : c'est une *fonctionnalité* d'accès à l'historique (aujourd'hui les messages au-delà des
20 derniers sont inaccessibles), pas un gain de performance, et elle demande un
`get_messages_before(session_id, before_id, limit)` dans `crates/memory` plus un
`UiToCoreMsg::LoadOlderMessages`.

**Docs mises à jour dans le même changement** : `CLAUDE.md` (architecture UI, modèle de threading),
`design.md` (nouvelle section « Défilement de la conversation », liste des `std-widgets` restants),
`summary.md`, `learn/01-overview.md`, `learn/03-slint-ui.md`, `learn/04-view-model.md`.

---

### 29. Découpage des réponses en blocs markdown, et copie du texte rendu (2026-09-02)

**Point de départ : une question.** « N'y a-t-il vraiment aucun moyen de laisser l'utilisateur
copier du `StyledText` sans repasser par le texte brut ? » Réponse vérifiée dans les sources de
Slint 1.17.1, pas dans la doc : l'API complète de l'élément tient en sept propriétés et un callback
(`i-slint-compiler-1.17.1/builtins.slint:731-753`) — `default-color`, `default-font-family`,
`default-font-size`, `horizontal/vertical-alignment`, `link-color`, `text`, `link-clicked`. Rien sur
la sélection, le curseur ou le presse-papiers. Le seul élément sélectionnable reste `TextInput`
(`read-only`, `select-all()`, `set-selection-offsets()`), et son `text` est un `string` nu, à style
uniforme : il ne peut pas porter de `styled-text`. Donc non, pas au niveau de l'élément.

**Deuxième constat, plus gênant que le premier.** `StyledText::from_markdown` renvoie `Err` dès
qu'**une seule** construction non supportée apparaît (`i-slint-core-1.17.1/styled_text.rs:53-62` :
`Err` si la liste d'erreurs n'est pas vide ; la liste des constructions concernées est dans
`i-slint-common-1.17.1/styled_text.rs:280-297` — titres, blocs de code, tableaux, citations, filets,
images, HTML, notes de bas de page, maths). Un seul ``` ``` ``` ou un `##` dans une réponse faisait
donc retomber **tout** le message en markdown brut. Ce n'était pas une hypothèse : c'était le
comportement en place.

**Ce qui a été fait.** Un nouveau module `crates/ui/src/markdown.rs` découpe la réponse en blocs de
premier niveau avec `pulldown-cmark` (déjà dans le graphe de dépendances via `i-slint-common`, même
version 0.13.4 — aucune unité de compilation supplémentaire), et `MessageBlock` (dans
`app-window.slint`) rend chaque bloc avec le composant qui lui convient : `StyledText` pour les
paragraphes et les listes, `CopiableText` pour les blocs de code (carte `bubble-bg`, Menlo, avec sa
propre action « Copier »), les titres, les tableaux, les citations, le HTML brut, et `HDivider` pour
un `---`. Le repli devient donc local au bloc fautif, et la sélection au clic-glisser revient
exactement là où on en a besoin — ce qu'on copie dans une réponse d'agent, c'est un bloc de code ou
un chemin, pas un paragraphe de prose.

La classification **n'est pas** une liste codée en dur de ce que Slint supporte : chaque paragraphe
ou liste est réellement passé à `StyledText::from_markdown` et retombe en texte brut si l'appel
échoue. Le module reste donc correct si le sous-ensemble supporté s'élargit.

**Côté copie**, trois actions par message au lieu de deux, plus une par bloc de code : « Copier »
copie désormais le texte *rendu* (marqueurs retirés, `markdown::plain_text`) et non plus la source —
c'est ce qu'on attend en collant une réponse ailleurs ; « Copier le markdown » garde la source ;
« Texte brut »/« Rendu » reste la seule façon de sélectionner un paragraphe stylé, et bascule la
ligne entière vers un `CopiableText` unique.

**Une contrainte de type découverte à la compilation** : `ChatMessageData` porte maintenant
`blocks: [MessageBlockData]`, donc un `ModelRc`, et un modèle Slint n'est pas `Send` — la ligne ne
pouvait plus traverser la closure de `upgrade_in_event_loop`. `sync_to_ui` construit donc un
`ChatRow` (mêmes champs, blocs en `Vec`) hors du thread UI, parsing markdown compris, et n'appelle
`ChatRow::into_data` — la seule partie qui crée le modèle — que dans la closure. Le diff ligne à
ligne de l'entrée 28 est intact : on ne re-parse que le message qui bouge.

**Vérifications.** 9 tests unitaires dans `markdown.rs` (bloc de code qui ne fait plus tomber le
message entier, fence non terminée pendant le streaming rendue quand même en code, listes gardées
en un seul bloc, citation/image/HTML repliés bloc par bloc, extraction du texte rendu). Trois
choses ont été mesurées plutôt que supposées :
- Un `[Struct]` **imbriqué dans un `struct`** compile et se charge bien depuis une fixture JSON
  (testé isolément avant d'écrire quoi que ce soit).
- Une fixture avec un bloc `kind: "styled"` **plante** `slint-viewer` (`binding was of the wrong
  type: ()`) : le loader JSON n'a pas de cas pour `styled-text`, le champ reste `Value::Void`. Les
  fixtures n'utilisent donc que les autres `kind`, ce qui a l'avantage d'exercer réellement le
  nouveau rendu dans `make ui-shot`.
- La branche `StyledText` de `MessageBlock`, elle, ne peut pas être rendue depuis une fixture (voir
  le point précédent) : elle a été vérifiée en exportant temporairement `MessageBlock` et en la
  rendant depuis un `.slint` jetable qui construit ses blocs à la main, avec la macro
  `@markdown("…")` — le seul moyen d'obtenir une valeur `styled-text` littérale dans du `.slint`.
  Gras, italique, code inline, lien en `Palette.accent` et puces s'affichent correctement ;
  l'`export` a ensuite été retiré.
- L'action « Copier » du bloc de code n'était pas alignée sur la marge droite du code : `alignment:
  start` sur son en-tête empêchait le ressort de distribuer l'espace (piège n° 10 de la skill
  Slint), et le padding propre au `TextButton` laissait le texte 17px à l'intérieur du bord contre
  9px à gauche. Corrigé, puis re-mesuré à l'échantillonnage de pixels : encre à x=749 pour un bord
  de carte à 758, soit les mêmes 9px des deux côtés.

**Non fait, délibérément.** Un `TextInput` invisible superposé au `StyledText` (les retours à la
ligne des deux couches divergent dès qu'une ligne contient du gras ou un lien — la sélection
tomberait à côté des glyphes) ; un `TextInput` par span, qui demanderait de réimplémenter le
découpage en lignes puisque Slint n'a pas de layout en flux inline, et où la sélection ne
traverserait toujours pas les frontières de span ; et un correctif en amont dans Slint (le ticket
qui a introduit l'élément, slint-ui/slint#9560, exclut explicitement l'édition et la sélection de
son périmètre ; #736, sélection sur `Text`, reste ouvert).

**Docs mises à jour dans le même changement** : `CLAUDE.md` (module `markdown.rs`, contrainte
`Send`, règles de fixtures), `design.md` (section « Rendu markdown des réponses de l'agent »
réécrite), `summary.md`, `learn/04-view-model.md`.

### 30. Endpoint Ollama configurable (2026-09-05)

L'adresse d'Ollama était codée en dur (`OllamaClient::new("http://localhost:11434", model)` dans
`main.rs`) — aucun moyen de pointer vers un Ollama exposé ailleurs (autre port, autre machine du
réseau local, conteneur) sans recompiler. Suit exactement le même schéma que le sélecteur de
modèle (`SetModel`/`handle_set_model`/`Pong`), déjà éprouvé :

- `lito_llm::OllamaClient` gagne `base_url()`/`set_base_url()` et une constante publique
  `DEFAULT_BASE_URL = "http://localhost:11434"` (l'adresse par défaut d'Ollama lui-même), pour que
  ni `main.rs` ni le panneau de réglages n'aient chacun leur propre copie de cette chaîne.
- `UiToCoreMsg::SetEndpoint { url }` / `CoreToUiMsg::Pong` gagne un champ `endpoint`.
  `handle_set_endpoint` (nouveau, dans `main.rs`) met à jour le client vivant, persiste dans
  `settings.ollama_endpoint` via `MemoryStore::set_setting` (même table clé/valeur que
  `selected_model`/`dark_mode`), puis relance `handle_ping` — donc `UiState::ollama_endpoint` n'avance
  qu'une fois le `Pong` revenu, jamais de façon optimiste : une URL invalide se traduit par « hors
  ligne » dans la titlebar plutôt que par un changement accepté en silence. Au démarrage, `main.rs`
  lit `settings.ollama_endpoint` et retombe sur `DEFAULT_BASE_URL` si rien n'est persisté (première
  ouverture, ou après une réinitialisation).
- Côté Slint : un badge « Réglages » (même traitement visuel bordé que `model-badge`, pour la même
  raison — c'est un contrôle qui ouvre un popup, pas une simple action) ouvre un `PopupWindow` avec
  un champ éditable, un bouton « Réinitialiser » (ne fait que réécrire le champ avec
  `default-ollama-endpoint`, ne sauvegarde pas tout seul) et un bouton « Enregistrer » (`ActionButton`
  ton succès) qui appelle `set-endpoint`. Le champ édité vit dans une propriété privée
  `endpoint-draft`, réinitialisée à `ollama-endpoint` (la valeur *confirmée*) à l'ouverture du popup —
  pas à la dernière frappe laissée en plan la fois précédente si le popup avait été fermé sans
  sauvegarder.

Vérifié : `cargo check --workspace`/`cargo clippy -- -D warnings` sans erreur ni warning, `make
ui-check` sans avertissement de propriété inconnue, `make ui-shot` (les trois fixtures, clair et
sombre) montre le badge « Réglages » correctement aligné à côté de « Sombre »/« Ping » dans la
titlebar sans rien casser autour. Le popup lui-même n'est pas visible dans un `--screenshot` headless
(il ne s'ouvre que sur clic, que `slint-viewer` ne simule pas) — son contenu suit le même gabarit que
`model-popup`, déjà vérifié visuellement à l'entrée 22bis. L'app a aussi été lancée (`cargo run -p
lito-app`) pour confirmer qu'elle démarre sans panique avec le nouveau câblage.

**Docs mises à jour dans le même changement** : `summary.md` (§ Modèle LLM, les deux enums
`UiToCoreMsg`/`CoreToUiMsg`, tableau État actuel).

### 31. Trois retouches visuelles rapportées par l'utilisateur (2026-09-05)

Trois défauts pointés sur une capture d'écran de l'app en thème sombre :

- **"+ Nouvelle tâche" mal aligné.** Deux `Label` séparés ("+" en `font-lg`/700, "Nouvelle tâche"
  en `font-md`/600), chacun repositionné par `Theme.optical-center-y` pour contrer le
  top-alignement par défaut d'un `HorizontalLayout` (voir piège n°9 de la skill Slint). Mesure
  pixel (`pxsample.py`) : les bas de glyphe tombaient bien sur la même ligne (y=70 des deux côtés),
  mais chaque symbole a son propre point d'ancrage optique dans les métriques de sa police — un
  glyphe "+" et des lettres n'ont pas le même centre optique même à hauteur de boîte identique — donc
  les deux nudges ne s'annulaient pas exactement. Remplacé par un unique `Label { text: "+ Nouvelle
  tâche"; }` : un seul run de texte n'a qu'une seule ligne de base, donc plus rien à désaligner.
  Centrage horizontal obtenu gratuitement (élément non positionné = centré dans son parent).
- **Barre de défilement claire en thème sombre.** `ScrollView`/`ListView` (std-widgets) ne lisent
  pas le `Palette` maison de l'app — ils suivent leur propre `Palette.color-scheme`, qui reflète
  l'apparence système par défaut, indépendamment du toggle "Sombre"/"Clair" de l'app (voulu
  découplé, voir design.md § Hors scope). `MainWindow` importe ce `Palette` sous l'alias
  `StdPalette` et force `StdPalette.color-scheme` à suivre `Palette.dark` (`init` + `changed
  effective-dark`, `effective-dark` étant un miroir local puisqu'un `changed` ne s'écrit que sur
  une propriété locale). Vérifié avec une sonde `.slint` isolée hors du projet (`ScrollView` +
  liste longue, plusieurs styles std-widgets) : le forcer a bien changé la couleur du thumb, dans
  le bon sens (`#26292d` visible sur fond `#14171c` en dark forcé, `#0a0c0f` quasi invisible en
  light forcé) — confirme que le mécanisme agit sur le bon token.
- **Barre de titre macOS non sombre.** La titlebar native (au-dessus de la titlebar maison de
  l'app) suit l'apparence système, pas le toggle de l'app — même cause racine que le point
  précédent, mais aucun levier Slint ne permet de la teindre : seul `no-frame` (suppression totale,
  y compris les boutons rouge/jaune/vert) est exposé en `.slint`. Choix utilisateur entre "no-frame"
  et "titlebar native transparente" (`AskUserQuestion`) : transparente, pour garder les boutons
  système et le glisser-déplacer natifs. Implémenté côté Rust dans `crates/app/src/main.rs`
  (`install_macos_titlebar_backend`, appelée avant toute création de fenêtre Slint) via
  `i_slint_backend_winit::Backend::builder().with_window_attributes_hook(...)` — pas
  `slint::winit_030`, qui exige la feature "unstable" — pour poser
  `with_titlebar_transparent(true)`, `with_fullsize_content_view(true)`, `with_title_hidden(true)`
  (méthodes de `winit::platform::macos::WindowAttributesExtMacOS`) sur la fenêtre avant sa
  création. `i-slint-backend-winit`/`winit` ajoutés en dépendances directes de `crates/app`
  (`[target.'cfg(target_os = "macos")'.dependencies]`), épinglées à la version exacte déjà
  résolue dans `Cargo.lock` pour éviter un second backend winit. Conséquence : les boutons système
  flottent désormais sur le coin haut-gauche de la titlebar maison, d'où `Theme.traffic-light-w`
  (78px, nouveau token) en `padding-left` de "Lito" à la place de `Theme.space-3`.

Vérifié : `cargo check --workspace`/`cargo clippy -p lito-app -- -D warnings` sans erreur ni
warning, `make ui-check`/`make ui-shot` sans avertissement, `cargo build -p lito-app` compile et
`cargo run -p lito-app` démarre sans panique (confirmé par process vivant + logs), mais le rendu
réel de la titlebar transparente macOS n'a **pas** pu être vérifié à l'écran dans cette session —
`screencapture` a renvoyé une image entièrement noire (pas de permission d'enregistrement d'écran
pour ce contexte d'automatisation) malgré un process confirmé actif. À vérifier visuellement par
l'utilisateur via `make run`.

**Docs mises à jour dans le même changement** : `design.md` (§ Palette, nouveau paragraphe sur
`StdPalette`/`color-scheme` ; § Layout/Titlebar, paragraphe sur la titlebar transparente et
`Theme.traffic-light-w`).

### 32. Désencombrement du chrome : icônes Lucide + moins de bordures (2026-09-06)

Demande utilisateur : l'interface était jugée trop verbeuse (trop de boutons à libellé texte),
avec une suggestion de librairie d'icônes compatible Slint et de bordures/séparateurs à retirer.
Proposition validée par l'utilisateur avant implémentation (`AskUserQuestion`).

- **Librairie retenue : [Lucide](https://lucide.dev)** (MIT, dérivé actif de Feather Icons).
  Choisie pour son trait (`stroke`, coins arrondis) identique à celui des icônes d'opération déjà
  dessinées à la main dans `crates/ui/ui/icons/` (`OpIcon`) — une bascule vers cette librairie est
  cohérente visuellement, pas une rupture de style. Les SVG sources (viewBox 24×24,
  `stroke: currentColor`) sont vendorisés (téléchargés depuis le dépôt GitHub officiel, pas de
  CDN à l'exécution — cohérent avec le principe local-first du projet), reformatés en une ligne
  et `stroke` fixé à `#000000` pour matcher la convention existante ; `Image { colorize }` les
  reteinte à l'affichage, donc la couleur source n'a pas d'importance. 14 icônes ajoutées : `sun`,
  `moon`, `settings`, `activity`, `copy`, `code`, `align-left` (en réalité l'icône Lucide
  `text-align-start`, renommée en amont — `align-left` a été retiré de Lucide), `chevron-down`,
  `chevron-up`, `x`, `play`, `undo-2`, `check`, `plus`.
- **Deux composants ajoutés** : `Icon` (mappe un nom vers son asset, même schéma que `OpIcon` mais
  pour le chrome générique) et `IconButton` (variante sans texte de `TextButton` — même ramp
  ghost hover/press, boîte carrée autour de l'icône, `accessible-label` puisqu'il n'y a plus de
  texte visible pour un lecteur d'écran).
- **Boutons texte → icône seule** : "Copier"/"Copier le markdown"/bascule "Texte brut"↔"Rendu"
  (actions par message et par bloc de code), "Afficher"/"Masquer" du raisonnement (icônes
  chevron, qui inversent selon l'état plutôt que de rester fixes), le toggle de thème
  (`sun`/`moon`, sélectionné selon `Palette.dark`), "Réglages" (`settings`), "Ping" (`activity`).
  Gardés en texte, délibérément : "Nouvelle tâche" (action primaire et rare — garde son libellé,
  seul le "+" devient une vraie icône) et "Annuler la dernière opération" du pied de sidebar
  (action de sécurité peu fréquente — même traitement, libellé conservé). "Réinitialiser" (popup
  Réglages) n'a pas été touché : action secondaire déjà dans un panneau réduit.
- **Glyphes Unicode → SVG** : `ActionButton` rendait son icône comme un `Label` de texte brut
  (`×`, `✓`, `↺`) ; passé à une vraie `Icon` (`x`, `play` pour "Exécuter" — plus explicite qu'un
  check pour une action d'exécution —, `undo-2`, `check` pour "Enregistrer"). Même chose pour le
  "×" de fermeture du panneau de plan (devenu un `IconButton`) et le "↺" du pied de sidebar. Ne
  restent comme glyphes texte que `→` (bouton d'envoi) et `○` (puce statique, pas un bouton).
- **Bordures retirées** : badge de modèle et bouton "Nouvelle tâche" (bordure 1px → aplat
  `surface-hover`/`surface-press`/`border` selon l'état, même ramp que `SearchField`) ; badge
  "Réglages" idem, en plus de passer en icône seule. Ces trois éléments étaient les seules
  pilules bordées d'une ligne de chrome par ailleurs assez dense. Gardées : la bordure des popups
  (`model-popup`/`settings-popup`, seul indice d'élévation de cette UI, pas d'ombre portée) et
  tous les `HDivider`/`VDivider` structurels (déjà réduits au minimum, voir entrée précédente sur
  les séparateurs dans `design.md`). La suppression du `VDivider` entre la sidebar de sessions et
  la colonne de chat a été évaluée puis **écartée** : le contraste `sidebar`/`bg` seul
  (`#fbfcfd`/`#ffffff` en thème clair) est trop faible pour porter la séparation sans lui.

Vérifié : `cargo check --workspace` sans erreur (aucune propriété/callback Rust touchée, seul le
contenu visuel du `.slint` a changé) ; `make ui-check` sans avertissement ; `make ui-shot` sur les
quatre fixtures (`demo-light`, `demo-dark`, `stress`, `streaming`) sans avertissement ; couleur de
fond échantillonnée au pixel (`pxsample.py`) pour confirmer que le thème sombre s'applique
toujours (`#14171c`, pas juste une impression visuelle sur le PNG downscalé) ; forme des icônes de
titlebar vérifiée par un dump ASCII de la zone (pas seulement leur présence/absence) en clair et
en sombre, confirmant la bascule `sun`↔`moon` selon le thème.

**Docs mises à jour dans le même changement** : `design.md` (§ Icônes — nouvelle sous-section
"Icônes plutôt que libellés" et inventaire des deux familles d'icônes ; § Séparateurs — bordures
retirées du badge de modèle et de "Nouvelle tâche" ; § Layout/Titlebar — badge "Réglages" et
toggle de thème/"Ping" décrits comme icône seule ; inventaire des composants — `Icon`/`IconButton`
ajoutés).

### 33. Panneau de plan : titre porteur d'état, étapes estompées si rejeté, prose non mono (2026-09-06)

Demande utilisateur (capture d'écran à l'appui) : la police mono de la description d'étape "n'a
pas un rendu très moderne", "Plan rejeté n'est pas aligné" et l'état rejeté "n'est pas vraiment en
emphase". Analysée avant implémentation (le mode auto a réfléchi puis proposé via
`AskUserQuestion`, comme demandé explicitement) :

- **Le désalignement, causé et pas juste constaté.** "Plan rejeté" était un `Label` texte inséré
  dans la ligne d'actions Rejeter/Exécuter/Annuler, dont le `padding-left: -Theme.space-3`
  n'existe que pour annuler le padding interne d'`ActionButton` (voir son propre commentaire dans
  le fichier) — un `Label` nu n'a pas ce padding à annuler, donc il rendait décalé de 12px vers la
  gauche par rapport au titre du panneau au-dessus. Un correctif de padding local aurait marché
  mais laissé le vrai problème (l'état vit dans un élément secondaire, facile à manquer) intact.
- **Solution retenue : porter l'état par le titre du panneau**, déjà l'élément le plus visible du
  bloc (gras, majuscules, tracké) et déjà dynamique pour l'état `"streaming"` ("PLAN EN COURS") —
  étendu aux trois autres : "PLAN PROPOSÉ" (pending, inchangé), "PLAN EXÉCUTÉ" (`done`, teinté
  `Palette.success`), "PLAN REJETÉ" (`rejected`, teinté `Palette.danger` — les mêmes couleurs
  sémantiques que `StatusBadge` utilise déjà pour "Fait"/"Échec", rien de nouveau). Le `Label`
  "Plan rejeté" est supprimé, pas juste repositionné — sa ligne d'actions rend vide pour l'état
  `"rejected"`, exactement comme elle le faisait déjà pour `"streaming"`.
- **Étapes estompées si rejeté.** `opacity: 0.5` sur la `ListView` des étapes quand
  `current-plan-status == "rejected"` — même idiome que les contrôles désactivés ailleurs dans ce
  fichier (`ActionButton`, `IconButton`, …). Sans ça, les badges "En attente" ambre des cartes
  d'étape laissaient croire que le plan allait quand même s'exécuter.
- **Description d'étape : police proportionnelle, pas `Menlo`.** `design.md` documentait ce
  Menlo comme pensé pour des chemins de fichiers, mais `PlanStep::description`
  (`agent/src/lib.rs`) est définie comme une phrase courte en langage naturel ("Create Captures
  folder on Desktop", dans la capture fournie) — un chemin peut y apparaître, mais ce n'en est pas
  un. Le mono sur de la prose est ce qui donnait le rendu "pas moderne" pointé par l'utilisateur ;
  retiré, `CopiableText` retombe sur la police proportionnelle par défaut, cohérente avec le reste
  du chrome.
- **Écarté après proposition** (non sélectionné par l'utilisateur dans `AskUserQuestion`) :
  tronquer l'UUID affiché ("Plan eb20d9d4…" au lieu de l'UUID complet) — resterait un id complet
  non tronqué pour l'instant.

Vérifié : `cargo check --workspace` sans erreur ; `make ui-check` sans avertissement ; `make
ui-shot` sur les quatre fixtures standard (aucune ne couvrait `"rejected"`/`"done"` avant cette
session) sans avertissement ni régression visuelle sur les descriptions à chemin (`stress.json`,
qui contient des chemins longs — le rendu proportionnel reste lisible, retourne à la ligne
proprement dans les 300px du panneau) ; deux fixtures scratch générées à la volée (`demo.json`
avec `current-plan-status` forcé à `"rejected"` puis à `"done"` + `current-undo-id`) et rendues
pour confirmer au pixel que le titre change bien de texte/couleur et que les cartes s'estompent
bien pour l'état rejeté — non ajoutées au dépôt (fixtures de vérification ponctuelle, pas des cas
de test permanents).

**Docs mises à jour dans le même changement** : `design.md` (§ Typographie — la note sur `Menlo`
corrigée ; § Sidebar de sessions / panneau de plan — nouveau paragraphe sur le titre porteur
d'état et l'estompage des étapes rejetées).

### 34. Exécution de commandes terminal + dispositif d'approbation (2026-09-06)

Ajout de `Operation::RunCommand` : Lito peut lancer une commande terminal, dans un plan **et** en
reconnaissance pendant la planification. Saut qualitatif — contrairement aux 7 primitives
existantes, une commande **n'a pas d'inverse** et **son effet n'est pas analysable à l'avance** —
donc l'essentiel du lot est le dispositif d'approbation qui la rend acceptable.

- **Deux nouveaux crates.** `crates/policy` (`lito-policy`) : `CommandPolicy::judge(&CommandSpec)
  -> Verdict` (`AutoAllow` / `NeedsApproval { rememberable }` / `Denied(Violation)`). Ordre
  d'évaluation strict : (1) liste noire dure `denylist.rs` **avant le mode**, donc `auto-unsafe`
  ne la contourne pas ; (2) `cwd` hors des racines autorisées (`~/Desktop`, `~/Documents`,
  `~/Downloads`, `~/Pictures`) → `Denied` dans tous les modes ; (3) `auto-unsafe` → `AutoAllow` ;
  (4) `auto-restrictif` + argv non-shell + `(argv canonique, racine)` dans l'allowlist →
  `AutoAllow` ; (5) sinon `NeedsApproval`, mémorisable sauf si `shell: true`. `readonly.rs` : une
  petite allowlist de motifs argv (`ls`, `git status`, `wc -l <path>`, …) consultée **uniquement**
  par la reconnaissance pour court-circuiter l'approbation même en mode `validation`.
  `crates/exec` (`lito-exec`) : `run(&CommandSpec)` via `tokio::process`, `stdin` fermé,
  **environnement reconstruit** (`PATH`/`HOME`/`USER`/`LANG`/`TERM=dumb` — pas de fuite de clé
  d'API du parent), sortie plafonnée à 64 Kio/flux, timeout 120 s puis `kill`. Un code de sortie
  non nul est un `Ok(CommandOutcome)`, pas un `Err`. 30 tests entre les deux crates.
- **`RUN_COMMAND` ne passe pas par `FsOps`.** Son bras dans `FsOps::execute_step` renvoie une
  `Err` explicite (commentée comme volontaire) — ce n'est ni une opération fichier ni une
  opération réversible, et le `match` exhaustif fait que l'oublier **casse la compilation**, ce
  qui est le bon échec. `app::dispatch_step` s'intercale devant : `RunCommand` →
  `CommandPolicy::judge` → approbation éventuelle → `lito_exec::run` ; tout le reste → `FsOps`
  inchangé.
- **Le point dur : faire attendre le core sur une réponse de l'UI.** L'ancien `select!` `biased`
  de `run_core_agent`, qui ne mettait que `SendMessage` en course contre le canal, a été
  **généralisé** en `drive_with_ui_pump(fut, ui_rx, deferred, gate)` — `SendMessage` **et**
  `ExecutePlan` passent par là. Un `ApprovalGate` (`Mutex<HashMap<request_id,
  oneshot::Sender<ApprovalDecision>>>`) porte l'aller-retour : `gate.request` envoie
  `CoreToUiMsg::ApprovalRequest` et `.await` le `oneshot`. L'annulation tombe toute seule :
  dropper le futur en attente droppe le `Receiver`, donc `request` rend `Deny` — aucune mécanique
  d'annulation dédiée.
- **P1.4 corrigé au passage.** Un refus de politique, une approbation refusée, un code de sortie
  non nul ou une erreur fs émettent `StepFailed` et arrêtent le plan en statut `"failed"` en base
  — là où le `?` précédent laissait le plan coincé en `"executing"` pour toujours.
- **Journalisation de l'irréversible.** Chaque commande exécutée est écrite dans la table
  `undo_log` de SQLite via `memory.log_undo` — sa **première utilisation réelle** (la méthode
  existait, inutilisée). `handle_undo` la relit pour renseigner `UndoDone { irreversible_commands
  }`, et l'UI pousse un message système « N commande(s) n'ont pas pu être annulées ».
- **Reconnaissance pendant la planification.** Quatrième forme de réponse du modèle :
  `{"command": ["git","status"], "cwd": "..."}` (champs requis disjoints de `FsQuery` et de
  `PlanStep`, vérifié par test). `Agent::respond` reçoit un trait `ReconRunner` (implémenté côté
  `app` par `GateReconRunner` — `agent` ne dépend donc **pas** de `policy`/`exec`). Résultat
  réinjecté comme message "user" sous `[COMMAND RESULT]`, **encadré et étiqueté « données non
  fiables »** dans le system prompt (la sortie d'une commande devient de l'entrée de prompt).
  `MAX_QUERY_ROUNDS` (6) couvre désormais requêtes fs **et** commandes.
- **UI.** Nouvelle icône `run-command.svg` (16×16, trait 1.35, sans remplissage) + bras
  `RUN_COMMAND` dans `OpIcon`. `PlanStepData` gagne `command`/`shell`/`cwd`/`irreversible`/
  `command-ran`/`exit-code`/`stdout`/`stderr`/`output-truncated` (tous ajoutés à
  `plan_steps_fingerprint`, sinon la ligne ne se repeint jamais). Carte d'approbation
  (`ApprovalCard`) **au-dessus du composer, dans la colonne de chat** — le seul endroit visible
  que la demande vienne d'une étape de plan ou de la reconnaissance : programme résolu en absolu,
  `argv` un argument par ligne en Menlo, `cwd`, badge « shell », mention « non annulable » en
  rouge, puis *Refuser* / *Autoriser* / *Autoriser et mémoriser* (masqué si `!rememberable`).
  **Aucun `Palette.accent`** (réservé au CTA du composer). Sortie de commande repliée par défaut
  sous l'étape via `CopiableText`. Panneau Réglages : sélecteur de mode à trois positions
  (`ModeOption`) + liste des commandes mémorisées avec un « × » par ligne. Nouvelle fixture
  `approval.json` + `RUN_COMMAND` ajouté à `stress.json`.

Vérifié : `make check && make clippy && make test` (107 tests, dont la matrice modes × allowlist ×
liste noire de `lito-policy` et les 8 tests d'exécution de `lito-exec`) ; `make ui-check` sans
avertissement ; `make ui-shot` sur les cinq fixtures (`demo-light`, `demo-dark`, `stress`,
`streaming`, `approval`) — PNG relus : la carte d'approbation, l'icône `RUN_COMMAND` (chevron),
la ligne `$ …` + `shell · cwd · non annulable`, et l'en-tête `SORTIE · CODE n` rendus correctement,
accents des majuscules (`TRONQUÉE`, `PLAN EXÉCUTÉ`) intacts. Fixture scratch `cmdsteps.json`
(deux étapes `RUN_COMMAND`, une réussie, une en échec) rendue pour vérifier l'affichage groupé —
non ajoutée au dépôt.

**Docs mises à jour dans le même changement** : `CLAUDE.md` (crates `policy`/`exec`, dispatch hors
`FsOps`, pompe généralisée + `ApprovalGate`) ; `summary.md` (modèle de sécurité = modes ; lignes
`policy`/`exec` du tableau des crates ; enums `UiToCoreMsg`/`CoreToUiMsg` ; §4 recon + pompe ; §5
état) ; `design.md` (carte d'approbation, marquage « non annulable », icône `run-command.svg`,
rappel que l'accent reste hors de ça) ; `roadmap.md` (P1.1 racines posées, P1.2 partiellement
couvert).

### 35. Chat : plus d'air entre les messages, bloc de raisonnement discret (2026-09-06)

Demande utilisateur : "faire respirer" le chat en espaçant les lignes de texte, et rendre le bloc
de raisonnement plus discret avec une icône à la place du mot "Raisonnement". Analysée avant
implémentation (réflexion demandée explicitement, proposée via `AskUserQuestion`) :

- **Constat qui a recadré la demande : Slint 1.17 n'expose pas d'interlignage.** Vérifié contre la
  doc vendorisée et l'API interne (`internal/core/styled_text.rs`,
  `internal/common/styled_text.rs`) : aucune propriété de type `line-height`/leading sur `Text`,
  `TextInput` ou `StyledText` pour du texte qui wrap. Le `Theme.line-height` existant ne sert qu'à
  la `min-height` des `Label` mono-ligne (accents sur majuscules), pas à l'espacement de lignes
  d'un paragraphe. La demande a donc été redirigée vers ce qui *est* contrôlable : l'espace entre
  les éléments (messages, blocs), pas entre les lignes d'un même paragraphe.
- **Hiérarchie tour/message introduite.** Avant, l'espace entre deux messages différents
  (`ChatMessageRow.padding-top`/`bottom`, `space-1` chacun → 8px cumulés) valait la même chose que
  l'espace entre les parties d'un même message (`spacing: space-2` sur la même ligne → 8px aussi) —
  rien ne distinguait "nouveau tour" de "suite du même message". Le padding de `ChatMessageRow` est
  passé à `space-2` (16px cumulés entre deux messages), le `spacing` interne inchangé : dense à
  l'intérieur d'un message, plus ouvert entre deux. Espacement entre blocs d'une même réponse
  (paragraphe/liste/code, `MessageBlock`) : `space-2` → `space-3`.
- **Bloc de raisonnement : fond et padding conditionnés à l'état.** `background`/`padding` de la
  carte (`Palette.sidebar-hover` + `Theme.space-2`) ne s'appliquent plus que si `live-thinking ||
  thinking-expanded` ; au repos replié c'est une ligne nue (icône + chevron, `padding: 0px`,
  fond transparent) qui se fond dans la conversation au lieu d'un bloc gris permanent pour une
  seule ligne de titre.
- **Icône à la place du mot.** Nouvelle icône `brain` (Lucide, même famille de trait que le reste
  du chrome — `sun`/`moon`/`settings`/etc., voir entrée 32) remplace le `Label` "Raisonnement".
  "Réflexion…" reste du texte pendant la génération : état transitoire, qui doit se lire comme
  "en cours" plutôt que "il y a un raisonnement ici". Chevron de bascule inchangé (`chevron-up`/
  `chevron-down`, `IconButton`, ajouté à l'entrée 32).
- **Écarté après vérification** (pas une option de `AskUserQuestion`, exclu avant même de
  proposer) : tenter de simuler un interlignage plus large via un hack (découpage manuel des
  lignes, wrapper par ligne) — jugé trop fragile pour le gain, alors que l'espacement inter-blocs
  donne un effet perçu proche pour un risque minime.

Vérifié : `cargo check --workspace` sans erreur ; `make ui-check` sans avertissement ; `make
ui-shot` sur les cinq fixtures (`demo-light`, `demo-dark`, `stress`, `streaming`, `approval`) sans
avertissement — PNG relus : bloc de raisonnement nu (icône + chevron, pas de carte) sur
`demo-light`/`stress` (repos), carte pleine + "Réflexion…" sur `streaming` (génération en cours).
Forme de l'icône `brain` vérifiée par dump ASCII de la zone (pas seulement rendue/pas rendue,
comme pour les icônes de l'entrée 32) pour confirmer un tracé reconnaissable, pas un bloc plein ou
un blanc.

**Docs mises à jour dans le même changement** : `design.md` (§ Typographie — nouvelle sous-section
"Espacement du chat et bloc de raisonnement" ; § Rampe d'interaction — note sur le chevron du
raisonnement mise à jour ; § Icônes — `brain` ajoutée à l'inventaire).

### 36. Diagnostic de connexion Ollama + modale de résultat (2026-09-06)

Demande utilisateur, après un cas concret : prompt lancé, Ollama en marche, mais **aucune réponse**
de l'IA (logs : « Starting chat stream request to Ollama » puis rien pendant ~2 min 30 jusqu'à
l'annulation manuelle). Sondage de l'instance : le modèle sélectionné était `nemotron-3-ultra:cloud`
(persisté dans `settings.selected_model`), et `/api/chat` pour ce modèle `:cloud` **ne renvoyait
rien** (`http=000`, 0 octet, timeout à 30 s). **Correction faite après coup** : l'utilisateur a
constaté que le modèle répondait plus tard *sans relancer l'app* — c'était donc (au moins en partie)
un **démarrage à froid** côté hébergé, pas forcément une session non authentifiée. D'où le
traitement du timeout cloud comme un cas explicitement ambigu (voir `CloudUnresponsive` ci-dessous),
et non comme un « il faut se connecter » péremptoire. Trois failles côté Lito rendaient ça muet
quoi qu'il en soit : `reqwest::Client::new()` sans timeout (attente infinie), `stream_chat` sans
`error_for_status()`, et `health_check()` qui ne teste que `/api/tags` (donc l'UI affiche
« connecté »).

- **`crates/llm` — `OllamaClient::diagnose() -> OllamaDiagnosis`.** Sonde réellement le modèle
  sélectionné : GET `/api/tags` (démon joignable ?), puis pour un modèle *local* un contrôle de
  présence via `list_models()`, puis une requête `/api/chat` `stream: true`, `think: false`,
  prompt « ping », le tout borné à `DIAGNOSE_TIMEOUT` (20 s) via `tokio::time::timeout` autour de
  `.send()` **et** du premier `bytes_stream().next()`. Variantes : `Ok`, `DaemonUnreachable`,
  `ModelNotFound`, `CloudSignInRequired` (modèle `:cloud` avec une **erreur d'auth explicite** —
  401/403 ou corps contenant « unauthorized »/« sign in »/« api key »…), `CloudUnresponsive`
  (modèle `:cloud` qui ne renvoie rien avant la limite — **cas ambigu** assumé comme tel : un
  premier appel à un modèle cloud passe par un routage côté hébergé qui peut mettre 1-2 min à
  démarrer à froid, *ou* la session n'est pas authentifiée ; on ne peut pas trancher sur un
  simple timeout, le message couvre les deux et met le « réessaie » en premier),
  `ServerError { status, body }`, `ModelUnresponsive { waited_secs }` (modèle **local**).
  `think: false` exprès : une
  trace de raisonnement ne ferait que retarder le premier octet qu'on chronomètre. `is_cloud_model()`
  (`pub`) : tag `cloud`, `*-cloud`, ou `-cloud` dans le nom. Deux tests hors-ligne
  (`diagnose_reports_unreachable_daemon` sur un port mort, `cloud_model_detection`).
- **Messages.** `UiToCoreMsg::DiagnoseOllama` ; `CoreToUiMsg::DiagnosisResult { ok, needs_signin,
  title, detail }` — chaînes **pré-formatées** en français côté `app` (`describe_diagnosis`), parce
  que `crates/ui` ne dépend pas de `crates/llm`. `handle_diagnose(llm, ui_tx, auto)` : si `auto` et
  connexion saine, on **n'ouvre pas** la modale (pas de pop-up « tout va bien » juste après une
  erreur) ; sinon on émet `DiagnosisResult`. Câblé sur `UiToCoreMsg::DiagnoseOllama` (bouton,
  `auto = false`) **et** sur le bras d'erreur de `SendMessage` dans `run_core_agent` (`auto = true`)
  — le `drive_with_ui_pump` du `SendMessage` distingue maintenant `Ok(Ok)` / `Ok(Err)` / `Err`
  (annulation) au lieu de laisser filer l'erreur vers le `if let Err` générique.
- **UI.** `UiState` gagne `diagnosing: bool` + `diagnosis: Option<DiagnosisView>` ; `sync_to_ui`
  pousse six scalaires (`diagnosis-visible`/`diagnosing`/`-ok`/`-needs-signin`/`-title`/`-detail`).
  Méthodes `ViewModelHandle::diagnose_ollama()` / `dismiss_diagnosis()`. Slint : bouton
  « Diagnostiquer la connexion » (section DIAGNOSTIC du panneau Réglages) + **modale** en enfant
  direct de la `Window` (dernier enfant → peint au-dessus, Slint n'a pas de z-index), scrim
  `#000000a6` avec un `TouchArea` qui gobe les clics **sans** fermer (une instruction `ollama
  signin` à moitié suivie ne doit pas se perdre sur un clic à côté). La carte est imbriquée dans
  des layouts de centrage (pas libre dans le `Rectangle` du scrim) : un enfant libre d'un
  `Rectangle` est mesuré sans contrainte de largeur d'abord, ce qui écrase le texte `wrap` sur une
  ligne puis le tronque quand la vraie largeur s'applique — le corps de texte passe par
  `CopiableText` (sélectionnable, et gère déjà l'entête d'accents). Fond de carte `Palette.sidebar`
  et non `surface` : en thème sombre `surface == bg`, une carte `surface` serait invisible sur le
  scrim. Bloc `ollama signin` (mono, encadré) affiché seulement si `needs_signin`. Nouvelle fixture
  `diagnosis.json` (+ ligne `make ui-shot`).

Vérifié : `make check && make clippy && make test` (llm passe de 1 à 3 tests ; suite complète
verte) ; `make ui-check` sans avertissement ; `make ui-shot` sur les six fixtures — PNG relus
(clair + sombre + états `ok` / `running` via fixtures scratch) : carte centrée, texte non tronqué,
bloc d'instructions complet, bouton « Fermer » à ton `success` quand `ok`. Sonde manuelle de
l'instance : `nemotron-3-ultra:cloud` → `http=000` / timeout au moment du test (puis fonctionnel
plus tard sans redémarrage — cf. démarrage à froid ci-dessus) ; `qwen3.5:4b` local → tokens
immédiats.

**Docs mises à jour dans le même changement** : `summary.md` (ligne `crates/llm` — `diagnose()` /
`OllamaDiagnosis` ; enums `UiToCoreMsg`/`CoreToUiMsg` ; nouvelle ligne du tableau d'état) ;
`design.md` (nouvelle sous-section « Modale de diagnostic Ollama ») ; `CLAUDE.md` (propriétés /
callbacks de `MainWindow`, note fixtures).

### 37. Le message utilisateur est persisté avant l'appel au modèle (2026-09-06)

Bug rapporté dans la foulée de l'entrée 36 : « mes conversations ne sont plus persistées ». Diag
en inspectant `~/Library/Application Support/lito/memory.sqlite` : 6 sessions, mais **aucun message
daté d'aujourd'hui** — le dernier remonte à ~1 mois — alors que deux sessions « Nouvelle tâche »
vides ont bien été créées le jour même. Cause : `handle_send_message` n'écrivait `messages` (user
+ assistant) qu'*après* `agent.respond(...).await?`. Le modèle sélectionné (`nemotron-3-ultra:cloud`)
échouant / expirant à chaque tour, le `?` sortait de la fonction avant toute persistance — le tour
entier, message utilisateur compris, était perdu au redémarrage. Ce n'est donc pas une régression
des entrées 35/36 : c'est le comportement de longue date, rendu visible par un modèle qui ne répond
jamais.

Correctif : `save_message(session_id, "user", …)` + `touch_session` + `set_session_title` (si
`is_first_turn`) + un `session_list_msg` best-effort sont remontés **avant** le `health_check` et
`agent.respond`. La réponse de l'assistant, le plan, et les `history.push(...)` en mémoire restent
réservés au succès — `agent.respond` réinjecte `prompt` lui-même, donc pousser dans `history` tôt
dupliquerait le tour dans le contexte modèle ; et `history` est de toute façon rechargé depuis la
base au redémarrage, donc la divergence base/mémoire d'un tour raté se résorbe seule. Commentaires
des bras `Ok(Err)` / `Err` (annulation) de `run_core_agent` mis à jour en conséquence.

Note sur le « cold start » cloud (question de l'utilisateur) : un modèle `:cloud` n'a pas de
chargement de poids *local* — rien ne tourne sur la machine. Mais le premier appel peut quand même
traîner : le service hébergé planifie/charge un très gros modèle côté serveur, la connexion au
service s'établit, ou il y a de la file d'attente. Le message de `CloudUnresponsive` a été reformulé
dans ce sens (plus de « démarrage à froid » qui laissait entendre un préchauffage local).

Vérifié : `make check && make clippy && make test` verts ; `cargo build --release -p lito-app` OK.
Chemin de persistance revu à la lecture (déplacement de 3 appels bien compris au-dessus d'un
`.await?` faillible, pas de double écriture : user tôt, assistant tard, titre tôt, `history` tard).

**Docs mises à jour dans le même changement** : `summary.md` (ligne « Persistance SQLite » du
tableau d'état).

### 38. Trois correctifs sur la boucle de reconnaissance et son rendu (2026-09-06)

Signalés ensemble à partir d'une session où « Regarde combien de PDF il y a dans mon dossier
Téléchargements » a fini sur `Error (Internal): gave up after 6 filesystem queries without a final
answer`, avec une pile d'indicateurs « Génération… » clignotants dans le chat.

1. **Un seul « Génération… » à la fois.** Chaque tour de requête laissait derrière lui son propre
   message assistant `streaming` (il portait la trace de raisonnement du tour mais jamais de
   contenu visible), et tous affichaient le caret « Génération… » jusqu'à `finish_streaming` en fin
   de tour. `view_model.rs` : nouvelle `settle_query_round`, appelée en tête du bras
   `CoreToUiMsg::Querying` — elle passe le dernier message assistant `streaming` à `streaming =
   false` (sa trace « Réflexion… » auto-dépliée devient un bloc replié ré-ouvrable) ou le supprime
   s'il n'a rien reçu. Seul le message assistant *courant* porte donc encore le caret.
2. **Icône cerveau cliquable.** `app-window.slint` : le glyphe `brain` du bloc de raisonnement est
   enveloppé dans un `TouchArea` (13×13, `enabled: !live-thinking`) qui bascule
   `thinking-expanded` — même effet que le chevron de droite, pour ouvrir le raisonnement passé
   d'un clic sur l'icône. Inerte pendant la génération live (la trace est déjà ouverte).
3. **Plus d'erreur sèche en fin de budget.** `crates/agent` : `MAX_QUERY_ROUNDS` passe de 6 à 10 ;
   le `anyhow::bail!` final est remplacé. Au dernier tour, un tour `user` injecté
   (`FINAL_ROUND_DIRECTIVE`) dit au modèle que son budget d'inspection est consommé et qu'il doit
   conclure (plan, ou réponse en clair — « aucun fichier ne correspond » compris) ; les branches
   `query`/`command` sont gardées par `!final_round`, et si le modèle renvoie malgré tout une
   requête/commande, `respond` répond `FINAL_ROUND_FALLBACK` (message en clair) au lieu de laisser
   du JSON brut dans le chat ou de renvoyer une erreur. La boucle `for` fait toujours un `return` à
   sa dernière itération (`unreachable!()` derrière).

Vérifié : `make ui-check` sans avertissement ; `cargo test -p lito-agent -p lito-ui` vert (31
tests) ; `cargo clippy -p lito-agent -p lito-ui` propre ; `make ui-shot` — six PNG relus, bloc de
raisonnement (replié dans `demo`/`stress`, live dans `streaming`) inchangé visuellement.

**Docs mises à jour dans le même changement** : `learn/07-agent.md` (§ boucle + § budget),
`learn/01-overview.md` (ligne du tableau), `llm_brief/summary.md` (§4), `roadmap.md` (P2.2 point 5).

---

### 39. Bloc de raisonnement : suppression de la carte dans tous les états (2026-09-07)

Suite de l'entrée 35. Cette entrée-là avait conditionné le fond `Palette.sidebar-hover` + le padding
de la carte à `live-thinking || thinking-expanded` : ligne nue au repos replié, carte grise au
dépliage. Retour utilisateur : la transition « pas de fond → fond gris » à l'ouverture se lit comme
un accordéon cassé. Deux sorties acceptables — une carte claire *permanente*, ou aucune carte ; la
seconde a été retenue (une carte permanente dépense un bloc gris entier pour une ligne de chrome sur
une réponse rarement ouverte).

- `app-window.slint`, `ChatMessageRow` : le `Rectangle` du bloc `if msg.thinking != ""` perd
  `background`, `border-radius` et `animate background` ; sa `VerticalLayout` interne perd le
  `padding` conditionnel. Il ne reste qu'un `Rectangle` transparent (conservé sans réindenter) qui
  enveloppe l'en-tête (icône `brain` + chevron / "Réflexion…") et, quand `live-thinking ||
  thinking-expanded`, la trace en `font-sm` / `text-faint` — qui coule directement sous l'en-tête,
  comme les blocs de la réponse coulent sous elle. Séparation portée par le glyphe, la petite
  police atténuée et le `spacing: space-2` de `ChatMessageRow` jusqu'à la réponse.
- Même fichier : l'`IconButton` "Afficher/Masquer le raisonnement" perd ses overrides
  `hover-bg: surface-press` / `press-bg: border` (ils n'existaient que pour s'écarter de la couleur
  de la carte quand `thinking-expanded`) — retour au ghost hover par défaut.

Vérifié : `make ui-check` sans avertissement ; `make ui-shot` — PNG relus (replié dans
`demo-light`/`demo-dark`, plus une fixture jetable `live` : plus aucune carte, trace en texte nu
aligné comme la réponse).

**Docs mises à jour dans le même changement** : `design.md` (§ "Bloc de raisonnement" réécrit,
daté 2026-09-07 ; ligne `bubble-bg` du tableau des tokens).

### 40. `Spinner` : indicateur d'attente quand rien ne stream (2026-09-07)

Retour utilisateur (capture à l'appui) : pendant qu'une commande de reconnaissance tourne, le chat
affiche la ligne `Commande : find . -maxdepth 1 …` **figée**, sans le moindre indicateur, pendant
tout l'aller-retour (politique + approbation éventuelle + `lito_exec::run`, jusqu'à 120 s). Idem
plus généralement quand le modèle réfléchit avant son premier token. Le `StreamingCaret` ne
couvrait pas ces cas : l'entrée 38 (`settle_query_round`) éteint le message assistant `streaming`
dès qu'un `Querying` arrive, et la ligne `Querying` elle-même ne portait aucun état.

Distinction posée : **caret = un flux commencé puis en pause** (il clignote) ; **`Spinner` = un
travail sans aucun flux de tokens** (il tourne). Trois emplacements pour le spinner.

- `app-window.slint` : nouveau composant `Spinner` (après `StreamingCaret`). Un `Rectangle`
  (`size`/`tint`, `tint` et pas `color` — réservé par `Rectangle`) portant une `Image`
  interne = `icons/loader.svg` (arc Lucide `loader-circle` neuf) dont la `transform-rotation`
  avance de 30° / 80 ms via un `Timer`. Rotation sur l'`Image`, pas la racine :
  `transform-rotation` est silencieusement ignorée sur l'élément racine d'un composant
  (`ui-check` le signale). Même parti pris que le caret : `Timer` qui incrémente une propriété,
  pas d'`animate` infinie.
- `ChatMessageRow` : le bloc indicateur (`if !is-user && msg.streaming`) choisit désormais
  `Spinner` vs `StreamingCaret` sur `activity-waiting` (`msg.role == "system" || msg.content ==
  ""`). Libellé : « Exécution en cours… » pour un message `system`, « Génération… » pour un
  assistant encore vide. Le caret reste pour un assistant qui a déjà du texte (flux en pause).
- Liste des étapes de plan : `Spinner` teinté `warn` à gauche du `StatusBadge` quand
  `step.status == "running"` — une étape peut rester sur un `RUN_COMMAND` lent ou une approbation.
- `view_model.rs` : `CoreToUiMsg::Querying` pousse le message `system` avec `streaming: true` (au
  lieu de `false`). Nouvelle `settle_query_spinners` (passe tous les messages `system` `streaming`
  à `false`), appelée en tête de `Token` / `Thinking` / `PlanStreaming` et dans `Querying`. Les
  cas de fin de tour (`MessageDone` / `PlanReady` / `Cancelled` / `Error`) sont déjà couverts par
  `finish_streaming`, qui conserve le message (il a du contenu) mais efface son `streaming`.
- `fixtures/streaming.json` : le message `system` « Listing ~/Downloads » reste réglé, un second
  « Commande : find … » `streaming: true` est ajouté, plus un message assistant vide `streaming`
  — la fixture montre les deux variantes de spinner. `approval.json` / `demo.json` couvrent déjà
  l'étape `running`.

Vérifié : `make ui-check` sans avertissement ; `make check` / `make clippy` / `make test` verts
(22 tests `lito-ui`) ; `make ui-shot` — `streaming.png` (spinner sous `Commande : …` et sous
« Génération… ») et `stress.png` (caret conservé sur le flux en pause, spinner `warn` sur l'étape
COPY `running`) relus.

**Docs mises à jour dans le même changement** : `design.md` (§ "Retour visuel pendant la
génération" — tableau étendu, paragraphe `Spinner`, entrée dans la liste des composants) ;
`summary.md` (ligne « Indicateur de génération »).

### 41. Bloc de raisonnement : aperçu de la trace + bouton texte, plus de chevron (2026-09-07)

Suite des entrées 35 et 39. Retour utilisateur : la ligne repliée « icône `brain` + chevron aligné
à droite » ne dit rien de ce que le modèle a pensé ; demande d'un aperçu des premiers mots + d'un
bouton "Afficher", chevron de droite supprimé, bouton "Voir moins" pour refermer.

- `ChatMessageData` (`app-window.slint`) : nouveau champ `thinking-preview: string`.
- `view_model.rs` : fn pure `thinking_preview(&str) -> String` — première ligne non vide, espaces
  compactés, coupée à 10 mots, `…` final si des mots ou des lignes ont été retirés. Renseignée
  dans `chat_row` ; `ChatRow` / `into_data` portent le champ. 5 tests unitaires (nouveau
  `#[cfg(test)] mod tests` dans `view_model.rs`, le premier de ce fichier).
- `ChatMessageRow` : l'en-tête du bloc `if msg.thinking != ""` devient un `HorizontalLayout`
  `alignment: start` : glyphe `brain` (toujours cliquable, `TouchArea` inchangé) + selon l'état
  soit "Réflexion…" (live), soit le `Label` d'aperçu (`wrap: no-wrap`, `overflow: elide`,
  `max-width: 360px` — l'élision Slint est le garde-fou fenêtre étroite, la coupe utile est celle
  de Rust ; rien en déplié), soit rien ; puis un `TextButton` "Afficher"/"Voir moins" (masqué en
  live) ; puis le spacer `horizontal-stretch: 1`. L'`IconButton` chevron et son wrapper de
  centrage sont supprimés. La trace complète (`live-thinking || thinking-expanded`) coule dessous,
  inchangée.
- `fixtures/*.json` : `thinking-preview` ajouté à chaque message des 5 fixtures de base (valeur
  dérivée du `thinking` existant), sinon l'interpréteur `slint-viewer` plante sur le champ absent.

Vérifié : `make ui-check` sans avertissement ; `cargo test -p lito-ui` vert (14 tests, dont les 5
nouveaux) ; `cargo clippy -p lito-ui` propre ; `make ui-shot` + 2 captures jetables (défaut
`thinking-expanded` basculé le temps d'un rendu) — replié (`brain` + aperçu 10 mots + "Afficher"),
déplié (`brain` + "Voir moins" + trace), live (`brain` + "Réflexion…" + trace, pas de bouton).

**Docs mises à jour dans le même changement** : `design.md` (§ "Bloc de raisonnement" réécrit :
aperçu + bouton ; § "Rampe d'interaction" — note chevron/override obsolète corrigée).

### 42. Coloration syntaxique des blocs de code et des commandes (2026-09-13)

Demande utilisateur : un rendu pour les blocs de code proposés par le modèle et pour les
commandes bash lancées par le modèle, police mono + coloration syntaxique. Les deux existaient déjà
en `Menlo, monospace` (bloc de code de l'entrée markdown, `ApprovalCard`, ligne `$ <commande>` du
panneau de plan) mais en une seule couleur uniforme.

**Piste explorée puis abandonnée** : `StyledText::from_markdown` avec des
`<span style="color:…">` autour de chaque token. La doc vendée
(`documentation/slint-docs/development/text-layout.md`) documente ce support HTML ; vérifié faux
sur la version réellement résolue par `Cargo.lock` (1.17.1, pas la 1.6 du `Cargo.toml`) — un test
Rust direct (`StyledText::from_markdown("<span style=\"color:#5d4a91\">let</span>…")`) renvoie
`Err("HTML tag <span> is not supported")`, confirmé aussi côté compilateur via `@markdown()` dans
un `.slint` jetable. `TextInput`/`Text` n'ont qu'une couleur unique et aucun des deux éléments ne
supporte de « runs » colorés — donc pas de mécanisme Slint pour colorer un token au sein d'une
ligne sans en faire un élément à part.

**Solution retenue** : `crates/ui/src/highlight.rs`, tokenizer maison (pas de dépendance —
`syntect` écarté par choix utilisateur, jugé disproportionné pour des extraits courts) couvrant
shell/rust/python/js-ts/json/toml/yaml/C-like + repli générique (chaînes/nombres/commentaires
seuls). Classe chaque token en une `TokenClass` (`Plain`/`Keyword`/`String`/`Comment`/`Number`/
`Function`) sérialisée en `string` côté Slint. Un bloc coloré devient une rangée de `CodeSpan`
(nouveau composant, un `TextInput` mono-ligne par token) plutôt qu'un seul élément de texte — donc
la sélection clic-glisser continue sur toute une ligne est perdue (compromis validé avec
l'utilisateur avant implémentation), mais chaque token reste sélectionnable individuellement et le
bouton « Copier » du bloc copie toujours tout. `global Syntax` (`color`/`italic`/`weight`, fonctions
pures) fait le lien classe → rendu ; 4 teintes neuves `Palette.syntax-keyword/-string/-number/
-function`, choisies à l'écart de `accent`/`success`/`warn`/`danger` sur le cercle chromatique
(règle « l'accent est réservé au CTA », voir plus haut).

Verrou technique rencontré : un `MessageBlockData`/`PlanStepData` construit **hors** thread UI (le
thread `pump` de `sync_to_ui`, voir la doc de `ChatRow`) ne peut porter aucun champ `[T]` — un
`ModelRc` n'est pas `Send`, donc tout ajout d'un tel champ casse la construction existante. Fixé en
ajoutant un type intermédiaire *Send* par niveau (`BlockRow` avant `MessageBlockData`,
`PlanStepRow` avant `PlanStepData`, chacun portant les spans en `Vec<highlight::Span>` brut) dont
`into_data()` (appelé sur le thread UI, dans la fermeture `upgrade_in_event_loop` ou dans
`ChatRow::into_data`) construit les `ModelRc` imbriqués au dernier moment — même schéma que
`ChatRow`/`ChatMessageData` déjà en place, étendu d'un niveau. Vérifié qu'un `[T]` imbriqué sur deux
niveaux (`MessageBlockData.lines: [CodeLineData]`, `CodeLineData.spans: [CodeSpanData]`) compile et
se charge correctement via `--load-data` (les blocs de code du chat le prouvent).

Deuxième bug, plus coûteux à isoler : le même patron (`ScrollView` dont la hauteur est liée au
`preferred-height` de son contenu, pour un défilement horizontal sans retour à la ligne) fonctionne
pour un bloc de code du chat (au-dessus d'un `ScrollView` simple) mais rend la ligne de commande
**invisible** dans le panneau de plan — `command-row.preferred-height` reste `0` quel que soit le
binding essayé, alors que `step.command-line.length` vaut bien 5 (`debug()` dans un `init`, voir
`.claude/skills/slint/SKILL.md`). Cause : la ligne vit dans une rangée d'un `ListView` (le panneau
de plan, contrairement au chat qui est un `ScrollView` + `VerticalLayout` classique — voir
"Architecture UI" plus haut) ; un `ScrollView` (donc un second `Flickable`) imbriqué dans une rangée
`ListView` ne résout jamais sa taille, probablement une interaction entre l'estimation de hauteur de
ligne du `ListView` et le binding circulaire propre au `Flickable`. Contournement : pas de
défilement horizontal ici, un simple `Rectangle { clip: true; }` autour de la rangée de `CodeSpan` —
et **`min-width: 0px`** dessus, sinon `clip` (qui n'affecte que le rendu, pas la négociation de
taille) laisse le `Rectangle` réclamer la largeur non tronquée de son contenu en amont, ce qui
élargissait toute la colonne `Theme.plan-w` (300px) et donc la fenêtre entière pour une commande
longue (vérifié par rendu avant/après avec une commande fabriquée dépassant largement le panneau).

`crates/ui/ui/fixtures/*.json` : chaque bloc `MessageBlockData` porte maintenant un champ
`"lines": []` (peuplé pour les 2 blocs `"code"` existants avec les spans réels produits par
`highlight::highlight_code`, générés via un test Rust jetable plutôt qu'à la main), chaque étape de
plan un `"command-line": []` (peuplé pour les 3 étapes `RUN_COMMAND` existantes), et
`approval.json` un nouveau `"approval-argv-class"` en regard de `"approval-argv"`.

Vérifié : `cargo test --workspace` vert (23 tests `lito-ui`, dont 8 nouveaux dans
`highlight::tests`) ; `cargo clippy --workspace -- -D warnings` propre ; `make ui-shot` — les six
captures relues, `demo-light`/`demo-dark` (bloc de code coloré, couleurs échantillonnées au pixel
près via `pxsample.py` et confirmées exactes), `stress`/`approval` (ligne de commande colorée dans
le panneau de plan et dans `ApprovalCard`, plus un rendu jetable avec une commande volontairement
trop longue pour vérifier l'absence de débordement hors fenêtre).

**Docs mises à jour dans le même changement** : `summary.md` (nouvelle ligne « Coloration
syntaxique »).

---

*Fin de l'historique — Projet Lito*