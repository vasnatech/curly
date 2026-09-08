# Curly GUI — User Guide

> Status: **early, first slice only** (M4, just started — see [DESIGN.md](DESIGN.md) §6/§9). The GUI can build a request, send it, and show the response. It does **not** yet have a collections/environments sidebar, multiple request tabs, saving, or theming — those are FR-18..24, not built yet. If you need collections, environments, `{{variable}}` substitution, saved requests, or history today, use [the CLI](CLI.md) — GUI and CLI share the same underlying storage, so anything you build with one will be usable from the other once the GUI grows storage integration.

## Launching it

The GUI is built into the same `curly` binary as the CLI (see DESIGN.md §3) — there's no separate install.

```sh
curly              # no arguments at all → launches the GUI
curly gui          # same thing, explicit subcommand
```

Any other invocation — a URL, a flag, or a subcommand like `env`/`collections`/`run` — behaves exactly as documented in [CLI.md](CLI.md); the GUI only launches when there's nothing else to do.

If you built with `cargo build --no-default-features` (the lean CLI-only build, no `eframe`/`egui`/windowing dependencies), both of the above print a clear error instead of launching anything.

## What's there today

- **Method + URL bar** — pick a method from the dropdown, type a URL.
- **Send** — click the button, press Enter in the URL field, or press Ctrl+Enter (Cmd+Enter on macOS) from anywhere in the window.
- **Headers** — an editable table: checkbox to enable/disable a header without deleting it, name/value fields, `+ Add header` to add another row. A header with an empty name is ignored even if checked.
- **Body** — a plain multi-line text box, sent as the raw request body (matching the CLI's `-d`/`--data`; no separate form/multipart/binary body modes in the GUI yet).
- **Response pane** — status (color-coded: green 2xx, yellow 3xx, red 4xx/5xx), elapsed time, body size, a collapsible list of response headers, and the body — pretty-printed automatically if it's JSON, shown as-is otherwise.

Sending is asynchronous — the window stays responsive while a request is in flight ("Sending…" on the button), and you can't fire a second request until the first one finishes.

## What's not there yet

Tracked in DESIGN.md's M4 entry — in short, everything past the single-request send/response loop:

- Collections/history/environments sidebar (FR-18)
- Multiple request tabs (FR-19)
- Save/rename/duplicate/move a request from the GUI (FR-20) — for now, build/edit saved requests with the CLI (`curly collections add-request ...`) and they'll be usable from the GUI once it gains storage integration
- An environment editor / `{{variable}}` resolution in the GUI (FR-21) — the URL/header/body fields send exactly what you type, no `{{var}}` substitution happens yet
- Syntax highlighting, in-body search, save-to-file, or image preview for responses (FR-22) — the body view is plain monospace text
- Ctrl/Cmd+S to save, new-tab shortcuts (FR-23) — Ctrl/Cmd+Enter to send already works
- Light/dark theme following the OS (FR-24) — currently whatever `eframe`'s own default is

## A note on this guide's own testing

Automated screenshotting wasn't available in the environment this was built in (no `xdotool`/`grim`/`gnome-screenshot`; the GNOME Shell and xdg-desktop-portal screenshot D-Bus APIs both need interactive consent that never resolves headlessly) — so the code's author could confirm the process launches cleanly but not see the window directly. The person running it, on their own real desktop, could: they confirmed the window renders correctly and a live send — GET `https://httpbin.org/get` — round-tripped end to end, response pretty-printed as shown above. So the send/response loop described in "What's there today" is confirmed working, not just inferred from the code; the exact visual layout (spacing, exact widget placement) is still code-derived rather than pixel-verified.
