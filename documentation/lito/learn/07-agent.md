# `crates/agent/src/lib.rs` — pour un débutant complet

> Suite de `06-fs-ops.md`. On remonte d'un cran : ce fichier ne touche jamais le disque, il ne fait
> que discuter avec le modèle de langage (via `crates/llm`) et décider si sa réponse est "juste du
> texte" ou "un plan d'actions à exécuter". C'est l'endroit où la frontière entre "planifier" et
> "exécuter" est tracée dans ce projet (voir `CLAUDE.md`).
>
> Prochain fichier du tour : `08-memory.md`.

---

## Le rôle de ce fichier, en une phrase

`Agent::respond(...)` prend un message de l'utilisateur, l'envoie au LLM local (Ollama, via
`crates/llm`), et renvoie soit une réponse conversationnelle normale, soit une liste de `PlanStep`
(les étapes concrètes à exécuter, définies dans `crates/protocol`, voir `10-protocol.md`) — jamais les
deux à la fois, et jamais en exécutant quoi que ce soit lui-même. Au passage, si le modèle configuré
est entraîné pour ça (`qwen3`, `deepseek-r1`, `qwq`, ...), `respond` relaie aussi sa trace de
raisonnement ("thinking") séparément de la réponse finale, via un second callback — voir la section
"Le flux distingue désormais deux sortes de morceaux" plus bas.

Il y a aussi une troisième forme de réponse possible, mais **interne** à `respond` : avant de donner sa
réponse finale, le modèle peut demander une requête filesystem en lecture seule ("liste le contenu de
ce dossier", "donne-moi la date de ce fichier") plutôt que d'inventer des noms de fichiers ou des dates
plausibles. `respond` exécute cette requête via `crates/fs` (`FsOps::run_query`, voir `06-fs-ops.md`),
renvoie le résultat au modèle, et reboucle — voir "La boucle de requêtes filesystem" plus bas. Ce
va-et-vient reste invisible pour l'appelant : `AgentResponse` n'a toujours que deux variantes, `Chat` et
`Plan`.

---

## Concept : `enum` pour représenter "l'un ou l'autre", version encore plus simple

```rust
pub enum AgentResponse {
    Chat(String),
    Plan(Vec<PlanStep>),
}
```

Tu as déjà vu des enums avec des champs nommés (`UiToCoreMsg::SendMessage { prompt, context }`, dans
`04-view-model.md`). Ici, `Chat(String)` et `Plan(Vec<PlanStep>)` utilisent la forme "tuple" : un seul
champ, sans nom, entre parenthèses. C'est équivalent, juste plus court à écrire quand il n'y a qu'une
seule donnée à porter. Une valeur `AgentResponse` est *soit* du texte de conversation, *soit* une liste
d'étapes — jamais un mélange des deux, ce qui reflète directement une règle du prompt système
(`SYSTEM_PROMPT`, voir plus bas) : "choisis exactement une de ces deux formes de réponse, ne mélange
jamais texte et JSON".

---

## Le "prompt système" : comment on parle au LLM

```rust
const SYSTEM_PROMPT: &str = r#"You are Lito, a friendly local file automation assistant ...

1. Conversation: if the user is chatting ... just reply normally in plain text.

2. Filesystem query: if you need real information about the filesystem ... respond with ONLY a raw
JSON object requesting it: {"query": "LIST", "path": "..."} or {"query": "STAT", "path": "..."}.

3. Action plan: if the user is asking you to actually perform file operations ...
respond with ONLY a raw JSON array of plan steps ...
"#;
```

`const NOM: Type = valeur;` déclare une **constante** : une valeur fixe, connue à la compilation, qui
ne change jamais pendant l'exécution du programme (contrairement à une variable `let`). `r#"..."#`
est une "raw string" : tout ce qu'il y a entre les guillemets est pris tel quel, y compris les
guillemets simples, les retours à la ligne, sans avoir besoin d'échapper quoi que ce soit avec des
`\"`. C'est le format pratique pour écrire un long bloc de texte comme ce prompt.

Ce texte-là n'est pas "du code" au sens strict : c'est l'instruction envoyée telle quelle au modèle de
langage à chaque requête, qui lui explique le format de réponse attendu (voir `crates/llm`,
`ChatMessage::system(...)`). Toute la logique de classification "conversation, requête filesystem, ou
plan ?" qui suit dans ce fichier existe *parce que* le modèle ne respecte pas toujours cette instruction
à la lettre (il peut entourer son JSON de balises Markdown, par exemple) — le code doit rester tolérant.

---

## Concept : les *generics* et les traits comme "contrat" (`F: FnMut(AgentEvent)`)

```rust
pub async fn respond<F>(
    &self,
    prompt: &str,
    context: &str,
    history: &[ChatMessage],
    llm: &OllamaClient,
    fs: &FsOps,
    mut on_event: F,
) -> Result<AgentResponse>
where
    F: FnMut(AgentEvent),
{
    ...
}
```

`<F>` déclare un **paramètre de type générique** : `respond` fonctionne avec n'importe quel type `F`,
tant qu'il respecte la contrainte donnée après `where` : `F: FnMut(AgentEvent)` veut dire "doit être
une closure qu'on peut appeler plusieurs fois, qui reçoit un `AgentEvent` et ne retourne rien". C'est
le même principe que le `FnOnce` vu dans `04-view-model.md`, mais en version "appelable plusieurs
fois" (`FnMut`) au lieu de "appelable une seule fois" (`FnOnce`) — logique ici, puisque `on_event`
est appelée à chaque nouveau morceau reçu du modèle.

### Pourquoi un enum plutôt que plusieurs closures

Cette fonction a longtemps pris **trois** closures positionnelles (`on_token`, `on_thinking`,
`on_query`), chacune avec son propre paramètre générique — plus un
`#[allow(clippy::too_many_arguments)]` pour faire taire l'avertissement. Quand le streaming des plans
a ajouté un quatrième événement, le site d'appel devenait quatre blocs `|x| { ... }` anonymes empilés
dont plus rien n'indiquait lequel était lequel. Un seul callback + un enum règle les deux problèmes :

```rust
pub enum AgentEvent<'a> {
    Token(&'a str),               // texte de la réponse, au fil de l'eau
    Thinking(&'a str),            // trace de raisonnement du modèle
    Querying(&'a str),            // « Listing ~/Desktop »
    PlanSteps(&'a [PlanStep]),    // les étapes du plan déjà écrites
}
```

Le `<'a>` est un **paramètre de durée de vie** : chaque variante emprunte de la donnée (`&'a str`)
au lieu de la posséder (`String`). Concrètement, ça veut dire qu'émettre un événement n'alloue rien
— on prête un morceau du tampon existant, la closure le lit, et c'est fini. Avec des `String`, chaque
token du modèle provoquerait une allocation aussitôt jetée. La contrepartie est que la closure ne peut
pas *garder* la référence au-delà de l'appel : si elle veut la conserver, elle doit la copier
(`.to_string()`) — ce que fait justement `crates/app/src/main.rs` pour l'envoyer dans un canal.

`fs: &FsOps` est une référence empruntée (lecture seule, pas `&mut`) vers les opérations filesystem de
`crates/fs`, utilisée uniquement pour répondre aux requêtes LIST/STAT que le modèle peut émettre (voir
plus bas) — `respond` ne l'utilise jamais pour exécuter un plan, ça reste le travail de
`crates/app::handle_execute_plan`.

`&[ChatMessage]` (avec des crochets) est une **slice** : une vue empruntée sur une séquence
contiguë de valeurs, ici un historique de messages — semblable à un `&Vec<ChatMessage>`, mais plus
générique (une slice peut aussi référencer un tableau de taille fixe, ou juste une partie d'un `Vec`).

---

## Le vrai défi de ce fichier : classifier une réponse *pendant* qu'elle arrive

Le commentaire au-dessus de `respond` explique le problème : le modèle répond en **streaming** (un
petit morceau de texte à la fois, voir `crates/llm`). Si la réponse est un plan, elle commence par
`[{"id":"a"...` — du JSON illisible tant qu'il n'est pas complet. Afficher ça token par token dans le
chat serait moche et confus. Mais si la réponse est une vraie conversation, on *veut* l'afficher au
fur et à mesure (l'effet "machine à écrire" habituel des chatbots).

Tout ce qui suit tourne à l'intérieur de `for round in 0..MAX_QUERY_ROUNDS { ... }` : la boucle décrite ici
peut s'exécuter plusieurs fois d'affilée si le modèle enchaîne des requêtes filesystem avant de donner
sa réponse finale (voir "La boucle de requêtes filesystem" plus bas) — chaque tour reconstruit un
`stream`/`buffer`/`streaming_live` neufs.

```rust
let mut stream = Box::pin(llm.stream_chat(messages.clone()).await?);
let mut buffer = String::new();
let mut streaming_live = false;
let mut plan_parser = StreamingPlanParser::new();
let mut plan_parsed_upto = 0;
while let Some(chunk) = stream.next().await {
    let token = match chunk? {
        ChatChunk::Thinking(text) => {
            on_event(AgentEvent::Thinking(&text));
            continue;
        }
        ChatChunk::Content(text) => text,
    };
    buffer.push_str(&token);

    if streaming_live {
        on_event(AgentEvent::Token(&token));
        continue;
    }

    let trimmed_start = buffer.trim_start();
    if trimmed_start.is_empty() {
        continue;
    }
    if looks_like_plan(trimmed_start) {
        // On retient le JSON brut hors du chat, mais on le donne au parseur
        // incrémental : les étapes complètes partent vers le panneau de plan.
        if plan_parser.feed(&buffer[plan_parsed_upto..]) {
            on_event(AgentEvent::PlanSteps(plan_parser.steps()));
        }
        plan_parsed_upto = buffer.len();
        continue;
    }
    streaming_live = true;
    on_event(AgentEvent::Token(&buffer)); // pas un plan : on rattrape tout d'un coup
}
```

### Le flux distingue désormais deux sortes de morceaux (`ChatChunk`)

`llm.stream_chat(...)` (voir `crates/llm/src/lib.rs`) ne renvoie plus directement des `String` : chaque
élément du flux est un `ChatChunk`, un enum à deux variantes — `Thinking(String)` pour un morceau de la
trace de raisonnement du modèle, `Content(String)` pour un morceau de la vraie réponse (texte de
conversation ou JSON de plan). Ollama expose ce `thinking` séparément du `content` habituel quand on
envoie `think: true` dans la requête et que le modèle est entraîné pour ça — sinon le champ est
simplement absent, et le flux ne produit que des `Content`.

Le `match chunk? { ... }` en tête de boucle sépare les deux cas *avant* toute la logique de
classification plan-vs-chat qui suit : un morceau `Thinking` part directement vers l'appelant puis
`continue` — il ne passe jamais par `buffer`, `looks_like_plan`, ni `extract_plan`, parce qu'une trace
de raisonnement ne ressemble jamais à un début de JSON et n'a aucune raison d'attendre la classification
pour s'afficher. Seuls les morceaux `Content` alimentent `buffer` et suivent la logique décrite
ci-dessous.

### Concept : `Stream` (une séquence asynchrone de valeurs) et `.next().await`

Un `Stream` est comme un `Iterator` (vu dans `06-fs-ops.md`), mais **asynchrone** : au lieu de
`.next()` qui retourne immédiatement la valeur suivante, `stream.next().await` retourne un `Future`
qu'il faut attendre — la valeur suivante peut ne pas être encore arrivée (elle vient du réseau, ici).
`while let Some(token) = stream.next().await { ... }` boucle tant que le flux produit des valeurs, et
s'arrête tout seul quand le flux est terminé (`None`) — exactement le même patron que la boucle de
canal vue dans `04-view-model.md`, mais appliqué à un flux réseau plutôt qu'à un `mpsc::Receiver`.

`Box::pin(...)` est nécessaire ici pour une raison technique liée aux streams asynchrones (ils doivent
souvent être "épinglés" en mémoire, c'est-à-dire garantis de ne jamais être déplacés, pour que leur
état interne reste valide) — retiens simplement que "streamer depuis une fonction async qui retourne
`impl Stream`" demande souvent ce `Box::pin` autour, tu n'as pas besoin de comprendre le détail pour
lire ce fichier.

### La logique de décision, étape par étape

1. Tant qu'on n'a encore vu **aucun** caractère non-blanc, on continue d'accumuler (`trimmed_start.is_empty()`).
2. Dès qu'on a du contenu, `looks_like_plan(trimmed_start)` regarde si ça *ressemble* au début d'un
   plan JSON (voir juste en dessous). Si oui, on continue à tout accumuler dans `buffer` sans rien
   afficher **dans le chat** — mais le texte retenu est passé au `StreamingPlanParser`, qui en
   extrait les étapes complètes (section suivante).
3. Si ça ne ressemble pas à un plan, on bascule `streaming_live = true` **et** on affiche d'un coup
   tout ce qu'on avait accumulé jusque-là (`AgentEvent::Token(&buffer)`) — rien n'est perdu, juste
   retardé de quelques tokens le temps de faire la décision.
4. Une fois `streaming_live` activé, chaque nouveau token est affiché immédiatement (plus de retenue).

```rust
fn looks_like_plan(text: &str) -> bool {
    text.starts_with('[') || text.starts_with('{') || text.starts_with("```")
}
```

Une simple vérification de préfixe : un plan JSON commence toujours par `[` (un tableau) ou `{` (un
objet, si le modèle n'en produit qu'un seul), ou par une balise de code Markdown (```` ``` ````) si le
modèle a désobéi à la consigne "pas de balise Markdown" du prompt système.

---

## `StreamingPlanParser` : montrer le plan pendant qu'il s'écrit

Retenir le JSON hors du chat est nécessaire, mais si on s'arrête là l'utilisateur ne voit
*strictement rien* pendant toute la génération d'un plan — c'est-à-dire pendant le cas d'usage
principal de l'app, et souvent pendant plusieurs dizaines de secondes sur un modèle local. Le
`StreamingPlanParser` récupère cette information : il regarde passer le JSON retenu et émet un
`AgentEvent::PlanSteps` chaque fois qu'un objet d'étape se referme, ce qui remplit le panneau de plan
ligne à ligne.

```rust
struct StreamingPlanParser {
    started: bool,      // a-t-on vu le '[' d'ouverture ?
    disabled: bool,     // a-t-on renoncé (forme inattendue) ?
    depth: usize,       // profondeur d'accolades DANS l'étape en cours
    in_string: bool,    // est-on à l'intérieur d'une chaîne JSON ?
    escaped: bool,      // le caractère précédent était-il un '\\' ?
    current: String,    // l'objet en cours d'accumulation
    steps: Vec<PlanStep>,
}
```

### Pourquoi écrire un automate à la main

`serde_json` n'a pas de mode incrémental : il parse un document **complet** ou échoue. Impossible donc
de lui donner `[{"id":"1","desc` et d'obtenir "une étape en cours". Le parseur fait donc le strict
minimum pour trouver les **frontières** d'objets — repérer où un `{` s'ouvre et où le `}`
correspondant se referme — puis confie chaque objet entier, lui, à `serde_json`. On ne réimplémente
pas un parseur JSON : on découpe, et on délègue.

Les trois champs `depth`/`in_string`/`escaped` sont ce qui rend ce découpage correct. Compter
naïvement les accolades casse dès qu'une étape `WRITE_FILE` a un `content` qui en contient :

```json
{"operation":"WRITE_FILE","content":"{\"k\": [1,2]}"}
```

Ici le `}` de la chaîne fermerait l'objet trop tôt. En sachant qu'on est à l'intérieur d'une chaîne
(`in_string`), et qu'un `"` précédé d'un `\\` (`escaped`) ne la termine pas, les accolades du contenu
sont correctement ignorées. C'est exactement le même genre de machine à états que le
`ThinkTagSplitter` plus bas dans le fichier — et le même genre de piège : le flux peut se couper
**n'importe où**, y compris au milieu d'un caractère de la chaîne.

### Ce que le parseur refuse de faire

`disabled` est aussi important que le reste. Le parseur se désarme dans trois cas :

- **La réponse commence par `{` et non `[`.** Un objet seul est ambigu : c'est soit un plan d'une
  étape, soit une requête `{"query": "LIST", ...}`. Streamer une requête dans le panneau de plan
  serait pire que de ne rien afficher, donc on ne devine pas — la classification de fin de flux
  tranchera.
- **Un objet complet ne se désérialise pas en `PlanStep`.** Ça veut dire que la forme supposée était
  la mauvaise ; on arrête plutôt que d'émettre du bruit.
- **Le `]` de fin est atteint.** Il n'y a plus rien à streamer.

Le principe général : ce parseur est un **agrément d'affichage**, jamais une source de vérité. La
seule chose qui décide qu'une réponse est un plan reste `extract_plan(&buffer)` à la fin du flux
(section suivante). Si les deux divergent — le parseur a montré des étapes, le plan final n'existe
pas — c'est le second qui gagne, et l'UI efface les étapes affichées (`discard_streaming_plan` dans
`crates/ui/src/view_model.rs`).

### Chaque événement porte la liste entière

`AgentEvent::PlanSteps(&[PlanStep])` envoie **toutes** les étapes connues, pas seulement la nouvelle.
C'est délibéré : le récepteur remplace ce qu'il a au lieu de l'accumuler, donc il n'a pas besoin d'un
message "remets à zéro" au début d'un plan, et il ne peut pas dériver si un message se perd ou si un
second tour de boucle recommence un plan. Un plan fait quelques étapes ; recopier ce vecteur à chaque
fois ne coûte rien face à la simplicité gagnée.

---

## Après le flux : essayer vraiment de parser un plan

```rust
if let Some(plan) = extract_plan(&buffer) {
    return Ok(plan);
}

if let Some(query) = extract_query(&buffer) {
    on_event(AgentEvent::Querying(&describe_query(&query)));
    let result_message = match fs.run_query(&query).await {
        Ok(result) => format_query_result(&result),
        Err(err) => format!("[FILESYSTEM QUERY ERROR] {} {}: {err}", query.query, query.path.display()),
    };
    messages.push(ChatMessage::assistant(buffer));
    messages.push(ChatMessage::user(result_message));
    continue; // reboucle : on redemande une complétion avec le résultat inclus
}

if !streaming_live {
    on_event(AgentEvent::Token(&buffer));
}
return Ok(AgentResponse::Chat(buffer));
```

Une fois le flux terminé, `extract_plan` tente réellement de parser `buffer` comme un plan JSON
(`looks_like_plan` n'était qu'une estimation *pendant* le streaming — ici, on vérifie pour de vrai).
Si ça réussit, tant pis pour ce qu'on avait éventuellement déjà affiché (dans ce cas précis, rien n'a
été affiché puisque `looks_like_plan` avait déjà bloqué l'affichage) : on retourne le plan.

Sinon, `extract_query` essaie de parser `buffer` comme une requête filesystem (voir la section suivante
pour le détail). Si ça marche, on exécute la requête, on ajoute la question du modèle (`buffer`, en tant
que tour "assistant") *et* le résultat (en tant que tour "user") à `messages`, puis `continue` fait
reboucler le `for` — un nouveau tour de streaming démarre avec cet historique augmenté, comme si
l'utilisateur avait répondu "voici ce que j'ai trouvé". Remarque que ni la question ni le résultat ne
touchent au `history` reçu en paramètre : ce va-et-vient reste entièrement local à cet appel de
`respond`, l'appelant ne voit jamais ces tours intermédiaires (voir `crates/app/src/main.rs`, qui ne
persiste que la réponse finale).

Si ni `extract_plan` ni `extract_query` ne matchent, et qu'on n'avait encore rien affiché
(`!streaming_live`, donc le texte "ressemblait" à un plan/une requête mais n'en était finalement ni
l'un ni l'autre), on rattrape en affichant tout le texte accumulé d'un coup, puis on retourne une
réponse `Chat` — ça sort de la fonction (`return`), donc ça sort aussi de la boucle `for`.

```rust
fn strip_code_fence(text: &str) -> &str {
    let trimmed = text.trim();
    let Some(rest) = trimmed.strip_prefix("```") else {
        return trimmed;
    };
    let rest = rest.strip_prefix("json").unwrap_or(rest);
    let rest = rest.strip_prefix('\n').unwrap_or(rest);
    rest.strip_suffix("```").unwrap_or(rest).trim()
}

fn extract_plan(buffer: &str) -> Option<AgentResponse> {
    let candidate = strip_code_fence(buffer);
    if candidate.starts_with('[') {
        if let Ok(steps) = serde_json::from_str::<Vec<PlanStep>>(candidate) {
            return Some(AgentResponse::Plan(steps));
        }
    } else if candidate.starts_with('{') {
        if let Ok(step) = serde_json::from_str::<PlanStep>(candidate) {
            return Some(AgentResponse::Plan(vec![step]));
        }
    }
    None
}
```

`strip_code_fence` nettoie une éventuelle balise Markdown ```` ```json ... ``` ```` autour du JSON —
`.strip_prefix(...)`/`.strip_suffix(...)` retournent `Option<&str>` (`Some` si le préfixe/suffixe était
bien là, `None` sinon), et `.unwrap_or(rest)` veut dire "si ça n'a pas matché, garde `rest` tel quel
plutôt que de planter" — un patron très courant pour du nettoyage de texte "best effort".

`serde_json::from_str::<Vec<PlanStep>>(candidate)` tente de désérialiser (transformer du texte JSON en
vraies valeurs Rust typées) `candidate` en un `Vec<PlanStep>`. `serde_json` est le crate standard pour
JSON en Rust ; il s'appuie sur les macros `#[derive(Serialize, Deserialize)]` que tu verras sur
`PlanStep` dans `10-protocol.md` pour savoir comment convertir chaque champ. `from_str::<Vec<PlanStep>>`
utilise la syntaxe "turbofish" `::<Type>` pour préciser explicitement le type de retour attendu, quand
le compilateur ne peut pas le deviner tout seul du contexte.

`extract_plan` accepte aussi un objet JSON unique (`{ ... }`, pas dans un tableau) comme un plan à une
seule étape — certains modèles produisent cette forme malgré la consigne demandant un tableau ; le
code s'adapte plutôt que d'échouer bêtement sur un détail de format.

---

## La boucle de requêtes filesystem

```rust
fn extract_query(buffer: &str) -> Option<FsQuery> {
    let candidate = strip_code_fence(buffer);
    if !candidate.starts_with('{') {
        return None;
    }
    serde_json::from_str::<FsQuery>(candidate).ok()
}
```

Même principe que `extract_plan`, en plus simple : `FsQuery` (défini dans `crates/protocol`, voir
`10-protocol.md`) n'a que deux champs requis, `query` (`"LIST"` ou `"STAT"`) et `path`. Comment
`extract_plan` et `extract_query` ne se marchent-ils pas dessus, vu qu'un objet `{...}` unique peut en
théorie matcher les deux ? Parce que leurs champs requis sont disjoints : un objet `PlanStep` valide n'a
pas de champ `query`, un objet `FsQuery` valide n'a pas de champ `id`/`operation` — `serde_json` échoue
proprement (`Result::Err`, transformé en `None` par `.ok()`) dès qu'un champ requis manque, donc
`extract_plan(json_de_requete)` et `extract_query(json_de_plan)` retournent tous les deux `None` sans
ambiguïté.

```rust
fn describe_query(query: &FsQuery) -> String {
    match query.query {
        QueryOp::List => format!("Listing {}", query.path.display()),
        QueryOp::Stat => format!("Reading metadata for {}", query.path.display()),
    }
}
```

Une simple traduction de la requête en phrase lisible, émise en `AgentEvent::Querying` pour que l'UI puisse
afficher "Listing ~/Desktop" pendant que l'utilisateur attend — sans ça, il n'y aurait aucun signe
visible que l'agent est en train de consulter le disque avant de répondre (voir `CoreToUiMsg::Querying`
dans `crates/ui/src/view_model.rs`).

```rust
fn format_query_result(result: &QueryResult) -> String {
    let body = serde_json::to_string(result).unwrap_or_else(|_| "{}".to_string());
    let note = if result.truncated { " (truncated - not every entry is shown)" } else { "" };
    format!("[FILESYSTEM QUERY RESULT]{note} {body}")
}
```

Le résultat est renvoyé au modèle sous forme de JSON brut, préfixé `[FILESYSTEM QUERY RESULT]`.
Pourquoi un préfixe texte plutôt qu'un vrai mécanisme de "tool result" ? Parce que l'API `/api/chat`
d'Ollama utilisée ici (`crates/llm::OllamaClient::stream_chat`) n'a que trois rôles de message
(`system`/`user`/`assistant`, voir `crates/llm/src/lib.rs`) — pas de rôle `tool` dédié comme celui
qu'exposent certains frameworks d'agents. Le préfixe sert donc uniquement à aider le modèle à distinguer
"ceci est un résultat de requête que j'ai moi-même demandé" de "ceci est un message tapé par
l'utilisateur", même si techniquement les deux arrivent comme un tour `role: "user"`.

Enfin, `MAX_QUERY_ROUNDS` (10) plafonne le nombre d'allers-retours. Au lieu d'échouer par
`anyhow::bail!(...)` quand le budget est épuisé, le **dernier tour** injecte un tour `user`
supplémentaire (`FINAL_ROUND_DIRECTIVE`) qui dit au modèle que son budget d'inspection est consommé,
que toute nouvelle `{"query"|"command"}` sera ignorée, et qu'il doit conclure maintenant — soit un
plan, soit une réponse en clair (y compris « aucun fichier ne correspond »). Les branches `query` /
`command` de la boucle sont donc gardées par `!final_round` ; si le modèle renvoie malgré tout une
requête/commande à ce dernier tour, `respond` répond avec `FINAL_ROUND_FALLBACK` (un message en clair)
plutôt que de laisser du JSON brut atterrir dans le chat ou de renvoyer une erreur interne. La boucle
`for` fait donc toujours un `return` à sa dernière itération — le `unreachable!()` qui la suit
documente cette invariance.

---

## Les tests : lire les intentions à travers les cas testés

En bas du fichier, `mod tests` (voir `06-fs-ops.md` pour l'explication de `#[cfg(test)]`) contient des
tests comme :

```rust
#[test]
fn recognizes_plan_and_chat_prefixes() {
    assert!(looks_like_plan("[{\"id\":\"a\"}]"));
    assert!(looks_like_plan("```json\n[]\n```"));
    assert!(!looks_like_plan("Sure, here's how you'd do that."));
}
```

`assert!(condition)` fait planter le test si `condition` est fausse (contrairement à `assert_eq!`, qui
compare deux valeurs — voir `06-fs-ops.md`). Ces tests sont une bonne façon de lire "ce que le code est
censé garantir" sans avoir à tout relire ligne par ligne : ils couvrent explicitement le cas d'un plan
en tableau, d'un plan en objet unique, d'un plan entouré de balises Markdown, et du texte de
conversation normal qui ne doit *jamais* être pris pour un plan.

Les tests du `StreamingPlanParser` suivent la même idée, mais avec un détail méthodologique qui vaut
d'être copié ailleurs : le helper `feed_char_by_char` alimente le parseur **un caractère à la fois**.

```rust
fn feed_char_by_char(json: &str) -> (StreamingPlanParser, usize) {
    let mut parser = StreamingPlanParser::new();
    let mut emissions = 0;
    for ch in json.chars() {
        if parser.feed(&ch.to_string()) {
            emissions += 1;
        }
    }
    (parser, emissions)
}
```

C'est le pire découpage possible, et donc le seul qui prouve vraiment quelque chose : n'importe quelle
supposition cachée du genre "une frontière de morceau HTTP coïncide avec une frontière JSON" casse
immédiatement. Un test qui donne le JSON entier d'un bloc passerait même avec un parseur naïf.

---

## Pour aller plus loin

| Sujet | Où |
|-------|-----|
| Generics et traits comme contraintes (`where F: ...`) | https://doc.rust-lang.org/book/ch10-00-generics.html |
| `Stream` (crate `tokio-stream`/`futures`) | https://docs.rs/tokio-stream/latest/tokio_stream/ |
| `serde_json` (sérialisation JSON) | https://docs.rs/serde_json/latest/serde_json/ |
| `Option::unwrap_or`/`strip_prefix`/`strip_suffix` | https://doc.rust-lang.org/std/primitive.str.html |
| Le fichier qui produit le flux consommé ici | `crates/llm/src/lib.rs` (voir `02-rust.md` pour un premier passage dessus) |
