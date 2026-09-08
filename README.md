# Curly

A lightweight REST client, written in Rust, with two faces sharing one core:

- **`curly` (CLI)** — a curl-like command-line tool for sending requests and managing saved collections/environments from the terminal.
- **Curly (GUI)** — a Postman-like desktop app for building, organizing, and inspecting HTTP requests visually.

Both ship as native, single-binary executables for Linux, macOS, and Windows.

## Status

M1 (core + CLI one-shot mode), M2 (local storage — saved requests, collections, environments, `{{variable}}` substitution, history), and M3 (import from Postman Collection v2.1/Environment/Globals and curl command strings) are implemented, plus several post-M2 additions (folders/sub-folders, project-local storage via `curly init`, post-response variable extraction + sessions). **M4 (GUI) is in progress** — it can open a project, browse collections, load a saved request, switch environments to resolve `{{variable}}` tokens, build a request from scratch, send it, and show the response; see [docs/GUI.md](docs/GUI.md) for exactly what's built vs. not yet. See [docs/REQUIREMENTS.md](docs/REQUIREMENTS.md) and [docs/DESIGN.md](docs/DESIGN.md) for scope and shipped-milestone status, and [docs/ROADMAP.md](docs/ROADMAP.md) for not-yet-committed feature ideas.

## Using the CLI

See [docs/CLI.md](docs/CLI.md) for the full user guide (install/build steps, every flag explained, examples).

```sh
cargo build --release
./target/release/curly https://httpbin.org/get
```

## Using the GUI

See [docs/GUI.md](docs/GUI.md). Launch it with `curly` (no arguments) or `curly gui`.

## Layout

```
curly/
├── crates/
│   ├── curly-core/   # shared HTTP client, data model, storage
│   ├── curly-cli/    # CLI binary ("curly"), also launches the GUI
│   └── curly-gui/    # egui/eframe GUI (lib, linked into the "curly" binary — M4, in progress)
├── docs/
└── Cargo.toml         # workspace root
```
