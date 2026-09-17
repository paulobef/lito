# `crates/ui/src/app_window.rs` + `crates/ui/ui/app-window.slint` — pour un débutant complet

> Suite de `04-view-model.md`. Ici on regarde comment la fenêtre elle-même est décrite (le fichier
> `.slint`) et comment le code Rust s'y connecte (`app_window.rs`). Ce couple de fichiers **a été
> complètement cassé** avant d'être réparé (voir `CLAUDE.md` et `03-slint-ui.md`) — comprendre pourquoi
> aide à ne pas reproduire le même bug.
>
> Version plus détaillée du même sujet : `documentation/lito/learn/03-slint-ui.md`.
> Prochain fichier du tour : `06-fs-ops.md`.

---

## Concept : qu'est-ce que Slint ?

[Slint](https://slint.dev/) est un framework d'interface graphique. Contrairement à des frameworks où
tu construis l'UI en appelant des fonctions Rust (`Button::new()`, `.on_click(...)`, etc.), Slint a
son propre petit langage déclaratif : tu décris l'UI dans un fichier `.slint` séparé (ici
`crates/ui/ui/app-window.slint`), et un outil (`slint-build`, appelé depuis `build.rs`) **génère du
code Rust** à partir de ce fichier au moment de la compilation. Le code Rust généré, tu ne l'écris
jamais toi-même et tu ne le vois même pas dans le dépôt (il est produit dans le dossier `target/`) —
mais tu peux l'utiliser comme n'importe quel autre type Rust.

C'est un peu comme un fichier `.proto` (protobuf) ou un schéma GraphQL, si tu en as déjà croisé : un
fichier source dans un langage différent, qui génère du code dans ton langage cible.

---

## Le fichier `.slint` : `app-window.slint`

```slint
export struct ChatMessageData {
    role: string,
    content: string,
}

export component MainWindow inherits Window {
    title: "Lito";
    width: 720px;
    height: 520px;

    in property <bool> is-processing: false;
    in property <string> ollama-model: "";
    in property <[ChatMessageData]> messages: [];

    callback send-message(string, string);
    callback ping();

    VerticalBox {
        // ... contenu de la fenêtre
    }
}
```

Quatre notions à connaître :

**1. `export component NomDuComposant inherits Window { ... }`** — définit un composant d'UI (ici,
toute la fenêtre principale) qui hérite du composant natif `Window`. `export` veut dire que ce
composant sera visible depuis le code Rust généré (donc utilisable comme `MainWindow::new()`).

**2. `in property <Type> nom: valeur_par_défaut;`** — une *propriété* est une donnée que le composant
expose. `in` veut dire que c'est le code Rust (l'extérieur) qui la fournit — tu ne peux pas la changer
depuis l'intérieur du `.slint`. Le compilateur Slint génère, pour chaque propriété `in`, une méthode
Rust `set_<nom>()` (les tirets `is-processing` deviennent des underscores `is_processing` côté Rust :
`set_is_processing(true)`). Il existe aussi `in-out property` (modifiable des deux côtés) et
`out property` (l'inverse de `in`, lue depuis Rust) — ce projet n'utilise que `in` pour l'instant.

**3. `callback nom(TypesDesArguments);`** — l'inverse d'une propriété : un événement que
**l'intérieur** du `.slint` déclenche (typiquement sur un clic), et que **Rust écoute**. Slint génère
`on_<nom>(closure)` côté Rust pour t'abonner, et rend `root.<nom>(arguments)` appelable depuis le
markup lui-même (voir plus bas, dans le bouton "Send").

**4. `export struct NomDeStruct { champ: type, ... }`** — un type de données pur (pas de logique),
utilisé ici pour les listes de messages/étapes de plan. Génère un `struct` Rust du même nom
(`ChatMessageData`), utilisable normalement des deux côtés.

`<[ChatMessageData]>` est un type "modèle" (liste) de `ChatMessageData` — côté Rust, ça devient un
`ModelRc<ChatMessageData>`, alimenté par exemple avec `ModelRc::new(VecModel::from(vec![...]))` (tu
verras cette ligne dans `view_model.rs`, section `sync_to_ui`).

### Ce que le langage `.slint` fait *pour* toi

```slint
for msg in root.messages: HorizontalBox {
    Text { text: msg.role + ":"; }
    CopiableText { text: msg.content; }
}

if root.show-plan: VerticalBox {
    // ...
}
```

`for ... in ... : Element { }` et `if condition : Element { }` sont **déclaratifs**, pas impératifs :
il n'y a pas de boucle qui "s'exécute" au sens classique — Slint garde ce bloc à jour automatiquement
à chaque fois que `root.messages` (ou `root.show-plan`) change. C'est la même idée que dans React/Vue :
tu décris "à quoi ça doit ressembler pour un état donné", pas "comment mettre à jour l'écran étape par
étape".

### Un composant "maison" : `CopiableText`

```slint
component CopiableText inherits TextInput {
    read-only: true;
    single-line: false;
    wrap: word-wrap;
}
```

Ce petit composant (pas `export`, donc utilisé seulement à l'intérieur de ce fichier) illustre
pourquoi le contenu du chat et des descriptions de plan utilise `TextInput` avec `read-only: true`
plutôt que le composant `Text` "normal" : un `Text` ne peut pas être sélectionné/copié par
l'utilisateur, alors qu'un `TextInput` en lecture seule le peut tout en restant non-éditable. Un
exemple concret de "chercher le bon composant standard plutôt que d'inventer quelque chose".

---

## `build.rs` : où la génération de code est déclenchée

Le fichier `crates/ui/build.rs` (pas montré ici en détail) appelle `slint_build::compile("ui/app-window.slint")`.
Un `build.rs` est un script Rust **spécial** : Cargo l'exécute automatiquement avant de compiler le
reste du crate, à chaque fois que le `.slint` change. C'est ce script qui produit le code Rust généré
que le fichier suivant (`lib.rs`) importe.

---

## `crates/ui/src/lib.rs` : isoler le code généré

```rust
mod generated {
    slint::include_modules!();
}
pub use generated::{ChatMessageData, MainWindow, PlanStepData};
pub use app_window::AppWindow;
```

`slint::include_modules!()` est une **macro** qui insère, à cet endroit précis, tout le code Rust que
`build.rs` a généré à partir du `.slint` — c'est ce qui fait exister `MainWindow`, `ChatMessageData`,
etc. comme de vrais types Rust.

Le composant généré à partir du `.slint` s'appelle **`MainWindow`**, exactement comme déclaré dans
`export component MainWindow`. Ce nom est délibérément choisi *différent* de `AppWindow` (le wrapper
Rust écrit à la main, voir plus bas) : avant la réparation de ce crate, le composant `.slint`
s'appelait aussi `AppWindow`, ce qui entrait en collision avec le wrapper Rust du même nom — c'était
littéralement le bug qui empêchait tout le crate de compiler (voir `CLAUDE.md`). Le `mod generated { }`
isole le code généré dans son propre "espace de noms" pour que ce genre de collision ne puisse plus se
reproduire, même si un jour quelqu'un renomme encore les choses. **Règle à retenir** : le composant
Slint et le wrapper Rust qui l'entoure ne doivent jamais porter le même nom.

---

## Concept : les *modules* Rust (`mod`)

Rust organise le code en **modules**, un peu comme des dossiers/namespaces : `mod generated { ... }`
crée un module nommé `generated` dont le contenu (ici, tout le code produit par
`slint::include_modules!()`) n'est visible depuis l'extérieur qu'en écrivant `generated::MainWindow`
— sauf si on le "réexporte" avec `pub use generated::MainWindow;`, ce qui rend `MainWindow`
directement accessible comme `lito_ui::MainWindow` depuis les autres crates. C'est exactement ce que
fait la ligne `pub use generated::{ChatMessageData, MainWindow, PlanStepData};`.

---

## `app_window.rs` : le wrapper Rust

```rust
pub struct AppWindow {
    window: MainWindow,
    view_model: ViewModelHandle,
}

impl AppWindow {
    pub fn new(view_model: ViewModelHandle) -> Result<Self, slint::PlatformError> {
        let window = MainWindow::new()?;
        let vm = view_model.clone();

        window.on_send_message(move |prompt, context| {
            vm.send_message(prompt.to_string(), context.to_string());
        });

        // ... on_execute_plan, on_undo, on_select_model, on_ping, chacun avec son propre vm.clone()

        Ok(Self { window, view_model })
    }
}
```

`AppWindow` est une petite `struct` "maison" qui **possède** l'instance `MainWindow` (générée par
Slint) et une copie du `ViewModelHandle` (vu dans `04-view-model.md`). Son rôle : au moment de la
création, brancher chaque `callback` Slint (déclaré dans le `.slint`) sur la méthode `ViewModelHandle`
correspondante.

### Concept : `Result<T, E>` et l'opérateur `?`

```rust
pub fn new(view_model: ViewModelHandle) -> Result<Self, slint::PlatformError> {
    let window = MainWindow::new()?;
    ...
}
```

`Result<T, E>` est le type Rust pour "une opération qui peut réussir (`Ok(valeur)`) ou échouer
(`Err(erreur)`)" — c'est l'équivalent d'une exception, mais rendu explicite dans le type de retour
plutôt que "invisible" comme un `throw` qui pourrait surgir n'importe où. `MainWindow::new()` retourne
un `Result` parce que la création d'une fenêtre native peut échouer (pas de serveur graphique
disponible, par exemple).

Le `?` à la fin de `MainWindow::new()?` est un raccourci : "si c'est `Err(e)`, arrête tout de suite la
fonction et retourne cette erreur (convertie si besoin) ; si c'est `Ok(valeur)`, continue avec
`valeur`". Sans le `?`, il faudrait écrire un `match` complet à chaque appel. Tu croiseras ce `?`
partout dans le projet dès qu'une fonction retourne un `Result`.

### Pourquoi un `.clone()` par callback ?

```rust
let vm = view_model.clone();
window.on_send_message(move |prompt, context| {
    vm.send_message(prompt.to_string(), context.to_string());
});

let vm = view_model.clone();
window.on_execute_plan(move |plan_id| { ... });
```

Chaque closure passée à `on_send_message`/`on_execute_plan`/etc. doit **posséder** (`move`) sa propre
copie de `vm` puisqu'elle peut être appelée à tout moment, indépendamment des autres. Comme
`ViewModelHandle` est `#[derive(Clone)]` et ne contient que des `Arc<...>` (vu dans
`04-view-model.md`), chaque `.clone()` est bon marché : ça ne copie pas `UiState`, seulement les
pointeurs partagés vers lui. C'est *le* motif à reconnaître : "un `.clone()` juste avant chaque
closure" en Rust asynchrone/UI ne veut presque jamais dire "dupliquer les données", mais "donner à
cette closure sa propre poignée vers la même donnée partagée".

### Concept : les traits, et pourquoi `use slint::ComponentHandle;` est nécessaire

```rust
use slint::ComponentHandle;
```

```rust
pub fn run(&self) -> Result<(), slint::PlatformError> {
    self.window.run()
}

pub fn as_weak(&self) -> slint::Weak<MainWindow> {
    self.window.as_weak()
}
```

Un **trait** est un ensemble de méthodes qu'un type peut "implémenter" — un peu comme une interface en
Java/TypeScript, mais qui peut aussi être ajoutée à des types qu'on ne possède pas soi-même. `.run()`
et `.as_weak()` ne sont pas des méthodes définies directement sur `MainWindow` : elles viennent du
trait `slint::ComponentHandle`, implémenté automatiquement par Slint pour tout composant généré. En
Rust, **pour appeler une méthode de trait, ce trait doit être "en portée"** (importé avec `use`) à
l'endroit de l'appel — sinon le compilateur refuse de compiler, avec une erreur qui dit en substance
"cette méthode existe, mais tu dois importer le trait qui la définit". C'est pour ça que la ligne
`use slint::ComponentHandle;` est nécessaire tout en haut du fichier, même si `ComponentHandle` n'est
jamais mentionné explicitement ailleurs.

`run(&self)` bloque l'appelant : elle affiche la fenêtre, démarre la boucle d'événements Slint, et ne
retourne qu'à la fermeture de la fenêtre (voir `09-main.md`, qui l'appelle depuis `main()`).

---

## Pour aller plus loin

| Sujet | Où |
|-------|-----|
| Slint language reference | https://docs.slint.dev/latest/docs/slint/ |
| Slint Rust API (`ComponentHandle`, `Weak`, `Model`) | https://docs.slint.dev/latest/docs/rust/slint/ |
| Traits en Rust | https://doc.rust-lang.org/book/ch10-02-traits.html |
| `Result<T, E>` et `?` | https://doc.rust-lang.org/book/ch09-02-recoverable-errors-with-result.html |
| Modules Rust (`mod`, `use`, `pub`) | https://doc.rust-lang.org/book/ch07-00-managing-growing-projects-with-packages-crates-and-modules.html |
| Doc officielle Slint vendorée dans ce dépôt | `documentation/slint-docs/` |
