# Design de référence — UI Lito

Ce document capture le système de design cible de l'UI Slint (`crates/ui`), issu d'une
itération de design menée hors-code (maquette HTML non versionnée) puis portée en Slint.
Objectif : que Claude Code (ou tout développeur) retrouve les décisions et leur *pourquoi*
sans avoir à re-dériver le style à chaque session, et sache ce qui est un choix délibéré
plutôt qu'un oubli.

Le style implémenté dans `crates/ui/ui/app-window.slint` (`global Palette` + composants
`Composer`/`ActionButton`/`StatusBadge`) est la source de vérité pour les valeurs exactes —
ce fichier explique les intentions et contraintes derrière ces valeurs, pas l'inverse. S'il y
a divergence entre les deux, le `.slint` a raison ; mets ce document à jour.

## Vérifier le rendu (avant toute affirmation visuelle)

`make ui-shot` rend l'UI en PNG sans lancer l'app (ni Ollama, ni SQLite), à partir des fixtures de
`crates/ui/ui/fixtures/` : `target/ui-shots/demo-light.png`, `demo-dark.png` et `stress.png`. C'est
la façon de vérifier qu'un changement de style fait ce qu'on croit — le code `.slint` qui *se lit*
correctement et qui *rend* mal est le mode d'échec normal ici. Voir `.claude/skills/slint/SKILL.md`.

## Direction générale

Inspiration : **IBM Carbon Design System**, adaptée à une app macOS native — pas un
skeuomorphisme mac, pas un thème "glassy"/vibrancy. Ambiance recherchée : professionnelle,
dense, sobre. Cartes délimitées par des bordures fines plutôt que des ombres, densité
d'information forte (paddings resserrés, texte 10–13px selon le rôle — voir "Typographie").

Un très léger rayon de coin (`Theme.radius`, 4px) est appliqué aux cartes/contrôles
autonomes (boutons, champs, puces, bulles, popups) — introduit après la rédaction initiale de
ce document (angles droits partout). Il ne touche **pas** les panneaux structurels (titlebar,
sidebar, colonnes principales) ni les `HDivider`/`VDivider`, qui restent à angle droit et
flush avec les bords de fenêtre et entre eux.

Points de repère explicitement écartés en cours d'itération (à ne pas réintroduire sans
qu'on le redemande) :
- Pas de dégradé chaleureux crème + accent terracotta (cliché de design généré par IA).
- Pas de bulles de chat pour les deux interlocuteurs — seul l'utilisateur a une bulle
  (grise neutre) ; les réponses de l'agent s'affichent en texte nu, façon Claude Desktop.
- Pas de couleur d'accent "décorative" : elle est réservée au seul vrai CTA de l'app (voir
  "Règle de couleur" plus bas).

## Palette (clair + sombre)

Définie dans `global Palette` en tête de `app-window.slint`. Chaque token de couleur est un
ternaire `Palette.dark ? <valeur sombre> : <valeur claire>` plutôt que deux palettes séparées,
piloté par le toggle "Sombre"/"Clair" de la titlebar (`toggle-theme`, persisté côté
`settings.dark_mode`). La palette sombre suit la même direction que la claire (gris froid, pas
chaud ; l'accent reste réservé au CTA, voir plus bas) plutôt que d'être une direction visuelle
séparée — choisie sans maquette sombre de référence, donc à considérer comme un premier jet
raisonnable plutôt que des valeurs figées.

Ce `Palette` maison ne couvre que les éléments dessinés par l'app. Les contrôles `std-widgets`
(le `ScrollView` du chat, les `ListView` de la sidebar et du panneau de plan) ne le lisent pas du
tout : ils suivent le `Palette.color-scheme` interne de std-widgets, qui par défaut reflète
l'apparence système — et pas notre toggle, qui en est délibérément découplé (voir "hors scope" plus
bas). Résultat mesuré : Mac en apparence Claire + toggle app sur Sombre donnait une barre de
défilement claire, quasi invisible sur fond sombre. `MainWindow` importe `Palette` de
`std-widgets.slint` sous l'alias `StdPalette` et force
`StdPalette.color-scheme` à suivre `Palette.dark` (`init` pour la première frame, `changed
effective-dark` ensuite — `effective-dark` est un miroir local de `Palette.dark`, un `changed`
handler ne s'écrit que sur une propriété locale). Vérifié avec une sonde isolée
(`ScrollView` + longue liste, `--style fluent`) : forcer `color-scheme` à `dark` donne un thumb
`#26292d` sur fond `#14171c` (visible) ; le forcer à `light` donne `#0a0c0f` (quasi invisible) —
confirme que le mécanisme change bien la bonne chose et dans le bon sens.

| Token | Rôle |
|---|---|
| `bg` / `surface` | Blanc quasi pur — délibéré : le fond doit être "pur", pas un gris cassé. |
| `sidebar` | Quasi identique à `bg` (`#fbfcfd` vs `#ffffff`) — la séparation vient de la bordure, pas d'un aplat de gris visible. |
| `border` | Gris à pointe froide (bleu-ardoise), pas un gris chaud/beige — choix explicite après plusieurs itérations ("plus froid"). |
| `text` / `text-muted` / `text-faint` | Hiérarchie à trois niveaux, toujours sur la même base froide. |
| `bubble-bg` | Gris neutre pour un **bloc de contenu rempli** : bulle utilisateur, cartes d'étape du plan. (Plus le bloc de raisonnement : il n'a plus d'aplat dans aucun état — voir "Bloc de raisonnement" plus bas.) |
| `surface-hover` / `surface-press` | Rampe d'interaction (voir ci-dessous). |
| `border-hover` | Bordure au survol des contrôles qui en gardent une (badge de modèle, "Nouvelle tâche", chip de plan). |
| `focus-ring` | Liseré de focus des champs texte — neutre fort, **pas** l'accent. |
| `accent` / `accent-strong` | Teal froid profond (`#0a6e77` clair). Historique : est passé par du vert forêt saturé, un mauve pastel, un gris froid, avant de se fixer sur ce teal — voir règle de couleur ci-dessous pour où il a le droit d'apparaître. |
| `success` / `warn` / `danger` (+ `-soft`) | Couleurs sémantiques **standards** (vert/jaune/rouge), volontairement indépendantes de `accent` — un statut "Fait" n'utilise jamais la couleur d'accent. |

### Rampe d'interaction (survol / enfoncé / sélectionné)

`sidebar-hover` portait à lui seul quatre rôles sans rapport — survol, enfoncé, remplissage du
bloc "Raisonnement", surlignage de la session active — donc survol et enfoncé étaient
littéralement la même valeur : indiscernables. Trois tokens les séparent désormais, et les
valeurs sont **mesurées, pas choisies à l'œil** :

```
repos #ffffff / #fbfcfd  →  survol #eef1f4  →  enfoncé / sélectionné #e4e8ed
```

Un premier jet utilisait un survol plus clair (`#f2f4f7`). Échantillonné avec `pxsample.py`, il
n'était qu'à 9 unités du `#fbfcfd` de la sidebar — à peine perceptible — et surtout il tombait
**plus clair** que le `#eef1f4` de la session active : pointer une ligne signalait donc moins
d'emphase que la ligne déjà sélectionnée, et l'enfoncer sautait par-dessus l'état qu'elle allait
prendre. La rampe est maintenant monotone, avec ≥ 10 unités par palier, aussi bien sur blanc que
sur le quasi-blanc de la sidebar.

Conséquence à retenir : le survol par défaut vaut exactement le remplissage standard d'un bloc
(`bubble-bg` = `#eef1f4`). Un contrôle posé **sur** un bloc rempli doit donc décaler ses deux
états d'un cran — `TextButton` et `IconButton` exposent tous deux `hover-bg`/`press-bg` pour ça.
Le bloc de raisonnement, qui était le cas d'usage historique de cet override, n'a plus d'aplat
dans aucun état depuis le 2026-09-07 (voir "Bloc de raisonnement" plus bas) et utilise donc le
ghost hover par défaut ; l'override reste disponible pour les cartes d'étape du plan et la bulle
utilisateur.

Durées : `Theme.anim-fast` (90ms) pour tout retour au pointeur, `Theme.anim-med` (160ms) pour un
changement que l'utilisateur n'a pas déclenché directement (le bouton d'envoi qui passe de gris à
accent quand le champ cesse d'être vide). Toute cible cliquable porte `mouse-cursor: pointer` —
aucune ne l'avait avant cette passe.

## Règle de couleur : l'accent est réservé au CTA

Règle explicite, redemandée plusieurs fois pendant l'itération donc à ne pas relâcher par
défaut : **`Palette.accent` n'apparaît que sur la vraie action principale**, aujourd'hui
uniquement le bouton d'envoi du `Composer`, et seulement quand il est actionnable (texte
non vide dans le champ). Partout ailleurs où on pourrait être tenté de mettre l'accent
(sélection active, indicateur "Fait", pastille de connexion, bouton "Nouvelle tâche"…), on
utilise le gris neutre ou la couleur sémantique correspondante (`success`/`warn`/`danger`),
jamais `accent`.

`ActionButton` (Exécuter/Annuler) suit la même logique : pas de remplissage accent, juste
une icône SVG (voir "Icônes" ci-dessous) teintée par `ActionTone` (`success` pour Exécuter,
`danger` pour Annuler).

## Carte d'approbation de commande (`RUN_COMMAND`)

`Operation::RunCommand` (2026-09-06) lance une vraie commande terminal : sans inverse, et
sans que Lito puisse dire à l'avance ce qu'elle va toucher. Deux marquages visuels en
découlent, et **ni l'un ni l'autre n'utilise `Palette.accent`** — la règle ci-dessus tient,
le poids sémantique passe par les icônes et par `danger`.

- **La carte d'approbation** (`ApprovalCard`) s'affiche **au-dessus du composer, dans la
  colonne de chat** — le seul endroit visible dans les deux cas où elle apparaît (une étape
  de plan `RUN_COMMAND`, ou une commande de reconnaissance émise pendant la planification).
  Le projet n'avait aucun motif de dialogue (les deux seuls `PopupWindow` sont le sélecteur
  de modèle et les Réglages) ; la carte est un simple `Rectangle` bordé `Palette.border` sur
  `Palette.surface`. Contenu : en-tête « APPROBATION REQUISE · <origine> » (`Étape 2/3` ou
  `Reconnaissance`), badge « shell » (`warn-soft`) si `shell: true`, programme résolu en
  chemin absolu (Menlo), `argv` un argument par ligne dans un bloc `bubble-bg` (Menlo), le
  `cwd`, la mention **« Cette commande ne peut pas être annulée. »** en `danger` italique,
  puis trois `ActionButton` — *Refuser* (`ActionTone.danger`), *Autoriser* (`neutral`),
  *Autoriser et mémoriser* (`success`, masqué si la commande n'est pas mémorisable, c.-à-d.
  `shell: true`).
- **Dans le panneau de plan**, une étape `RUN_COMMAND` montre sous sa description la ligne
  `$ <commande>` (Menlo, **colorée par token** depuis le 2026-09-13 — voir "Coloration
  syntaxique" ci-dessous), puis `shell · <cwd> · non annulable` en `text-faint`, et une fois
  la commande exécutée un en-tête `SORTIE · CODE n · TRONQUÉE` avec un chevron de
  dépliage (repliée par défaut) révélant `stdout`/`stderr` en `CopiableText` Menlo (`stdout`/
  `stderr` restent en une seule couleur - non tokenisés, ce n'est pas du code).
- **Panneau Réglages** : nouvelle section « EXÉCUTION DE COMMANDES » — un `ModeOption` (radio
  + titre + description) par mode d'approbation (`Validation` / `Auto — restrictif` / `Auto —
  non sûr`), puis « COMMANDES MÉMORISÉES » listant les entrées `(argv, racine)` avec un
  « × » par ligne (`forget-allowlist-entry`). Une permission qu'on ne peut ni inspecter ni
  révoquer n'en est pas une.

## Coloration syntaxique (blocs de code et commandes) (2026-09-13)

Les blocs de code d'une réponse (`MessageBlock`, kind `"code"`) et la ligne `$ <commande>` d'une
étape `RUN_COMMAND` dans le panneau de plan sont colorés par token (mot-clé/chaîne/commentaire/
nombre/nom de commande), pas seulement en `Menlo, monospace` uniforme. `ApprovalCard` (programme +
`argv`) l'est aussi, mais par *ligne* entière plutôt que par token — un élément d'`argv` est un seul
argument au niveau OS, le re-découper en mots le représenterait mal.

**Pourquoi une rangée de petits éléments plutôt qu'un seul bloc de texte coloré** : `TextInput`/
`Text` n'exposent qu'une seule couleur uniforme, et `StyledText` (l'autre primitive texte de Slint)
n'a pas de mécanisme pour colorer un passage arbitraire en dehors du sous-ensemble CommonMark qu'il
supporte — `<span style="color:…">` est documenté en amont mais vérifié **non fonctionnel** sur la
version réellement résolue (1.17.1 : `Cargo.lock`, pas le `"1.6"` du `Cargo.toml`), qui renvoie
`Err("HTML tag <span> is not supported")`. `CodeSpan` (nouveau composant, `app-window.slint`) est
donc un `TextInput` mono-ligne par token, coloré via `global Syntax` (`color`/`italic`/`weight`,
fonctions pures qui lisent la classe du token et retombent sur une couleur "plain" passée par
l'appelant — pas la même selon le contexte : `Palette.text` dans un bloc de code, `Palette.
text-muted` sur la ligne de commande du panneau de plan). Compromis assumé : la sélection
clic-glisser continue sur toute une ligne est perdue (chaque token reste sélectionnable
individuellement, et le bouton « Copier » du bloc copie toujours tout) — validé avec l'utilisateur
avant l'implémentation plutôt que découvert après coup.

**Classification** : `crates/ui/src/highlight.rs`, tokenizer maison sans dépendance (pas
`syntect` — écarté par choix explicite, jugé disproportionné pour des extraits courts de chat) :
shell/rust/python/js-ts/json/toml/yaml/C-like, plus un repli générique (chaînes/nombres/
commentaires seuls) pour un langage non reconnu. Best-effort assumé, pas un vrai analyseur — voir
le commentaire de tête du module.

**Palette** : quatre tokens neufs sur `global Palette`, `syntax-keyword`/`syntax-string`/
`syntax-number`/`syntax-function`, choisis à l'écart de `accent` (~185°, teal), `success` (~130°,
vert), `warn` (~45°, olive) et `danger` (~10°, rouge-orangé) sur le cercle chromatique — la règle
"l'accent est réservé au CTA" (voir plus haut) s'étend ici : aucune teinte de coloration syntaxique
ne doit pouvoir se confondre avec une couleur de statut ou avec l'accent. Les commentaires n'ont pas
de teinte dédiée : ils reprennent `text-faint` en italique, cohérent avec le reste du chrome "en
retrait".

**Piège rencontré, à ne pas réintroduire** : un `ScrollView` dont la hauteur est liée au
`preferred-height` de son contenu (pour un défilement horizontal sans retour à la ligne, le
traitement des blocs de code du chat) **ne résout jamais sa taille à l'intérieur d'une rangée de
`ListView`** — vérifié (`debug()` : `preferred-height` reste `0` quels que soient les bindings
essayés, alors que les données sont bien présentes). Le panneau de plan est un `ListView`
(contrairement au chat, un `ScrollView` + `VerticalLayout` classique — voir "Architecture UI" du
`CLAUDE.md` racine), donc la ligne de commande n'utilise **pas** ce mécanisme : un simple
`Rectangle { clip: true; min-width: 0px; }` autour de la rangée de `CodeSpan`, qui tronque plutôt
que de défiler. Le `min-width: 0px` n'est pas cosmétique : `clip` n'affecte que le rendu, pas la
négociation de taille amont, et son absence laissait le `Rectangle` réclamer la largeur non
tronquée de son contenu, élargissant toute la colonne `Theme.plan-w` (300px) — et donc la fenêtre
entière — pour une commande assez longue.

## Modale de diagnostic Ollama (2026-09-06)

Le bouton « Diagnostiquer la connexion » (section DIAGNOSTIC du panneau Réglages) lance
`OllamaClient::diagnose` — une vraie requête au modèle sélectionné, cloud compris — et son
résultat s'affiche dans une **modale**, pas dans le chat. C'est le premier (et le seul) motif
de dialogue centré + scrim du projet : contrairement à `ApprovalCard`, qui est rattachée à un
tour de conversation et vit donc dans la colonne de chat, le diagnostic est un état applicatif
global (« la connexion elle-même est cassée ») qui doit interrompre le reste.

- **Structure.** Enfant direct de la `Window`, déclaré en dernier → peint au-dessus (Slint n'a
  pas de z-index ; ordre de peinture = ordre du document). Scrim `#000000a6` plein écran avec
  un `TouchArea` qui **absorbe les clics sans fermer** — une instruction `ollama signin` à
  moitié suivie ne doit pas se perdre sur un clic à côté ; la fermeture est explicite (« × »
  en-tête + bouton « Fermer »).
- **Carte.** Fond `Palette.sidebar` et non `surface` : en thème sombre `surface == bg`, donc
  une carte `surface` disparaîtrait sur le scrim. Imbriquée dans des layouts de centrage (et
  non librement positionnée dans le `Rectangle` du scrim) pour que le texte `wrap` soit mesuré
  à la bonne largeur — un enfant libre d'un `Rectangle` est d'abord mesuré sans contrainte, ce
  qui écrase le texte sur une ligne puis le tronque. Le corps passe par `CopiableText`
  (sélectionnable : on veut pouvoir copier un nom de modèle ou un message d'erreur).
- **Contenu variable.** Titre + détail viennent pré-formatés du core (`describe_diagnosis`).
  Le bloc d'instructions `ollama signin` (mono, encadré) apparaît quand `needs_signin` — cas
  `CloudSignInRequired` (erreur d'auth explicite) **et** `CloudUnresponsive` (modèle cloud
  muet : le détail mène alors avec « démarrage à froid, réessaie », le bloc `signin` est la
  piste de repli). Le bouton « Fermer » prend le ton `success` quand la connexion est saine,
  `neutral` sinon. Pendant la sonde : « Diagnostic en cours… », sans bouton de fermeture
  (borné à 20 s côté core).
- **Déclenchement automatique.** La modale s'ouvre aussi seule après une génération qui échoue
  (`handle_diagnose(.., auto = true)`), mais un résultat sain est alors silencieux — pas de
  pop-up « tout va bien » juste après une erreur.

## Statuts standards

`StatusBadge` mappe `PlanStepData.status` (`"pending"`/`"running"`/`"done"`/`"error"`, déjà
ce vocabulaire côté `view_model.rs`, rien à changer côté Rust) vers :
- **Fait** → vert (`success`)
- **En attente** / **En cours** → jaune (`warn`)
- **Échec** → rouge (`danger`)

`StatusBadge` est dimensionné sur son propre texte (`width: label.preferred-width + …`) : en
simple enfant de layout il s'étirait sur toute la largeur restante de la ligne et se lisait
comme une bannière colorée, pas comme une pastille.

Chaque carte d'étape a aussi, à gauche, un petit badge carré au fond `*-soft` de ce même statut
(mêmes trois couleurs) contenant une **icône SVG** qui identifie l'opération — un second signal
visuel, pas un remplacement de `StatusBadge`.

Les icônes vivent dans `crates/ui/ui/icons/`, en deux familles :
- les icônes d'opération (`mkdir`, `move`, `copy`, `delete`, `rename`, `symlink`, `write-file`,
  `run-command`, plus `plan` pour le chip du chat) : dessinées à la main pour ce projet, SVG
  16×16, trait `1.35px`. Le composant `OpIcon` fait le mapping `PlanStepData.operation` → asset ;
  `run-command.svg` (un chevron `>` + une ligne, style « prompt terminal ») a été ajouté le
  2026-09-06 avec `Operation::RunCommand` ;
- les icônes de chrome génériques (`sun`, `moon`, `settings`, `activity`, `copy`, `code`,
  `align-left`, `chevron-down`, `chevron-up`, `x`, `play`, `undo-2`, `check`, `plus`, `brain`) :
  vendorisées depuis [Lucide](https://lucide.dev) (MIT, dérivé de Feather actif) — même famille de
  trait (`stroke`, coins arrondis) que les icônes d'opération faites main, choisie pour cette
  cohérence plutôt que pour recréer un style. Le composant `Icon` fait le mapping nom → asset ;
  `IconButton` l'enveloppe dans le même langage "ghost" que `TextButton` (pas de bordure/fond au
  repos, révélés au survol/press) pour toute action dont l'icône seule suffit — voir
  "Icônes plutôt que libellés" plus bas. `brain` (2026-09-06) identifie le bloc de raisonnement du
  chat, à la place du mot "Raisonnement" — voir "Espacement du chat et bloc de raisonnement".

Toutes sans remplissage (`fill: none`), teintées à l'affichage par `Image { colorize: … }` avec
la couleur sémantique voulue — c'est ce qui permet de vendoriser un SVG à trait noir et de
l'afficher dans n'importe quelle couleur/thème sans le retoucher.

> Les icônes d'opération remplacent les glyphes texte utilisés jusqu'au 2026-08-06 (`▢` MKDIR,
> `⧉` COPY, `✎` RENAME, `▤` WRITE_FILE), dont **4 sur 7 étaient invisibles** avec la police
> système macOS — pas de carré « tofu », rien du tout, aucun avertissement. Le même risque
> existait pour les glyphes Unicode utilisés ailleurs comme icônes de boutons (`×`, `✓`, `↺`,
> `+`) : remplacés eux aussi par des SVG Lucide le 2026-09-06, pour un rendu au trait uniforme
> plutôt que dépendant des métriques de la police système. Il ne reste que deux glyphes texte
> dans le fichier : `→` (bouton d'envoi du `Composer`) et `○` (puce statique devant "Tout
> s'exécute en local") — ni l'un ni l'autre n'est un bouton, et les deux ont été vérifiés au
> rendu. Règle qui reste valable : **ne jamais ajouter un glyphe texte décoratif sans l'avoir
> rendu**.

### Icônes plutôt que libellés

Passe du 2026-09-06 : plusieurs `TextButton` à libellé ("Copier", "Copier le markdown", le
bascule "Texte brut"/"Rendu", "Afficher"/"Masquer" du raisonnement, le toggle de thème, "Ping")
sont devenus des `IconButton` sans texte visible, pour désencombrer le chrome — ces actions sont
soit répétées (une par message/bloc de code), soit déjà lisibles au pictogramme seul. Chacun
porte `accessible-label` (texte du libellé d'origine) puisqu'il n'y a plus de texte visible pour
un lecteur d'écran. Ce qui **n'est pas** passé en icône seule : "Nouvelle tâche" (action
primaire et rare — garde une icône `plus` **et** son libellé) et "Annuler la dernière opération"
du pied de sidebar (action de sécurité peu fréquente — garde son libellé, seul son glyphe `↺`
est devenu une vraie icône). "Réinitialiser" (popup Réglages) reste un `TextButton` texte : action
secondaire à l'intérieur d'un panneau déjà réduit, pas de gain à en faire une icône ambiguë.

## Typographie

Pas de police custom embarquée : `default-font-family` est volontairement laissé non défini
pour que Slint pioche la police système (San Francisco sur macOS) — c'est le choix le plus
"natif macOS" possible, pas un oubli. `"Menlo"` (mono système) reste utilisée pour le contenu
qui est réellement du code (blocs de code du chat, voir tableau ci-dessous), **pas** pour la
description des cartes d'étape de plan (`step.description`) — jusqu'au 2026-09-06 elle l'était
aussi, sur l'hypothèse que ce champ contient un chemin de fichier ; `PlanStep::description`
(`agent/src/lib.rs`) est en réalité définie comme une phrase courte en langage naturel ("Create
Captures folder on Desktop"), qui contient parfois un chemin mais n'en est pas un. Le mono sur de
la prose lisait comme un rendu daté plutôt que technique — retiré, cette description utilise
maintenant la police proportionnelle comme le reste du chrome.

**Échelle typographique** (global `Theme` dans `app-window.slint`) — quatre rôles, et rien
d'autre :

| token | taille | usage |
|---|---|---|
| `font-xs` | 10px | méta : en-têtes de groupe de dates, "N étapes", pied de sidebar, `StatusBadge` |
| `font-sm` | 11px | chrome secondaire : libellés, chips, chemins, nom de modèle |
| `font-md` | 12px | chrome primaire : titres de session, libellés de boutons |
| `font-lg` | 14px | contenu : messages de chat, saisie du `Composer`, titre "Lito" (13px → 14px, cf. addendum 2026-09-09 bis) |

Une seule valeur de `letter-spacing` (`tracking`, 0.5px), pour les micro-labels en capitales.
**Deux familles**, pas une : le chrome est en police système macOS ; la surface de lecture
(`CopiableText`, le `StyledText` des réponses, la saisie du `Composer`) est en **Open Sans
embarquée**, via le token `Theme.body-font` — voir « Police de corps : Open Sans embarquée »
plus bas pour le pourquoi (métriques verticales, interligne de la prose).

Ces tokens remplacent les 8 tailles de police (dont `12.8px`, `11.5px`, `9.5px`) qui avaient
dérivé faute d'échelle nommée. Même logique pour l'espacement : une rampe de 4px
(`space-1`…`space-6` = 4/8/12/16/20/24px) remplace les 17 valeurs d'avant, y compris des paddings
sub-pixel (`0.4px`, `0.5px`, `0.6px`) qui n'étaient pas des choix de design mais de la
compensation manuelle de désalignements. **Toute nouvelle valeur passe par un token.**

### `Label` : pourquoi tout le chrome passe par un wrapper

Un `Text` Slint a exactement la hauteur de sa `font-size`, et les glyphes sont rognés à cette
boîte : **tout signe qui dépasse la hauteur de capitale est coupé**. En français cela veut dire
tous les accents sur majuscules — "PLAN PROPOSÉ" s'affichait "PLAN PROPOSE", "Échec" "Echec",
sans avertissement. `Label` (= `Text` + `min-height: font-size * 1.45` + centrage vertical)
donne la place nécessaire ; `CopiableText` applique le même correctif via `height:
preferred-height + space-1` (sa première ligne est calée en y=0, donc c'était la seule ligne
touchée). Utiliser `Label` — pas `Text` — pour tout libellé.

### Rendu markdown des réponses de l'agent

Une réponse `assistant` n'est pas rendue d'un bloc : elle est **découpée en blocs de premier
niveau** côté Rust (`crates/ui/src/markdown.rs::split_blocks`, avec `pulldown-cmark` — déjà dans
le graphe de dépendances via `i-slint-common`), et chaque bloc est rendu par le composant qui lui
convient (`MessageBlock` dans `crates/ui/ui/app-window.slint`) :

| bloc | rendu | sélectionnable |
|---|---|---|
| paragraphe | `StyledText` (gras/italique/barré/code inline/liens/listes) | non |
| item de liste (à puces ou numéroté) | un `StyledText` par item de premier niveau, marqueur inclus — voir « Écart entre blocs » | non |
| bloc de code | carte `Palette.bubble-bg`, rangée de `CodeSpan` (Menlo, colorés par token — voir "Coloration syntaxique" plus haut) dans un `ScrollView` horizontal, + action « Copier » | par token |
| titre | `CopiableText` gras (un seul cran de taille pour h1–h2) | oui |
| filet `---` | `HDivider` | — |
| tout ce que `StyledText` refuse (tableau, citation, image, HTML brut) | `CopiableText` du markdown source | oui |

Le découpage n'est pas une liste codée en dur de ce que `StyledText` accepte : chaque paragraphe
ou liste est réellement passé à `StyledText::from_markdown`, et retombe en texte brut si l'appel
renvoie `Err`. Il reste donc correct si le sous-ensemble supporté par Slint s'élargit.

Deux raisons à ce découpage, dans cet ordre :

1. **`from_markdown` échoue sur tout le message dès qu'une seule construction non supportée
   apparaît** (`i-slint-core-1.17.1/styled_text.rs:53-62` : `Err` si la liste d'erreurs n'est pas
   vide). Un seul ``` ``` ``` ou un `##` faisait donc basculer la réponse entière en markdown
   brut. Par bloc, seul le bloc fautif retombe.
2. **`StyledText` n'a aucune sélection de texte** — l'élément n'expose ni curseur, ni ancre, ni
   propriété de sélection (`i-slint-compiler-1.17.1/builtins.slint:731-753`), par choix délibéré
   en amont (slint-ui/slint#9560 exclut explicitement l'édition/la sélection de son périmètre ;
   le ticket générique de sélection sur `Text`, #736, reste ouvert). Les blocs rendus par
   `CopiableText` sont des `TextInput` : ils se sélectionnent au clic-glisser comme le reste. Le
   bloc de code — ce qu'on copie réellement dans une réponse — en fait partie.

Une superposition d'un `TextInput` invisible sous le `StyledText` a été envisagée et rejetée : le
texte en gras/lien est plus large que le même texte en clair, donc les retours à la ligne des deux
couches divergent dès qu'une ligne contient du formatage — la sélection tomberait visiblement à
côté des glyphes.

Le parsing se fait côté Rust parce que `.slint` ne peut pas invoquer ce parseur sur une chaîne
dynamique : seule la macro `@markdown()` le fait, et uniquement sur le texte littéral de sa propre
source ; toute valeur interpolée dedans est échappée, pas parsée. `ChatMessageData` porte donc
`content` (le markdown source), `content-plain` (le même texte sans les marqueurs) et
`blocks: [MessageBlockData]`.

Deux cas gardent délibérément le rendu « un seul `CopiableText` du texte brut », signalé par une
liste `blocks` vide :
- Les messages `system` — ce sont des chaînes générées par l'app (chemins de fichiers, etc.),
  jamais du markdown ; un chemin contenant un `_` ou un `*` isolé serait mal interprété par le
  parseur CommonMark si on le lui donnait.
- Les fixtures de `crates/ui/ui/fixtures/*.json` sous `slint-viewer --load-data`, sauf quand
  elles décrivent explicitement des blocs. **Un bloc de fixture ne doit jamais utiliser
  `kind: "styled"`** : le loader JSON de l'interpréteur n'a pas de cas pour le type `styled-text`
  (voir `slint-interpreter-*/json.rs::value_from_json`), donc `content-styled` reste `Value::Void`
  et instancier un `StyledText` dessus fait planter `slint-viewer` (`binding was of the wrong
  type: ()` — vérifié). Les autres `kind` passent par `CopiableText` et se chargent sans
  problème. De même, `"blocks": []` doit être écrit explicitement sur chaque message : un champ
  absent est `Value::Void`, et l'interpréteur ne court-circuite pas ses opérateurs.

Trois actions sous chaque message `assistant`, plus une par bloc de code :
- **« Copier »** — copie `content-plain`, c'est-à-dire ce qui est *affiché*, marqueurs markdown
  retirés (`markdown::plain_text`) : c'est ce qu'on attend en collant une réponse dans une note ou
  un message. Passe par le crate `copypasta` (`ViewModelHandle::copy_message`), déjà présent dans
  `Cargo.lock` en dépendance transitive : c'est ce que `i-slint-backend-winit` utilise pour le
  Ctrl+C des `TextInput` existants, donc rien de nouveau ne tourne sur macOS ici.
- **« Copier le markdown »** — copie `content`, la source telle que le modèle l'a écrite.
- **« Texte brut » / « Rendu »** — bascule la ligne vers un `CopiableText` unique de la source
  markdown, et inversement. Le découpage en blocs rend déjà sélectionnable tout ce qui n'est pas
  un paragraphe stylé ; cette bascule reste le seul moyen de sélectionner un paragraphe. État
  purement local à `ChatMessageRow` (`private property <bool> show-raw`, même mécanisme que
  `thinking-expanded`) : il se réinitialise si `root.messages` est reconstruit en bloc (nouveau
  message envoyé) pendant que la bascule est active — accepté, comme pour `thinking-expanded`.
- **« Copier » sur un bloc de code** — copie le code seul, sans les délimiteurs.

Le callback `link-clicked` de `StyledText` n'est pas encore câblé (les liens s'affichent dans
`Palette.accent` mais ne s'ouvrent pas au clic).

### Espacement du chat et bloc de raisonnement (2026-09-06)

**Pas de véritable interlignage possible.** Demande initiale : plus d'espace *entre les lignes*
d'un même paragraphe. Vérifié contre la doc vendorisée et l'API interne (`internal/core/
styled_text.rs`, `internal/common/styled_text.rs`) : ni `Text`, ni `TextInput`, ni `StyledText`
n'exposent de propriété de type `line-height`/leading pour du texte qui retourne à la ligne — le
`Theme.line-height` de ce fichier ne sert qu'à calculer la `min-height` des `Label` mono-ligne
(voir plus haut), pas à espacer les lignes d'un bloc qui wrap. Le seul levier réel pour "faire
respirer" le texte est donc l'espace **entre les éléments** (messages, blocs), pas l'interlignage
lui-même.

**Deux ramps de respiration, pas une.** Avant cette passe, l'espace entre deux messages
*différents* (`ChatMessageRow.padding-top`/`padding-bottom`, `space-1` chacun → 8px cumulés entre
deux lignes) valait exactement la même chose que l'espace entre les parties d'un **même** message
(bulle/raisonnement/corps/actions, `spacing: space-2` sur la même ligne → 8px aussi) : rien ne
distinguait visuellement "nouveau tour de parole" de "suite du même message". Le padding de
`ChatMessageRow` est passé à `space-2` (16px cumulés entre deux messages), sans toucher au
`spacing: space-2` interne — la hiérarchie est maintenant : dense à l'intérieur d'un message,
plus ouvert entre deux messages. **Mise à jour 2026-09-09** (voir addendum en bas de section) :
ce padding est passé à `space-3` (24px cumulés) quand la rampe inter-blocs d'une réponse a été
élargie, pour que la frontière entre deux tours reste l'écart le plus large à l'écran.

**Écart entre blocs d'une réponse : piloté bloc par bloc, pas une valeur unique.** L'espace
entre les `MessageBlock` d'une réponse est passé par `space-2` → `space-3` → `space-4`, puis a
été abandonné comme *valeur de layout unique* : une seule valeur ne peut pas à la fois grouper
une liste à puces et détacher un titre de section de cette liste. Le `VerticalLayout` des blocs
est maintenant `spacing: 0` et chaque bloc porte un `padding-top` (`MessageBlock.gap-top`, fixé
par la boucle `for` de `ChatMessageRow`) :

- titre : `space-6` (24px) — nouvelle section, décollée de celle du dessus ;
- item de liste précédé d'un autre item de liste : `space-3` (12px) — la suite se lit comme
  *une* liste, mais plus comme un mur (une liste entière est un seul `StyledText` qui empile ses
  items sans interligne, cf. « Pas de véritable interlignage » ci-dessus — c'est la densité que
  cette passe corrige) ;
- tout le reste, y compris le premier item d'une liste : `space-4` (16px) ;
- premier bloc de la réponse : rien.

Valeurs d'origine (2026-09-06) : `space-5` / `space-2` / `space-3`. Voir l'addendum
2026-09-09 ci-dessous pour l'élargissement.

**Les listes sont éclatées en un bloc par item côté Rust.** `markdown::split_blocks` sort chaque
item de premier niveau d'une liste à puces/numérotée dans son propre `Block`
(`list_item: true`), marqueur `- `/`N. ` conservé (donc numérotation et imbrication
préservées). Sans ça, les puces resteraient collées : `StyledText` n'expose aucun réglage
d'interligne. `plain_text` rejoint deux items consécutifs par un seul `\n` (et non `\n\n`) pour
qu'une liste copiée reste une liste. Test : `list_items_become_separate_blocks_keeping_nesting`,
`ordered_list_items_keep_their_number_when_split`. Corollaire fixtures : une puce = un bloc
`"list-item": true`, et **tout** bloc de fixture doit porter `"list-item"` (sinon
`msg.blocks[i - 1].list-item` fait `Void & Void` et plante `slint-viewer`).

**Bloc de raisonnement : sans carte, avec aperçu et bouton texte (2026-09-07).** Historique :
d'abord une carte `Palette.sidebar-hover` pleine affichée en permanence dès que `msg.thinking`
n'est pas vide ; puis (2026-09-06) cette carte réduite aux seuls états `live-thinking ||
thinking-expanded`, laissant une ligne nue au repos replié. Ce second état gardait un défaut : le
fond gris *apparaissait à partir de rien* au dépliage, une transition qui se lisait comme un bug
d'accordéon (fond absent → fond gris). La carte est maintenant supprimée dans **tous** les états.
Options écartées : une carte claire permanente (elle dépense un bloc gris entier pour une ligne de
chrome sur une réponse que la plupart des gens n'ouvrent jamais) ; garder le saut.

La ligne repliée n'est plus « icône + chevron » nu, mais **`brain` + aperçu + bouton "Afficher"** :
- **Aperçu.** `ChatMessageData.thinking-preview`, calculé côté Rust (`view_model::thinking_preview`,
  testé) : première ligne non vide de la trace, espaces compactés, coupée à ~10 mots, `…` final si
  quelque chose a été retiré. Rendu dans un `Label` `wrap: no-wrap` + `overflow: elide` +
  `max-width: 360px` — l'élision Slint sert de garde-fou pour une fenêtre étroite, mais la coupe
  utile est celle de Rust. Il donne un aperçu de *ce sur quoi* le modèle réfléchissait sans rien
  déplier.
- **Bouton, pas chevron.** Le chevron aligné à droite est supprimé ; un `TextButton` (langage
  visuel ghost habituel) porte seul l'affordance : "Afficher" replié, "Voir moins" déplié. Le
  glyphe `brain` reste cliquable (même bascule) en complément. Corollaire : l'ancien `IconButton`
  et ses overrides `hover-bg`/`press-bg` (qui servaient à s'écarter de la couleur de carte) ont
  disparu.
- **Déplié / en cours.** La trace complète coule sous l'en-tête en petit `text-faint`, exactement
  comme les blocs de la réponse coulent sous elle ; la séparation est portée par le glyphe, la
  petite police atténuée et l'écart `spacing: space-2` de `ChatMessageRow`, pas par un aplat.
  Déplié, l'en-tête se réduit à `brain` + "Voir moins" (pas d'aperçu). En live, l'aperçu et le
  bouton laissent place à "Réflexion…" en toutes lettres et le bouton est masqué : cet état est
  transitoire, doit se lire comme "en cours", et la trace ne doit pas être refermable en plein run.

`brain` reste le substitut du mot "Raisonnement" (Lucide, cohérent avec le reste des icônes de
chrome — voir "Icônes plutôt que libellés" plus haut ; délibérément pas un glyphe "sparkle",
cliché de design IA déjà écarté ailleurs dans ce document).

**Addendum 2026-09-09 — élargissement de la rampe de respiration.** Deuxième retour « texte
encore trop compact ». Rappel du constat de 2026-09-06 : Slint n'a *aucun* réglage d'interligne
(`line-height`/leading) sur `Text`/`TextInput`/`StyledText` — reconfirmé contre la doc en ligne
(`reference/elements/text/` : seul `letter-spacing` existe). À l'intérieur d'un paragraphe qui
wrap, il n'y a donc rien à donner ; l'écart *entre* blocs doit porter toute la sensation d'air.
Chaque cran de la rampe inter-blocs de `ChatMessageRow` est monté d'un `space-*` :
titre `space-5`→`space-6` (24px), suite de liste `space-2`→`space-3` (12px), reste
`space-3`→`space-4` (16px). En conséquence, `padding-top`/`padding-bottom` de `ChatMessageRow`
passe de `space-2` à `space-3` (24px cumulés entre deux tours) pour que la frontière de tour
reste le plus grand écart à l'écran, devant le plus grand écart intra-réponse (16px). Nouveau
token `Theme.space-6` (24px). Non touché : `spacing: space-2` interne (bulle/raisonnement/corps/
actions — ce sont les parties d'*une* unité, elles doivent rester serrées).

**Addendum 2026-09-09 bis — recherche upstream + choix de rester en 1.17.1.** Troisième retour, et
question explicite « fais l'upgrade Slint 1.18 ». Recherche faite sur les issues Slint avant de
lancer quoi que ce soit :

- `line-height` comme propriété directe : **issue #7782 « closed as not planned »**. Le mainteneur
  (discussion #5920) refuse le modèle « half-leading » de CSS et a préféré exposer `font-metrics`
  (fait : `Text.font-metrics`, lecture seule — ne permet pas de *poser* un leading).
- `line-height-factor` (facteur multipliant la hauteur de ligne naturelle de la police) : ajouté
  par la **PR #12649** (mergée le 2026-08-19), listé dans le `CHANGELOG.md` sous `[1.18.0] -
  Unreleased` comme couvrant `Text`, `TextInput` **et `StyledText`**.

**Décision : on RESTE en Slint 1.17.1 (release stable), on ne passe PAS à la bêta 1.18.**
Raison, vérifiée dans l'arbre à `master@21edd5f9` (2026-09-09), pas seulement dans le changelog :

- `line-height-factor` est bien sur `Text` (`ComplexText`) et `TextInput` — `internal/compiler/
  builtin_elements.rs` + `internal/core/items/text.rs` (`pub line_height_factor: Property<f32>`).
- **mais PAS sur `StyledText`** : dans la PR #12649, `tronical` a écrit « I'm not sure about having
  this for StyledText TBH. I'd remove it » et ça a été suivi — `StyledTextItem` (le builtin exporté
  comme `StyledText`) n'a ni le champ ni la propriété sur `master` aujourd'hui. Le changelog
  « et `StyledText` » est en avance sur le code.
- Or Lito rend **la prose et les listes** via `StyledText` (`MessageBlock`, `if block.kind ==
  "styled"`). Donc le cas le plus courant — un paragraphe de réponse normal — ne gagnerait
  *toujours pas* d'interligne réglable, même sur la bêta 1.18. Seuls les `CopiableText`
  (`TextInput` : trace de raisonnement, code, titres, tableaux, vue « texte brut », bulle
  utilisateur) en bénéficieraient.
- Le coût d'une dépendance git épinglée sur un commit de `master` (bêta mouvante ; MSRV Rust 1.92
  / edition 2024 pour les crates slint ; `slint-viewer` à recompiler depuis les sources pour que
  `make ui-*` comprenne la nouvelle prop ; rename `viewport-*`→`content-*`, alias gardés mais à
  surveiller côté scroll du chat) n'est pas justifié par un gain qui n'atteint pas la prose.

**À revisiter quand** : soit `StyledText` obtient `line-height-factor` (surveiller un suivi de
#12649 / les notes de version), soit **1.18.0 sort en stable sur crates.io** (la dernière release
y est 1.17.1). À ce moment-là : bump `slint`/`slint-build`/`i-slint-backend-winit`, `make
install-viewer` sur la nouvelle version, puis `line-height-factor: ~1.4` sur `CopiableText` et le
fallback plein-texte — la prose `StyledText` continuera de dépendre de la rampe inter-blocs tant
que le builtin n'a pas la prop.

En attendant, deux leviers 1.17.1 ajoutés (ils restent utiles même après un futur passage à 1.18,
puisqu'ils portent la prose) :

1. **Police de corps `font-lg` 13px → 14px.** Le ratio d'interligne ne change pas, mais 13px était
   petit pour du corps de texte ; en absolu le rendu est moins serré. Touche aussi la saisie du
   `Composer` (cohérent, même rôle « contenu »).
2. **Mesure de lecture bornée.** Nouveau token `Theme.read-measure` (560px, ~78 caractères à
   `font-lg`). La réponse de l'assistant, le fallback plein-texte et la trace de raisonnement sont
   enveloppés dans `HorizontalLayout { alignment: start }` avec `max-width: Theme.read-measure`
   (un `max-width` nu sur un enfant `VerticalLayout` d'un `VerticalLayout` est ignoré — vérifié en
   scratchpad ; c'est l'`alignment: start` du `HorizontalLayout` qui rend le cap effectif). Des
   lignes plus courtes se lisent moins « mur » à interligne égal. À la largeur de fenêtre par
   défaut (~1080px → colonne de chat ~540px) le cap ne mord pas ; il protège surtout les fenêtres
   larges, c'est-à-dire exactement le cas où c'était le pire.

### Police de corps : Open Sans embarquée (2026-09-10)

**Décision retenue pour l'interligne de la prose** (`StyledText`, qui n'a *aucun* réglage de
line-height ni en 1.17.1 ni sur la bêta 1.18) : **embarquer une face de corps aux métriques
verticales plus généreuses**, la seule voie qui agisse réellement sur l'interligne d'un
paragraphe qui wrap sans changer de version de Slint.

**Mécanisme, mesuré.** Sur le chemin skia, Slint empile les lignes à `ascent − descent` de la
police (`i-slint-core/textlayout.rs` : `y += self.layout.font.height()`, et
`FontMetrics::height() = ascent − descent` — **le terme line-gap est ignoré**). Donc le levier
n'est pas « une belle police de lecture » mais une face au ratio `(ascent − descent)/em`
réellement grand. Mesures scratchpad (`StyledText`, 14px, avance par ligne) :

| Police | px/ligne | vs système |
|---|---|---|
| Arial / Helvetica (≈ SF système) | ~16–17 | — |
| Lato / Source Sans Pro | ~17–17.6 | ~0 % |
| Roboto | ~16.6 | ~0 % |
| Roboto Slab | ~18.5 | +11 % |
| **Open Sans** | **~19.2** | **+15 %** |

Charter / Palatino / Georgia : testés, **≈ système** (c'est pourquoi le premier essai de simple
`default-font-family` n'avait rien donné). Open Sans est le sans le plus aéré des faces libres
courantes, et reste neutre (pas de bascule « éditoriale » comme un slab serif).

**Portée.** Open Sans **seulement sur la surface de lecture** : le composant `CopiableText`
(défaut `font-family: Theme.body-font`), le `StyledText` des réponses (`default-font-family:
Theme.body-font`), et la saisie du `Composer` + son placeholder. Le **chrome reste en police
système macOS** — `Label`, boutons, sidebar, panneau de plan : c'est là que sont calibrés
`optical-center-y` et l'anti-rognage d'accents, et le contraste chrome/contenu est voulu. Les
instances `CopiableText` de code/commande gardent leur override local `"Menlo, monospace"` (il
gagne sur le défaut du composant). Nouveau token `Theme.body-font` (`"Open Sans"` ; `""` =
retour à la police système).

**Fichiers.** `crates/ui/ui/fonts/OpenSans-{Regular,Bold,Italic,BoldItalic}.ttf` (~600 Ko au
total), statiques (pas le variable, pour que la sélection graisse/italique de Slint 1.17.1 soit
sûre), embarqués + enregistrés automatiquement par `slint_build::compile` via les
`import "fonts/OpenSans-*.ttf";` en tête de `app-window.slint` — **aucun code Rust**. Licence
**SIL OFL 1.1**, texte dans `crates/ui/ui/fonts/OFL.txt` (attribution : « The Open Sans Project
Authors », dépôt `googlefonts/opensans`) ; à reporter dans les mentions légales de l'app le jour
où il y en a.

**Toujours utiles en complément** (portent la densité perçue, pas l'interligne) : `font-lg` 13 →
14px, et `Theme.read-measure` (560px, mesure de lecture bornée via `HorizontalLayout {
alignment: start }` + `max-width` — un `max-width` nu sur un enfant `VerticalLayout` est ignoré ;
au défaut ~1080px la colonne fait ~540px donc le cap ne mord pas, il protège les fenêtres
larges). Rampe inter-blocs : titre `space-6` (24px), suite de liste `space-3` (12px), reste
`space-4` (16px) ; entre deux tours `space-3` de chaque côté (24px cumulés).

**Pistes écartées** : `line-height-factor` (n'existe pas pour `StyledText`, cf. addendum ci-dessus
sur le choix de rester en 1.17.1) ; éclater les paragraphes `StyledText` sur les sauts durs du
modèle (rien sur le wrap d'affichage, coût `markdown.rs` + sémantique « Copier ») ; injecter des
`\n` dans les `CopiableText` ; forker Slint. **Correctif propre à terme** : 1.18 stable +
`line-height-factor` étendu à `StyledText` — on pourra alors alléger la rampe et/ou la police.

### Centrage optique des libellés de bouton

`vertical-alignment: center` centre la boîte de ligne du texte (métriques ascent+descent de
la police), pas l'encre réellement dessinée. Une chaîne sans lettre à descendante (g/j/p/q/y —
la quasi-totalité du texte d'interface en français) a son encre confinée à la moitié
supérieure de cette boîte, donc elle paraît décalée vers le haut même dans une boîte
parfaitement centrée géométriquement — mesuré ~0.6 à 1.2px, de façon reproductible, sur
"+ Nouvelle tâche" et les `ActionButton`. Corrigé par `Theme.optical-center-y(parent-height,
self-height)`, appliqué via `y: …` sur l'instance (pas sur la définition de `Label` : `parent`
n'est pas accessible à la racine d'un composant). À réserver aux libellés courts et centrés
dans une boîte plus grande que leur texte (boutons, pastilles) — le corps de texte n'en a pas
besoin, et un texte avec descendante rendrait la correction contre-productive.

Un défaut plus grave a été trouvé au passage sur `ActionButton` : un `height` fixe combiné à un
`padding` sur le `HorizontalLayout` interne étirait ses enfants sur une hauteur (30 − 2×8 =
14px) plus petite que le `min-height` réel des `Label` à ces tailles de police (~15-17px) — le
texte était donc écrasé dans une boîte trop petite, avec un biais mesuré de 2 à 2.7px. Corrigé
en enveloppant la ligne icône+libellé dans un `VerticalLayout { alignment: center }`, qui la
laisse prendre sa hauteur naturelle puis la centre (même technique que le sélecteur de modèle
et le bouton d'envoi, voir "Layout" plus haut) — un `height` fixe sur un conteneur combiné à du
`padding` sur son contenu peut écraser ce contenu sous son `min-height` ; préférer envelopper
plutôt que contraindre.

### Séparateurs

Toute ligne de séparation est un élément dédié (`HDivider` / `VDivider`, 1px), **jamais** un
`border-width` sur le conteneur : une bordure de conteneur se dessine sur les quatre côtés, donc
elle doublait le divider de la section voisine (trait de 2px visible côté sidebar sous la
titlebar) et peignait un cadre parasite le long des bords de fenêtre. Les `border-width`
restants ne concernent que des contrôles autonomes (`Composer`, `SearchField` — transparente au
repos, révélée au focus —, chip de plan, les popups `model-popup`/`settings-popup`).

Le badge de modèle et le bouton "Nouvelle tâche" avaient chacun une bordure 1px jusqu'au
2026-09-06 ; retirée au profit d'un aplat (repos `surface-hover`, survol `surface-press`, appui
`border` — même ramp que `SearchField`) : deux pilules à peu près vides sur la ligne de chrome
la plus chargée de l'app, dont la seule fonction était de se signaler comme cliquables — un fond
plein fait ce travail sans hairline. Le badge "Réglages" a suivi le même traitement en devenant
icône seule (voir "Icônes plutôt que libellés"). Les popups qu'ouvrent ces deux badges gardent
leur bordure : c'est le seul indice d'élévation de cette UI (pas d'ombre portée), donc la retirer
les fondrait dans le fond.

**Ce qui a été retiré** une fois que les éléments réagissent au survol : la structure dessinée au
trait devenait redondante avec la structure dessinée par l'interaction. Sont passés de 8 `HDivider`
à 4 :
- les deux hairlines internes de la sidebar ("Nouvelle tâche" / recherche / liste) — les deux
  contrôles forment un seul bloc d'en-tête de 92px, et l'en-tête de groupe de dates ("Aujourd'hui")
  sépare déjà la liste ;
- celle entre le titre du panneau de plan et sa ligne d'actions — les boutons y sont des ghosts,
  un divider aurait été l'élément le plus lourd des deux.

**Ce qui reste** est porteur, pas décoratif : sous la titlebar, au-dessus du composer, au-dessus du
pied de sidebar (ces deux derniers partagent `Theme.bottom-bar-h` pour former une ligne continue),
et au-dessus de la liste d'étapes. Ce sont les limites des zones scrollables — sans elles le contenu
défile visuellement dans le composer.

Les **cartes d'étape** sont passées d'un contour 1px à un aplat `bubble-bg` : `surface` sur le fond
`sidebar` du panneau ne fait que ~2 unités d'écart, donc une carte sans bordure y serait invisible,
alors que `bubble-bg` (le token "bloc rempli" déjà utilisé par les bulles et le bloc de
raisonnement) s'en détache nettement dans les deux thèmes.

## Layout

- **Titlebar** (`Theme.titlebar-h`, 38px) : "Lito" occupe une cellule de largeur fixe **identique
  à celle de la sidebar de sessions** (`Theme.sidebar-w`, 220px), prolongée par un `VDivider` à
  cette frontière : la colonne de gauche est ainsi continue de la titlebar au bas de la fenêtre,
  sur le même pixel. La fenêtre macOS elle-même utilise une titlebar native transparente et
  "full-size-content" (`install_macos_titlebar_backend` dans `crates/app/src/main.rs`, via un
  `Backend::builder().with_window_attributes_hook(...)` d'`i-slint-backend-winit` — pas
  `slint::winit_030`, qui exige la feature "unstable" — configurée avant toute création de
  fenêtre Slint) : les boutons rouge/jaune/vert restent natifs et fonctionnels (fermeture,
  glisser-déplacer de la fenêtre), mais il n'y a plus de bande macOS séparée au-dessus de cette
  ligne — cette bande suivait l'apparence système, pas le thème "Sombre"/"Clair" de l'app (qui en
  est délibérément découplé, voir plus bas), et restait donc claire quand l'app passait sombre. Le
  titre natif est masqué (`with_title_hidden`) puisque "Lito" ci-dessous en tient déjà lieu.
  Conséquence directe : les boutons systèmes flottent au-dessus du coin haut-gauche de cette ligne,
  d'où `Theme.traffic-light-w` (78px) en `padding-left` de "Lito" au lieu de `Theme.space-3` — sans
  cet espace, le texte rendait sous les boutons. Vient ensuite le **contrôle unique fusionné**
  statut+sélecteur de modèle
  (pastille de connexion + nom du modèle courant, dans un conteneur rempli de 24px de haut — sans
  bordure depuis le 2026-09-06, voir "Séparateurs" — centré verticalement par un
  `VerticalLayout { alignment: center }` — un `height` posé directement sur un enfant de
  `HorizontalLayout` est respecté mais **aligné en haut**, et `max-height` y est ignoré, ce qui
  faisait s'étirer le sélecteur sur toute la hauteur de la barre), puis un espaceur, puis le
  toggle de thème (icône seule, `sun`/`moon`), puis le badge "Réglages" (icône seule, `settings`),
  puis "Ping" (icône seule, `activity`).
- Le sélecteur de modèle est un badge cliquable qui ouvre un petit `PopupWindow` maison listant
  `available-models` (une ligne par modèle, surlignage neutre sur le modèle courant), **pas** un
  `ComboBox` de `std-widgets` : son `min-width` interne (`max(160px, ...)`, non redimensionnable
  proprement) et son binding `model`/`current-index` se sont révélés peu fiables à piloter depuis
  le code hôte à chaque cycle de synchronisation d'état (le sélecteur affichait une case vide) —
  voir les entrées 22bis/22ter de `documentation/lito/llm_brief/history.md`.
- Le badge "Réglages" ouvre le même genre de `PopupWindow` maison que le sélecteur de modèle,
  pour la même raison (contrôle qui ouvre un panneau, pas une simple action comme le toggle de
  thème ou "Ping") — mais, comme le sélecteur de modèle, sans bordure propre depuis le
  2026-09-06 : un aplat suffit à le signaler comme cliquable (voir "Séparateurs"). Son popup
  contient un champ d'endpoint Ollama éditable, "Réinitialiser" (recharge la valeur par défaut
  d'Ollama dans le champ sans sauvegarder) et "Enregistrer" (`ActionButton`, ton succès) — voir
  entrée 30 de `history.md`.
- **Trois colonnes** : sidebar de sessions à largeur fixe (220px, toujours visible) — chat en
  `horizontal-stretch: 1` — inspecteur de plan à largeur fixe (300px), visible seulement quand
  `show-plan` est vrai. Les deux sidebars fixes suivent le même traitement visuel
  (`Palette.sidebar` + un `VDivider` côté chat, pas d'ombre).
- **Barre du bas** : le bloc `Composer` de la colonne chat et le pied de la sidebar de sessions
  lisent tous les deux `Theme.bottom-bar-h` (64px = `space-3` + contrôle 36px + `space-4`), et
  chacun est précédé de son propre `HDivider` — les deux traits tombent donc sur la même ligne
  au pixel près, au lieu de deux hauteurs calculées séparément qui « tombaient presque » pareil
  (vérifié par échantillonnage de pixels, pas à l'œil).
- **Fenêtre redimensionnable** : `preferred-width`/`preferred-height` (pas `width`/`height`,
  qui figeraient la taille) + `min-width: 640px` / `min-height: 420px`. La mise en page est
  pensée pour être responsive (stretch/reflow) plutôt que pour une taille de fenêtre fixe.

## Sidebar de sessions

Colonne de gauche listant les sessions (conversations), groupées par date ("Aujourd'hui",
"Hier", puis "3 juin" etc. — calculé côté Rust dans `crates/app/src/main.rs::format_group_label`,
pas en Slint) et filtrables via un champ de recherche (`SearchField`, filtre client-side dans
`sync_to_ui`, pas d'aller-retour vers le core). La ligne active est surlignée en gris neutre
(`Palette.sidebar-hover` + bordure), jamais en accent — même règle que partout ailleurs dans
cette UI. Pied de sidebar ancré en bas (hors de la `ListView`, qui absorbe l'espace restant via
`vertical-stretch: 1`) : "Annuler la dernière opération" (reprend l'undo filesystem existant) et
un indicateur statique "Tout s'exécute en local", qui remplace le texte d'aide qui était
auparavant sous le `Composer`.

Dans le chat, le message qui vient de proposer un plan affiche un chip "Plan proposé · N étapes"
cliquable qui rouvre le panneau inspecteur (`open-plan`/`close-plan`, état purement local côté
`ViewModelHandle` — pas de round-trip vers le core, voir `crates/ui/src/view_model.rs`). Le panneau
lui-même a maintenant trois états pilotés par `current-plan-status` : `"pending"` (Rejeter/Exécuter),
`"done"` (Annuler), `"rejected"` (aucun bouton — rien n'est exécutable pour une proposition
rejetée). "Rejeter" est distinct d'"Annuler" : rejeter une proposition n'a jamais touché au disque,
donc pas d'entrée dans le undo log filesystem — contrairement à "Annuler" qui reprend
`FsOps::undo`.

Le titre du panneau (« PLAN PROPOSÉ », gras, majuscules, tracké — l'élément le plus visible du
bloc) porte l'état plutôt qu'une ligne annexe : « PLAN REJETÉ » en `Palette.danger`, « PLAN
EXÉCUTÉ » en `Palette.success`. Jusqu'au 2026-09-06 l'état rejeté était un `Label` texte
("Plan rejeté", gris, italique) inséré dans la ligne d'actions — une ligne dont le
`padding-left: -Theme.space-3` est calibré pour annuler le padding interne d'`ActionButton`
(voir plus haut) ; un `Label` nu n'a pas ce padding à annuler, donc il rendait visiblement décalé
de 12px par rapport au titre au-dessus. Faire porter l'état par le titre supprime la ligne
fautive plutôt que de corriger son padding, et gagne en emphase par la même occasion (couleur +
position la plus visible du panneau, au lieu d'une ligne grise facile à manquer). Les cartes
d'étape d'un plan rejeté sont en plus estompées (`opacity: 0.5` sur la `ListView`, même idiome que
les contrôles désactivés ailleurs) — sans ça leurs badges "En attente" ambre laissaient croire
que l'exécution allait quand même avoir lieu.

## Retour visuel pendant la génération

Une génération locale dure des dizaines de secondes. Trois surfaces doivent dire "ça travaille",
chacune à sa manière, sans se répéter :

| Surface | Signal | Pourquoi celui-là |
|---|---|---|
| Chat — flux en pause | `StreamingCaret` (bloc clignotant) sur le message assistant en cours | le texte qui s'écrit est déjà un signal ; le caret ne sert qu'aux **pauses** d'un flux commencé (modèle lent, un `Token` puis plus rien) où le flux s'arrête sans être fini |
| Chat — attente sans flux | `Spinner` (arc qui tourne) + libellé italique : « Exécution en cours… » sous une ligne `Commande : …` / `Listing …` (message `system` `streaming`), « Génération… » sous un message assistant encore vide | rien ne stream — une commande de reconnaissance / requête disque tourne et le modèle est bloqué dessus, ou le modèle réfléchit avant son premier token. Un bloc qui clignote se lit « en pause » ; ici il faut « ça travaille », donc ça **tourne**. Avant, le message `Commande : …` restait figé sans indicateur pendant tout l'aller-retour — lu comme un gel de l'UI |
| Panneau de plan — plan en cours d'écriture | titre « PLAN EN COURS », ligne italique « Le modèle écrit le plan… », étapes qui apparaissent une à une | les étapes qui arrivent *sont* le signal ; y ajouter un caret faisait doublon (essayé, rendu, retiré) |
| Panneau de plan — étape en cours d'exécution | `Spinner` (teinté `warn`) à gauche du badge « En cours » | une étape peut rester sur un `RUN_COMMAND` lent ou un aller-retour d'approbation ; le badge seul ne dit pas si c'est vivant ou coincé |
| Composer | bouton « Arrêter » à droite du champ grisé | seule action possible pendant la génération |

**`StreamingCaret`** est un bloc de 7×`font-lg` qui clignote à 520 ms, piloté par un `Timer` qui
bascule un booléen — pas par une `animate` infinie : une animation a besoin d'une propriété qui change
pour avoir quelque chose à interpoler. L'animation ne fait qu'adoucir les bords du basculement.

**`Spinner`** est un `Rectangle` portant une `Image` (`icons/loader.svg`, l'arc Lucide `loader-circle`)
dont la `transform-rotation` avance de 30° toutes les 80 ms via un `Timer` — même raison que le caret
pour ne pas utiliser une `animate` infinie, et `transform-rotation` est silencieusement ignorée sur
l'élément racine d'un composant, d'où l'`Image` interne. `size` et `tint` sont réglables (`tint`
plutôt que `color`, réservé par `Rectangle`). Il marque un travail **sans aucun flux de tokens**, là
où le caret marque un flux commencé puis en pause — d'où « ça tourne » vs « ça clignote ».
Côté état (`view_model.rs`) : le message `system` de `CoreToUiMsg::Querying` est désormais poussé
avec `streaming: true` ; `settle_query_spinners` l'éteint dès que le modèle reprend
(`Token`/`Thinking`/`PlanStreaming`, ou un nouveau `Querying`), `finish_streaming` s'en charge en
fin de tour. `settle_query_round` ne regardait que le dernier message *assistant*, jamais la ligne
`Querying` une fois qu'un message la recouvrait.

Il est posé sur **sa propre ligne** sous le texte, pas à la fin de la dernière ligne. Slint n'a pas de
mise en page en flux inline : placer un élément après du texte qui a été coupé en lignes obligerait à
réimplémenter la coupure de lignes. Le compromis est assumé, et le caret en début de ligne suivante se
lit comme un curseur de terminal.

Le bouton **« Arrêter » apparaît à côté du composer, il ne le remplace pas** : substituer un bouton au
champ ferait sauter la géométrie de la barre du bas à chaque envoi et à chaque fin de réponse. Le
champ reste visible, grisé, à sa place.

### `"streaming"` : le quatrième statut de plan

`current-plan-status` vaut `"pending"`, `"done"`, `"rejected"` — ou `"streaming"` pendant que le
modèle écrit. Ce dernier n'est pas un statut comme les autres : le plan n'a **pas encore d'identité**
(pas de `plan-id`, rien en base, rien d'exécutable). Le panneau masque donc dans cet état la ligne
« Plan &lt;id&gt; » et les boutons Exécuter/Rejeter, qui n'auraient rien à désigner. Si le tour se
termine sans qu'un vrai plan en sorte (JSON invalide, ou « Arrêter »), les étapes affichées sont
effacées — voir `discard_streaming_plan` dans `crates/ui/src/view_model.rs`.

## Défilement de la conversation

La colonne de conversation est une `ScrollView` sur une `VerticalLayout`, pas une `ListView`.
`ListView` virtualise, ce qui n'apporte rien à cette taille de liste (la conversation est plafonnée
à `MAX_HISTORY_MESSAGES` messages) et coûtait deux défauts visibles à l'écran : la hauteur de son
viewport est extrapolée d'une moyenne glissante des hauteurs de lignes déjà instanciées, donc avec
des messages allant d'une ligne à quarante l'ascenseur changeait de taille et la position dérivait
pendant le défilement ; et son `viewport-y` appartient au layout runtime (la passe flickable du
compilateur saute délibérément de le lier pour `ListView`), donc rien côté Slint ne pouvait le
piloter.

Le suivi automatique de la réponse en cours est un compromis : suivre toujours empêcherait de
relire ce qui précède, ne jamais suivre obligerait à faire défiler à la main pendant toute la
génération. La règle retenue : le suivi est **armé** par une action qui veut dire « montre-moi la
suite » (envoyer un message, ouvrir une session), **abandonné** dès que l'utilisateur remonte de
plus de `Theme.stick-slack` (24 px) au-dessus du bas, et **réarmé** s'il redescend. 24 px est plus
grand qu'un cran de molette — un cran isolé ne détache donc pas la vue — et plus petit qu'un
message, donc remonter pour lire détache bien.

## Composants custom (et pourquoi ils existent)

`std-widgets` (`Button`, `LineEdit`, `ComboBox`) a été abandonné pour tout ce qui doit respecter
les angles droits et les couleurs de la palette, ou qu'il faut piloter fiablement depuis Rust :
leur chrome de plateforme (coins arrondis, fond natif) n'est pas resurfaçable via les propriétés
exposées, et `ComboBox` en particulier s'est révélé peu fiable à piloter depuis le code hôte
(voir "Titlebar" plus haut) — remplacé par un badge texte cliquable maison. Restent de
`std-widgets` : `ListView` pour la sidebar et les étapes de plan, et `ScrollView` pour la
conversation (voir "Défilement de la conversation" ci-dessous).

- **`Label`** — `Text` + boîte de ligne de 1.45em + centrage vertical, pour ne pas rogner les
  accents sur majuscules (voir "Typographie"). À utiliser pour **tout** libellé de chrome.
- **`HDivider` / `VDivider`** — les seuls traits de séparation (voir "Séparateurs").
- **`OpIcon`** — mappe `PlanStepData.operation` vers l'icône SVG de `crates/ui/ui/icons/`.
- **`Icon`** — mappe un nom d'icône de chrome générique (`sun`, `copy`, `chevron-down`, …) vers
  son SVG Lucide, teinté par `colorize` (voir "Icônes").
- **`IconButton`** — `TextButton` sans texte : même ramp de survol/appui, boîte carrée autour
  d'une `Icon`, `accessible-label` pour compenser l'absence de libellé visible.
- **`StreamingCaret`** — le bloc clignotant : un flux de tokens commencé puis en pause (voir
  "Retour visuel pendant la génération").
- **`Spinner`** — l'arc qui tourne : un travail sans aucun flux (commande de reconnaissance /
  requête disque en cours, modèle qui réfléchit avant son premier token, étape de plan en
  exécution). `size`/`tint` réglables (voir "Retour visuel pendant la génération").
- **`Composer`** — remplace `LineEdit` : `TextInput` brut dans un conteneur bordé (36px), avec le
  bouton d'envoi (28px) intégré dans le même groupe visuel et centré verticalement par son propre
  `VerticalLayout { alignment: center }`.
- **`SearchField`** — variante allégée de `Composer` sans bouton, pour le filtre de sessions.
- **`ActionButton`** — remplace `Button` pour Exécuter/Rejeter/Annuler, et Refuser/Autoriser
  dans la carte d'approbation.
- **`StatusBadge`** — pastille de statut sémantique pour les étapes de plan.
- **`ApprovalCard`** — la carte d'approbation d'une commande `RUN_COMMAND`, au-dessus du
  composer (voir "Carte d'approbation de commande").
- **`ModeOption`** — une ligne radio + titre + description du sélecteur de mode d'approbation,
  dans le panneau Réglages.
- **`SessionRow`** — ligne de la sidebar (titre + sous-titre + en-tête de groupe optionnel),
  surlignage neutre quand active.

## Hors scope (pas des oublis)

Les trois points précédemment listés ici — sidebar de sessions, vrai "rejeter le plan", thème
sombre — ont été implémentés (voir "Sidebar de sessions" plus haut et l'entrée 22 de
`documentation/lito/llm_brief/history.md`). Restent hors scope :
- **Dossier de travail courant** — `send-message` accepte un `context` (chemin de dossier) mais
  l'UI l'envoie toujours vide (`root.send-message(self.text, "")`) ; il n'y a pas de sélecteur de
  dossier. Le sous-titre d'une session dans la sidebar n'affiche donc que "N étapes", jamais un
  nom de dossier, tant que ce concept n'existe pas côté UI.
- **Suppression de session** — créer/lister/switcher existe, pas supprimer.
- **Détection d'apparence système** pour le thème — le toggle "Sombre"/"Clair" est manuel
  uniquement, pas de suivi automatique du thème macOS.
- **Focus clavier visible ailleurs que sur les champs texte** — `Composer` et `SearchField` ont
  un liseré de focus, mais les boutons ne sont pas atteignables au clavier (pas de `FocusScope`),
  donc il n'y a pas d'anneau de focus à leur donner. C'est un vrai manque d'accessibilité, pas une
  décision de design.

Voir `documentation/lito/llm_brief/history.md`, entrées 21 (portage du design initial) et 22
(sessions/thème sombre/rejeter/chip de plan), pour le détail des changements Slint.
