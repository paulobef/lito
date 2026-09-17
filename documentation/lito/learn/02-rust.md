# Niveau Rust — Architecture et détails d'implémentation

> Ce document explique le code Rust tel qu'il existe aujourd'hui, pour un développeur qui découvre
> Rust. Il pointe les concepts clés, les patterns utilisés, et ce qui reste à faire.
>
> Pour la partie interface graphique (Slint), voir `documentation/lito/learn/03-slint-ui.md`.

---

## Structure du workspace Cargo

Le workspace vit **à la racine du dépôt** (pas dans un sous-dossier `lito-v2/` ou `lito-rust/` —
ces anciens sous-dossiers ont été supprimés) :

```
lito/
├── Cargo.toml              # Workspace root + dépendances partagées
├── Makefile                 # build / run / check / clippy / ui-shot / package / dmg
├── crates/
│   ├── app/                 # Binaire principal (entry point) : lito-app
│   ├── ui/                  # UI Slint + ViewModel
│   ├── protocol/            # Vocabulaire partagé : Operation, PlanStep, FsQuery, ErrorCode, PlanId
│   ├── agent/                # Génération : prompt + historique → réponse ou Vec<PlanStep>
│   ├── fs/                  # Opérations fichiers, requêtes LIST/STAT, undo log
│   ├── llm/                 # Client HTTP Ollama
│   └── memory/               # Persistance SQLite (sessions, messages, plans, réglages)
├── Info.plist               # Métadonnées macOS pour le bundle .app
└── assets/                  # Icônes, ressources du bundle
```

### `Cargo.toml` (workspace)

```toml
[workspace]
members = [
    "crates/app", "crates/ui", "crates/protocol",
    "crates/agent", "crates/fs", "crates/llm", "crates/memory",
]
resolver = "2"

[workspace.dependencies]
tokio = { version = "1.40", features = ["full"] }
serde = { version = "1.0", features = ["derive"] }
rusqlite = { version = "0.31", features = ["bundled"] }
reqwest = { version = "0.12", features = ["json", "rustls-tls", "stream"] }
slint = { version = "1.6", features = ["renderer-skia"] }
slint-build = "1.6"
uuid = { version = "1.8", features = ["v4", "serde", "v7"] }
anyhow = "1.0"
# ... (voir Cargo.toml pour la liste complète : notify, walkdir, keyring,
#      lopdf/docx-rs/calamine pour le parsing de documents, fastembed pour les embeddings —
#      ces cinq-là sont déclarées mais aucune crate ne les utilise encore)
```

**Points clés :**
- **Workspace** : compilation unique, dépendances partagées, `cargo build` compile tout.
- **`workspace.dependencies`** : versions centralisées — chaque crate fait
  `tokio = { workspace = true }` plutôt que de fixer sa propre version.
- La version `slint` installée est en réalité **1.17.1** au moment de l'écriture (le `1.6` du
  `Cargo.toml` est une contrainte minimale que `cargo` résout vers la dernière compatible — vérifie
  toujours `Cargo.lock` si tu tombes sur une API qui ne correspond pas à la doc que tu lis).

---

## `crates/app` — Point d'entrée binaire

**Fichier** : `crates/app/src/main.rs`

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let agent = Agent::new();
    let memory = MemoryStore::new(None)?;
    // Le modèle, le thème et l'endpoint sont relus depuis la table `settings` avant de construire
    // le client - `DEFAULT_BASE_URL` est l'adresse par défaut d'Ollama lui-même.
    let model = memory.get_setting("selected_model")?.unwrap_or_else(|| "qwen3.5:9b".to_string());
    let endpoint = memory.get_setting("ollama_endpoint")?.unwrap_or_else(|| lito_llm::DEFAULT_BASE_URL.to_string());
    let llm = OllamaClient::new(endpoint, model);
    let fs_ops = FsOps::new();

    let (ui_to_core_tx, ui_to_core_rx) = mpsc::unbounded_channel::<UiToCoreMsg>();
    let (core_to_ui_tx, core_to_ui_rx) = mpsc::unbounded_channel::<CoreToUiMsg>();

    let agent_handle = tokio::spawn(run_core_agent(
        agent, llm, memory, fs_ops, ui_to_core_rx, core_to_ui_tx,
    ));

    let view_model = ViewModel::new(agent_handle);
    view_model.handle().set_channels(lito_ui::ChannelHandles { ui_to_core_tx, core_to_ui_rx });

    let window = lito_ui::AppWindow::new(view_model.handle())?;
    view_model.handle().set_window(window.as_weak());
    view_model.handle().ping();

    window.run()?;       // bloque le thread principal (event loop Slint)
    view_model.shutdown(); // abort la tâche core agent quand la fenêtre se ferme

    Ok(())
}
```

**Ce qui se passe :**
- `#[tokio::main]` démarre un runtime Tokio **multi-thread** (par défaut).
- Le cœur métier (`run_core_agent`) tourne dans une tâche `tokio::spawn`ée — donc potentiellement sur
  n'importe quel thread du pool Tokio.
- `window.run()` **bloque le thread principal** pour faire tourner la boucle d'événements Slint : rien
  d'autre ne s'exécute sur ce thread tant que la fenêtre est ouverte. C'est pour ça que la
  communication UI ↔ core doit passer par des canaux thread-safe (`mpsc`) et par
  `Weak::upgrade_in_event_loop` plutôt que par de l'état partagé muté directement — voir
  `03-slint-ui.md`.

### La boucle de dispatch, et pourquoi elle n'est pas un simple `while let`

`run_core_agent` dispatche chaque `UiToCoreMsg` vers un handler dédié (`handle_send_message`,
`handle_execute_plan`, `handle_undo`, `handle_new_session`, `handle_switch_session`,
`handle_set_model`, `handle_set_theme`, `handle_ping`) et remonte toute erreur sous forme de
`CoreToUiMsg::Error`.

Une génération est la seule chose assez longue pour qu'on veuille l'interrompre. Or tant que la
boucle `await`ait `handle_send_message`, elle ne lisait plus le canal — un `Cancel` serait resté dans
la file jusqu'à ce que la génération soit… terminée. D'où la forme actuelle :

```rust
let send = handle_send_message(/* ... */);
tokio::pin!(send);

let result = loop {
    tokio::select! {
        biased;                                    // la réponse est toujours poll-ée en premier
        result = &mut send => break result,
        incoming = ui_rx.recv() => match incoming {
            Some(UiToCoreMsg::Cancel) => { cancelled = true; break Ok(()); }
            Some(other) => deferred.push_back(other),   // repris au tour suivant
            None => { cancelled = true; break Ok(()); }
        },
    }
};
```

Trois choses à comprendre ici :

- **`tokio::pin!`** — `select!` a besoin de re-`poll`er le même futur à chaque itération, donc de le
  tenir à une adresse stable. `pin!` le fixe sur la pile ; sans ça `&mut send` ne serait pas `Unpin`
  et ne compilerait pas.
- **Dropper le futur *est* l'annulation.** En Rust un futur qui n'est plus poll-é ne s'exécute plus,
  et son `Drop` libère tout ce qu'il tenait : ici le `Receiver` du stream de `stream_chat`, dont la
  tâche émettrice échoue alors à envoyer, sort de sa boucle, et drop la réponse `reqwest` — ce qui
  ferme la connexion HTTP vers Ollama. Il n'y a pas de `CancellationToken` ni de flag à vérifier :
  l'arrêt est structurel.
- **`deferred`** — les messages arrivés pendant la génération (changer de session, par exemple : la
  sidebar reste cliquable) sont mis de côté dans une `VecDeque` et rejoués avant de retourner lire le
  canal, plutôt que d'être perdus.

Rien du tour annulé n'est persisté : `handle_send_message` n'écrit `history` et SQLite qu'à la toute
fin, et n'y arrive jamais.

---

## `crates/protocol` — vocabulaire partagé

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString)]
#[strum(serialize_all = "SCREAMING_SNAKE_CASE")]
pub enum Operation { Mkdir, Move, Copy, Delete, Rename, Symlink, WriteFile }

pub struct PlanStep {
    pub id: String,
    pub description: String,
    pub operation: Operation,
    pub path: Option<PathBuf>,
    pub target: Option<PathBuf>,
    pub content: Option<String>,   // WRITE_FILE uniquement
}

pub enum QueryOp { List, Stat }
pub struct FsQuery { pub query: QueryOp, pub path: PathBuf }
pub struct QueryResult { pub query: QueryOp, pub path: PathBuf,
                         pub entries: Vec<FileMetadata>, pub truncated: bool }
```

Ce crate ne contient plus que ce qui est réellement partagé entre `agent`, `fs`, `memory`, `app` et
`ui`. Les anciens enums `ClientMessage`/`ServerMessage` (sérialisation NDJSON externally-tagged,
hérités de l'architecture socket/pipe) et le type `SessionId` ont été **supprimés** : rien ne les
construisait ni ne les parsait. La vraie communication UI ↔ core passe par `UiToCoreMsg`/`CoreToUiMsg`
définis dans `crates/ui` — pas de `Serialize`/`Deserialize`, pas de coût de sérialisation puisque tout
est dans le même process. N'y réintroduis pas un enum de messages sans avoir vérifié que ces deux-là
ne couvrent pas déjà le besoin.

Le casing `SCREAMING_SNAKE_CASE` reste, lui : c'est ce que le prompt système demande au modèle
d'écrire, donc c'est bien un contrat vivant.

---

## `crates/agent` — Génération et planification

```rust
pub enum AgentResponse { Chat(String), Plan(Vec<PlanStep>) }

pub enum AgentEvent<'a> {
    Token(&'a str),               // texte de la réponse, au fil de l'eau
    Thinking(&'a str),            // trace de raisonnement
    Querying(&'a str),            // « Listing ~/Desktop »
    PlanSteps(&'a [PlanStep]),    // les étapes déjà écrites, liste complète à chaque fois
}

impl Agent {
    pub async fn respond<F: FnMut(AgentEvent)>(
        &self, prompt: &str, context: &str, history: &[ChatMessage],
        llm: &OllamaClient, fs: &FsOps, on_event: F,
    ) -> Result<AgentResponse> { /* ... */ }
}
```

`Agent` ne détient aucun état — `respond` prend `&self`. C'est **volontairement** un type léger : il
ne fait *que* de la génération. L'exécution et l'undo sont de la responsabilité de `FsOps` (voir plus
bas) — `Agent` n'a pas de méthode `execute`/`undo`.

Trois points d'implémentation méritent d'être connus avant de toucher au fichier (le détail est dans
`07-agent.md`) :

- **Un seul callback, pas quatre.** `on_event: FnMut(AgentEvent)` a remplacé les closures
  positionnelles `on_token`/`on_thinking`/`on_query` : à trois c'était déjà limite, à quatre le site
  d'appel devenait illisible. Un `enum` à durée de vie empruntée (`&'a str`, `&'a [PlanStep]`) évite
  en prime d'allouer une `String` par token pour la jeter aussitôt.
- **Le buffering plan/chat.** Tant qu'on ne sait pas si la réponse est du texte ou du JSON, les
  tokens sont retenus. Un plan à un caractère près (`[{"id":"a"…`) n'a aucun sens dans une bulle de
  chat. La décision se prend sur le premier caractère non-blanc (`looks_like_plan`), et si la réponse
  ne parse finalement ni en plan ni en requête, tout le tampon est émis d'un coup.
- **`StreamingPlanParser`.** Le texte retenu n'est pas perdu pour autant : il alimente un petit
  automate qui repère les frontières d'objets JSON (en suivant l'état chaîne/échappement, pour que
  les accolades dans un `content` de WRITE_FILE ne comptent pas) et donne chaque objet complet à
  `serde_json`. `serde_json` n'a pas de mode incrémental, d'où l'automate. Il ne traite **que** les
  tableaux de premier niveau : une réponse commençant par `{` est ambiguë (plan d'une étape ou
  requête `{"query": …}`), donc il se désarme plutôt que de deviner.

---

## `crates/llm` — Client Ollama

```rust
pub struct OllamaClient { client: reqwest::Client, base_url: String, model: String }

pub struct ChatMessage { pub role: String, pub content: String }
pub enum ChatChunk { Thinking(String), Content(String) }

impl OllamaClient {
    pub async fn health_check(&self) -> bool { /* GET /api/tags */ }
    pub async fn list_models(&self) -> Result<Vec<String>> { /* GET /api/tags */ }
    pub async fn complete(&self, system: &str, prompt: &str) -> Result<String> { /* /api/generate */ }
    pub async fn stream(&self, system: &str, prompt: &str)
        -> Result<impl Stream<Item = Result<String>>> { /* /api/generate, stream: true */ }
    pub async fn stream_chat(&self, messages: Vec<ChatMessage>)
        -> Result<impl Stream<Item = Result<ChatChunk>>> { /* /api/chat, stream: true */ }
    pub fn set_model(&mut self, model: impl Into<String>);
}
```

`stream_chat` est ce que l'app utilise réellement : contrairement à `/api/generate`, `/api/chat` prend
la liste des tours précédents, donc le modèle voit la conversation. La requête envoie toujours
`think: true` — les modèles de raisonnement (qwen3, deepseek-r1, qwq…) remplissent alors un champ
`thinking` séparé, et les autres l'ignorent silencieusement, donc c'est sans risque.

**Le détail qui compte dans les deux méthodes de streaming** : Ollama répond en NDJSON, mais les
frontières de lignes ne coïncident pas avec les morceaux renvoyés par `bytes_stream()`. Un morceau
HTTP peut contenir zéro, une, ou plusieurs lignes complètes, et se terminer au milieu d'une. Les deux
méthodes accumulent donc dans un `line_buf` et ne parsent que jusqu'au dernier `\n`. Le tout est
transposé dans un canal `mpsc` plutôt que dans une chaîne de combinateurs : le fan-out (un morceau →
n tokens) et le tampon de ligne sont beaucoup plus lisibles dans une boucle de tâche.

Toutes les méthodes prennent `&self` sauf `set_model` : le client ne mute rien, il peut être partagé
par référence entre tâches sans `Mutex`.

---

## `crates/fs` — Exécution des opérations, requêtes, undo log

```rust
pub struct FsOps { undo_log: Vec<UndoEntry> }

struct UndoEntry { session_id: String, operation: UndoOperation }

impl FsOps {
    pub async fn execute_step(&mut self, step: &PlanStep, session_id: &str) -> Result<()> {
        match step.operation {
            Operation::Mkdir => self.create_dir_all(/* ... */).await,
            Operation::Move | Operation::Rename => self.move_file(/* ... */).await,
            Operation::WriteFile => self.write_file(/* ... */).await,
            // ... Copy, Delete, Symlink de la même façon
        }
    }

    /// Lecture seule : ne passe pas par l'undo log (rien n'est muté).
    pub async fn run_query(&self, query: &FsQuery) -> Result<QueryResult> { /* LIST / STAT */ }

    pub async fn undo(&mut self, session_id: &str) -> Result<()> {
        let entries: Vec<_> = self.undo_log.iter()
            .filter(|e| e.session_id == session_id).cloned().collect();
        for entry in entries.into_iter().rev() {
            self.reverse_operation(&entry.operation).await?;
        }
        self.undo_log.retain(|e| e.session_id != session_id);
        Ok(())
    }
}
```

**Points clés :**
- Chaque opération mutante (`create_dir_all`, `move_file`, `copy_file`, `delete`, `write_file`,
  `create_symlink`) pousse un `UndoEntry` **taggé par `session_id`** — c'est ce qui permet à
  `undo(session_id)` de ne rejouer que les opérations d'une exécution de plan donnée, dans l'ordre
  inverse.
- `delete` ne supprime jamais réellement un fichier : il le renomme vers un chemin `.bak.<uuid>`, ce
  qui rend l'undo trivial (renommer en sens inverse) tant que le backup n'a pas été nettoyé.
- `session_id` est une simple `String` (un UUID généré par `crates/app/src/main.rs` avec
  `uuid::Uuid::new_v4().to_string()`), pas un type `SessionId` dédié — volontairement, pour éviter une
  couche de parsing/`FromStr` qui n'apportait rien tant que rien d'autre dans le code ne consomme un
  `SessionId` typé. Attention au faux ami : ce `session_id`-là identifie **une exécution de plan**,
  pas la conversation.
- `run_query` plafonne un LIST à `MAX_LIST_ENTRIES` (500) et signale la troncature dans
  `QueryResult::truncated`, pour que le modèle sache qu'il n'a pas tout vu.
- `require_path`/`require_pair` sont de petits helpers qui transforment un `PlanStep.path`/`target`
  manquant (`Option<PathBuf>` à `None`) en une erreur `anyhow` explicite plutôt qu'un panic plus loin.

> ⚠️ Il n'y a **aucune** validation de chemin aujourd'hui : rien n'empêche un plan de viser
> `/System` ou `~/.ssh`. C'est l'objet de la phase 1 de `roadmap.md` (`PathPolicy`).

---

## `crates/memory` — Persistance SQLite

```rust
pub struct MemoryStore { conn: std::sync::Mutex<Connection> }
```

Le schéma couvre les **sessions** (`create_session`, `list_sessions`, `touch_session`,
`set_session_title`), les **messages** d'une session (`save_message` — avec une colonne `thinking`
pour la trace de raisonnement, `get_recent_messages_for_session`), les **plans**
(`save_plan`, `get_plan`, `update_plan_status`, `latest_plan_for_session`) et des **réglages**
clé/valeur (`get_setting`/`set_setting`, utilisés pour le modèle sélectionné et le thème).

Le point le plus important ici n'est pas fonctionnel mais **de concurrence** : `rusqlite::Connection`
est `Send` mais pas `Sync` (il contient des `RefCell` internes pour le cache de requêtes préparées).
`run_core_agent` tourne dans une tâche `tokio::spawn`ée et garde des `&MemoryStore` vivantes à travers
des points `.await` (par exemple dans `handle_send_message`, qui fait `llm.health_check().await` puis
utilise encore `memory` après) — pour qu'un `Future` soit `Send` (requis par `tokio::spawn`), toute
référence partagée qu'il traverse doit pointer vers un type `Sync`. D'où le `Mutex<Connection>` :
chaque méthode locke brièvement le mutex, exécute sa requête SQLite (synchrone — `rusqlite` n'est pas
async), puis relâche le lock avant de retourner. Si tu vois une erreur de compilation `future cannot
be sent between threads safely` qui pointe vers `RefCell`/`InnerConnection`, c'est très probablement
le même problème ailleurs — enveloppe la ressource non-`Sync` dans un `Mutex` plutôt que d'essayer de
contourner le bound `Send` de `tokio::spawn`.

---

## Compiler et tester

```bash
cd lito   # racine du dépôt — plus de sous-dossier lito-v2/

cargo check --workspace     # ou: make check
cargo build --release       # ou: make build
cargo run -p lito-app       # ou: make run
cargo test --workspace      # ou: make test — attention, crates/llm a un test qui requiert Ollama
cargo clippy --workspace -- -D warnings   # ou: make clippy
```

Pour tout changement visuel, il y a une boucle séparée (`make ui-check`, `make ui-shot`) décrite dans
`03-slint-ui.md` et dans le skill `.claude/skills/slint/SKILL.md`.

---

## Prochaines étapes Rust (prioritaires)

Le détail, avec critères d'acceptation, est dans `documentation/lito/roadmap.md`.

1. **`PathPolicy` et confinement** (phase 1) : la lacune la plus sérieuse du code actuel.
2. **Tool-calling natif Ollama** (P2.1) : `stream_chat` accepterait des `tools` et émettrait un
   `ChatChunk::ToolCall`. `looks_like_plan`, `strip_code_fence` et `StreamingPlanParser` deviendraient
   inutiles sur ce chemin — mais le chemin JSON-in-text actuel reste nécessaire en repli pour les
   modèles sans support des outils.
3. **Mock HTTP pour `crates/llm`** (P0.4) : `wiremock`, notamment pour tester `stream_chat` (NDJSON
   coupé n'importe où, champ `thinking` présent ou absent, troncature UTF-8 au milieu d'un caractère
   multi-octets).
4. **Surveillance de fichiers** : `notify`/`walkdir` sont des dépendances du workspace mais aucune
   crate ne les utilise encore — les variants `CoreToUiMsg::FileCreated`/`FileMoved` existent déjà
   côté UI et ne sont émis nulle part.
5. **RAG local** (`fastembed`, `sqlite-vec`) : dépendances présentes, aucune implémentation.
