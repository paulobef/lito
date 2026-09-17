# Fixtures de rendu pour `slint-viewer --load-data`

Ces fichiers JSON peuplent `MainWindow` avec un état réaliste pour pouvoir **rendre l'UI en PNG
sans lancer l'app** (donc sans Ollama, sans base SQLite, sans core agent) :

```bash
make ui-shot     # rend demo (clair + sombre), stress et streaming dans target/ui-shots/
```

Pourquoi ça existe : sans données, `slint-viewer --screenshot` sur `app-window.slint` ne montre
qu'une coquille vide — toutes les listes (`messages`, `plan-steps`, `sessions`) sont alimentées
depuis Rust et sont vides par défaut. Or c'est précisément avec du contenu réel que les bugs de mise
en page apparaissent. Voir `.claude/skills/slint/SKILL.md`, section « Voir les pixels ».

## Format

Les clés sont les propriétés `in`/`in-out` de premier niveau de `MainWindow`, en kebab-case exactement
comme déclarées dans `app-window.slint`. Une clé portant le nom d'un `global` règle les propriétés de
ce global — c'est comme ça qu'on force le thème sombre :

```json
{ "Palette": { "dark": true } }
```

Une clé inconnue est ignorée avec un `Warning: ignoring unknown property: …` (utile pour repérer une
propriété renommée : si le warning apparaît, la fixture a dérivé du `.slint`).

## Les trois fixtures

| Fichier | Rôle |
|---|---|
| `demo.json` | État nominal, plausible : une session active, deux messages, un plan de 3 étapes en attente. C'est la capture « à quoi ressemble l'app ». |
| `stress.json` | État hostile : chemins très longs et insécables, titres de session qui débordent, les **7** opérations (`MKDIR`/`MOVE`/`COPY`/`DELETE`/`RENAME`/`SYMLINK`/`WRITE_FILE`), les 4 statuts, une trace de raisonnement longue en streaming, et assez de sessions pour forcer le défilement. C'est celle qui casse les mises en page. |
| `streaming.json` | État transitoire : `is-processing: true` (bouton « Arrêter » visible), un message assistant en cours sans contenu (caret + « Génération… »), et un plan en statut `"streaming"` — donc sans `current-plan-id`, sans boutons Exécuter/Rejeter, avec le titre « PLAN EN COURS ». C'est le seul moyen de rendre ces états, qui n'existent que pendant une génération. |

Rends **les trois**. Un layout qui tient sur `demo.json` mais casse sur `stress.json` est un layout
cassé — c'est `stress.json` qui a révélé que 4 des 7 glyphes d'opération ne s'affichaient pas du
tout avec la police système (remplacés depuis par les icônes SVG de `crates/ui/ui/icons/`), et que
"Échec" perdait son accent.

## Maintenance

Ces fixtures dupliquent la forme de `ChatMessageData` / `PlanStepData` / `SessionRowData` et des
propriétés de `MainWindow`. Rien ne les synchronise automatiquement : si tu ajoutes ou renommes un
champ dans `app-window.slint`, mets-les à jour dans le même changement, sinon le warning
« unknown property » ci-dessus finira par apparaître au prochain rendu.
