# Niveau Slint — Interface graphique et threading

> Ce fichier remplace l'ancien `03-swift.md` : il n'y a plus de code Swift/AppKit dans ce dépôt.
> L'interface est maintenant écrite en [Slint](https://slint.dev/), un langage déclaratif compilé en
> Rust, intégré directement dans le binaire `lito-app` (pas de fenêtre séparée, pas d'IPC).
>
> Pour la doc officielle vendored de Slint, voir `documentation/slint-docs/`. Ce document explique
> uniquement **comment ce projet l'utilise**.

---

## Où vit l'UI

```
crates/ui/
├── build.rs                 # slint_build::compile("ui/app-window.slint")
├── ui/
│   └── app-window.slint      # markup Slint : structs, propriétés, callbacks, layout
└── src/
    ├── lib.rs                # mod generated { slint::include_modules!() } + re-exports
    ├── app_window.rs         # AppWindow : wrapper Rust autour du composant généré
    └── view_model.rs         # ViewModel / ViewModelHandle : état + pont vers le core agent
```

---

## Le fichier `.slint`

Extrait de `crates/ui/ui/app-window.slint` :

```slint
export struct ChatMessageData { role: string, content: string }
export struct PlanStepData { step: int, total: int, description: string, operation: string, status: string }

export component MainWindow inherits Window {
    in property <bool> is-processing: false;
    in property <bool> ollama-connected: false;
    in property <[ChatMessageData]> messages: [];
    in property <[PlanStepData]> plan-steps: [];
    // ... current-plan-id, current-session-id, show-plan, ollama-model

    callback send-message(string, string);
    callback execute-plan(string);
    callback undo(string);
    callback ping();

    VerticalBox {
        // ScrollView, pas ListView : voir design.md, "Défilement de la conversation"
        chat-scroll := ScrollView {
            VerticalLayout {
                alignment: start;
                for msg in root.messages: HorizontalBox { /* ... */ }
            }
        }
        if root.show-plan: VerticalBox { /* liste des étapes + boutons Execute/Undo */ }
        HorizontalBox { /* champ de saisie + bouton Send + bouton Ping */ }
    }
}
```

**Concepts clés Slint :**
- **`in property <T> name: default;`** : une propriété que le code Rust peut définir (`in`) — le
  compilateur Slint génère un `set_<name>()` (kebab-case → snake_case, donc `is-processing` devient
  `set_is_processing`).
- **`callback name(Args...);`** : un événement que Slint déclenche et que Rust écoute — génère un
  `on_<name>(closure)` côté Rust et un `root.<name>(...)` appelable depuis le markup (ex: dans
  `clicked => { root.send-message(...); }`).
- **`export struct` / `<[T]>`** : un type de données struct exporté génère un type Rust du même nom
  (`ChatMessageData`, `PlanStepData`) ; `<[T]>` est un modèle (liste) — côté Rust ça devient un
  `ModelRc<T>`, alimenté par ex. via `ModelRc::new(VecModel::from(vec![...]))`.
- **`for x in model : Element { }`** et **`if cond : Element { }`** : boucle et affichage conditionnel
  déclaratifs — pas de `if`/`for` impératif dans le markup, tout est réactif aux changements de
  propriété.

`build.rs` appelle `slint_build::compile("ui/app-window.slint")` ; c'est ce qui génère, à la
compilation, un module Rust contenant `struct MainWindow`, ses `new()`/`set_*`/`on_*`, etc.

---

## Le piège du nommage : `MainWindow` vs `AppWindow`

Le composant Slint s'appelle `MainWindow`, **pas** `AppWindow` — et c'est délibéré. Le composant
généré par `slint::include_modules!()` a le même nom que le composant `export`é dans le `.slint`. Si
on l'avait laissé s'appeler `AppWindow` (comme il s'appelait à l'origine), il serait entré en conflit
avec le wrapper Rust `AppWindow` qu'on écrit à la main dans `app_window.rs` — c'était d'ailleurs
exactement le bug qui empêchait `crates/ui` de compiler avant ce refactor (`crate::ui::AppWindow`
n'existait nulle part, et le vrai composant généré vivait à la racine du crate, pas dans un module
`ui`).

La solution retenue dans `crates/ui/src/lib.rs` :

```rust
mod generated {
    slint::include_modules!();   // génère `MainWindow` ici, isolé dans son propre module
}
pub use generated::{ChatMessageData, MainWindow, PlanStepData};
pub use app_window::AppWindow;   // notre wrapper, nom différent, pas de collision
```

Si tu renommes le composant `.slint`, garde cette séparation : le composant généré et le wrapper Rust
ne doivent jamais partager un nom.

---

## `AppWindow` (`app_window.rs`) — le wrapper

```rust
pub struct AppWindow { window: MainWindow, view_model: ViewModelHandle }

impl AppWindow {
    pub fn new(view_model: ViewModelHandle) -> Result<Self, slint::PlatformError> {
        let window = MainWindow::new()?;
        let vm = view_model.clone();
        window.on_send_message(move |prompt, context| {
            vm.send_message(prompt.to_string(), context.to_string());
        });
        // ... on_execute_plan, on_undo, on_ping, un `vm.clone()` par callback
        Ok(Self { window, view_model })
    }

    pub fn run(&self) -> Result<(), slint::PlatformError> { self.window.run() }
    pub fn as_weak(&self) -> slint::Weak<MainWindow> { self.window.as_weak() }
}
```

`run()` et `as_weak()` viennent du trait `slint::ComponentHandle`, implémenté automatiquement pour
tout composant généré — il faut `use slint::ComponentHandle;` en scope pour que ces appels résolvent
(ce sont des méthodes de trait, pas des méthodes inhérentes). `run(&self)` lance
`show()` + la boucle d'événements Slint + `hide()` ; il **bloque** l'appelant jusqu'à la fermeture de
la fenêtre.

Chaque callback appelle directement une méthode **synchrone** de `ViewModelHandle` (`send_message`,
`execute_plan`, `undo`, `ping`) — pas besoin de `slint::spawn_local` ici, parce que ces callbacks
s'exécutent déjà sur le thread UI, et que ces méthodes ne font qu'écrire dans un `Mutex` d'état local
et envoyer sur un `mpsc::UnboundedSender` (une opération non bloquante, pas un `.await`).

---

## `ViewModel` / `ViewModelHandle` (`view_model.rs`) et le threading

C'est la partie la plus subtile du crate. Deux directions de flux, deux contraintes différentes :

### UI → Core (synchrone, déjà sur le bon thread)

Un clic déclenche un callback Slint → callback appelle `ViewModelHandle::send_message(...)` →
`send_to_core` fait `tx.send(msg)` sur un `mpsc::UnboundedSender` (envoi non bloquant, ne requiert pas
`.await`). Rien de spécial à faire ici : Slint garantit que ses callbacks tournent sur le thread UI.

### Core → UI (asynchrone, sur un thread Tokio quelconque)

`ViewModelHandle::set_channels` démarre, via `tokio::spawn`, une boucle qui tourne pour toute la durée
de vie de l'app :

```rust
tokio::spawn(async move {
    let mut last_sync: Option<Instant> = None;
    while let Some(msg) = core_to_ui_rx.recv().await {
        handle.apply_core_message(msg);              // mute UiState, ne touche pas l'UI
        while let Ok(msg) = core_to_ui_rx.try_recv() {
            handle.apply_core_message(msg);          // tout ce qui est déjà en file
        }
        // ... attend le reste de MIN_SYNC_INTERVAL si besoin, puis redrain
        handle.sync_to_ui();                          // un seul push pour tout le lot
        last_sync = Some(Instant::now());
    }
});
```

Noter la séparation : `apply_core_message` **ne pousse rien** vers l'UI. Un modèle local émet des
tokens bien plus vite que 30 Hz, et chaque push coûte un saut de thread plus une passe de layout sur
la conversation — donc on applique tout immédiatement à l'état, et on ne limite que le push.

Cette boucle tourne sur un thread du pool Tokio — **pas** le thread UI. Or Slint interdit d'appeler
`.upgrade()` sur un `Weak<MainWindow>` depuis un autre thread que celui qui a créé la fenêtre (ça
retourne simplement `None`, silencieusement). La solution est `Weak::upgrade_in_event_loop` :

```rust
fn sync_to_ui(&self) {
    let Some(window) = self.ui_window.lock().unwrap().clone() else { return };
    let state = self.state.lock().unwrap().clone();
    // ... construire les ModelRc<ChatMessageData>/<PlanStepData> à partir de `state`

    let _ = window.upgrade_in_event_loop(move |window| {
        window.set_is_processing(state.is_processing);
        // ... tous les autres set_*
    });
}
```

`upgrade_in_event_loop` poste la closure sur la boucle d'événements Slint (peu importe le thread
appelant, car `Weak<T>` est `Send`/`Sync` même si `T` ne l'est pas) ; elle sera exécutée avec un
composant "fort" (upgradé) **sur le thread UI**, à la prochaine itération de la boucle. C'est le seul
moyen sûr de pousser un changement d'état depuis une tâche Tokio vers l'UI Slint dans cette
architecture — si tu ajoutes un nouveau point de mise à jour de l'UI depuis du code asynchrone, passe
par `sync_to_ui` (ou le même pattern), ne stocke pas et n'upgrade pas un `Weak` toi-même.

### `ViewModel` vs `ViewModelHandle`

- `ViewModel` est possédé par `main.rs` : il détient le `JoinHandle` de la tâche core agent
  (`agent_handle`) et peut l'`abort()` via `shutdown()` à la fermeture de la fenêtre.
- `ViewModelHandle` est `Clone` (état partagé derrière des `Arc<Mutex<...>>>`) — c'est ce qu'on
  distribue aux callbacks Slint et à la tâche `run_core_agent` via `.handle()`.

---

## Étendre l'UI

Pour ajouter un nouvel élément d'état visible dans la fenêtre :
1. Déclarer la propriété dans `app-window.slint` (`in property <T> mon-champ: default;`).
2. L'ajouter à `UiState` dans `view_model.rs`.
3. La renseigner dans `sync_to_ui()` (`window.set_mon_champ(...)`).

Pour ajouter une nouvelle action déclenchée par l'utilisateur :
1. Déclarer `callback mon-action(Args...);` dans le `.slint` et l'appeler depuis le markup
   (`clicked => { root.mon-action(...); }`).
2. Ajouter `window.on_mon_action(move |args| { vm.mon_action(args); })` dans `app_window.rs`.
3. Ajouter la méthode correspondante sur `ViewModelHandle`, qui pousse un nouveau variant de
   `UiToCoreMsg` (à ajouter aussi) sur le canal sortant.

---

## Ressources utiles

| Sujet | Lien |
|-------|------|
| Doc officielle Slint (vendored) | `documentation/slint-docs/` |
| Slint language reference | https://docs.slint.dev/latest/docs/slint/ |
| Slint Rust API (`ComponentHandle`, `Weak`, `Model`) | https://docs.slint.dev/latest/docs/rust/slint/ |
| Tokio `mpsc` | https://docs.rs/tokio/latest/tokio/sync/mpsc/ |
