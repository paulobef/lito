# `crates/memory/src/lib.rs` — pour un débutant complet

> Suite de `07-agent.md`. Ce fichier est court, mais il illustre bien une contrainte Rust
> "invisible" tant qu'on n'y a pas été confronté : pourquoi une simple connexion SQLite doit être
> enveloppée dans un `Mutex` pour ce projet précis. C'est un bon exemple de "concurrence" (plusieurs
> bouts de code qui pourraient s'exécuter en même temps) plutôt que de syntaxe.
>
> Prochain fichier du tour : `09-main.md`.

---

## Le rôle de ce fichier, en une phrase

`MemoryStore` sauvegarde dans un fichier SQLite tout ce qui doit survivre au redémarrage de l'app : les
plans générés, l'historique de conversation, le journal d'annulation, et quelques réglages (comme le
modèle Ollama sélectionné).

---

## Concept : qu'est-ce que SQLite, et le crate `rusqlite` ?

SQLite est une base de données **entière contenue dans un seul fichier** sur le disque (ici
`memory.sqlite`) — pas de serveur séparé à lancer, contrairement à Postgres/MySQL. `rusqlite` est le
crate Rust qui permet d'ouvrir ce fichier et d'y exécuter du SQL classique. Si tu ne connais pas le
SQL : c'est un langage pour décrire des tables de données et des requêtes dessus (`CREATE TABLE`,
`INSERT`, `SELECT`, `UPDATE`...) — les bases restent les mêmes ici que dans n'importe quel tutoriel SQL
généraliste, seule la façon de les appeler depuis Rust est spécifique à `rusqlite`.

```rust
pub struct MemoryStore {
    conn: Mutex<Connection>,
}
```

`Connection` (type de `rusqlite`) représente la connexion ouverte vers le fichier `.sqlite`. On revient
juste après sur pourquoi elle est enveloppée dans un `Mutex` (déjà vu dans `04-view-model.md`).

---

## Créer la base et son schéma

```rust
pub fn new(path: Option<PathBuf>) -> Result<Self> {
    let db_path = path.unwrap_or_else(|| {
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("lito")
            .join("memory.sqlite")
    });

    std::fs::create_dir_all(db_path.parent().unwrap())?;
    let conn = Connection::open(&db_path)?;

    conn.execute_batch(r#"
        CREATE TABLE IF NOT EXISTS plans ( ... );
        CREATE TABLE IF NOT EXISTS undo_log ( ... );
        CREATE TABLE IF NOT EXISTS settings ( ... );
        CREATE TABLE IF NOT EXISTS messages ( ... );
    "#)?;

    Ok(Self { conn: Mutex::new(conn) })
}
```

- `path: Option<PathBuf>` : le chemin de la base est optionnel (voir `04-view-model.md` pour
  `Option<T>`) — en usage normal (`main.rs`, voir `09-main.md`) on passe `None`, et le code choisit lui-même
  un emplacement standard via `dirs::data_dir()` (le dossier de données de l'application sur macOS/
  Linux/Windows, fourni par le crate `dirs`) ; les tests, eux, passent `Some(chemin_temporaire)` pour
  ne jamais toucher au vrai fichier de l'utilisateur (voir la fin de ce document).
- `.unwrap_or_else(|| ...)` : si `path` est `None`, exécute cette closure pour calculer une valeur par
  défaut (paresseusement — la closure n'est appelée que si vraiment nécessaire). C'est la version
  "closure" de `.unwrap_or(valeur)` vu dans `07-agent.md`, utile quand calculer la valeur par défaut a
  un coût (ici, appeler `dirs::data_dir()`).
- `CREATE TABLE IF NOT EXISTS ...` : crée chaque table seulement si elle n'existe pas déjà — donc ce
  code peut tourner à chaque démarrage de l'app sans jamais effacer les données existantes.
  `execute_batch` exécute plusieurs instructions SQL d'un coup (contrairement à `.execute()`, utilisé
  plus loin, qui n'en exécute qu'une).

### Les quatre tables, en une ligne chacune

- `plans` : chaque plan généré par le modèle (le JSON des étapes, son statut `pending`/`executing`/
  `done`, et l'`undo_id` de sa session d'exécution une fois lancé).
- `undo_log` : une trace de chaque action annulable (redondante avec le journal en mémoire de
  `crates/fs`, voir `06-fs-ops.md` — celle-ci persiste, l'autre ne survit pas à un redémarrage).
- `settings` : des paires clé/valeur génériques (utilisé aujourd'hui pour retenir le modèle Ollama
  choisi par l'utilisateur).
- `messages` : l'historique de conversation, tour par tour, pour que le chat survive à un
  redémarrage de l'app.

---

## Concept, rappel approfondi : pourquoi `Mutex<Connection>` et pas juste `Connection` ?

C'est le commentaire le plus important du fichier :

```rust
/// The connection is guarded by a `Mutex` (rather than exposed as a bare
/// `rusqlite::Connection`, which is `Send` but not `Sync`) so a `&MemoryStore`
/// can be held across `.await` points in the core agent's tokio task.
```

Deux nouveaux mots-clés à comprendre : `Send` et `Sync`. Ce sont des **traits marqueurs** (des traits
sans aucune méthode, juste une "étiquette" que le compilateur vérifie automatiquement) :
- **`Send`** : un type `T` est `Send` si une valeur de ce type peut être **déplacée** en toute sécurité
  vers un autre thread.
- **`Sync`** : un type `T` est `Sync` si une **référence** partagée `&T` peut être utilisée en toute
  sécurité depuis plusieurs threads en même temps.

`rusqlite::Connection` est `Send` (tu peux la déplacer sur un autre thread) mais **pas** `Sync` (elle
contient en interne des structures — un cache de requêtes préparées — qui ne supportent pas d'être lues
depuis plusieurs threads à la fois sans protection).

Pourquoi ça compte ici : `run_core_agent` (voir `09-main.md`) tourne dans une tâche lancée par
`tokio::spawn`, et garde une référence `&MemoryStore` vivante **à travers des points `.await`** — par
exemple, `handle_send_message` fait `llm.health_check().await`, *puis* utilise encore `memory` un peu
plus loin. Pour que `tokio::spawn` accepte cette tâche, le `Future` correspondant doit être `Send`
dans son ensemble (parce que le runtime Tokio peut, en théorie, faire migrer une tâche suspendue d'un
thread de son pool à un autre entre deux réveils) — ce qui veut dire que **tout ce que ce Future touche
à travers un `.await` doit être `Sync`** (pour qu'une référence partagée dessus reste valable après un
changement de thread). `Connection` seule échouerait ce test ; `Mutex<Connection>` le passe, parce que
`Mutex<T>` est `Sync` dès que `T` est `Send` — le `Mutex` "répare" cette propriété en garantissant qu'un
seul accès a lieu à la fois, peu importe le thread.

**Retiens le symptôme, pas seulement l'explication** : si un jour tu vois une erreur de compilation du
genre *"future cannot be sent between threads safely"* qui pointe vers un type interne comme
`RefCell`/`InnerConnection`, c'est très probablement le même problème ailleurs — la correction typique
est d'envelopper la ressource non-`Sync` dans un `Mutex`, pas de chercher à contourner la contrainte de
`tokio::spawn`.

---

## Le patron répété dans chaque méthode publique

```rust
pub fn save_plan(&self, _conversation_id: &str, plan_id: &str, prompt: &str, context: &str, steps: &[PlanStep]) -> Result<()> {
    let now = Utc::now().timestamp();
    let steps_json = serde_json::to_string(steps)?;

    self.conn.lock().unwrap().execute(
        "INSERT INTO plans (id, prompt, context, steps, status, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![plan_id, prompt, context, steps_json, "pending", now],
    )?;

    Ok(())
}
```

- `_conversation_id: &str` : le préfixe `_` sur un nom de paramètre est la convention Rust pour "ce
  paramètre existe dans la signature mais n'est délibérément pas utilisé pour l'instant" — ça évite
  l'avertissement du compilateur ("variable inutilisée") sans supprimer le paramètre. Le commentaire
  juste au-dessus explique pourquoi : ce champ est prévu pour une future vue "historique multi-
  conversation", mais le schéma actuel ne groupe pas encore par conversation.
- `self.conn.lock().unwrap()` : verrouille le mutex (voir `04-view-model.md`) pour obtenir un accès
  exclusif et temporaire à la `Connection` — le verrou est relâché automatiquement dès que cette valeur
  temporaire sort de portée (ici, à la fin de l'expression `.execute(...)`).
- `params![plan_id, prompt, context, steps_json, "pending", now]` : la macro `params!` de `rusqlite`
  construit la liste des valeurs à substituer aux `?1`, `?2`, ... dans la requête SQL — c'est la façon
  sûre de passer des valeurs dynamiques dans du SQL (ça évite les injections SQL, contrairement à
  concaténer les valeurs directement dans la chaîne de requête).
- `serde_json::to_string(steps)?` : sérialise (transforme en texte JSON) la liste des `PlanStep`
  avant de l'insérer dans la colonne `steps` (déclarée `TEXT` dans le schéma) — SQLite n'a pas de type
  "liste structurée" natif ici, donc le JSON sert de format de stockage pour une donnée complexe dans
  une seule colonne texte.

Toutes les autres méthodes (`update_plan_status`, `get_plan`, `log_undo`, `get_undo_actions`,
`save_message`, `get_recent_messages`, `get_setting`, `set_setting`) suivent exactement ce même patron
: verrouiller, exécuter une requête SQL paramétrée, convertir le résultat, relâcher le verrou (implicite
à la fin de la méthode).

---

## Lire un résultat qui peut ne pas exister : `get_plan`

```rust
pub fn get_plan(&self, plan_id: &str) -> Result<Option<Vec<PlanStep>>> {
    let conn = self.conn.lock().unwrap();
    let mut stmt = conn.prepare("SELECT steps FROM plans WHERE id = ?1")?;
    let steps_json: String = match stmt.query_row(params![plan_id], |row| row.get(0)) {
        Ok(json) => json,
        Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
        Err(e) => return Err(e.into()),
    };

    let steps: Vec<PlanStep> = serde_json::from_str(&steps_json)?;
    Ok(Some(steps))
}
```

Le type de retour `Result<Option<Vec<PlanStep>>>` combine les deux idées vues jusqu'ici : `Result`
("est-ce que la requête elle-même a échoué, ex. base corrompue ?") **autour de** `Option`
("est-ce qu'un plan avec cet id existe, ou pas ?"). Ce `match` distingue explicitement trois issues :
`Ok(json)` (une ligne trouvée, on continue), `Err(QueryReturnedNoRows)` (aucune ligne — ce n'est *pas*
vraiment une erreur du point de vue de l'appelant, donc on retourne `Ok(None)` plutôt que de propager
une erreur), et n'importe quelle autre `Err(e)` (une vraie erreur, qu'on convertit avec `.into()` en
`anyhow::Error` et qu'on propage). C'est un bon exemple de "toutes les erreurs ne se valent pas" : une
ligne absente est un résultat normal à gérer, pas un échec technique.

---

## Les tests : une base de données jetable pour chaque test

```rust
#[test]
fn test_memory_store() {
    let dir = tempdir().unwrap();
    let store = MemoryStore::new(Some(dir.path().join("test.sqlite"))).unwrap();
    // ...
}
```

`tempdir()` (du crate `tempfile`, listé en `[dev-dependencies]` — des dépendances utilisées *seulement*
pour les tests, jamais dans le binaire livré) crée un dossier temporaire, automatiquement supprimé
quand `dir` sort de portée à la fin du test. Passer `Some(dir.path().join("test.sqlite"))` à
`MemoryStore::new` (au lieu de `None`) évite que les tests touchent le vrai fichier `memory.sqlite` de
l'utilisateur — c'est exactement le paramètre `path: Option<PathBuf>` vu plus haut qui rend ça
possible.

---

## Pour aller plus loin

| Sujet | Où |
|-------|-----|
| `rusqlite` (API complète) | https://docs.rs/rusqlite/latest/rusqlite/ |
| `Send`/`Sync` expliqués en détail | https://doc.rust-lang.org/nomicon/send-and-sync.html |
| SQLite lui-même (langage SQL) | https://www.sqlite.org/lang.html |
| `tempfile` (dossiers/fichiers temporaires pour les tests) | https://docs.rs/tempfile/latest/tempfile/ |
