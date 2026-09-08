# Curly GUI — User Guide

> Status: **early, two slices so far** (M4, just started — see [DESIGN.md](DESIGN.md) §6/§9). The GUI can open a project (see its collections/environments listed), build a request, send it, and show the response. It does **not** yet let you click a saved request to load it, open more than one project at once, save/edit requests, or theme — those are still FR-18's rest, plus FR-19..24, not built yet. If you need saved requests actually *usable* today (running them, not just seeing their names), `{{variable}}` substitution, or history, use [the CLI](CLI.md) — GUI and CLI share the same underlying storage, so anything you build with one is already sitting there for the other, waiting on the GUI to grow the rest of the browsing UI.

## Launching it

The GUI is built into the same `curly` binary as the CLI (see DESIGN.md §3) — there's no separate install.

```sh
curly              # no arguments at all → launches the GUI
curly gui          # same thing, explicit subcommand
```

Any other invocation — a URL, a flag, or a subcommand like `env`/`collections`/`run` — behaves exactly as documented in [CLI.md](CLI.md); the GUI only launches when there's nothing else to do.

If you built with `cargo build --no-default-features` (the lean CLI-only build, no `eframe`/`egui`/windowing dependencies), both of the above print a clear error instead of launching anything.

## What's there today

### Projects

A **project** is exactly what the CLI already calls a project-local `.curly` directory (`curly init`) — the GUI didn't invent a new concept, it's the same `Storage` root. On launch, the GUI auto-opens a project the same way the CLI resolves storage by default: an auto-detected `.curly` walking up from wherever the GUI process's working directory happens to be, `CURLY_DATA_DIR`, or the OS-wide default location (labeled **"Default"** in the sidebar) if none of those find anything.

The left sidebar shows:
- The active project's label (the folder name a `.curly` lives in, or "Default") and its full path.
- **"Open Project…"** — a native folder picker. Pick any directory: if it already has a `.curly` in it, that project opens; if not, one is created there on the spot (same as running `curly init` in that folder), then opens. Unlike the CLI's own auto-detection, this does **not** search parent directories — a folder dialog already lets you navigate to exactly the directory you want, so opening `Documents/my-api` won't accidentally reopen some unrelated project three levels up.
- Read-only **Collections** and **Environments** lists for the active project (names only — clicking one doesn't do anything yet, see below).

**Only one project is open at a time right now** — picking a new one replaces the current one, it doesn't add a tab. This is deliberate groundwork, not a missing feature that slipped through: the underlying state is already a list (so a future "open several projects, switch between them" doesn't need a data-model change, just a UI for it), but only single-project interaction is wired up so far.

### Sending a request

- **Method + URL bar** — pick a method from the dropdown, type a URL.
- **Send** — click the button, press Enter in the URL field, or press Ctrl+Enter (Cmd+Enter on macOS) from anywhere in the window.
- **Headers** — an editable table: checkbox to enable/disable a header without deleting it, name/value fields, `+ Add header` to add another row. A header with an empty name is ignored even if checked.
- **Body** — a plain multi-line text box, sent as the raw request body (matching the CLI's `-d`/`--data`; no separate form/multipart/binary body modes in the GUI yet).
- **Response pane** — status (color-coded: green 2xx, yellow 3xx, red 4xx/5xx), elapsed time, body size, a collapsible list of response headers, and the body — pretty-printed automatically if it's JSON, shown as-is otherwise.

Sending is asynchronous — the window stays responsive while a request is in flight ("Sending…" on the button), and you can't fire a second request until the first one finishes. This part is completely independent of which project (if any) is open — typing a URL and sending it works the same regardless.

## What's not there yet

Tracked in DESIGN.md's M4 entry:

- Clicking a collection to see its requests, or a request to load it into the editor (rest of FR-18)
- Multiple projects open simultaneously (the data model already supports it — see "Projects" above — the interaction doesn't yet)
- Multiple request tabs (FR-19)
- Save/rename/duplicate/move a request from the GUI (FR-20) — for now, build/edit saved requests with the CLI (`curly collections add-request ...`); they already show up in the sidebar's Collections list once you do
- An environment editor / `{{variable}}` resolution when sending from the GUI (FR-21) — the environments list is just names for now; the URL/header/body fields send exactly what you type, no `{{var}}` substitution happens yet
- Syntax highlighting, in-body search, save-to-file, or image preview for responses (FR-22) — the body view is plain monospace text
- Ctrl/Cmd+S to save, new-tab shortcuts (FR-23) — Ctrl/Cmd+Enter to send already works
- Light/dark theme following the OS (FR-24) — currently whatever `eframe`'s own default is

## A note on this guide's own testing

Automated screenshotting wasn't available in the environment this was built in (no `xdotool`/`grim`/`gnome-screenshot`; the GNOME Shell and xdg-desktop-portal screenshot D-Bus APIs both need interactive consent that never resolves headlessly) — so the code's author could confirm the process launches cleanly but not see the window directly. The person running it, on their own real desktop, confirmed it twice: first that a live send (GET `https://httpbin.org/get`) round-tripped end to end with the response pretty-printed as expected, then — after the project-support slice landed — that clicking "Open Project…" and picking a real project folder correctly updated the sidebar's label and populated the Collections/Environments lists with that project's actual saved data. Both are human-confirmed working, not just inferred from clean process exit; the exact visual layout (spacing, precise widget placement) is still code-derived rather than pixel-verified.
