# Lito v2 Makefile - Rust + Slint build

.PHONY: build run clean check fmt clippy ui-check ui-shot ui-watch install-viewer

SLINT_UI  := crates/ui/ui/app-window.slint
FIXTURES  := crates/ui/ui/fixtures
SHOTS     := target/ui-shots

# Default target
all: build

# Build the project
build:
	cargo build --release

# Build in debug mode
debug:
	cargo build

# Run the application
run:
	cargo run -p lito-app

# Run with RUST_LOG for debugging
run-debug:
	RUST_LOG=info,lito=debug cargo run -p lito-app

# Clean build artifacts
clean:
	cargo clean
	rm -rf target

# Check code for errors
check:
	cargo check --workspace

# Format code
fmt:
	cargo fmt --all

# Run clippy linter
clippy:
	cargo clippy --workspace -- -D warnings

# Build documentation
docs:
	cargo doc --workspace --no-deps --open

# Run tests
test:
	cargo test --workspace

# Package for macOS (creates .app bundle)
package:
	mkdir -p target/release/Lito.app/Contents/MacOS
	mkdir -p target/release/Lito.app/Contents/Resources
	cp target/release/lito-app target/release/Lito.app/Contents/MacOS/Lito
	cp -r assets/ target/release/Lito.app/Contents/Resources/
	cp Info.plist target/release/Lito.app/Contents/
	codesign --deep --force --sign - target/release/Lito.app

# Create a DMG for distribution
dmg: package
	hdiutil create -volname "Lito" -srcfolder target/release/Lito.app -ov -format UDZO target/release/Lito.dmg

# Install cargo bundle for packaging
install-bundle:
	cargo install cargo-bundle

# Generate Slint bindings
slint-gen:
	cd crates/ui && cargo build

# --- UI rendering loop (no app, no Ollama - see .claude/skills/slint/SKILL.md) ---

# Install the previewer. Pin to the same minor as the `slint` crate in Cargo.lock
# so the .slint dialect the viewer compiles matches what the app compiles.
install-viewer:
	cargo install slint-viewer --version "^1.17" --locked

# Type-check the .slint file on its own (~0.05s). Prints warnings too, e.g.
# "padding only has effect on layout elements". Exit 1 on error.
ui-check:
	slint-viewer --check $(SLINT_UI)

# Render the UI to PNG with realistic data, without launching the app.
# ALWAYS look at the output before claiming a visual change works.
ui-shot: ui-check
	@mkdir -p $(SHOTS)
	@python3 -c "import json,sys; d=json.load(open('$(FIXTURES)/demo.json')); d['Palette']={'dark':True}; json.dump(d,open('$(SHOTS)/.demo-dark.json','w'))"
	slint-viewer --load-data $(FIXTURES)/demo.json   --screenshot $(SHOTS)/demo-light.png  $(SLINT_UI)
	slint-viewer --load-data $(SHOTS)/.demo-dark.json --screenshot $(SHOTS)/demo-dark.png   $(SLINT_UI)
	slint-viewer --load-data $(FIXTURES)/stress.json  --screenshot $(SHOTS)/stress.png      $(SLINT_UI)
	slint-viewer --load-data $(FIXTURES)/streaming.json --screenshot $(SHOTS)/streaming.png  $(SLINT_UI)
	slint-viewer --load-data $(FIXTURES)/approval.json --screenshot $(SHOTS)/approval.png   $(SLINT_UI)
	slint-viewer --load-data $(FIXTURES)/diagnosis.json --screenshot $(SHOTS)/diagnosis.png $(SLINT_UI)
	@echo "Wrote $(SHOTS)/{demo-light,demo-dark,stress,streaming,approval,diagnosis}.png"

# Live preview that reloads on save. Interactive - for the user, not for CI.
ui-watch:
	slint-viewer --auto-reload --load-data $(FIXTURES)/demo.json $(SLINT_UI)

# Help message
help:
	@echo "Lito v2 Makefile - Available targets:"
	@echo "  build       - Build release binary"
	@echo "  debug       - Build debug binary"
	@echo "  run         - Run the application"
	@echo "  run-debug   - Run with debug logging"
	@echo "  clean       - Clean build artifacts"
	@echo "  check       - Check code for errors"
	@echo "  fmt         - Format code"
	@echo "  clippy      - Run linter"
	@echo "  docs        - Build documentation"
	@echo "  test        - Run tests"
	@echo "  ui-check    - Type-check the .slint file alone (fast)"
	@echo "  ui-shot     - Render the UI to PNGs with fixture data (look at them!)"
	@echo "  ui-watch    - Live-reloading preview of the .slint file"
	@echo "  install-viewer - Install slint-viewer (needed by the ui-* targets)"
	@echo "  package     - Create macOS .app bundle"
	@echo "  dmg         - Create DMG for distribution"
	@echo "  slint-gen   - Generate Slint bindings"
	@echo "  help        - Show this help message"