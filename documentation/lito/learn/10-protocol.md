# `crates/protocol/src/lib.rs` — pour un débutant complet (mention honorable)

> Dernier fichier du tour, après `09-main.md`. C'est le plus petit fichier de tous ceux couverts ici,
> mais il vaut la peine d'être lu pour une raison précise : c'est un exemple concret de code **gardé
> volontairement dans le dépôt bien qu'il ne soit plus vraiment "branché"** (voir `CLAUDE.md`) — utile
> à savoir reconnaître pour ne pas le prendre, par erreur, pour le vrai protocole de communication de
> l'UI (qui est `UiToCoreMsg`/`CoreToUiMsg`, vus dans `04-view-model.md`).

---

## Le rôle de ce fichier, en une phrase

`lito-protocol` définit un petit vocabulaire de types **partagés entre plusieurs crates** du projet :
`Operation`, `PlanStep`, `ErrorCode`, `PlanId`, et — depuis l'ajout des requêtes filesystem LIST/STAT —
`QueryOp`, `FsQuery`, `FileMetadata`, `QueryResult`. Ce sont les seuls types qui traversent réellement
les frontières entre `agent`, `fs`, `memory`, `app` et `ui` — tout le reste de la communication (les
messages UI ↔ core) est interne à `crates/ui` (voir `04-view-model.md`).

Les deux familles de types se ressemblent (des enums SCREAMING_SNAKE_CASE + des structs Serde) mais
jouent des rôles différents : `Operation`/`PlanStep` décrivent une opération qui **modifie** le disque et
passe par le undo log de `crates/fs` (voir `06-fs-ops.md`) ; `QueryOp`/`FsQuery`/`FileMetadata`/
`QueryResult` décrivent une requête en **lecture seule**, jamais journalisée, que `crates/agent` peut
émettre avant de committer à un plan (voir `07-agent.md`, section "La boucle de requêtes filesystem").

---

## Pourquoi un crate séparé, juste pour quelques types ?

Rappel du concept de workspace (vu dans `04-view-model.md`) : plusieurs crates compilés ensemble. Si
`crates/agent` et `crates/fs` avaient chacun leur propre définition de `PlanStep`, il n'y aurait aucun
moyen de faire circuler un plan de l'un vers l'autre sans les convertir manuellement à chaque passage
— et rien ne garantirait que les deux définitions restent en phase quand l'une change. En regroupant
ces types dans un crate à part (`lito-protocol`), dont dépendent tous les autres, **tout le monde
partage exactement la même définition** : changer un champ de `PlanStep` ici se répercute
immédiatement (et le compilateur signalera toute incohérence) dans `agent`, `fs`, `memory`, `app`, `ui`.

```
lito-protocol
      ↑ ↑ ↑ ↑ ↑
      │ │ │ │ └── lito-ui
      │ │ │ └──── lito-app
      │ │ └────── lito-memory
      │ └──────── lito-fs
      └────────── lito-agent
```

---

## `Operation` : une enum "simple" (sans données), avec des dérivations pour le texte

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString)]
#[strum(serialize_all = "SCREAMING_SNAKE_CASE")]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Operation {
    Mkdir,
    Move,
    Copy,
    Delete,
    Rename,
    Symlink,
    WriteFile,
}
```

Une enum sans données sur ses variants (contrairement à `UiToCoreMsg` ou `UndoOperation`, vus dans
`04-view-model.md`/`06-fs-ops.md`) — juste une liste fermée de choix possibles, l'opération demandée
par une étape de plan.

Nouvelles dérivations à connaître :
- **`PartialEq, Eq`** : permettent de comparer deux `Operation` avec `==` (`PartialEq`) ; `Eq` est une
  garantie supplémentaire ("cette égalité est totale et cohérente" — nécessaire par exemple pour
  utiliser le type comme clé de `HashMap`, même si ce n'est pas fait ici). En pratique, tu ajoutes
  presque toujours `PartialEq, Eq` ensemble sur un type simple que tu veux comparer avec `==`.
- **`Copy`** : d'habitude, assigner une valeur Rust à une nouvelle variable (ou la passer à une
  fonction) *déplace* la valeur (l'original n'est plus utilisable ensuite — c'est la règle
  d'"ownership" de Rust). `Copy` change ce comportement pour ce type précis : l'assigner en crée une
  **copie bit à bit** silencieuse, et l'original reste utilisable. Seuls les types "petits et simples"
  (comme cette enum, qui ne contient aucune donnée allouée dynamiquement) peuvent être `Copy`.
- **`Display`** et **`EnumString`** (du crate `strum`) : génèrent respectivement une conversion *vers*
  du texte (`operation.to_string()` produit `"MKDIR"`) et *depuis* du texte (`"MKDIR".parse::<Operation>()`)
  — pratique pour afficher l'opération dans l'UI (voir `04-view-model.md`,
  `PlanStepView::operation: s.operation.to_string()`) sans écrire ce mapping à la main.
- **`Serialize, Deserialize`** (de `serde`) : permettent de convertir vers/depuis JSON — utilisées ici
  pour que `PlanStep` (voir plus bas) — dont `Operation` est un champ — puisse être parsé depuis la
  réponse JSON du LLM (`serde_json::from_str::<Vec<PlanStep>>`, vu dans `07-agent.md`).

`#[strum(serialize_all = "SCREAMING_SNAKE_CASE")]` et `#[serde(rename_all = "SCREAMING_SNAKE_CASE")]`
configurent ces dérivations pour utiliser des majuscules avec underscores (`WRITE_FILE`) plutôt que le
nom Rust exact (`WriteFile`) — c'est ce format que le prompt système d'`Agent` (`07-agent.md`) demande
explicitement au modèle d'utiliser dans le JSON qu'il produit.

---

## `PlanStep` : une struct avec des champs optionnels et un contrôle fin du JSON

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    pub id: String,
    pub description: String,
    pub operation: Operation,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}
```

Tu as déjà vu `Option<T>` (`04-view-model.md`) et les structs (`04-view-model.md` aussi). Ici,
`path`/`target`/`content` sont `Option<...>` parce que **selon l'opération**, tous les champs ne sont
pas pertinents (`MKDIR` n'a pas besoin de `target`, seul `WRITE_FILE` utilise `content` — voir
`06-fs-ops.md`, `require_path`/`require_pair`, qui transforment ces `Option` manquants en erreurs
explicites au moment de l'exécution plutôt qu'à la compilation).

`#[serde(skip_serializing_if = "Option::is_none")]` : quand on sérialise un `PlanStep` *vers* du JSON
(par exemple pour l'enregistrer dans SQLite via `serde_json::to_string`, voir `08-memory.md`), ce
champ est **complètement omis** du JSON produit s'il vaut `None`, plutôt que d'apparaître comme
`"path": null`. Un détail de propreté du format stocké, sans conséquence fonctionnelle ici puisque la
désérialisation gère `None` de toute façon si le champ est absent.

---

## `ErrorCode` : une enum d'erreurs avec des messages, via `thiserror`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, EnumString, Error)]
#[strum(serialize_all = "snake_case")]
pub enum ErrorCode {
    #[error("Ollama service is offline")]
    OllamaOffline,
    #[error("Plan generation failed")]
    PlanFailed,
    // ...
}
```

`Error` (du crate `thiserror`) est une dérivation qui transforme cette enum en un vrai type d'erreur
Rust (il implémente le trait standard `std::error::Error`) — chaque variant porte son propre message
lisible via l'attribut `#[error("...")]`, affiché automatiquement quand on formatte la valeur avec
`{}` (`Display`, généré ici par `thiserror` plutôt que `strum::Display` cette fois — les deux crates
peuvent chacun générer une implémentation de `Display`, il ne faut juste pas dériver les deux en même
temps sur un seul type, ce que ce code ne fait pas ici : `EnumString` sans `strum::Display`). Ce type
est utilisé dans `CoreToUiMsg::Error { code: ErrorCode, message: String }` (vu dans
`04-view-model.md`) pour donner à l'UI un code d'erreur *catégorisé*, en plus du message textuel libre.

---

## `PlanId` : le patron du "newtype"

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PlanId(pub Uuid);

impl PlanId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl std::fmt::Display for PlanId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
```

`struct PlanId(pub Uuid);` est une struct-tuple à un seul champ, non nommé — un patron très courant en
Rust appelé **"newtype"** : envelopper un type existant (`Uuid`) dans un nouveau type distinct, pour
que le compilateur t'empêche de confondre, par exemple, un `PlanId` avec un `Uuid` qui représenterait
autre chose (un id de session, un id utilisateur...) même si les deux sont "juste un UUID" en dessous.
`Self(Uuid::new_v4())` construit la valeur : `Self` est un raccourci pour "le type courant" (ici
`PlanId`), et `(...)` l'appelle comme une fonction — c'est ainsi qu'on construit une struct-tuple.

Remarque, en comparant avec `06-fs-ops.md`/`09-main.md` : le `session_id` utilisé pour le undo log
est une simple `String` généré inline avec `uuid::Uuid::new_v4().to_string()`, **pas** un `PlanId`
typé comme celui-ci — un choix assumé documenté dans `CLAUDE.md` ("volontairement, pour éviter une
couche de parsing qui n'apportait rien tant que rien d'autre ne consomme un `SessionId` typé"). `PlanId`
existe donc, mais tout le code de ce dépôt ne l'utilise pas systématiquement partout où un identifiant
apparaît — un bon rappel que la présence d'un type dans `protocol` ne garantit pas qu'il est
utilisé partout où on pourrait s'y attendre.

`impl std::fmt::Display for PlanId { ... }` implémente **manuellement** le trait `Display` (plutôt que
de le dériver comme pour `Operation`) — nécessaire ici puisqu'un simple newtype autour d'un `Uuid` n'a
pas de dérivation automatique de `Display` fournie par un des crates du projet ; l'implémentation
délègue simplement à celle de `Uuid` (`self.0`, le champ interne) via `write!`.

---

## Ce qui a été supprimé de ce fichier, et pourquoi c'est instructif

D'après `CLAUDE.md`, ce fichier définissait auparavant aussi `ClientMessage`/`ServerMessage` : des
enums avec une sérialisation JSON "externally tagged" (`#[serde(tag = "...")]`), héritées de
l'ancienne architecture (une app SwiftUI qui communiquait avec un processus Rust séparé via des
messages JSON sur stdin/stdout). Une fois l'app devenue un seul binaire (voir `CLAUDE.md`, section
"Architecture"), plus aucun endroit du code ne construisait ni ne parsait ces deux types — la vraie
communication UI ↔ core passe par des enums Rust natives (`UiToCoreMsg`/`CoreToUiMsg`, vues dans
`04-view-model.md`), jamais sérialisées, puisque tout tourne dans le même process. Ces deux types ont
donc été supprimés (avec la dépendance `serde_json` qui ne servait plus qu'à eux dans ce crate) —
`Operation` et `PlanStep`, eux, sont restés, parce qu'ils sont réellement utilisés partout.

La leçon à en tirer, utile au-delà de ce seul fichier : quand un changement d'architecture rend un
type "orphelin" (plus construit ni consommé nulle part), il vaut mieux le supprimer complètement que
de le laisser traîner "au cas où" — sinon, un futur lecteur (toi, ou un LLM) peut facilement le
prendre pour le vrai contrat de communication actuel, alors qu'il ne l'est plus.

---

## Pour aller plus loin

| Sujet | Où |
|-------|-----|
| Le patron "newtype" en Rust | https://doc.rust-lang.org/rust-by-example/generics/new_types.html |
| `thiserror` (dériver `std::error::Error`) | https://docs.rs/thiserror/latest/thiserror/ |
| `strum` (`Display`/`EnumString` pour les enums) | https://docs.rs/strum/latest/strum/ |
| `Copy` vs déplacement ("move") | https://doc.rust-lang.org/book/ch04-01-what-is-ownership.html |
| `serde` : attributs de sérialisation (`skip_serializing_if`, `rename_all`, `tag`) | https://serde.rs/attributes.html |
