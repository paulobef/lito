# `crates/fs/src/lib.rs` — pour un débutant complet

> Suite de `05-app-window.md`. On quitte l'interface graphique pour la partie "sensible" du projet :
> le seul endroit du code qui **touche réellement le disque** (créer, déplacer, copier, supprimer des
> fichiers). D'après `CLAUDE.md` : "bugs ici veut dire perte de données, pas juste un souci visuel" —
> c'est le fichier à comprendre le mieux avant d'y toucher.
>
> Prochain fichier du tour : `07-agent.md`.

---

## Le rôle de ce fichier, en une phrase

`FsOps` exécute *réellement* les opérations demandées par le plan (créer un dossier, déplacer un
fichier, etc.), et journalise chaque opération pour pouvoir l'annuler ("undo") plus tard. Il ne décide
*jamais* quoi faire — ça, c'est le rôle de `crates/agent` (voir `07-agent.md`). `FsOps` ne fait
qu'exécuter des ordres déjà pris, et se souvenir comment les défaire.

---

## Concept : `async fn` et pourquoi (presque) tout ici est asynchrone

```rust
pub async fn create_dir_all(&mut self, path: &Path, session_id: &str) -> Result<()> {
    fs::create_dir_all(path).await?;
    self.log_undo(session_id, UndoOperation::CreateDir { path: path.to_path_buf() });
    Ok(())
}
```

Une fonction déclarée `async fn` ne s'exécute pas immédiatement quand on l'appelle : elle produit une
valeur "en attente" (un `Future`) qu'il faut `.await`er pour obtenir le résultat. Concrètement, ici,
`fs::create_dir_all(path)` vient de `tokio::fs` (pas `std::fs` !) — la version asynchrone des
opérations fichiers de la bibliothèque standard. La différence pratique : pendant qu'on attend que le
disque réponde, le thread qui exécute ce code peut aller faire autre chose (traiter un autre message,
par exemple), plutôt que de rester bloqué à ne rien faire. C'est tout l'intérêt de faire tourner ce
projet sur un runtime asynchrone (Tokio).

---

## Concept : `Result<T, E>` et l'opérateur `?` (rappel, très utilisé ici)

```rust
pub async fn move_file(&mut self, from: &Path, to: &Path, session_id: &str) -> Result<()> {
    fs::rename(from, to).await?;
    self.log_undo(session_id, UndoOperation::Move { from: from.to_path_buf(), to: to.to_path_buf() });
    Ok(())
}
```

`Result<()>` ici (avec un seul paramètre de type) est un raccourci défini par le crate `anyhow` :
`anyhow::Result<T>` est équivalent à `std::result::Result<T, anyhow::Error>`. `anyhow::Error` est un
type d'erreur "boîte noire" qui peut représenter n'importe quelle erreur (I/O, JSON invalide, etc.) —
pratique dans une fonction comme celle-ci, où l'échec le plus probable vient de `tokio::fs`, mais on
ne veut pas s'embêter à définir un type d'erreur maison rien que pour ce cas. `()` (le "type unité") en
position de succès veut dire "cette fonction ne retourne aucune valeur utile en cas de réussite, juste
le fait que ça a marché" — l'équivalent Rust d'une fonction `void` qui peut échouer.

Chaque `.await?` fait deux choses d'un coup : attendre le résultat de l'opération asynchrone, *et*
propager immédiatement l'erreur si elle échoue (voir `05-app-window.md` pour le détail du `?`).

---

## Concept : `Path` vs `PathBuf`

```rust
pub async fn move_file(&mut self, from: &Path, to: &Path, session_id: &str) -> Result<()> { ... }

struct UndoEntry {
    session_id: String,
    operation: UndoOperation,
}

enum UndoOperation {
    Move { from: PathBuf, to: PathBuf },
    // ...
}
```

C'est le même principe que `&str` vs `String` (une donnée "empruntée" vs "possédée" — voir
`04-view-model.md`), mais pour des chemins de fichiers : `&Path` est une référence *empruntée* vers un
chemin (utilisée comme paramètre de fonction, on ne fait que lire), `PathBuf` est un chemin *possédé*,
qu'on peut garder après la fin de la fonction. `path.to_path_buf()` convertit un `&Path` emprunté en
`PathBuf` possédé — nécessaire ici parce que `UndoEntry` doit **garder** ce chemin en mémoire
longtemps après l'appel de `move_file`, pour pouvoir l'utiliser plus tard dans `undo()`.

---

## Concept : une `enum` avec des données différentes par variant

```rust
enum UndoOperation {
    CreateDir { path: PathBuf },
    Move { from: PathBuf, to: PathBuf },
    Copy { to: PathBuf },
    Delete { path: PathBuf, backup: PathBuf },
    Symlink { link: PathBuf },
    WriteFile { path: PathBuf, previous: Option<PathBuf> },
}
```

Comme vu dans `04-view-model.md` avec `UiToCoreMsg`, chaque variant d'une `enum` peut porter des
données **différentes**. Ici, `UndoOperation` enregistre exactement ce qu'il faut pour **défaire**
chaque type d'opération : pour annuler un `Move`, il suffit de connaître `from`/`to` (et faire le
déplacement en sens inverse) ; pour annuler un `Delete`, il faut connaître où est le fichier de
sauvegarde (`backup`, voir plus bas) ; pour un `WriteFile`, `previous: Option<PathBuf>` peut valoir
`None` (le fichier n'existait pas avant, donc "annuler" = juste le supprimer) ou `Some(backup)` (il
existait, on a une sauvegarde à restaurer).

---

## Le "undo log" : comment l'annulation fonctionne

```rust
pub struct FsOps {
    undo_log: Vec<UndoEntry>,
}

fn log_undo(&mut self, session_id: &str, operation: UndoOperation) {
    self.undo_log.push(UndoEntry {
        session_id: session_id.to_string(),
        operation,
    });
}
```

Chaque opération qui modifie réellement le disque (`create_dir_all`, `move_file`, `copy_file`,
`delete`, `write_file`, `create_symlink`) se termine en appelant `log_undo`, qui ajoute une `UndoEntry`
à la fin d'un `Vec` en mémoire. Chaque entrée est taguée avec un `session_id` — une simple `String`
(un UUID généré ailleurs dans le projet, voir `09-main.md`), **pas** un type dédié : c'est un choix
volontaire pour éviter une couche de conversion inutile tant que rien d'autre dans le code n'a besoin
d'un type `SessionId` distinct d'une `String`.

```rust
pub async fn undo(&mut self, session_id: &str) -> Result<()> {
    let entries: Vec<_> = self
        .undo_log
        .iter()
        .filter(|e| e.session_id == session_id)
        .cloned()
        .collect();

    for entry in entries.into_iter().rev() {
        self.reverse_operation(&entry.operation).await?;
    }
    self.undo_log.retain(|e| e.session_id != session_id);
    Ok(())
}
```

### Concept : les itérateurs (`iter()`, `.filter()`, `.cloned()`, `.collect()`, `.rev()`)

Une chaîne comme `self.undo_log.iter().filter(...).cloned().collect()` est un **pipeline
d'itérateur**, très idiomatique en Rust (proche de `.filter().map()` en JavaScript, ou d'une
list-comprehension Python) :
- `.iter()` : parcourt le `Vec` en produisant des références (`&UndoEntry`), sans le vider ni le
  copier.
- `.filter(|e| e.session_id == session_id)` : ne garde que les entrées qui appartiennent à cette
  session — la closure reçoit une référence et retourne un `bool`.
- `.cloned()` : transforme chaque `&UndoEntry` en un `UndoEntry` possédé (une copie) — nécessaire ici
  car on va vouloir consommer ces entrées après avoir fini de parcourir `self.undo_log`, et on ne peut
  pas garder des références vers `self.undo_log` tout en appelant `self.reverse_operation(&self)` plus
  bas (ça emprunterait `self` deux fois en même temps, ce que le compilateur refuse).
- `.collect()` : matérialise le résultat du pipeline dans un vrai `Vec` (le type exact, `Vec<UndoEntry>`,
  est déduit du contexte — d'où le `_` dans `Vec<_>`, qui veut dire "devine le type toi-même").
- `.rev()` (sur `entries.into_iter()`) : parcourt la liste **à l'envers**. C'est essentiel : si un
  plan a fait "créer le dossier A, puis déplacer un fichier dedans", il faut défaire dans l'ordre
  inverse (déplacer le fichier en arrière, *puis* supprimer le dossier vide) — annuler dans l'ordre
  d'origine casserait sur la première étape.

`self.undo_log.retain(|e| e.session_id != session_id);` supprime ensuite ces entrées du journal : une
fois annulées, elles n'ont plus de raison d'y rester (les rejouer une deuxième fois n'aurait pas de
sens, le fichier n'est déjà plus là où l'entrée le pense).

---

## Pourquoi `delete` ne supprime jamais vraiment un fichier

```rust
pub async fn delete(&mut self, path: &Path, session_id: &str) -> Result<()> {
    let backup = path.with_extension(format!("bak.{}", Uuid::new_v4()));
    fs::rename(path, &backup).await?;
    self.log_undo(session_id, UndoOperation::Delete { path: path.to_path_buf(), backup });
    Ok(())
}
```

Au lieu d'appeler une vraie fonction de suppression, `delete` **renomme** le fichier vers un chemin
`<nom_original>.bak.<uuid-aléatoire>` (grâce au crate `uuid`, qui génère des identifiants uniques —
`Uuid::new_v4()` produit un UUID aléatoire "version 4"). Ça rend l'annulation triviale : "annuler une
suppression" = renommer le backup vers son emplacement d'origine (voir `reverse_operation` plus bas).
C'est un bon exemple de design "safety-first" pour une app qui manipule les fichiers de l'utilisateur
sur la base de ce qu'un LLM local a décidé de faire.

---

## `reverse_operation` : `match` pour défaire chaque cas

```rust
async fn reverse_operation(&self, op: &UndoOperation) -> Result<()> {
    match op {
        UndoOperation::CreateDir { path } => {
            if path.exists() { fs::remove_dir_all(path).await?; }
        }
        UndoOperation::Move { from, to } => {
            if to.exists() { fs::rename(to, from).await?; }
        }
        UndoOperation::Delete { path, backup } => {
            if backup.exists() { fs::rename(backup, path).await?; }
        }
        // ...
    }
    Ok(())
}
```

Encore un `match` exhaustif sur une `enum` (voir `04-view-model.md`) : chaque variant sait comment se
défaire lui-même. Remarque le motif répété `if xxx.exists() { ... }` : ce n'est pas une vérification
"au cas où", c'est une protection délibérée contre le cas où l'utilisateur (ou un autre programme) a
déjà touché ce fichier entre le moment de l'opération et le moment de l'undo — mieux vaut ne rien
faire que planter ou écraser quelque chose d'inattendu.

---

## `#[cfg(unix)]` / `#[cfg(windows)]` : compiler du code différent selon la plateforme

```rust
pub async fn create_symlink(&mut self, target: &Path, link: &Path, session_id: &str) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        symlink(target, link)?;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::symlink_file;
        symlink_file(target, link)?;
    }
    self.log_undo(session_id, UndoOperation::Symlink { link: link.to_path_buf() });
    Ok(())
}
```

`#[cfg(unix)]` (et `#[cfg(windows)]`) sont des attributs de **compilation conditionnelle** : le bloc
qui suit n'est **inclus dans le binaire final** que si on compile pour la plateforme correspondante.
Créer un lien symbolique n'est pas la même opération système sous Unix (macOS/Linux) que sous Windows
— la bibliothèque standard expose donc deux fonctions différentes, et ce code choisit la bonne à la
compilation plutôt qu'à l'exécution (il n'y a pas de `if` runtime ici : sur macOS, le bloc `#[cfg(windows)]`
n'existe même pas dans le binaire compilé).

---

## Un petit détail utile pour l'utilisateur : `expand_tilde`

```rust
fn expand_tilde(path: &Path) -> PathBuf {
    let Ok(rest) = path.strip_prefix("~") else {
        return path.to_path_buf();
    };
    match dirs::home_dir() {
        Some(home) => home.join(rest),
        None => path.to_path_buf(),
    }
}
```

Le LLM qui génère les plans ne connaît pas le vrai chemin du dossier personnel de l'utilisateur, donc
il écrit parfois des chemins comme `~/Downloads/foo.txt` littéralement. Sans ce traitement, le système
de fichiers chercherait un dossier réellement appelé `~` (qui n'existe pas), relatif au dossier courant
du programme — pas du tout ce que l'utilisateur ou le modèle avaient en tête. `expand_tilde` remplace
ce `~` par le vrai dossier personnel, trouvé via le crate `dirs` (`dirs::home_dir()`).

- `path.strip_prefix("~")` retourne un `Result` : `Ok(reste_du_chemin)` si le chemin commence bien par
  `~`, sinon une erreur. `let Ok(rest) = ... else { return ...; }` (encore le "let-else" vu dans
  `04-view-model.md`) : "si ça ne commence pas par `~`, retourne le chemin inchangé tout de suite".
- `dirs::home_dir()` retourne un `Option<PathBuf>` (voir `04-view-model.md` pour `Option`) : `None` si
  le dossier personnel n'a pas pu être déterminé pour une raison ou une autre — dans ce cas, le code
  choisit délibérément de garder le chemin d'origine (avec son `~` littéral) plutôt que de planter,
  pour que l'erreur qui suivra plus loin mentionne encore le chemin que le modèle avait demandé.

---

## Les requêtes en lecture seule : `run_query`/`list_dir`/`stat`

```rust
pub async fn run_query(&self, query: &FsQuery) -> Result<QueryResult> {
    let path = expand_tilde(&query.path);
    let entries = match query.query {
        QueryOp::List => self.list_dir(&path).await?,
        QueryOp::Stat => vec![self.stat(&path).await?],
    };
    let truncated = query.query == QueryOp::List && entries.len() >= MAX_LIST_ENTRIES;
    Ok(QueryResult { query: query.query, path: query.path.clone(), entries, truncated })
}
```

Tout ce qu'on a vu jusqu'ici (`create_dir_all`, `move_file`, `delete`, ...) *modifie* le disque et passe
par `log_undo`. `run_query` est différent : c'est de la **lecture seule** (lister le contenu d'un
dossier, ou lire la taille/date d'un fichier), donc il n'y a rien à journaliser, rien à pouvoir défaire
— d'où `&self` plutôt que `&mut self` dans la signature (comparer avec `create_dir_all(&mut self, ...)`
plus haut : pas de `mut` ici veut dire "cette méthode ne modifie pas `FsOps` lui-même"). `list_dir`
plafonne le nombre d'entrées à `MAX_LIST_ENTRIES` (500) : un dossier `Downloads` avec des milliers de
fichiers renverrait sinon un JSON énorme au modèle, pour rien — `truncated: true` prévient juste le
modèle que la liste n'est pas complète.

Pourquoi cette méthode existe : le modèle qui planifie (`crates/agent`) ne connaît pas d'avance le
contenu réel du disque de l'utilisateur. Avant, s'il fallait "renommer tous les screenshots du Bureau
avec leur date", il devait *deviner* des noms de fichiers plausibles — dangereux. Maintenant, il peut
répondre par une requête JSON (`{"query": "LIST", "path": "~/Desktop"}`) au lieu d'un plan, `crates/agent`
appelle `run_query`, et renvoie le résultat réel au modèle avant de lui redemander une réponse (voir
`07-agent.md` pour la boucle complète).

---

## Les petits *helpers* de validation : `require_path`/`require_pair`

```rust
fn require_path<'a>(path: &'a Option<PathBuf>, op: &str) -> Result<&'a Path> {
    path.as_deref()
        .ok_or_else(|| anyhow::anyhow!("{op} step requires `path`"))
}
```

`PlanStep::path` est un `Option<PathBuf>` (un plan peut, en théorie, arriver sans chemin renseigné —
un bug du modèle, par exemple). `require_path` convertit ce `Option` en une erreur explicite et lisible
(`anyhow::anyhow!(...)` construit un message d'erreur formaté) plutôt que de laisser un `.unwrap()`
paniquer plus loin dans le code avec un message cryptique. `<'a>` ici est une **durée de vie**
("lifetime") — un concept plus avancé qui dit essentiellement "la référence retournée reste valide
aussi longtemps que le `path` qu'on lui a passé en entrée". Tu n'as pas besoin de maîtriser les
lifetimes pour comprendre ce fichier ; retiens juste que Rust les utilise pour garantir, à la
compilation, qu'aucune référence retournée ne pointe vers une donnée qui a déjà disparu.

---

## Les tests, en bas du fichier

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_leading_tilde() {
        let home = dirs::home_dir().expect("test environment must have a home dir");
        assert_eq!(expand_tilde(Path::new("~/Downloads/foo.txt")), home.join("Downloads/foo.txt"));
    }
}
```

`#[cfg(test)] mod tests { ... }` : un module qui n'existe **que** quand on compile en mode test
(`cargo test`), jamais dans le binaire final livré à l'utilisateur. `use super::*;` importe tout ce
qui est défini dans le fichier parent (`expand_tilde`, etc.) dans ce module de test. `#[test]` marque
une fonction comme un test individuel, lancé par `cargo test`. `assert_eq!(a, b)` fait planter le test
si `a != b`, en affichant les deux valeurs pour comprendre pourquoi. Lancer uniquement les tests de ce
crate : `cargo test -p lito-fs` (voir la racine de `CLAUDE.md` pour la commande générale).

---

## Pour aller plus loin

| Sujet | Où |
|-------|-----|
| `tokio::fs` (opérations fichiers async) | https://docs.rs/tokio/latest/tokio/fs/ |
| Itérateurs (`.iter()`, `.filter()`, `.collect()`...) | https://doc.rust-lang.org/book/ch13-02-iterators.html |
| `Path`/`PathBuf` | https://doc.rust-lang.org/std/path/ |
| Compilation conditionnelle (`#[cfg(...)]`) | https://doc.rust-lang.org/reference/conditional-compilation.html |
| Écrire des tests (`#[test]`) | https://doc.rust-lang.org/book/ch11-01-writing-tests.html |
