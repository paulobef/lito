# Contribuer à Lito

## Avant de committer

Lance, dans cet ordre (ou directement `make ci`, qui reproduit le job CI `check`) :

```bash
make fmt      # cargo fmt --all
make clippy   # cargo clippy --workspace -- -D warnings
make test     # cargo test --workspace
make ui-check # type-check crates/ui/ui/app-window.slint (si un .slint a été touché)
```

Les quatre doivent passer sans avertissement avant toute PR ou commit sur `main`. La CI GitHub
Actions (`.github/workflows/ci.yml`) le fait automatiquement sur chaque push/PR, sur
`macos-latest`, plus un `cargo build --release`.
`crates/llm`'s `test_ollama_client` est marqué `#[ignore]` (nécessite un serveur Ollama local sur
`localhost:11434`, non mocké — voir roadmap P0.4) ; `make test`/la CI ne le lancent donc pas. Pour
le lancer explicitement : `cargo test -p lito-llm -- --ignored`.

Si le changement touche `crates/ui`, lance aussi `make ui-shot` et relis les captures dans
`target/ui-shots/` avant de considérer le rendu correct — voir CLAUDE.md, section "UI
architecture", et le skill `slint-lito`. La CI le fait aussi (job `ui`) et publie les PNG en
artefact de build pour la relecture depuis la PR.

## Convention de commit

Message impératif, court, qui décrit le changement fonctionnel plutôt que les fichiers touchés
(« Add path confinement policy to crates/fs », pas « update lib.rs »). Un corps optionnel explique
le *pourquoi* quand ce n'est pas évident depuis le diff. Pas de préfixe imposé (`feat:`, `fix:`…).

## Documentation à tenir à jour dans le même changement

`documentation/lito/` n'est régénéré par rien : si le changement touche l'architecture, les
crates, les commandes, ou une convention, mets à jour dans le **même commit** :

- `documentation/lito/llm_brief/summary.md` et `history.md`
- `documentation/lito/design.md` pour tout changement visuel ou de système de design
- `documentation/lito/roadmap.md` pour cocher/annoter un item livré
- `CLAUDE.md` lui-même si l'architecture change

Voir `CLAUDE.md` à la racine pour le détail de l'architecture, des crates, et des gotchas Slint
avant de toucher `crates/ui`.
