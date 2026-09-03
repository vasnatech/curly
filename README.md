# Curly

A lightweight REST client, written in Rust, with two faces sharing one core:

- **`curly` (CLI)** — a curl-like command-line tool for sending requests and managing saved collections/environments from the terminal.
- **Curly (GUI)** — a Postman-like desktop app for building, organizing, and inspecting HTTP requests visually.

Both ship as native, single-binary executables for Linux, macOS, and Windows.

## Status

Pre-implementation. See [docs/REQUIREMENTS.md](docs/REQUIREMENTS.md) and [docs/DESIGN.md](docs/DESIGN.md).

## Layout (planned)

```
curly/
├── crates/
│   ├── curly-core/   # shared HTTP client, data model, storage
│   ├── curly-cli/    # CLI binary
│   └── curly-gui/    # GUI binary
├── docs/
└── Cargo.toml         # workspace root
```
