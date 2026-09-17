---
name: slint-lito
description: Lito-repo companion to the official `slint` skill. Load BOTH whenever touching crates/ui, a .slint file, or the Rust↔Slint boundary (app_window.rs / view_model.rs). The official `slint` skill covers the language, layout, gotchas, interop and MCP server; THIS file is only what's specific to this repo — the `make ui-shot` render loop and a dozen layout bugs already hit and fixed here against Slint 1.17.1.
---

# Slint in the Lito repo

Read the official **`slint`** skill first — it covers the language, layout,
compile gotchas, interop and the MCP server. This file adds only what that skill
can't know: the repo's render workflow, its conventions that deviate from the
official advice, and the specific layout bugs its history is made of
(`documentation/lito/llm_brief/history.md`, entries 22–24). Every gotcha below
passed code review and rendered visibly wrong.

## Render loop (repo-specific)

```bash
make ui-shot   # runs ui-check, then writes target/ui-shots/{demo-light,demo-dark,stress}.png
```

- Read all three PNGs after any visual/layout change. Never write "this is now
  aligned" without having looked at a post-change PNG; if you can't render, say
  the claim is unverified.
- Fixtures live in `crates/ui/ui/fixtures/`. Always render `stress.json` too —
  long unbroken paths, overflowing titles and many rows break layouts the happy
  path doesn't (it's what exposed 4 invisible operation glyphs).
- Color or 1px claim → sample pixels, the PNG you see is downscaled:
  `python3 crates/ui/ui/pxsample.py target/ui-shots/demo-dark.png a=110,400 b=500,400`
- Computed geometry ("inspect element") → `debug()` in an `init` handler (prints
  to stderr even under `--screenshot`; divide lengths by `1px` to print them):
  `debug("row", row.width/1px, row.height/1px, row.absolute-position.x/1px);`
- `make ui-watch` = live-reloading preview window for the user.
- Iterate on one component via a throwaway `.slint` in the scratchpad rather than
  hunting it inside the whole app.

## Repo conventions that override the official skill's advice

- **`std-widgets` is dropped on purpose** — only `ListView` is kept. Every other
  primitive is hand-rolled and reused once (`Label`, `CopiableText`,
  `HDivider`/`VDivider`, `OpIcon`). Don't reach for `Button` / `LineEdit` /
  `ComboBox` / `VerticalBox`, and ignore the official skill's "prefer std-widgets"
  and `Palette`-based guidance. See `documentation/lito/design.md`.
- **Sizes in `px`, not `rem`.** Read the `global Theme` tokens (four font roles, a
  4px spacing ramp) instead of literals — sub-pixel paddings like `0.4px` are
  drift, not design.
- Rust boundary: the Slint-generated component is `MainWindow`, re-exported under
  that name so it can't collide with the hand-written `AppWindow` wrapper.
  Background task → UI must go through `Weak::upgrade_in_event_loop` — read
  CLAUDE.md "UI architecture" and follow `view_model.rs::sync_to_ui` before
  editing that seam.
- Vendored upstream docs: `documentation/slint-docs/` (reference only, don't
  edit). Version skew: `Cargo.toml` pins `slint = "1.6"` but `Cargo.lock`
  resolves **1.17.1** — that's what compiles and what every gotcha here was
  verified against.

## Layout bugs already hit here (verified on Slint 1.17.1 with slint-viewer + debug())

1. **`Text` top-aligns the moment its box is taller than the text**
   (`vertical-alignment` defaults to `top`). Free child of a `Rectangle`: box =
   content height, positioning rule centers it, fine. Child of a **layout**, or
   given an explicit `height`: box stretched to full height, glyphs at its top →
   add `vertical-alignment: center`.
2. **A non-growing sibling of a `vertical-stretch: 1` element can compete for the
   leftover space** instead of sizing to content. Set `vertical-stretch: 0` (resp.
   `horizontal-stretch: 0`) explicitly on every non-growing sibling.
3. **`std-widgets` carry hardcoded internal sizing** that fights a dense layout —
   `ComboBox` forces `min-width: max(160px, …)` + `horizontal-stretch: 1`
   regardless of content. Check the vendored def
   (`~/.cargo/registry/src/*/i-slint-compiler-*/widgets/cupertino/*.slint`) before
   instantiating one, or replace it with a small custom component (what happened
   to the model selector).
4. **Don't reassign a Rust-driven `ModelRc`/`VecModel` on unrelated state ticks**
   (e.g. rebuilding it in `sync_to_ui` per streamed token). Some widgets reset
   internal derived state on *any* new model instance — `ComboBoxBase`'s
   `changed model => reset-current()` intermittently blanked the selector. Cache
   what was last pushed; touch the property only when the data changed.
5. **Two elements that must share a boundary must derive their size from the same
   value**, not independently-computed content sizes that look close —
   font-metric line-heights drift at render time and a near-alignment reads as a
   bug. Use one explicit constant, or compute one side from the other's known
   total. `debug()` both `absolute-position`s to check.
6. **A missing glyph renders as nothing** — no tofu box, no warning. 4 of 7
   operation glyphs (`▢ ⧉ ✎ ▤`) were invisible in the macOS system font. Fixed
   with real `.svg` assets in `crates/ui/ui/icons/` behind `OpIcon`, tinted via
   `Image { colorize: ... }`. `resvg` is already in the dep graph
   (`cargo tree -p lito-ui -e features -i resvg`). Render any glyph before trusting it.
7. **A filling child with a background hides its parent's border.** `border-width`
   reserves no layout space and paints inside the parent's bounds; the child gets
   the full box and paints after. Fix: inset the child (layout + `padding` = border
   width), drop the child's background, or draw the divider as a separate 1px
   `Rectangle` sibling.
8. **A `Text` box is exactly `font-size` tall; anything above cap height clips** —
   every uppercase accent in French chrome ("PLAN PROPOSÉ" → "PLAN PROPOSE",
   "Échec" → "Echec"), silent, identical under femtovg and skia. Fix:
   `min-height: self.font-size * 1.45` + `vertical-alignment: center` (Slint rejects
   `height` + `min-height` together; `min-height` is the one that also works for
   layout children). Wrapped once as `Label` / `CopiableText` — use those.
9. **A `HorizontalLayout`'s cross axis ignores what you expect**: a child's
   `height` is honoured but the child is **top-aligned** (`y=0`), and `max-height`
   is **ignored outright**. Center a fixed-size child on the cross axis by wrapping
   it in `VerticalLayout { alignment: center }`.
10. **A layout with no `alignment` hands leftover space to its children**, and a
    `Text` child then centers its own text in the oversized cell — a flush-left
    row renders indented and looks centered. Set `alignment: start` on any layout
    whose children shouldn't grow.
11. **`vertical-alignment: center` centers the font's line box, not the ink.** A
    short descender-less string (most French UI chrome — no g/j/p/q/y) renders
    visibly high (~0.6–1.2px) in a box that is itself perfectly centered —
    optical-vs-metric centering, not a Slint defect. Fix with a deliberate integer
    nudge at the call site: `y: (parent.height - self.height) / 2 + 1px`. Confirm
    with a luminosity-weighted ink centroid, excluding border-colored pixels.
12. **A fixed container `height` + `padding` on its inner layout can squeeze
    content below its own `min-height`.** A layout stretches children to its
    externally-fixed cross size regardless of `min-height`; padding eats the rest,
    so `padding-top + padding-bottom + child min-height > container height`
    compresses the child *past* its `min-height`. Fix: wrap the content row in
    `VerticalLayout { alignment: center }` so it takes its natural height and is
    centered as a block.
13. **`parent` is not resolvable at a component's root** —
    `component Divider inherits Rectangle { width: parent.width; }` won't compile.
    Pin only the component's own axis (`height: 1px`) and let the enclosing layout
    stretch the other; put any `parent`-dependent binding at the *call site*.

**Two false mechanisms — do not reinstate** (both tested false on 1.17.1):
`border-width` does *not* shift where children start or reserve any layout space
(the real bug is #7 — the border being *covered*). A layout nested in a non-layout
element does *not* size to its content — layouts fill their parent, so if
`alignment: center` has no room to distribute, some *ancestor* is content-sized;
walk up with `debug()` on each level's `width` and fix the narrow one. Lesson: a
plausible mechanism invented to explain a real symptom is still a fabrication, and
it outlives the bug. Measure the mechanism before writing it down.
