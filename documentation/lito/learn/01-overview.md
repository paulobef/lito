# Vue d'ensemble de l'application Lito

## Qu'est-ce que Lito ?

Lito est une application macOS native qui s'appuie sur un LLM local (Ollama) pour transformer une
demande en langage naturel en un plan d'actions sur le système de fichiers (créer un dossier,
déplacer, copier, supprimer, renommer, écrire un fichier, créer un lien symbolique), puis exécuter ce
plan avec un historique d'annulation (undo) systématique.

**Il n'existe qu'une seule implémentation** : un **binaire Rust unique**, avec une interface Slint
native compilée dans le même exécutable. Il n'y a ni processus séparé, ni IPC, ni ligne Swift/AppKit —
tout tourne dans un seul process, sur un seul runtime Tokio.

> Une ancienne implémentation (app SwiftUI + binaire Rust "sidecar" communiquant par pipes stdin/stdout
> ou par socket Unix) a existé en parallèle sous `LitoApp/` et `lito-rust/`. Elle a été **supprimée** :
> ce document, et le reste de `documentation/`, ne décrivent plus que l'architecture actuelle.

---

## Architecture globale

```
┌───────────────────────────────────────────────────────────────────┐
│                       lito-app (1 seul process)                    │
│                                                                     │
│   ┌─────────────────────┐   mpsc channels   ┌──────────────────┐  │
│   │   UI (Slint)         │ ─────────────────▶ │  Core agent loop │  │
│   │   AppWindow          │  UiToCoreMsg        │  run_core_agent  │  │
│   │   ViewModel(Handle)  │ ◀───────────────── │                  │  │
│   └─────────────────────┘   CoreToUiMsg       └────────┬─────────┘  │
│                                                          │           │
│                                    ┌─────────────────────┼────────┐ │
│                                    ▼                      ▼        │ │
│                              lito-agent              lito-fs       │ │
│                              (planning,               (exécution   │ │
│                               via lito-llm)             + undo log) │ │
│                                    │                      │         │ │
│                                    ▼                      ▼         │ │
│                              lito-llm               lito-memory    │ │
│                              (client Ollama          (SQLite:      │ │
│                               HTTP)                   sessions,    │ │
│                                                       messages,    │ │
│                                                       plans)       │ │
└───────────────────────────────────────────────────────────────────┘
```

### Composants principaux

| Composant | Rôle | Crate |
|-----------|------|-------|
| **UI Slint** | Fenêtre, sidebar de sessions, chat, panneau de plan, composer | `lito-ui` (`crates/ui`) |
| **ViewModel / ViewModelHandle** | État UI partagé, pont synchrone/asynchrone vers le core agent | `lito-ui` |
| **Core agent loop** | Boucle `run_core_agent` : reçoit les `UiToCoreMsg`, orchestre génération/exécution/undo | `crates/app/src/main.rs` |
| **Agent** | `Agent::respond` : prompt + historique → réponse conversationnelle **ou** `Vec<PlanStep>`, avec boucle de requêtes LIST/STAT | `lito-agent` |
| **OllamaClient** | Client HTTP Ollama : health check, `/api/tags`, `/api/generate`, `/api/chat` en streaming | `lito-llm` |
| **FsOps** | Exécution réelle des opérations fichiers, requêtes LIST/STAT, undo log en mémoire | `lito-fs` |
| **MemoryStore** | Persistance SQLite : sessions, messages (avec trace de raisonnement), plans, réglages | `lito-memory` |
| **protocol** | Vocabulaire partagé : `Operation`, `PlanStep`, `FsQuery`/`QueryResult`, `ErrorCode`, `PlanId` | `lito-protocol` |

---

## Flux de communication (intra-process)

Il n'y a pas de sérialisation JSON dans le chemin critique : la communication UI ↔ core se fait via
deux canaux `tokio::sync::mpsc::unbounded_channel` typés (définis dans `crates/ui/src/view_model.rs`) :

1. **UI → Core** (`UiToCoreMsg`) : `SendMessage`, `Cancel`, `ExecutePlan`, `Undo`, `RejectPlan`,
   `NewSession`, `SwitchSession`, `SetModel`, `SetTheme`, `SetEndpoint`, `Ping`.
2. **Core → UI** (`CoreToUiMsg`) : `SessionSwitched`, `SessionList`, `Token`, `Thinking`, `Querying`,
   `PlanStreaming`, `MessageDone`, `Cancelled`, `PlanReady`, `PlanRejected`, `StepStart`, `StepDone`,
   `PlanDone`, `UndoDone`, `FileCreated`, `FileMoved`, `Pong`, `Error`.

Déroulé typique d'une demande utilisateur :

1. L'utilisateur tape un prompt et appuie sur Entrée dans la fenêtre Slint.
2. Le callback Slint `send-message` appelle `ViewModelHandle::send_message` (synchrone), qui met à
   jour l'état UI de façon optimiste (message utilisateur + message assistant vide marqué
   `streaming`) puis envoie `UiToCoreMsg::SendMessage` sur le canal.
3. `run_core_agent` (tâche `tokio::spawn`ée depuis `main.rs`) reçoit le message et appelle
   `Agent::respond(prompt, context, history, &llm, &fs, on_event)`. Contrairement au reste des
   handlers, celui-ci est attendu **à côté** du canal entrant (`tokio::select!`), pour qu'un
   `UiToCoreMsg::Cancel` puisse interrompre la génération en cours.
4. `Agent::respond` streame la réponse d'Ollama (`/api/chat`) et émet des `AgentEvent` au fil de
   l'eau, que `main.rs` traduit en `CoreToUiMsg` :
   - `Token` — texte de la réponse, mot à mot ;
   - `Thinking` — trace de raisonnement d'un modèle « thinking » ;
   - `Querying` — l'agent inspecte le disque (LIST/STAT) avant de répondre ;
   - `PlanSteps` → `PlanStreaming` — les étapes du plan déjà écrites par le modèle, au fur et à
     mesure qu'elles se referment dans le JSON.
5. À la fin du tour, la réponse est classée : soit `AgentResponse::Chat` (→ `MessageDone`), soit
   `AgentResponse::Plan` (→ le plan est persisté puis renvoyé via `PlanReady`). Le tour complet
   (message utilisateur, réponse, trace de raisonnement) est écrit en SQLite.
6. Une tâche dédiée (le "pump" démarré par `ViewModelHandle::set_channels`) reçoit chaque message et
   appelle `apply_core_message`, qui met à jour `UiState` — sans toucher à l'UI. Le pump vide ensuite
   tout ce qui est déjà en file dans le même lot, puis pousse l'état vers la fenêtre Slint via
   `sync_to_ui`/`Weak::upgrade_in_event_loop`, au plus 30 fois par seconde (voir
   `documentation/lito/learn/03-slint-ui.md` pour le détail du threading et
   `04-view-model.md` pour le lot et le diff).
7. Si l'utilisateur clique "Exécuter", `UiToCoreMsg::ExecutePlan` déclenche `handle_execute_plan` :
   chargement du plan sauvegardé depuis SQLite, exécution étape par étape via `FsOps::execute_step`,
   notifications `StepStart`/`StepDone` à chaque étape, puis `PlanDone` avec un `undo_id`.
8. Si l'utilisateur clique "Annuler", `UiToCoreMsg::Undo { session_id }` appelle
   `FsOps::undo(session_id)` qui rejoue en sens inverse toutes les opérations loggées sous cet
   identifiant d'exécution.

### Streaming : ce que l'utilisateur voit pendant que le modèle écrit

C'est un point de conception à part entière, pas un détail d'implémentation — voir
`documentation/lito/learn/07-agent.md` pour le mécanisme complet.

| Nature de la réponse | Ce qui est streamé | Où |
|---|---|---|
| Conversation | chaque token dès son arrivée | bulle de chat |
| Raisonnement (modèles « thinking ») | chaque morceau de trace | bloc « Réflexion… » repliable |
| Plan | chaque **étape** dès que son objet JSON se referme | panneau de plan, état `streaming` |
| Requête LIST/STAT | une ligne de statut par requête | message système dans le chat |

Le JSON brut d'un plan n'est **jamais** affiché dans le chat : tant que la réponse ressemble à un
plan, les tokens sont retenus (ils ne veulent rien dire un caractère à la fois) et confiés à
`StreamingPlanParser`, qui en extrait des `PlanStep` complets. Un caret clignotant
(`StreamingCaret`) marque la génération en cours, et le bouton « Arrêter » du composer envoie
`UiToCoreMsg::Cancel`, qui coupe réellement la connexion HTTP vers Ollama.

---

## État actuel

| Fonctionnalité | Statut |
|----------------|--------|
| `cargo check --workspace` / `clippy -D warnings` | ✅ propres |
| UI Slint (sessions, chat, plan, thème clair/sombre) | ✅ fonctionnelle, rendue et vérifiée en PNG (`make ui-shot`) |
| Conversation multi-tour (`Agent::respond`) | ✅ historique borné à `MAX_HISTORY_MESSAGES`, envoyé à `/api/chat` |
| Streaming token par token (chat, raisonnement) | ✅ de `stream_chat` jusqu'à l'UI |
| Streaming des plans étape par étape | ✅ `StreamingPlanParser` → `PlanStreaming` |
| Interruption de la génération | ✅ « Arrêter » → `Cancel` → le futur est droppé, la requête HTTP se ferme |
| Requêtes filesystem de l'agent (LIST/STAT) | ✅ bouclées, plafonnées par `MAX_QUERY_ROUNDS` |
| Exécution de plan (`FsOps::execute_step`) | ✅ les 7 opérations (MKDIR/MOVE/COPY/DELETE/RENAME/SYMLINK/WRITE_FILE) |
| Undo par exécution (`FsOps::undo`) | ✅ undo log en mémoire, groupé par `session_id` d'exécution |
| Persistance SQLite (`MemoryStore`) | ✅ sessions, messages + trace de raisonnement, plans, réglages |
| Sélection du modèle, thème persisté | ✅ table `settings` |
| Confinement des chemins / validation de sécurité | ❌ pas de `PathPolicy` — voir `roadmap.md`, phase 1 |
| Tool-calling natif Ollama | ❌ toujours du JSON-in-text deviné à la lecture — voir `roadmap.md` P2.1 |
| Surveillance de dossiers (`notify`) | ❌ pas branchée (dépendance présente, non utilisée) |
| Recherche vectorielle / RAG (`fastembed`, `sqlite-vec`) | ❌ pas implémentée |
| Packaging `.app` / DMG | ✅ cibles Makefile (`make package`, `make dmg`), ad-hoc codesign |

---

## Prochaines étapes logiques

Elles sont priorisées, chiffrées et accompagnées de critères d'acceptation dans
`documentation/lito/roadmap.md` — ce qui suit n'en est qu'un résumé.

1. **Sécurité et confinement** (phase 1) : aucune politique de chemins aujourd'hui, l'agent peut
   proposer une opération n'importe où sur le disque.
2. **Tool-calling natif** (P2.1) : remplacer la devinette `looks_like_plan` par des outils déclarés
   avec un schéma. Le `StreamingPlanParser` disparaîtrait avec elle, les tool calls étant déjà
   structurés.
3. **Boîte à outils élargie** (P2.2) : `read_file`, `find` récursif, extraction PDF/DOCX/XLSX.
4. **Tests** : `crates/llm` a un test qui requiert un Ollama local ; un mock HTTP (`wiremock`) est
   prévu en P0.4.
