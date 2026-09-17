# Lito v2 - Rust + Slint Edition

Lito is a local-first, privacy-first macOS file automation app with natural language interface, powered by Rust and Slint UI.

## Architecture

- **Single binary**: No IPC, no separate processes
- **Rust + Slint**: Native performance with modern UI
- **Modular design**: Agent, FS, LLM, and Memory crates
- **Local-first**: All processing happens on-device
- **Privacy-first**: No data leaves your machine

## Getting Started

### Prerequisites

- Rust 1.75+ (install via [rustup](https://rustup.rs/))
- macOS 12.0+ (Monterey or later)
- Ollama running locally (for LLM capabilities)

### Build & Run

```bash
# Build and run
make run

# Or with debug logging
make run-debug
```

### Development

```bash
# Check code
make check

# Format code
make fmt

# Run tests
make test

# Build documentation
make docs
```

## Project Structure

```
lito/
├── Cargo.toml          # Workspace configuration
├── crates/
│   ├── app/            # Main application binary
│   ├── ui/             # Slint UI components
│   ├── protocol/       # Shared types (no serialization needed)
│   ├── agent/          # Planning and orchestration
│   ├── fs/             # Filesystem operations
│   ├── llm/            # Ollama client
│   └── memory/         # SQLite persistence
├── Makefile           # Build automation
├── Info.plist         # macOS app metadata
├── assets/             # App icons and resources
└── documentation/      # Design docs + vendored Slint reference docs
```

## Key Features

- **Natural language file automation**: Describe what you want, Lito makes it happen
- **Undo support**: Every action is reversible
- **Local LLM**: Uses Ollama for privacy-preserving AI
- **Real-time feedback**: See what's happening as it happens
- **Minimalist UI**: Designed for focus and productivity

## License

MIT License