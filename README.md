# Curly

A lightweight REST client, written in Rust, with two faces sharing one core:

- **`curly` (CLI)** — a curl-like command-line tool for sending requests and managing saved collections/environments from the terminal. Ships as a native, single-binary executable for Linux, macOS, and Windows.
- **Curly (GUI)** — a Postman-like desktop app for building, organizing, and inspecting HTTP requests visually. Now a separate **Tauri + React + TypeScript** app (`curly-gui-tauri/`), not bundled into the `curly` binary — see "Using the GUI" below.

## Status

M1 (core + CLI one-shot mode), M2 (local storage — saved requests, collections, environments, `{{variable}}` substitution, history), and M3 (import from Postman Collection v2.1/Environment/Globals and curl command strings) are implemented, plus several post-M2 additions (folders/sub-folders, project-local storage via `curly init`, post-response variable extraction + sessions). **GUI (M4 + M8's redesign, now M9's rewrite) reaches the full designed feature set** — open/switch projects, browse collections with per-row rename/duplicate/delete/new-folder/new-request/save-all, environments with create/edit/delete, multiple request tabs with dirty tracking, ad-hoc variable overrides, an extraction-rule editor, dedicated Collection/Folder/Environment tabs, and a response pane with JSON syntax highlighting, search, and per-request history; see [docs/GUI.md](docs/GUI.md) for the full user guide and what's still not there (a packaged build, an automated frontend test suite). See [docs/REQUIREMENTS.md](docs/REQUIREMENTS.md) and [docs/DESIGN.md](docs/DESIGN.md) for scope and shipped-milestone status (§9's M9 entry has the GUI rewrite's own story), and [docs/ROADMAP.md](docs/ROADMAP.md) for not-yet-committed feature ideas.

## Using the CLI

See [docs/CLI.md](docs/CLI.md) for the full user guide (install/build steps, every flag explained, examples).

```sh
cd rust && cargo build --release
./target/release/curly https://httpbin.org/get
```

## Using the GUI

See [docs/GUI.md](docs/GUI.md) for the full guide, including Linux build prerequisites. In short:

```sh
cd curly-gui-tauri
npm install        # first time only
npm run tauri dev
```

## Layout

```
curly/
├── rust/              # the Rust implementation — the reference while the port runs
│   ├── Cargo.toml     #   workspace root
│   └── crates/
│       ├── curly-core/   # lib: HTTP execution, data model, storage, import, substitution
│       └── curly-cli/    # bin "curly": the clap-based CLI
├── ziman/             # the Ziman implementation (in progress — see docs/ZIMAN.md)
│   └── src/{main,test}/ziman/
├── curly-gui-tauri/   # the GUI: Tauri (Rust shell) + React/TypeScript, its own Cargo workspace
│   ├── src-tauri/     #   Rust shell — Tauri commands wrapping curly-core
│   └── src/           #   React frontend
├── docs/
├── run-all.sh         # every check, both implementations, plus the cross-check between them
└── parity.sh          # the cross-check on its own
```

**One component is being reimplemented, not two.** The Rust implementation is the reference the
Ziman one is measured against — `parity.sh` asks both the same questions over the same `.curly/`
tree and diffs the answers, which is what makes "replaces" a claim a test can fail on. The React
frontend is shared and is not being rewritten.
