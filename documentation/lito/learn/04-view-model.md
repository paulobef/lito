# `crates/ui/src/view_model.rs` — pour un débutant complet en Rust

> Ce document explique ce fichier **en partant de zéro** : il ne suppose aucune connaissance de Rust
> au-delà de « c'est un langage de programmation ». Si un mot te semble déjà connu, tu peux sauter le
> paragraphe qui l'explique.
>
> Ce fichier est **le plus subtil du projet** (voir `CLAUDE.md`) : c'est le pont entre l'interface
> graphique (Slint) et le reste du programme (le "core agent"). Beaucoup de concepts Rust de base y
> sont utilisés en même temps, ce qui en fait un bon point de départ pour apprendre le langage à
> travers du vrai code.
>
> Pour la suite du tour : `05-app-window.md` (le fichier qui utilise `ViewModelHandle`).
> Pour une explication plus avancée/complète du même fichier : `03-slint-ui.md`.

---

## Le rôle de ce fichier, en une phrase

Lito a deux "mondes" qui tournent en parallèle : la fenêtre (Slint) que l'utilisateur voit et clique,
et la logique métier (le "core agent") qui parle à Ollama et au système de fichiers. Ces deux mondes
ne peuvent pas se parler directement — `view_model.rs` est l'objet qui fait le lien : il garde l'état
affiché à l'écran, reçoit les clics de l'utilisateur pour les transmettre au core, et reçoit les
réponses du core pour les répercuter à l'écran.

---

## Concept : qu'est-ce qu'un "crate" ?

En Rust, un **crate** est l'équivalent d'une "librairie" ou d'un "package" dans d'autres langages
(un `.jar` en Java, un package npm en JavaScript...). Ce dépôt est un **workspace** : plusieurs
crates (`crates/ui`, `crates/fs`, `crates/agent`, ...) compilés ensemble, chacun avec sa
responsabilité. `view_model.rs` fait partie du crate `lito-ui` (dossier `crates/ui`).

Au tout début du fichier :

```rust
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use slint::{ModelRc, VecModel, Weak};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use lito_protocol::{ErrorCode, Operation, PlanStep};

use crate::generated::{ChatMessageData, MainWindow, PlanStepData};
```

Chaque ligne `use` importe un nom depuis un autre endroit :
- `std::...` : la bibliothèque standard de Rust, toujours disponible, aucune dépendance à ajouter.
- `slint::...`, `tokio::...` : des crates externes, listées dans `Cargo.toml` (voir `02-rust.md` pour
  le détail du fichier `Cargo.toml`). `slint` est le framework d'interface graphique ; `tokio` est le
  *runtime asynchrone* (explication plus bas).
- `lito_protocol::...` : un **autre crate de ce même projet** (`crates/protocol`), qui définit des
  types partagés comme `PlanStep`.
- `crate::generated::...` : du code généré automatiquement à partir du fichier `.slint` par
  `build.rs` (voir `05-app-window.md`) — `MainWindow` n'est écrit nulle part à la main, il est
  produit par le compilateur Slint à partir de `crates/ui/ui/app-window.slint`.

---

## Concept : `struct` (une structure de données)

```rust
#[derive(Debug, Clone, Default)]
pub struct UiState {
    pub is_processing: bool,
    pub ollama_connected: bool,
    pub ollama_model: String,
    pub available_models: Vec<String>,
    pub current_plan_id: String,
    pub current_session_id: String,
    pub show_plan: bool,
    pub messages: VecDeque<ChatMessage>,
    pub plan_steps: Vec<PlanStepView>,
}

/// Chat message for UI display
#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: String, // "user", "assistant", "system"
    pub content: String,
    pub streaming: bool,
    pub thinking: Option<String>,
}
```

Une `struct` regroupe plusieurs valeurs sous un seul nom, comme un objet avec seulement des champs
(pas de méthodes attachées directement dans sa définition — les méthodes s'ajoutent séparément avec
un bloc `impl`, voir plus bas). `UiState` est **tout ce que l'interface a besoin d'afficher** : est-ce
qu'on est en train de traiter une demande (`is_processing`), la liste des messages du chat
(`messages`), les étapes du plan en cours (`plan_steps`), etc.

Remarque : la trace de raisonnement ("thinking") n'est **pas** un champ à part sur `UiState` — elle
vit sur `ChatMessage.thinking: Option<String>`, un champ par message plutôt qu'un accumulateur global.
Une version antérieure de ce fichier gardait `thinking`/`is_thinking` directement sur `UiState`
(la trace du tour en cours, effacée au tour suivant) ; ça a été remplacé par un champ par message pour
que la trace de raisonnement de chaque tour **persiste avec ce tour** — elle peut être rechargée depuis
SQLite au démarrage (voir `CoreToUiMsg::SessionSwitched` plus bas et `08-memory.md`) exactement comme le
`content` du message, plutôt que de disparaître au redémarrage de l'app ou dès que le tour suivant
commence.

- `Vec<String>` : un tableau (liste) redimensionnable de `String`. `Vec<T>` est le type de liste "par
  défaut" en Rust, équivalent d'une `ArrayList`/liste Python/tableau JS.
- `VecDeque<ChatMessage>` : comme un `Vec`, mais optimisé pour ajouter/retirer des éléments **des deux
  côtés** (au début et à la fin) efficacement — utile ici car on ajoute des messages à la fin, mais on
  pourrait vouloir purger les plus anciens au début un jour.
- `bool`, `String` : types de base — un booléen et une chaîne de caractères "possédée" (par
  opposition à `&str`, une chaîne juste "empruntée" — voir plus bas la section sur l'emprunt).

### `#[derive(...)]`

`#[derive(Debug, Clone, Default)]` est une **macro d'attribut** : elle demande au compilateur de
générer automatiquement du code pour cette struct, sans que tu aies à l'écrire toi-même :
- `Debug` : permet d'afficher la valeur avec `{:?}` (utile pour déboguer avec `println!` ou les logs).
- `Clone` : permet de dupliquer la valeur avec `.clone()` (une vraie copie indépendante en mémoire).
- `Default` : permet de créer une valeur "par défaut" avec `UiState::default()` (ici : `false` pour
  les booléens, chaînes vides, listes vides — la valeur "zéro" de chaque champ).

Ces trois dérivations reviennent tout le temps dans ce projet ; une fois que tu les reconnais, tu
peux lire `#[derive(Debug, Clone, Default)]` comme "cette struct sait s'afficher pour le debug, se
copier, et a une valeur par défaut", sans qu'aucune de ces trois choses ne soit écrite à la main.

---

## Concept : `enum` (une valeur qui est l'un de plusieurs cas possibles)

```rust
pub enum UiToCoreMsg {
    SendMessage { prompt: String, context: String },
    ExecutePlan { plan_id: String },
    Undo { session_id: String },
    SetModel { model: String },
    Ping,
}
```

Une `enum` (énumération) décrit une valeur qui ne peut être **qu'un seul** de plusieurs cas nommés
("variants"). Ici, un `UiToCoreMsg` est *soit* un `SendMessage` (avec un `prompt` et un `context`),
*soit* un `ExecutePlan` (avec un `plan_id`), *soit* ... etc. C'est très différent d'une struct : une
struct a *tous* ses champs en même temps, une enum n'en a qu'**un seul cas actif** à la fois, comme
une union taguée.

C'est le mécanisme central de toute la communication dans ce fichier : `UiToCoreMsg` liste tout ce
que l'UI peut demander au core (envoyer un message, exécuter un plan, annuler, changer de modèle,
"ping" de santé), et `CoreToUiMsg` (juste après dans le fichier) liste tout ce que le core peut
répondre (`Token`, `PlanReady`, `Error`, ...). Si demain tu ajoutes une nouvelle action utilisateur,
tu ajoutes un nouveau variant ici.

Pour lire/traiter une enum, on utilise `match` (voir plus bas dans `apply_core_message`).

---

## Concept : `Option<T>` — "peut-être une valeur, peut-être rien"

Tu ne le vois pas directement dans ce fichier sous cette forme, mais `CoreToUiMsg::Pong` contient un
`available_models: Vec<String>` qui peut être vide — et plus loin dans le projet (`crates/fs`,
`crates/memory`), tu croiseras beaucoup `Option<T>`. Le principe : Rust n'a pas de `null`. Une valeur
qui peut être absente est explicitement typée `Option<T>`, avec deux cas possibles : `Some(valeur)`
ou `None`. Ça force le code à gérer le cas "il n'y a rien" au moment de la compilation plutôt que de
planter à l'exécution avec un `NullPointerException`. Retiens ce nom, il revient dans presque tous
les autres fichiers du projet.

---

## Concept : `&self` vs `&mut self` (emprunter vs emprunter en mutable)

```rust
impl ViewModelHandle {
    pub fn set_window(&self, window: Weak<MainWindow>) { ... }
    fn send_to_core(&self, msg: UiToCoreMsg) { ... }
    pub fn send_message(&self, prompt: String, context: String) { ... }
}
```

Un bloc `impl NomDuType { ... }` ajoute des méthodes à un type. Le premier paramètre d'une méthode est
presque toujours une forme de `self` (le "récepteur" — l'objet sur lequel on appelle la méthode, comme
`this` dans d'autres langages) :
- `&self` : la méthode **emprunte** (lit) `self` sans le modifier. Tu peux appeler plusieurs méthodes
  `&self` "en même temps" (au sens de plusieurs emprunts en lecture simultanés).
- `&mut self` : la méthode a besoin de **modifier** `self`. Rust n'autorise **qu'un seul** emprunt
  mutable actif à la fois sur une valeur donnée — c'est la règle centrale du système d'emprunt
  ("borrow checker") qui empêche toute une classe de bugs de concurrence à la compilation.

Toutes les méthodes publiques de `ViewModelHandle` ici sont `&self`, jamais `&mut self` — c'est un
choix délibéré : l'état mutable réel (`UiState`) est caché **derrière** un `Mutex` (voir juste après),
donc l'objet `ViewModelHandle` lui-même n'a jamais besoin d'être emprunté en mutable pour changer son
contenu.

---

## Concept : `Arc<Mutex<T>>` — partager une valeur entre plusieurs endroits, en sécurité

```rust
#[derive(Clone)]
pub struct ViewModelHandle {
    state: Arc<Mutex<UiState>>,
    ui_window: Arc<Mutex<Option<Weak<MainWindow>>>>,
    ui_to_core_tx: Arc<Mutex<Option<mpsc::UnboundedSender<UiToCoreMsg>>>>,
}
```

Deux briques s'empilent ici :

- **`Mutex<T>`** ("mutual exclusion") : une boîte qui contient une valeur `T` et garantit qu'**une
  seule partie du code à la fois** peut y accéder. Pour lire ou modifier la valeur à l'intérieur, il
  faut d'abord "verrouiller" le mutex avec `.lock()`, ce qui rend un accès temporaire à la valeur ; le
  verrou se relâche automatiquement quand cet accès sort de portée. C'est nécessaire ici parce que
  `UiState` va être lu et modifié depuis plusieurs endroits différents (callbacks Slint, tâche
  asynchrone qui reçoit les messages du core) — sans `Mutex`, deux accès simultanés à la même mémoire
  seraient un bug de "data race", que Rust refuse de compiler.
- **`Arc<T>`** ("Atomically Reference Counted") : un pointeur partagé vers une valeur, qui compte
  combien de "propriétaires" pointent vers elle et libère la mémoire seulement quand le dernier
  disparaît. `Arc` permet à plusieurs parties du code (potentiellement sur des threads différents) de
  pointer vers **la même donnée en mémoire**, plutôt que chacune sa copie.

`Arc<Mutex<T>>` est un patron très courant en Rust : "une valeur partagée entre plusieurs endroits,
protégée contre les accès simultanés". C'est exactement ce que `ViewModelHandle` a besoin, puisqu'il
est `#[derive(Clone)]` et distribué à plusieurs callbacks (voir `05-app-window.md`) — chaque `.clone()`
d'un `ViewModelHandle` obtient une **nouvelle référence vers le même `Arc`**, pas une copie de
`UiState` : modifier l'état via un clone est visible depuis tous les autres clones.

```rust
pub fn update_ui_state<F>(&self, f: F)
where
    F: FnOnce(&mut UiState),
{
    self.mutate_state(f);   // verrouille, applique, relâche
    self.sync_to_ui();
}

fn mutate_state<F>(&self, f: F)
where
    F: FnOnce(&mut UiState),
{
    let mut state = self.state.lock().unwrap();
    f(&mut state);
}
```

Il y a bien **deux** fonctions, et la distinction compte. `update_ui_state` mute *et* pousse vers
l'UI : c'est ce qu'utilisent les méthodes publiques appelées depuis les callbacks Slint
(`send_message`, `ping`…), qui tournent déjà sur le thread UI et veulent un effet immédiat.
`mutate_state` mute seulement : c'est ce qu'utilise `apply_core_message`, parce que le pump décide
lui-même quand pousser (voir plus bas).

- `.lock()` retourne un `Result` (le verrouillage peut échouer si un autre thread a paniqué en tenant
  le verrou) ; `.unwrap()` dit "je considère que ça ne peut pas arriver ici, panique si ça arrive".
  Tu verras `.unwrap()` après beaucoup de `.lock()` dans ce projet — c'est le style standard pour un
  `Mutex` qu'on ne s'attend jamais à voir "empoisonné".
- Le verrou pris dans `mutate_state` est relâché à la fin de cette fonction (Rust libère un
  `MutexGuard` quand il sort de portée), donc **avant** que `update_ui_state` n'appelle
  `sync_to_ui()` — important, parce que `sync_to_ui()` va lui-même vouloir verrouiller `self.state`
  (voir plus bas) ; si le verrou était encore tenu, ce serait un deadlock (le thread s'attend
  lui-même indéfiniment).

---

## Concept : les closures (fonctions "anonymes" qu'on passe en argument)

`f: F where F: FnOnce(&mut UiState)` : `update_ui_state` prend en paramètre **une fonction** (une
"closure"), pas juste une donnée. `FnOnce(&mut UiState)` veut dire "un bloc de code appelable une fois,
qui reçoit un `&mut UiState`". Tous les appels du fichier ressemblent à :

```rust
self.update_ui_state(|state| {
    state.is_processing = true;
    state.messages.push_back(ChatMessage { .. });
});
```

`|state| { ... }` est la closure elle-même : la syntaxe `|arguments| corps` définit une fonction sans
nom, qui "capture" son environnement (les variables autour d'elle) si besoin. Ce patron —
"`update_ui_state` verrouille le mutex, applique ta closure, puis rafraîchit l'UI" — évite de répéter
`lock()` / modification / `sync_to_ui()` à chaque endroit qui touche l'état.

---

## Concept : canaux asynchrones (`tokio::sync::mpsc`)

```rust
pub struct ChannelHandles {
    pub ui_to_core_tx: mpsc::UnboundedSender<UiToCoreMsg>,
    pub core_to_ui_rx: mpsc::UnboundedReceiver<CoreToUiMsg>,
}
```

Un **channel** (canal) est une file d'attente entre deux bouts de code, souvent sur des threads/tâches
différentes : un côté `Sender` y dépose des valeurs avec `.send(valeur)`, l'autre côté `Receiver` les
récupère dans l'ordre avec `.recv().await`. `mpsc` veut dire "multi-producer, single-consumer" :
plusieurs `Sender` peuvent exister (ici, chaque clone de `ViewModelHandle` a le sien via l'`Arc`), mais
un seul `Receiver` les consomme. `Unbounded` veut dire que la file n'a pas de taille limite — envoyer
ne bloque donc jamais (contrairement à un channel "borné").

C'est le mécanisme **exact** utilisé pour le flux UI → core (`ui_to_core_tx`) et core → UI
(`core_to_ui_rx`) mentionné dans `CLAUDE.md`. Pas de JSON, pas de sérialisation : juste des valeurs
Rust typées qui voyagent d'une tâche à l'autre en mémoire.

```rust
pub fn set_channels(&self, channels: ChannelHandles) {
    *self.ui_to_core_tx.lock().unwrap() = Some(channels.ui_to_core_tx);

    let handle = self.clone();
    let mut core_to_ui_rx = channels.core_to_ui_rx;
    tokio::spawn(async move {
        let mut last_sync: Option<Instant> = None;
        while let Some(msg) = core_to_ui_rx.recv().await {
            handle.apply_core_message(msg);
            // Tout ce qui est déjà arrivé rejoint le même lot.
            while let Ok(msg) = core_to_ui_rx.try_recv() {
                handle.apply_core_message(msg);
            }
            if let Some(elapsed) = last_sync.map(|t| t.elapsed()) {
                if elapsed < MIN_SYNC_INTERVAL {
                    tokio::time::sleep(MIN_SYNC_INTERVAL - elapsed).await;
                    while let Ok(msg) = core_to_ui_rx.try_recv() {
                        handle.apply_core_message(msg);
                    }
                }
            }
            handle.sync_to_ui();
            last_sync = Some(Instant::now());
        }
    });
}
```

Pourquoi ce lot plutôt qu'un simple `handle.apply_core_message(msg)` par message : pendant une
génération, le core envoie un `Token` toutes les quelques millisecondes — bien plus vite qu'un écran
ne peut afficher. Chaque push vers l'UI coûte un saut vers le thread Slint plus une passe de layout
sur toute la conversation. Donc chaque message est appliqué à `UiState` dès son arrivée (rien n'est
perdu ni retardé dans le modèle), et seul le **push** est limité à `MIN_SYNC_INTERVAL` (33 ms,
~30 Hz). Le premier message d'une rafale part immédiatement : une réponse isolée n'attend pas.
Mesuré : 121 tokens à ~125/s donnent 29 pushes.

- `try_recv()` (contrairement à `recv().await`) ne bloque pas : il rend `Err` immédiatement si la
  file est vide. C'est ce qui permet de vider ce qui est là sans attendre ce qui ne l'est pas encore.

- `tokio::spawn(async move { ... })` démarre une **tâche asynchrone** : un bout de travail qui tourne
  "en arrière-plan", planifié par le runtime Tokio (voir la section suivante sur `async`/`await`). Le
  mot `move` dit à la closure de **prendre possession** des variables qu'elle utilise (ici `handle` et
  `core_to_ui_rx`) plutôt que de les emprunter — nécessaire car la tâche va survivre bien après la fin
  de la fonction `set_channels`.
- `while let Some(msg) = core_to_ui_rx.recv().await { ... }` est une boucle qui tourne tant que
  `.recv()` retourne `Some(msg)` (un nouveau message est arrivé) ; elle s'arrête d'elle-même quand
  `.recv()` retourne `None`, ce qui arrive quand **tous les `Sender` correspondants ont été détruits**
  (plus personne ne peut jamais envoyer de nouveau message). C'est cette boucle qui tourne pour toute
  la durée de vie de l'app (voir `CLAUDE.md` : "rien d'autre ne consomme ce receiver").

---

## Concept : `async`/`await`, et pourquoi ce fichier n'en a presque pas

`async fn` (ou `async move { }`) définit du code qui peut être **suspendu** en attendant quelque chose
(une réponse réseau, un message sur un channel...) sans bloquer le thread pendant l'attente ; `.await`
est le point où on suspend l'exécution jusqu'à ce que le résultat soit prêt. C'est le mécanisme qui
permet à Tokio de faire tourner beaucoup de tâches sur un petit nombre de threads réels.

Remarque importante : **presque aucune méthode publique de `ViewModelHandle` n'est `async`**
(`send_message`, `execute_plan`, `undo`, `ping`... sont toutes des fonctions synchrones normales). Ce
n'est pas un oubli : ces méthodes sont appelées directement depuis des callbacks Slint qui tournent
déjà sur le thread de l'interface, et tout ce qu'elles font (verrouiller un mutex, envoyer sur un
channel non-bloquant) est instantané — il n'y a jamais besoin d'attendre quoi que ce soit. Le seul
endroit vraiment asynchrone du fichier est la boucle de `set_channels` ci-dessus.

---

## Concept : `Weak<T>` — une référence qui n'empêche pas la valeur de disparaître

```rust
ui_window: Arc<Mutex<Option<Weak<MainWindow>>>>,
```

Un `Weak<T>` est comme un `Arc<T>` mais qui **ne compte pas** comme "propriétaire" — il ne garde pas
la valeur en vie. Pour l'utiliser, il faut d'abord l'"upgrader" (`.upgrade()`), ce qui rend soit
`Some(valeur)` (si elle existe encore) soit `None` (si elle a été détruite entre-temps). Ici, la
fenêtre Slint (`MainWindow`) est possédée ailleurs (par `AppWindow`, voir `05-app-window.md`) ; le
`ViewModelHandle` n'en garde qu'une référence faible pour ne jamais l'empêcher de se fermer — et
doit vérifier à chaque usage qu'elle est toujours là :

```rust
fn sync_to_ui(&self) {
    let Some(window) = self.ui_window.lock().unwrap().clone() else {
        return;
    };
    ...
}
```

`let Some(x) = ... else { return; }` est un raccourci Rust ("let-else") : "si ce n'est pas `Some`,
sors tout de suite de la fonction ; sinon continue avec `x`".

---

## Le point le plus délicat : `sync_to_ui` et `upgrade_in_event_loop`

```rust
let _ = window.upgrade_in_event_loop(move |window| {
    window.set_is_processing(state.is_processing);
    window.set_ollama_connected(state.ollama_connected);
    // ...
});
```

C'est la partie que `CLAUDE.md` appelle "load-bearing" : **il est interdit d'appeler `.upgrade()` sur
un `Weak<MainWindow>` depuis n'importe quel thread**, seulement depuis le thread qui a créé la fenêtre
Slint. Or `sync_to_ui()` peut être appelée depuis la tâche `tokio::spawn`ée de `set_channels`, qui
tourne sur un thread du pool Tokio (pas forcément le même que celui de la fenêtre).

`Weak::upgrade_in_event_loop(closure)` est fourni par Slint justement pour ce cas : au lieu d'upgrader
tout de suite, il **poste la closure** sur la boucle d'événements de l'interface, qui l'exécutera plus
tard, sur le bon thread, avec la fenêtre "forte" et garantie vivante à ce moment-là. C'est le seul
chemin sûr pour pousser une mise à jour d'état vers l'UI depuis du code asynchrone dans ce projet — si
tu ajoutes un nouvel endroit qui doit mettre à jour l'UI depuis une tâche Tokio, passe par ce même
patron plutôt que de stocker et upgrader un `Weak` toi-même.

---

## Pourquoi `sync_to_ui` ne remplace pas les modèles

Le réflexe naturel, pour pousser une liste vers Slint, est de reconstruire un modèle et de
l'assigner :

```rust
window.set_messages(ModelRc::new(VecModel::from(messages)));   // ce que faisait ce code
```

C'est correct, et c'était la principale source de saccades de l'interface. Un *repeater* Slint
(le `for msg in root.messages` du `.slint`) compare le `ModelRc` entrant **par pointeur** : à toute
nouvelle instance, même au contenu identique, il jette toutes ses lignes instanciées *et* son
`RepeaterLayoutState` — l'ancre de scroll, la hauteur moyenne des lignes, la position précédente du
viewport (voir `Repeater::model` dans `i-slint-core`). Comme `sync_to_ui` tournait à chaque token,
la conversation perdait son ancrage de scroll des dizaines de fois par seconde, et les `TextInput`
sous-jacents étaient recréés — emportant au passage toute sélection de texte en cours.

Maintenant, `sync_to_ui` installe **une seule fois** son propre `VecModel` (drapeau
`messages_model_installed`), puis le mute sur place :

```rust
let model = window.get_messages();
if let Some(vec_model) = model.as_any().downcast_ref::<VecModel<ChatMessageData>>() {
    while vec_model.row_count() > message_count {
        vec_model.remove(vec_model.row_count() - 1);
    }
    for (i, row) in message_rows {
        if i < vec_model.row_count() { vec_model.set_row_data(i, row); }
        else                          { vec_model.push(row); }
    }
}
```

Quelles lignes ont changé se décide avec une **empreinte** (`message_fingerprint` : un hachage du
rôle, du contenu, de la trace de raisonnement…) comparée à celle du sync précédent. Bénéfice
secondaire mais important : on ne construit la ligne que pour les messages changés, donc le découpage
markdown (`markdown::split_blocks`, qui appelle `StyledText::from_markdown` bloc par bloc) ne
re-parse que le message qui bouge, au lieu de toute la conversation à chaque token. `plan_steps` et
`sessions` suivent la même idée en plus grossier — remplacés d'un bloc, mais seulement si leur
empreinte a changé.

Une nuance de type au passage : la ligne construite hors du thread UI est un `ChatRow`, pas un
`ChatMessageData`. Les deux ne diffèrent que par un champ — les blocs du message, qui côté Slint
sont un modèle (`ModelRc`), et un modèle Slint n'est pas `Send`, donc il ne peut pas traverser la
closure de `upgrade_in_event_loop`. `ChatRow` les garde en `Vec` ; `ChatRow::into_data`, appelé
dans la closure, les emballe dans un `VecModel` une fois arrivé sur le bon thread.

- `as_any().downcast_ref::<T>()` est la façon Rust de demander « ce trait-object est-il en fait un
  `T` ? ». `ModelRc::as_any` délègue au modèle interne, donc ça rend bien le `VecModel` posé au
  premier sync.

---

## Lire `apply_core_message` : `match` sur une enum

```rust
fn apply_core_message(&self, msg: CoreToUiMsg) {
    match msg {
        CoreToUiMsg::SessionSwitched { session_id, messages, plan } => { ... }
        CoreToUiMsg::Token { content } => { ... }
        CoreToUiMsg::Thinking { content } => { ... }
        CoreToUiMsg::PlanStreaming { steps } => { ... }
        CoreToUiMsg::PlanReady { plan_id, steps } => { ... }
        CoreToUiMsg::Cancelled => { ... }
        // ...
    }
}
```

`CoreToUiMsg::SessionSwitched { .. }` arrive au démarrage puis à chaque changement de conversation :
`main.rs` relit les derniers tours de la session depuis SQLite
(`MemoryStore::get_recent_messages_for_session`, voir `08-memory.md`) et les envoie tels quels, chacun
comme un triplet `(role, content, thinking)`, accompagnés du dernier plan de cette session s'il y en a
un. La branche correspondante repeint entièrement l'état : elle vide `state.messages`, y pousse chaque
triplet comme un `ChatMessage` non-`streaming`, et restaure le panneau de plan — c'est ce qui fait
apparaître les tours (et leurs traces de raisonnement) d'une conversation précédente plutôt que de
repartir d'un affichage vide.

`CoreToUiMsg::Thinking { content }` accumule dans le champ `thinking` **du dernier message assistant en
cours de streaming** (`state.messages.back_mut()`), exactement comme `Token` accumule dans son
`content` — sauf que `Thinking` écrit dans `thinking` et `Token` dans `content`. C'est un flux à part
parce qu'un même tour de conversation peut recevoir des morceaux `Thinking` *puis* des morceaux `Token`
(voir `07-agent.md` : le modèle pense d'abord, répond ensuite) — les mélanger dans `content`
afficherait la trace de raisonnement comme si c'était la réponse elle-même. Si aucun message assistant
n'est encore en streaming (le tout premier morceau reçu pour ce tour est un `Thinking`), la branche en
crée un nouveau avec `content` vide et `thinking: Some(content)`, pour que les morceaux `Token` qui
arrivent ensuite trouvent un message existant à compléter.

`match` est l'outil central pour lire une `enum` : chaque branche correspond à un variant possible, et
Rust **t'oblige à couvrir tous les cas** (le code ne compile pas si tu oublies un variant de
`CoreToUiMsg` — un vrai filet de sécurité quand on ajoute une nouvelle sorte de message plus tard).
`CoreToUiMsg::Token { content } =>` extrait directement le champ `content` du variant dans une
variable utilisable dans cette branche : c'est le "pattern matching" (filtrage par motif).

Repère aussi ce petit bout, dans la branche `Token` :

```rust
CoreToUiMsg::Token { content } => {
    self.update_ui_state(|state| match state.messages.back_mut() {
        Some(last) if last.role == "assistant" && last.streaming => {
            last.content.push_str(&content);
        }
        _ => state.messages.push_back(ChatMessage { .. }),
    });
}
```

`Some(last) if ...` est un **garde de motif** ("match guard") : ce n'est pas juste "est-ce que
`back_mut()` retourne `Some` ?", mais "est-ce que c'est `Some`, **et en plus** cette condition est
vraie ?". `_` est le motif "n'importe quoi d'autre" (catch-all), utilisé ici pour "sinon, ajoute un
nouveau message" — cette logique gère le cas où le modèle envoie sa réponse token par token
(streaming) : le premier token crée un nouveau message "assistant", les suivants sont juste ajoutés au
message existant tant qu'il est encore `streaming`.

---

## Un état transitoire à connaître : le plan en cours d'écriture

`CoreToUiMsg::PlanStreaming { steps }` arrive pendant que le modèle écrit un plan, avec à chaque fois
**la liste complète** des étapes déjà écrites (voir `07-agent.md`). La branche ne les accumule donc
pas : elle remplace `state.plan_steps`, ouvre le panneau, et pose
`state.current_plan_status = "streaming"`.

Ce statut est le point important. Les autres valeurs (`"pending"`, `"done"`, `"rejected"`) décrivent
un plan qui **existe** : il a un `plan_id`, il est en base, on peut l'exécuter. `"streaming"` décrit
un plan qui n'existe pas encore — pas d'identifiant, rien de persisté, rien d'exécutable. C'est
pourquoi `app-window.slint` masque les boutons Exécuter/Rejeter et la ligne « Plan &lt;id&gt; » dans
cet état : ils n'auraient rien à désigner.

Corollaire : quelqu'un doit nettoyer si le plan n'arrive jamais. Le modèle peut très bien produire du
JSON qui ressemblait à un plan et ne parse finalement pas, ou l'utilisateur peut appuyer sur
« Arrêter ». D'où le helper `discard_streaming_plan(state)`, appelé par les branches `MessageDone`,
`Cancelled` et `Error` : si le statut est encore `"streaming"` à ce moment-là, aucun `PlanReady` ne
viendra jamais réclamer ces étapes, donc on les efface. S'il vaut autre chose, c'est qu'un vrai plan
a pris la place — on n'y touche pas.

`CoreToUiMsg::Cancelled` est distinct de `MessageDone` pour une raison de vérité, pas d'affichage :
le tour n'a pas eu lieu. Il n'a pas été sauvegardé, il n'est pas dans `history`, il ne sera pas là au
prochain démarrage. La branche clôt le message en cours, nettoie le plan éventuel, et ajoute une ligne
système « Génération interrompue. » pour que l'écran ne laisse pas croire à une réponse complète.

---

## Pour aller plus loin

| Sujet | Où |
|-------|-----|
| Doc officielle `Arc`/`Mutex` | https://doc.rust-lang.org/std/sync/ |
| Doc officielle `Option`/pattern matching | https://doc.rust-lang.org/book/ch06-00-enums.html |
| Closures (`Fn`/`FnMut`/`FnOnce`) | https://doc.rust-lang.org/book/ch13-01-closures.html |
| Tokio `mpsc` | https://docs.rs/tokio/latest/tokio/sync/mpsc/ |
| Slint `Weak`/`upgrade_in_event_loop` | https://docs.slint.dev/latest/docs/rust/slint/struct.weak/ |
| Version plus avancée de ce même sujet | `documentation/lito/learn/03-slint-ui.md` |
