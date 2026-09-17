# `crates/app/src/main.rs` — pour un débutant complet

> Suite de `08-memory.md`. C'est le fichier le plus court du tour, mais c'est le "plan de câblage" :
> il assemble tous les morceaux vus jusqu'ici (`ViewModelHandle`, `AppWindow`, `Agent`, `FsOps`,
> `MemoryStore`, `OllamaClient`) en un seul programme qui démarre. Le lire après tous les autres aide
> à voir comment chaque pièce s'imbrique dans les autres.
>
> Dernier fichier "de production" du tour : `10-protocol.md` (honorable mention).

---

## Le rôle de ce fichier, en une phrase

`main()` construit une instance de chaque service (agent, LLM, mémoire, fichiers), crée les deux
canaux de communication UI ↔ core, lance la boucle métier en arrière-plan, puis affiche la fenêtre —
et `run_core_agent` (juste après `main`) est la boucle qui reçoit les messages de l'UI et orchestre les
autres services en réponse.

---

## Concept : `#[tokio::main]` et pourquoi `main` peut être `async`

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    ...
}
```

En Rust "normal" (sans framework), `fn main()` ne peut **pas** être `async` — le langage n'a pas de
runtime asynchrone intégré, il faut en choisir un et le démarrer explicitement. `#[tokio::main]` est
une macro qui fait exactement ça : elle transforme cette fonction en un `fn main()` classique et
synchrone qui, en interne, démarre un runtime Tokio (multi-thread par défaut — plusieurs threads réels
du système d'exploitation qui se partagent l'exécution des tâches asynchrones), puis exécute le corps
`async` que tu as écrit dedans. C'est la porte d'entrée qui rend possible tout le reste (`tokio::spawn`,
`.await`, les canaux `mpsc`...) vu dans les fichiers précédents.

`anyhow::Result<()>` comme type de retour de `main` : si `main` retourne `Err(e)`, le programme
s'arrête avec un message d'erreur affiché automatiquement et un code de sortie non-nul — pratique pour
propager une erreur d'initialisation (ex. `MemoryStore::new(None)?` juste après) sans avoir à
`.unwrap()` ou écrire un `match` explicite dans `main` lui-même.

---

## Initialiser les services

```rust
let agent = Agent::new();
let memory = MemoryStore::new(None)?;
let model = memory.get_setting("selected_model")?.unwrap_or_else(|| "qwen3.5:9b".to_string());
let endpoint = memory.get_setting("ollama_endpoint")?.unwrap_or_else(|| lito_llm::DEFAULT_BASE_URL.to_string());
let llm = OllamaClient::new(endpoint, model);
let fs_ops = FsOps::new();
```

Chaque ligne construit un objet vu dans un fichier précédent : `Agent` (`07-agent.md`), `MemoryStore`
(`08-memory.md`, avec `None` pour utiliser l'emplacement par défaut), `OllamaClient` (mentionné dans
`07-agent.md`), `FsOps` (`06-fs-ops.md`). Remarque `memory.get_setting("selected_model")?` : le modèle
Ollama choisi par l'utilisateur lors d'une session précédente est relu depuis SQLite au démarrage —
`.unwrap_or_else(|| "qwen3.5:9b".to_string())` fournit un modèle par défaut si aucun réglage n'a encore
été sauvegardé (même patron `unwrap_or_else` que dans `08-memory.md`). `ollama_endpoint` suit
exactement le même patron, avec `lito_llm::DEFAULT_BASE_URL` (`http://localhost:11434`, l'adresse
par défaut d'Ollama) comme valeur par défaut plutôt qu'une chaîne codée en dur ici - modifiable
ensuite depuis le panneau "Réglages" de la titlebar (voir plus bas, `handle_set_endpoint`).

---

## Créer les canaux, puis lancer la tâche de fond

```rust
let (ui_to_core_tx, ui_to_core_rx) = mpsc::unbounded_channel::<UiToCoreMsg>();
let (core_to_ui_tx, core_to_ui_rx) = mpsc::unbounded_channel::<CoreToUiMsg>();

let agent_handle = tokio::spawn(run_core_agent(
    agent, llm, memory, fs_ops, ui_to_core_rx, core_to_ui_tx,
));
```

`mpsc::unbounded_channel::<T>()` retourne une **paire** `(Sender<T>, Receiver<T>)` — c'est la
destructuration par tuple : `let (a, b) = fonction_qui_retourne_deux_valeurs();`. Deux canaux
distincts sont créés, un par direction (voir `04-view-model.md` pour l'explication de `mpsc`).

`tokio::spawn(run_core_agent(...))` démarre `run_core_agent` comme une **tâche indépendante** qui va
tourner en parallèle du reste. Remarque que `run_core_agent` est appelée ici comme n'importe quelle
fonction `async` — `tokio::spawn` prend le `Future` qu'elle produit et le confie au runtime pour
exécution ; l'appel ne bloque pas `main`, qui continue tout de suite à la ligne suivante.
`tokio::spawn` retourne un `JoinHandle<T>` (ici `T = ()`), qui permet plus tard soit d'attendre que la
tâche se termine, soit — comme ici, via `view_model.shutdown()` — de l'**annuler** de force
(`.abort()`).

**Important** : `agent`, `llm`, `memory`, `fs_ops` sont **déplacés** (move) dans `run_core_agent` ici
— après cette ligne, `main` ne peut plus du tout les utiliser (ils n'existent plus dans son scope).
C'est voulu : ces services n'ont besoin d'exister que dans la boucle de fond, jamais dans `main` lui-
même après ce point.

---

## Brancher la fenêtre

```rust
let view_model = ViewModel::new(agent_handle);
view_model.handle().set_channels(lito_ui::ChannelHandles { ui_to_core_tx, core_to_ui_rx });

let window = lito_ui::AppWindow::new(view_model.handle())?;
view_model.handle().set_window(window.as_weak());
view_model.handle().ping();

window.run()?;
view_model.shutdown();
```

Chaque étape a été expliquée en détail dans un fichier précédent :
- `ViewModel::new(agent_handle)` : garde le `JoinHandle` de la tâche de fond pour pouvoir l'arrêter
  plus tard (`04-view-model.md`).
- `.set_channels(...)` : donne au `ViewModelHandle` les deux bouts de canaux, et démarre la boucle qui
  pompe les messages `core_to_ui_rx` (`04-view-model.md`).
- `lito_ui::AppWindow::new(...)` : crée la fenêtre Slint et branche chaque callback (`05-app-window.md`).
- `.set_window(window.as_weak())` : donne au `ViewModelHandle` une référence *faible* vers la fenêtre,
  utilisée par `sync_to_ui`/`upgrade_in_event_loop` (`04-view-model.md`).
- `.ping()` : envoie tout de suite un `UiToCoreMsg::Ping`, pour connaître l'état d'Ollama dès l'
  ouverture de l'app plutôt que d'attendre une action de l'utilisateur.
- `window.run()?` : **bloque** le thread principal jusqu'à la fermeture de la fenêtre (voir
  `05-app-window.md` pour `ComponentHandle::run`).
- `view_model.shutdown()` : une fois la fenêtre fermée (donc `window.run()` revenu), annule la tâche
  de fond — pas la peine de la laisser tourner sans interface pour la piloter.

---

## `run_core_agent` : la vraie boucle métier

```rust
async fn run_core_agent(
    agent: Agent,
    mut llm: OllamaClient,
    memory: MemoryStore,
    mut fs_ops: FsOps,
    mut ui_rx: mpsc::UnboundedReceiver<UiToCoreMsg>,
    ui_tx: mpsc::UnboundedSender<CoreToUiMsg>,
) {
    let rows = memory
        .get_recent_messages(MAX_HISTORY_MESSAGES)
        .unwrap_or_default();

    let mut history: Vec<ChatMessage> = rows
        .iter()
        .map(|(role, content, _)| ChatMessage { role: role.clone(), content: content.clone() })
        .collect();

    let _ = ui_tx.send(CoreToUiMsg::SessionSwitched {
        session_id: active_session_id.clone(),
        messages: rows,
        plan,                       // le dernier plan de cette session, s'il y en a un
    });

    loop {
        // ... voir « La boucle principale » ci-dessous
        let result = match msg {
            UiToCoreMsg::SendMessage { prompt, context } => handle_send_message(...).await,
            UiToCoreMsg::ExecutePlan { plan_id } => handle_execute_plan(...).await,
            UiToCoreMsg::Undo { session_id } => handle_undo(...).await,
            UiToCoreMsg::SetModel { model } => handle_set_model(...).await,
            UiToCoreMsg::Ping => handle_ping(...).await,
        };

        if let Err(e) = result {
            tracing::error!("Core agent error: {}", e);
            let _ = ui_tx.send(CoreToUiMsg::Error { code: ..., message: e.to_string() });
        }
    }
}
```

### Pourquoi `mut llm` et `mut fs_ops`, mais pas `mut agent`/`mut memory` ?

Regarde les paramètres : `mut llm: OllamaClient`, `mut fs_ops: FsOps`, mais `agent: Agent` et
`memory: MemoryStore` sans `mut`. Ça reflète directement ce que chaque type a besoin de modifier :
`FsOps::execute_step`/`undo` prennent `&mut self` (le journal d'annulation change, voir
`06-fs-ops.md`) ; `OllamaClient::set_model` prend `&mut self` (changer le modèle utilisé, voir
`handle_set_model` plus bas). `Agent` (voir `07-agent.md`) et `MemoryStore` (voir `08-memory.md`) n'ont
en revanche **que** des méthodes `&self` — `Agent` ne détient aucun état, et `MemoryStore` cache sa
mutation réelle derrière un `Mutex<Connection>` interne. Rust t'oblige à déclarer `mut` explicitement
sur chaque variable dès qu'une seule de ses méthodes utilisées en aura besoin — c'est un signal de
lecture utile : "ce type-là va changer d'état pendant la boucle, celui-ci non".

### La boucle principale : un grand `match` sur les messages de l'UI

Cette boucle tourne pour toute la durée de vie de l'app, réveillée à chaque nouveau `UiToCoreMsg`
envoyé depuis un callback Slint. Le `match` (voir `06-fs-ops.md` pour un premier `match` exhaustif)
dispatch chaque variant vers une fonction `handle_*` dédiée — garder chaque cas dans sa propre
fonction plutôt que tout écrire inline dans la boucle est ce qui rend `handle_send_message`,
`handle_execute_plan`, etc. lisibles indépendamment les unes des autres.

Elle a *presque* la forme du `while let Some(msg) = ui_rx.recv().await` vu dans `04-view-model.md`,
avec une différence qui mérite d'être comprise :

```rust
let mut deferred: VecDeque<UiToCoreMsg> = VecDeque::new();

loop {
    let msg = match deferred.pop_front() {
        Some(msg) => msg,
        None => match ui_rx.recv().await {
            Some(msg) => msg,
            None => break,
        },
    };
    // ... match msg { ... }
}
```

Le problème que ça résout : tant que la boucle `await`ait un handler, elle ne lisait plus le canal.
Pour tous les handlers c'est sans conséquence (ils durent quelques millisecondes), mais une
génération LLM dure des dizaines de secondes — et pendant ce temps, un `UiToCoreMsg::Cancel` envoyé
par le bouton « Arrêter » serait resté coincé dans la file jusqu'à ce que la génération soit…
terminée. Le bouton n'aurait rien annulé du tout.

La branche `SendMessage` attend donc son handler **à côté** du canal, avec `tokio::select!` :

```rust
let send = handle_send_message(/* ... */);
tokio::pin!(send);

let result = loop {
    tokio::select! {
        biased;                                    // la réponse est toujours poll-ée en premier
        result = &mut send => break result,
        incoming = ui_rx.recv() => match incoming {
            Some(UiToCoreMsg::Cancel) => { cancelled = true; break Ok(()); }
            Some(other) => deferred.push_back(other),
            None => { cancelled = true; break Ok(()); }
        },
    }
};
```

- **`tokio::select!`** attend plusieurs futurs en parallèle et exécute la branche du premier prêt. Ici
  : "soit la réponse arrive, soit un message arrive". `biased;` force l'ordre d'évaluation des
  branches (par défaut il est aléatoire) — sans lui, un `Cancel` arrivant dans le même instant qu'une
  réponse déjà terminée pourrait gagner le tirage et faire annoncer une interruption pour un travail
  en réalité fini.
- **`tokio::pin!`** fixe le futur à une adresse mémoire stable, ce dont `select!` a besoin pour le
  re-`poll`er à chaque itération de la boucle.
- **Annuler, c'est dropper le futur.** En Rust, un futur qui n'est plus poll-é ne s'exécute plus, et
  son `Drop` libère ce qu'il tenait : ici la réception du stream Ollama, ce qui fait échouer la tâche
  émettrice, qui sort de sa boucle et drop la réponse `reqwest` — la connexion HTTP se ferme pour de
  bon. Il n'y a aucun drapeau "annulé" à vérifier quelque part : l'arrêt est structurel. Corollaire :
  rien du tour annulé n'est persisté, puisque `handle_send_message` n'écrit `history` et SQLite qu'à
  sa toute fin, où il n'arrive jamais.
- **`deferred`** garde les messages arrivés pendant la génération (changer de session, par exemple :
  la sidebar reste cliquable même si le composer est grisé), rejoués au tour suivant plutôt que
  perdus.

`if let Err(e) = result { ... }` : après avoir traité le message, si le handler a retourné une erreur
(tous retournent `anyhow::Result<()>`), elle est loggée (`tracing::error!`, le système de logs
structuré utilisé dans tout le projet) et renvoyée à l'UI sous forme de `CoreToUiMsg::Error` — plutôt
que de faire planter toute la boucle (et donc toute l'app) sur une seule requête ratée.

---

## `handle_send_message` : le chemin le plus long

```rust
async fn handle_send_message(
    agent: &Agent, llm: &OllamaClient, memory: &MemoryStore, fs_ops: &FsOps,
    ui_tx: &mpsc::UnboundedSender<CoreToUiMsg>,
    history: &mut Vec<ChatMessage>,
    prompt: String, context: String,
) -> anyhow::Result<()> {
    if !llm.health_check().await {
        return Err(anyhow::anyhow!("Ollama is not available"));
    }

    let response = agent.respond(&prompt, &context, history, llm, fs_ops, |event| {
        let msg = match event {
            AgentEvent::Token(token) => CoreToUiMsg::Token { content: token.to_string() },
            AgentEvent::Thinking(chunk) => {
                thinking.push_str(chunk);   // accumulé pour la persistance, voir plus bas
                CoreToUiMsg::Thinking { content: chunk.to_string() }
            }
            AgentEvent::Querying(description) => {
                CoreToUiMsg::Querying { description: description.to_string() }
            }
            AgentEvent::PlanSteps(steps) => CoreToUiMsg::PlanStreaming { steps: steps.to_vec() },
        };
        let _ = ui_tx.send(msg);
    }).await?;

    // ... sauvegarde en mémoire + historique + envoi du résultat final à l'UI
}
```

Cette closure unique est **le** fil qui relie `crates/agent` (qui ne sait rien de l'UI) à `crates/ui`
(qui ne sait rien du LLM) : `main.rs` est le seul endroit qui connaît les deux à la fois, et il ne fait
ici qu'une chose — traduire chaque `AgentEvent` en son `CoreToUiMsg` (voir `07-agent.md` pour l'enum
côté agent). C'est une pure table de correspondance, ce qui est exactement ce qu'on veut à une
frontière de crates : aucune décision n'est prise ici.

Le `.to_string()`/`.to_vec()` sur chaque variante n'est pas de la maladresse : `AgentEvent` **emprunte**
sa donnée (`&'a str`, `&'a [PlanStep]`) pour éviter d'allouer à chaque token, mais l'envoyer dans un
canal veut dire qu'elle doit survivre à l'appel — donc à ce moment précis, et seulement à ce
moment-là, on en prend une copie possédée.

`fs_ops: &FsOps` (une référence en lecture seule, pas `&mut`) est passé tel quel à `respond` : c'est
`crates/agent` qui décide quand l'utiliser pour répondre à une requête du modèle, `main.rs` ne fait
que le brancher.

Une seule branche fait autre chose que traduire : `AgentEvent::Thinking` accumule aussi le morceau
dans une `String thinking` locale à `handle_send_message` (`thinking.push_str(chunk)`), pour avoir la
trace de raisonnement **complète** du tour une fois le stream terminé, pas seulement morceau par
morceau. C'est cette `String` (convertie en `Option<&str>`, `None` si elle est restée vide — les
modèles non-"thinking" n'émettent jamais de morceau `Thinking`) qui est passée à `memory.save_message`.
Cette accumulation est aussi la raison pour laquelle la closure est `FnMut` et non `Fn` : elle mute
une variable capturée de son environnement.

La suite de la fonction sauvegarde le tour de conversation (`memory.save_message("assistant", ...,
thinking)`, voir `08-memory.md`) à la fois en mémoire (le `Vec<ChatMessage> history`, tronqué à
`MAX_HISTORY_MESSAGES` avec `history.drain(0..excess)` s'il devient trop long — cette copie en mémoire
ne garde que `role`/`content`, pas `thinking`, puisqu'elle sert uniquement à reconstruire le prompt
envoyé au modèle) et en SQLite (pour survivre à un redémarrage, `thinking` inclus) — puis, selon que
`agent.respond` a renvoyé `AgentResponse::Plan` ou `AgentResponse::Chat` (`07-agent.md`), sauvegarde le
plan et notifie `PlanReady`, ou notifie simplement `MessageDone`.

---

## `handle_execute_plan` : exécuter un plan sauvegardé, étape par étape

```rust
async fn handle_execute_plan(fs_ops: &mut FsOps, memory: &MemoryStore, ui_tx: ..., plan_id: String) -> anyhow::Result<()> {
    let steps = memory.get_plan(&plan_id)?.ok_or_else(|| anyhow::anyhow!("no plan found for id {plan_id}"))?;

    let session_id = uuid::Uuid::new_v4().to_string();
    memory.update_plan_status(&plan_id, "executing", Some(session_id.clone()))?;

    for (i, step) in steps.iter().enumerate() {
        ui_tx.send(CoreToUiMsg::StepStart { step: i + 1, total: steps.len(), operation: step.operation })?;
        fs_ops.execute_step(step, &session_id).await?;
        ui_tx.send(CoreToUiMsg::StepDone { step: i + 1, total: steps.len() })?;
    }

    ui_tx.send(CoreToUiMsg::PlanDone { undo_id: session_id.clone() })?;
    memory.update_plan_status(&plan_id, "done", Some(session_id))?;
    Ok(())
}
```

`.ok_or_else(|| ...)` transforme un `Option<Vec<PlanStep>>` (`None` si le plan n'a jamais été
sauvegardé, ou plus existe) en `Result`, avec un message d'erreur explicite — même patron que
`require_path` dans `06-fs-ops.md`.

Un seul `session_id` (généré ici, une seule fois, avec `uuid::Uuid::new_v4()`) est partagé par
**toutes** les étapes de cette exécution — c'est exactement ce que `FsOps::undo(session_id)`
(`06-fs-ops.md`) utilisera plus tard pour retrouver et défaire tout ce qu'a fait ce plan précis, en un
seul geste. `for (i, step) in steps.iter().enumerate()` : `.enumerate()` transforme un itérateur de
valeurs en itérateur de paires `(index, valeur)` — pratique ici pour connaître le numéro de l'étape en
cours (`step: i + 1`, en commençant à 1 plutôt que 0 pour l'affichage humain) sans compteur manuel.

---

## `handle_ping`/`handle_set_model`/`handle_set_endpoint` : vérifier/changer le modèle ou l'endpoint

```rust
async fn handle_ping(llm: &OllamaClient, ui_tx: ...) -> anyhow::Result<()> {
    let ollama_ok = llm.health_check().await;
    let available_models = llm.list_models().await.unwrap_or_default();
    ui_tx.send(CoreToUiMsg::Pong {
        ollama: ollama_ok,
        model: llm.model().to_string(),
        available_models,
        endpoint: llm.base_url().to_string(),
    })?;
    Ok(())
}

async fn handle_set_model(llm: &mut OllamaClient, memory: &MemoryStore, ui_tx: ..., model: String) -> anyhow::Result<()> {
    llm.set_model(model.clone());
    memory.set_setting("selected_model", &model)?;
    handle_ping(llm, ui_tx).await
}

async fn handle_set_endpoint(llm: &mut OllamaClient, memory: &MemoryStore, ui_tx: ..., url: String) -> anyhow::Result<()> {
    llm.set_base_url(url.clone());
    memory.set_setting("ollama_endpoint", &url)?;
    handle_ping(llm, ui_tx).await
}
```

`handle_set_model` change le modèle actif, le sauvegarde en base (pour le retrouver au prochain
démarrage — c'est exactement ce que `main()` relit avec `memory.get_setting("selected_model")` en
tout début de fichier), puis **réutilise** `handle_ping` pour renvoyer un état à jour à l'UI — plutôt
que de dupliquer la logique "vérifier Ollama + lister les modèles + notifier". `handle_set_endpoint`
(depuis le panneau "Réglages" de la titlebar) est la même fonction au champ près : elle change
l'adresse plutôt que le modèle (`settings.ollama_endpoint` au lieu de `settings.selected_model`),
mais réutilise `handle_ping` de la même façon — si l'URL est mauvaise, `health_check()` échoue et
l'UI affiche "hors ligne" au lieu de laisser croire que le changement a marché. Au tout début de
`main()`, `memory.get_setting("ollama_endpoint")` retombe sur `lito_llm::DEFAULT_BASE_URL` si rien
n'a encore été enregistré (première ouverture de l'app). Un bon exemple de petite fonction
réutilisée telle quelle depuis une autre, sans over-engineering, appliqué deux fois de suite.

---

## Pour aller plus loin

| Sujet | Où |
|-------|-----|
| `#[tokio::main]` | https://docs.rs/tokio/latest/tokio/attr.main.html |
| `tokio::spawn` / `JoinHandle` | https://docs.rs/tokio/latest/tokio/task/ |
| `tracing` (logs structurés) | https://docs.rs/tracing/latest/tracing/ |
| Destructuration de tuples (`let (a, b) = ...`) | https://doc.rust-lang.org/book/ch18-03-pattern-syntax.html |
| Vue d'ensemble de l'architecture (niveau "vue du dessus") | `documentation/lito/learn/01-overview.md` |
