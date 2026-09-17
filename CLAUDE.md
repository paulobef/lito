# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this repo is

Lito is a local-first, privacy-first macOS app that automates file operations from natural-language
requests, using a locally-run LLM (Ollama) for planning and a small set of primitives — the
reversible, always-undo-logged filesystem ops (mkdir/move/copy/delete/rename/symlink/write_file)
plus `RUN_COMMAND`, which runs a real terminal command and, unlike the others, has no inverse and
can't be analyzed ahead of time, so it goes through an approval policy (`crates/policy`) and a
per-command approval round-trip rather than `FsOps`. There is **no git repo** initialized at the
root — treat any "recent changes" questions by reading file contents/timestamps, not `git log`.

This used to be two parallel implementations (a SwiftUI host app + Rust sidecar, and a separate
Rust+Slint rewrite living in `lito-v2/`). The Swift/sidecar implementation, the `protocol/`
JSON-Schema doc, and a standalone sandbox-entitlements experiment have all been deleted; **this repo
is now exclusively the single-binary Rust + Slint rewrite**, flattened from `lito-v2/` up to the
repo root. `Cargo.toml`, `Makefile`, `crates/`, `Info.plist`, and `assets/` all live at the repo root
now — there is no `lito-v2/` directory anymore. `cargo check --workspace` passes cleanly.

## Architecture

Single Rust binary (`lito-app`), single process, no IPC: the Slint UI and the core agent logic run
in the same process on the same `tokio` multi-threaded runtime, talking over `tokio::sync::mpsc`
channels (`UiToCoreMsg` going in, `CoreToUiMsg` coming back — both defined in `crates/ui`) instead of
serialized JSON. `crates/protocol` now holds only the shared vocabulary that's actually used
across crates — `Operation`, `PlanStep`, `ErrorCode`, `PlanId` — imported by `agent`, `fs`,
`memory`, `app`, and `ui`. It used to also define an older `ClientMessage`/`ServerMessage` NDJSON
shape plus a `SessionId` type inherited from the pre-rewrite (SwiftUI host + Rust sidecar)
design; those were confirmed unused anywhere outside the file itself and were deleted, along with
the now-unneeded `serde_json` dependency. Don't reintroduce a wire-message enum here without
checking whether `UiToCoreMsg`/`CoreToUiMsg` (the real UI protocol) already covers the need.

Crate layout (`crates/`):
- `app/` — binary entry point (`main.rs`): builds `Agent`, `OllamaClient`, `MemoryStore`, `FsOps`,
  wires the UI↔core `mpsc` channels, spawns `run_core_agent` on `tokio::spawn`, then blocks the main
  thread running the Slint event loop via `AppWindow::run()`
- `ui/` — Slint UI + `ViewModel`. See "UI architecture" below — this crate is the one that used to
  be entirely broken and has since been fixed and restructured.
- `protocol/` — legacy shared types, kept for potential persistence/debugging (see above)
- `agent/` — `Agent::respond(...)`: builds a system prompt instructing the model to return a raw
  JSON array of `PlanStep`s (or a plain chat reply, a `{"query":...}` filesystem query, or a
  `{"command":[...],"cwd":"..."}` reconnaissance command), then classifies/parses it. `Agent`
  holds no state and is planning-only — it does **not** execute or undo plans (see `fs/`). It
  runs reconnaissance commands through a `ReconRunner` trait it's handed (implemented by `app` as
  `GateReconRunner` over the policy + approval gate), so `agent` doesn't depend on `policy`/`exec`.
- `fs/` — `FsOps`: real filesystem ops (`create_dir_all`/`move_file`/`copy_file`/`delete`/
  `create_symlink`) plus an in-memory undo log. `execute_step(step, session_id)` dispatches a
  `PlanStep` to the right op; every mutating op logs an `UndoEntry` tagged with `session_id`, and
  `undo(session_id)` replays only that session's entries in reverse. **`Operation::RunCommand` is
  deliberately *not* handled here** — its `execute_step` arm returns an explicit `Err`; commands
  are neither file ops nor reversible, so `app::dispatch_step` intercepts them before `FsOps` and
  routes them through `CommandPolicy` + the approval gate + `lito_exec`. `session_id` is a plain
  freshly-generated UUID string minted per plan execution in `app/main.rs` — there's no `Agent`- or
  `SessionId`-typed indirection here, deliberately (see git history / this file's note below on why
  that indirection was removed).
- `llm/` — `OllamaClient`: health check, non-streaming `complete`, streaming `stream` over
  `/api/generate`. All methods take `&self` — the client holds no mutable state.
- `policy/` — `lito-policy`: the approval rules for terminal commands (`Operation::RunCommand`).
  `CommandPolicy::judge(&CommandSpec) -> Verdict` (`AutoAllow` / `NeedsApproval` / `Denied`),
  evaluated denylist-first so `auto-unsafe` can't bypass the hard refusals, then a
  `cwd`-must-be-inside-an-authorized-root check, then the `ApprovalMode`. `denylist.rs` is the
  hard, mode-independent blocklist (tested exhaustively); `readonly.rs` is a small argv-pattern
  allowlist consulted *only* by reconnaissance commands to skip the approval prompt. No UI, no
  filesystem writes — fully unit-tested. Sized to also host `P1.1`'s `PathPolicy` later.
- `exec/` — `lito-exec`: `run(&CommandSpec) -> CommandOutcome`. Assumes the policy already
  cleared the spec; only concerns itself with running the process safely — `stdin` nulled, a
  **rebuilt** (not inherited) environment (`PATH`/`HOME`/`USER`/`LANG`/`TERM=dumb` only, so no
  parent API key leaks in), 64 KiB per-stream output cap, 120s timeout-then-kill. A non-zero exit
  is `Ok`, not `Err`.
- `memory/` — SQLite persistence (`rusqlite`, bundled) for plans and an undo-log table. The
  connection is wrapped in `std::sync::Mutex<Connection>` (not exposed as a bare `Connection`)
  specifically because `rusqlite::Connection` is `Send` but not `Sync`, and `run_core_agent` in
  `app/main.rs` holds `&MemoryStore` across `.await` points inside a `tokio::spawn`'d (thread-movable)
  future — that requires `MemoryStore: Sync`, which the bare connection doesn't satisfy.

### Command execution and the approval round-trip

`run_core_agent` in `app/main.rs` drives long-running work (`SendMessage`, and now `ExecutePlan`)
through **one** generalized pump, `drive_with_ui_pump(fut, ui_rx, deferred, gate)` — the old
inline `biased` `select!` from the `SendMessage` handler, lifted out so plan execution shares it
instead of growing a second, subtly different copy. It routes `ApprovalResponse` to the
`ApprovalGate`, treats `Cancel`/channel-close as an interrupt, and defers everything else.

`ApprovalGate` holds a `Mutex<HashMap<request_id, oneshot::Sender<ApprovalDecision>>>`. A
`RunCommand` step (or a reconnaissance command) that needs approval calls `gate.request(...)`,
which sends `CoreToUiMsg::ApprovalRequest` and `.await`s the `oneshot`. Cancellation needs no
dedicated code: dropping the awaiting future drops the receiver, so `request` resolves to `Deny`.

`app::dispatch_step` sits in front of `FsOps::execute_step`: `RunCommand` → `CommandPolicy::judge`
→ (approval) → `lito_exec::run`; everything else → `FsOps` unchanged. A policy refusal, denied
approval, non-zero exit, or filesystem error emits `StepFailed` and stops the plan with a
`"failed"` DB status (previously a `?` left the plan stuck at `"executing"`). Each executed
command is journalled via `memory.log_undo` (its first real caller); `handle_undo` reads that
back to report how many commands couldn't be reversed.

## UI architecture (`crates/ui`) — read this before touching the crate

This crate previously failed `cargo check` entirely: the Slint-generated component was named
`AppWindow`, colliding with a hand-written Rust wrapper struct also named `AppWindow`; the wrapper
referenced a nonexistent `crate::ui::AppWindow` path; `ViewModel` was imported but never defined
(only `ViewModelHandle` existed); and the `.slint` file declared no properties or callbacks at all,
so every `window.on_*`/`window.set_*` call in the Rust code was operating on methods that didn't
exist. All of that has been fixed; the current structure is:

- `crates/ui/ui/app-window.slint` defines the actual UI (`export component MainWindow`, sessions
  sidebar, chat list, plan-step list, prompt input) and explicitly declares every property
  (`is-processing`, `ollama-connected`, `ollama-model`, `current-plan-id`, `current-plan-status`,
  `current-undo-id` — the fs undo-log session id, unrelated to a conversation session despite the
  similar name — `show-plan`, `messages: [ChatMessageData]` (each carrying
  `blocks: [MessageBlockData]`, see below), `plan-steps: [PlanStepData]`,
  `active-session-id`, `sessions: [SessionRowData]`, the `approval-*` set, and the `diagnosis-*`
  set — `diagnosis-visible`/`diagnosing`/`diagnosis-ok`/`diagnosis-needs-signin`/`diagnosis-title`/
  `diagnosis-detail`, driving the Ollama-connectivity modal) and callback (`send-message`,
  `execute-plan`, `undo`, `reject-plan`, `open-plan`, `close-plan`, `new-session`,
  `select-session`, `search-sessions`, `select-model`, `toggle-theme`, `ping`, `set-endpoint`,
  `diagnose-ollama`, `dismiss-diagnosis`, the `approve-*`/`*-approval-mode` command-approval set)
  that the Rust side drives. It also
  holds the UI's design system: a `global Theme` (four font-size roles, a 4px spacing ramp, a
  `read-measure` reading-width cap, a `body-font` token, and the structural constants two columns
  must agree on — `titlebar-h`, `sidebar-w`, `plan-w`, `bottom-bar-h`) plus small shared
  components. Two type families: chrome uses the macOS system font; the reading surface
  (`CopiableText`, the reply `StyledText`, the composer input) uses **Open Sans**, bundled as
  `crates/ui/ui/fonts/OpenSans-*.ttf` (SIL OFL, `OFL.txt` alongside) and auto-registered by
  `slint_build` from the `import "fonts/…ttf"` lines at the top of `app-window.slint` — no Rust.
  It exists for one measured reason: `StyledText` has no line-height knob (not on 1.17.1, not on
  the 1.18 beta), and Open Sans' taller vertical metrics are the only way to loosen prose leading.
  See `documentation/lito/design.md`, "Police de corps". Use them rather than fresh literals: `Label`
  instead of a bare `Text` (a `Text`'s box is exactly its font-size, so uppercase accents — É, À —
  are silently clipped off), `HDivider`/`VDivider` instead of a `border-width` on a container
  (a container border draws on all four sides and doubles up against the neighbouring section's
  divider), `OpIcon` + `crates/ui/ui/icons/*.svg` for operation icons, and
  `Theme.optical-center-y(parent-height, self-height)` on a `Label` instance (not on `Label`'s own
  definition — `parent` isn't resolvable at a component root) for a short, descender-less label
  centered in a box taller than its own text, which otherwise reads as visibly too high even
  though its box is geometrically centered (`vertical-alignment: center` centers the font's line
  metrics, not the actual ink). Each of those exists because of a rendered, measured bug — see
  `documentation/lito/design.md`.
  The chat column is a plain `ScrollView` over a `VerticalLayout`, **not** a `ListView`. `ListView`
  virtualizes, which buys nothing at this list's size (the chat holds at most
  `MAX_HISTORY_MESSAGES` rows) and costs two things that were visible: its viewport height is
  extrapolated from a running average of the instantiated rows' heights, so with chat rows ranging
  from one line to forty the scrollbar kept resizing and the position drifted; and its `viewport-y`
  is owned by the runtime layout (the compiler's flickable pass explicitly skips binding it for
  `ListView`), so nothing on the Slint side can drive it. The sidebar and plan-step lists are still
  `ListView`s. Keeping the newest reply in view is `chat-scroll`'s `changed viewport-height`
  handler, gated on the private `stick-to-bottom` flag: armed by sending a message or opening a
  session, dropped once the user scrolls more than `Theme.stick-slack` above the bottom, re-armed
  when they scroll back down. That handler is the only place `viewport-y` is written.
- `crates/ui/build.rs` compiles it via `slint_build::compile`.
- `crates/ui/src/lib.rs` wraps `slint::include_modules!()` in a private `mod generated { ... }` and
  re-exports `generated::MainWindow` under that name — **not** `AppWindow` — specifically so it
  can't collide with the hand-written `AppWindow` wrapper in `app_window.rs`. If you rename the
  Slint component, keep this indirection; don't let the generated struct and the wrapper struct
  share a name again.
- `crates/ui/src/app_window.rs` — `AppWindow` wraps a `MainWindow` instance, wires each Slint
  callback to a plain (non-async) call on a cloned `ViewModelHandle`, and exposes `run()`/`as_weak()`
  by delegating to the Slint `ComponentHandle` trait (`use slint::ComponentHandle;` is required for
  `.run()`/`.as_weak()` to resolve — they're trait methods, not inherent ones).
- `crates/ui/src/markdown.rs` — splits an assistant reply's markdown into top-level blocks
  (`split_blocks`) and strips it to plain text for the "Copier" action (`plain_text`), using
  `pulldown-cmark` (already in the dependency graph via `i-slint-common`). It exists because
  `StyledText::from_markdown` returns `Err` for the *whole* string as soon as one unsupported
  construct appears — one fenced code block or `##` used to drop a whole reply back to raw
  markdown — and because `StyledText` has no text selection at all, while the blocks rendered
  through `CopiableText` (code, tables, headings) do. Classification asks
  `StyledText::from_markdown` per block rather than hardcoding the supported subset, so it stays
  correct if Slint's subset grows. A bullet/numbered list is additionally split into one block
  per top-level item (`list_item: true`, marker kept), because a whole list is a single
  `StyledText` that stacks its item paragraphs with zero leading and no line-height knob — the
  per-item blocks let `ChatMessageRow` put a real gap between bullets. See
  `documentation/lito/design.md`, "Rendu markdown".
- `crates/ui/src/highlight.rs` — hand-rolled, no-dependency syntax highlighting for fenced code
  blocks and `RUN_COMMAND` lines: `highlight_code`/`highlight_shell_command`/
  `classify_shell_word` classify each token into a `TokenClass` (`Plain`/`Keyword`/`String`/
  `Comment`/`Number`/`Function`). It exists because `TextInput`/`Text` (what `CopiableText` wraps)
  have one uniform `color` and `StyledText::from_markdown`'s `<span style="color:...">` support —
  documented upstream — is verified *not* wired up on the pinned 1.17.1 (`StyledTextFromMarkdownError:
  "HTML tag <span> is not supported"`), so a colored block is a row of small single-line `CodeSpan`
  (`TextInput`) instances (`app-window.slint`) instead of one text element per line — trading
  continuous click-drag selection across a whole line for per-token color (each token stays
  independently selectable; the block's "Copier" still copies everything). See
  `documentation/lito/design.md`, "Coloration syntaxique", including a `ListView`-nested-`ScrollView`
  sizing bug hit and worked around there (plan panel's command line).
- `crates/ui/src/view_model.rs` — `ViewModel` owns the core agent's `JoinHandle` and hands out
  cloneable `ViewModelHandle`s; `ViewModelHandle` holds `UiState` behind a `Mutex`, a
  `Weak<MainWindow>` for pushing updates to the UI, and the outgoing `mpsc::UnboundedSender`.
  `set_channels()` spawns a `tokio::spawn`'d pump loop that reads `CoreToUiMsg` off the incoming
  `mpsc::UnboundedReceiver` for the lifetime of the app and calls `apply_core_message` — nothing
  else drives that receiver, so don't add a second consumer of it.

**Applying state and pushing it to the UI are deliberately separate; keep them that way.**
`apply_core_message` only mutates `UiState`. The pump loop then drains everything already queued
into the same batch and calls `sync_to_ui()` **once**, at most every `MIN_SYNC_INTERVAL` (33ms,
~30 Hz); the first message of a burst still syncs immediately. Public methods called from Slint
callbacks (`send_message` etc.) use `update_ui_state`, which mutates *and* syncs, because they're
already on the UI thread and want their effect visible at once. Don't make `apply_core_message`
sync directly — a local model emits tokens several times faster than 30 Hz, and each sync costs a
thread hop plus a layout pass over the chat.

`sync_to_ui` never reassigns a model it doesn't have to. `messages` is diffed row by row against
`message_fingerprints` and mutated in place through the `VecModel` it installed on the first sync
(tracked by `messages_model_installed`); `plan_steps` and `sessions` are replaced wholesale but only
when their fingerprint changed. This is not micro-optimization: a Slint repeater compares the
incoming `ModelRc` **by pointer** and, on any new instance, throws away every instantiated row and
its `RepeaterLayoutState` — scroll anchor, item-height average, previous viewport position (see
`Repeater::model` in `i-slint-core`). Reassigning on every streamed token therefore reset the chat's
scroll anchoring tens of times a second and destroyed the `TextInput` instances underneath, taking
any in-progress text selection with them. Measured after the change: 121 tokens at ~125/s produced
29 UI syncs, each touching exactly 1 row and reparsing 1 message's markdown instead of the whole
conversation's.

A Slint model is not `Send`, so a `ChatMessageData` — which now holds `blocks` as a `ModelRc` —
can't cross into `upgrade_in_event_loop`'s closure. `sync_to_ui` therefore builds each changed row
as a `ChatRow` (the same fields, blocks as a plain `Vec`) off the UI thread, markdown parsing
included, and calls `ChatRow::into_data` — the only part that creates the model — inside the
closure. Keep any new model-valued row field on that side of the line. The same split now goes one
level deeper for a `ChatRow`'s own blocks (`BlockRow`, plain `Vec<Vec<highlight::Span>>` for a code
block's highlighted lines instead of `MessageBlockData`'s `ModelRc<CodeLineData>`) and for
`PlanStepData` (`PlanStepRow`, plain `Vec<highlight::Span>` for `command-line`) — any struct that
ends up inside a `Vec` built off the UI thread needs the same treatment the moment it grows an
array-typed field, since the array becomes a `ModelRc` (`!Send`) regardless of nesting depth.

**Threading model, read carefully before editing `sync_to_ui`/`upgrade_in_event_loop`:** UI callback
handlers (`on_send_message` etc.) already run on the Slint UI thread, so `ViewModelHandle::send_message`
et al. are plain synchronous functions — no `slint::spawn_local` needed. The *other* direction is not
on the UI thread: the `core_to_ui_rx` pump runs on a `tokio` worker thread, so `sync_to_ui()` cannot
call `.upgrade()` on the stored `Weak<MainWindow>` directly (Slint enforces that a component only
upgrades on the thread that created it) — it uses `Weak::upgrade_in_event_loop(closure)` instead,
which marshals the closure onto the Slint event loop thread. If you add a new place that pushes state
into the UI from a spawned task, go through `sync_to_ui`/`upgrade_in_event_loop` rather than storing
and upgrading a `Weak` yourself.

**Visual/layout changes: render the UI and look at it. Don't reason from the code alone.**
`.slint` code that reads correctly and renders wrong is the normal failure mode here, not an edge
case — this repo's history (`documentation/lito/llm_brief/history.md`, entries 22/22bis/22ter) is
mostly made of it: a `Text` top-aligned instead of centered, a std-widget's hardcoded internal sizing
fighting the surrounding layout, two elements that had to share a boundary drifting apart. Load
**both** the official `slint` skill (language, layout, gotchas, interop, MCP) and this repo's
`slint-lito` skill before touching a `.slint` file — `slint-lito` holds the `make ui-shot` render
loop and the dozen repo-specific layout bugs already hit and fixed here.

```bash
make ui-check    # slint-viewer --check on app-window.slint (~0.05s, warnings included)
make ui-shot     # render target/ui-shots/{demo-light,demo-dark,stress}.png — then Read them
```

`slint-viewer --screenshot` renders the `.slint` file headlessly, with no app, no Ollama and no
SQLite; `--load-data` feeds it the fixtures in `crates/ui/ui/fixtures/` so the lists aren't empty.
**This means the rendered pixels are directly visible to you** — an earlier version of this file
claimed the opposite and that claim was wrong. Render before asserting a visual fix works, and
render `stress.json` too: it's the fixture with the long paths and overflowing titles, and it's what
revealed that 4 of the 7 plan-step operation glyphs didn't render at all in the system font (since
fixed with real SVG icons — see entry 24 of `history.md`).

For claims about a specific color or a 1px alignment, sample the pixels rather than eyeballing the
downscaled PNG: `python3 crates/ui/ui/pxsample.py target/ui-shots/demo-light.png a=100,38 b=100,39`.

Two gotchas previously documented here were tested against Slint 1.17.1 and turned out to be false
(`border-width` insetting children; a nested layout sizing to its content rather than its parent) —
see the "Two false mechanisms — do not reinstate" note at the end of the `slint-lito` skill.

## Commands

```bash
make check       # cargo check --workspace
make build        # cargo build --release
make run          # cargo run -p lito-app
make run-debug    # RUST_LOG=info,lito=debug cargo run -p lito-app
make fmt          # cargo fmt --all
make clippy       # cargo clippy --workspace -- -D warnings
make test         # cargo test --workspace
make docs         # cargo doc --workspace --no-deps --open
make package      # bundle target/release/lito-app into a .app (macOS, ad-hoc codesign)
make dmg          # package + hdiutil dmg

make install-viewer  # cargo install slint-viewer (pinned ^1.17, matching Cargo.lock)
make ui-check        # type-check crates/ui/ui/app-window.slint alone (fast)
make ui-shot         # render the UI to PNGs using crates/ui/ui/fixtures/ data
make ui-watch        # live-reloading preview window (interactive, for the user)
```

Note the Slint version skew: `Cargo.toml` says `slint = "1.6"` but `Cargo.lock` resolves to
**1.17.1** — that's what actually builds, and what the `ui-*` targets are pinned against.

Running a single test: `cargo test -p <crate-name> <test_name>` (e.g.
`cargo test -p lito-memory test_memory_store`). Note `crates/llm`'s `test_ollama_client` requires a
live Ollama server on `localhost:11434` to pass — it's not mocked.

## Suggesting test prompts

Whenever you implement a feature that needs a natural-language prompt to exercise it (e.g. anything
touching `agent::plan`, plan execution, or UI flows driven by user chat input), always suggest one or
two example test prompts the user can type into the app to try it out — don't make the user think of
one themselves.

## Conventions worth knowing before editing

- Everything in `crates/protocol` uses `strum`/Serde casing inherited from the old wire protocol
  (`SCREAMING_SNAKE_CASE` for `Operation`, `snake_case` + externally-tagged for
  `ClientMessage`/`ServerMessage`). Match it if you ever do wire these types back up to something,
  but don't assume they're load-bearing today — see "Architecture" above.
- `documentation/lito/` holds this project's own authored docs — `llm_brief/summary.md` and
  `history.md`, `learn/*.md`, `design.md`, and `roadmap.md` (the path to a production-ready release:
  numbered items P0.1…P5.5, each with acceptance criteria and a ready-to-paste build prompt; it also
  records the verified gaps in the current code, so update it as items land) — as opposed to
  `documentation/slint-docs/`, which is
  vendored third-party reference material (see below). The `lito/` files are meant to bootstrap
  another LLM's or developer's context. They've been rewritten to match the current
  single-binary-Rust-+-Slint state — if you make an architectural change, update them in the same
  change, since nothing regenerates them automatically.
- `documentation/lito/design.md` is the UI design-system reference (palette tokens, the "accent is
  CTA-only" color rule, typography/layout decisions, and — just as important — what was deliberately
  *not* done and why, e.g. no dark theme, no session-history sidebar). Read it before making visual
  changes to `crates/ui`, and update it in the same change if a design decision changes so it doesn't
  silently drift from what `app-window.slint` actually does.
- Always think about whether `documentation/lito/learn/*.md`, `documentation/lito/design.md`, and
  `documentation/lito/llm_brief/summary.md`/`history.md` need updating as part of whatever change
  you're making, not just for architectural changes — new crates, renamed modules, changed
  commands/conventions, etc. can all make them stale too.
- `documentation/slint-docs/` is a vendored copy of the official Slint documentation, unrelated to
  this project's own docs — treat it as reference material, not something to edit.
- `crates/ui/ui/fixtures/*.json` mirror `MainWindow`'s properties and the `ChatMessageData` /
  `MessageBlockData` / `PlanStepData` / `SessionRowData` structs, and nothing keeps them in sync
  automatically. If you add or rename a field in `app-window.slint`, update the fixtures in the
  same change — otherwise `make ui-shot` starts printing `Warning: ignoring unknown property: …`
  and silently renders a state that no longer matches the app. Two fixture rules exist because the
  interpreter's JSON loader crashes rather than defaulting: every message needs `"blocks": []`
  spelled out (a missing field is `Value::Void`, and the interpreter's operators don't
  short-circuit — `msg.blocks[i - 1].list-item` in `ChatMessageRow` does `Void & Void` and panics
  `slint-viewer` if any block omits `"list-item"`), and **no fixture block may use `kind:
  "styled"`** — the loader has no case for a `styled-text` field, so instantiating a `StyledText`
  on it panics `slint-viewer` outright (`binding was of the wrong type: ()`, verified). The other
  kinds render through `CopiableText` and load fine. A bullet/numbered list is split into one
  block per top-level item on the Rust side (`markdown::split_blocks`), each carrying `"list-item":
  true` and its own `- `/`N. ` marker — mirror that in fixtures (one block per bullet) rather than
  a single multi-line `text` block. Same crash-rather-than-default rule for the two fields
  `highlight.rs` added: every block needs `"lines": []` (real span data for a `"code"` block, empty
  otherwise) and every plan step needs `"command-line": []` (real spans for a `RUN_COMMAND` step
  with a `command`) — see `documentation/lito/design.md`, "Coloration syntaxique".
