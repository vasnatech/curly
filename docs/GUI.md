# Curly GUI — User Guide

> Status: **early, several slices so far** (M4, still in progress — see [DESIGN.md](DESIGN.md) §6/§9). The GUI can open a project, browse its collections as a tree, click a saved request to load it, create/edit/delete environments and switch between them to resolve `{{variable}}` tokens, build a request from scratch, send it, and show the response. It does **not** yet open more than one project at once, save/edit *requests*, offer multiple tabs, or theme — those are still the rest of FR-19..24. If you need history, `--var` overrides, or saved-request extraction/session-writing today, use [the CLI](CLI.md) — GUI and CLI share the same underlying storage, so anything you build or extract with one is already usable by the other.

## Launching it

The GUI is built into the same `curly` binary as the CLI (see DESIGN.md §3) — there's no separate install.

```sh
curly              # no arguments at all → launches the GUI
curly gui          # same thing, explicit subcommand
```

Any other invocation — a URL, a flag, or a subcommand like `env`/`collections`/`run` — behaves exactly as documented in [CLI.md](CLI.md); the GUI only launches when there's nothing else to do.

If you built with `cargo build --no-default-features` (the lean CLI-only build, no `eframe`/`egui`/windowing dependencies), both of the above print a clear error instead of launching anything.

### The window icon, and a one-time step on Linux

The window carries a `{ }` icon (`crates/curly-gui/assets/icon.svg`, baked into the binary as a PNG via `include_bytes!` — there's no file to install just to see it in the window itself, on any OS).

On GNOME's Wayland session specifically, that in-window icon is **not** what the dock, Alt-Tab switcher, or Activities overview show — those resolve a running window's icon by matching its Wayland app id against an installed `.desktop` file, not the window's own icon hint (X11 window managers and the Windows/macOS taskbar use the in-window hint directly and need nothing extra). If you're on GNOME/Wayland and see a generic icon in the dock instead of `{ }`, run this once per machine:

```sh
./scripts/linux-install-desktop-entry.sh
```

It builds the release binary if needed, installs `~/.local/share/applications/tech.vasnatech.curly.desktop` plus the icon into `~/.local/share/icons/hicolor/`, and refreshes the desktop/icon caches. It's safe to re-run (it just overwrites the same files) — re-run it if you move the repo checkout or rebuild the binary somewhere else, since the `.desktop` file points at this checkout's `target/release/curly` by absolute path. Restart the GUI afterward for the dock to pick it up. To undo it, the script's own output prints the exact `rm` command.

## What's there today

### Projects

A **project** is exactly what the CLI already calls a project-local `.curly` directory (`curly init`) — the GUI didn't invent a new concept, it's the same `Storage` root. On launch, the GUI auto-opens a project the same way the CLI resolves storage by default: an auto-detected `.curly` walking up from wherever the GUI process's working directory happens to be, `CURLY_DATA_DIR`, or the OS-wide default location (labeled **"Default"** in the sidebar) if none of those find anything.

The left sidebar shows:
- The active project's label (the folder name a `.curly` lives in, or "Default") and its full path.
- **"Open Project…"** — a native folder picker. Pick any directory: if it already has a `.curly` in it, that project opens; if not, one is created there on the spot (same as running `curly init` in that folder), then opens. Unlike the CLI's own auto-detection, this does **not** search parent directories — a folder dialog already lets you navigate to exactly the directory you want, so opening `Documents/my-api` won't accidentally reopen some unrelated project three levels up.
- **Collections** — the active project's collections, each expandable into the same folder/request tree `curly collections show` prints. **Click any request to load it into the editor.**
- **Environments** — click a name to make it the active environment and open its variable editor, or **"(none selected)"** to go back to none (see "Environments and `{{variable}}` substitution" below).

**Only one project is open at a time right now** — picking a new one replaces the current one, it doesn't add a tab. This is deliberate groundwork, not a missing feature that slipped through: the underlying state is already a list (so a future "open several projects, switch between them" doesn't need a data-model change, just a UI for it), but only single-project interaction is wired up so far.

### Loading a saved request

Clicking a request in the sidebar's Collections tree loads it into the editor: method, URL, and headers always carry over. Beyond that:

- A plain text (`raw`) body loads into the Body box as-is.
- A `Bearer` token auth folds into an `Authorization: Bearer <token>` header (the editor has no separate auth concept yet, so this is the closest equivalent).
- Anything the editor **can't** represent — a form/multipart/binary body, `Basic` auth, or a `{{variable}}` token the *currently selected environment* doesn't define — shows as a yellow notice above the editor instead of being silently dropped or silently sent wrong. If you load a request that uses `{{BASE_URL}}` and no environment (or the wrong one) is selected, you'll see the literal `{{BASE_URL}}` in the URL/header fields plus a notice naming exactly which variables are undefined; pick the right environment in the sidebar (or add the missing variable to it) and the notice clears on its own — no need to reload the request.

### Environments and `{{variable}}` substitution

Clicking a name in the sidebar's Environments list makes it the **active environment** for the editor — every `{{variable}}` token in the URL, headers, and body resolves against it when you hit Send, using the exact same precedence `curly run` uses (low to high): the always-merged-in `global` environment (if one exists), then the selected environment, then that environment's session (variables written by `curly run --extract-*` on a previous CLI run — the GUI doesn't do extraction itself yet, but reads sessions the CLI already wrote). Click **"(none selected)"** to go back to no environment — only `global` and the `global`-scoped session still apply then, matching what `curly run` does when `--env` is omitted.

An undefined variable fails the send outright with a clear error naming every undefined token (not just the first) — never silently goes out as a literal `{{token}}`, and never silently picks a value from the wrong environment.

### Creating and editing environments

- **"new environment" field + `+ New`** — type a name and click it (or it's disabled while the field is empty) to create an empty environment; it's selected immediately, ready for you to add variables to.
- Selecting any environment (new or existing) opens an **"Edit `<name>`"** panel below the list: each variable is a row of KEY / value / a `secret` checkbox (mirrors `curly env set ... --secret` — masks nothing in this editor itself, since you need to see the value to edit it, but is saved the same way and still masked by `curly env show`) / a `✕` to remove the row, plus **`+ Add variable`**.
- Nothing is written to disk until you click **Save** — a half-typed row never leaks into what a Send resolves against, since `Save` (not the in-memory editor) is what `merged_variables` reads back from. A row with a blank key is silently skipped on save, same rule `build_request` already applies to a blank-name header.
- **Delete environment** removes it immediately — no confirmation dialog, matching `curly env delete`'s own behavior. Deleting the currently-active one also clears the active-environment selection.
- All of this writes to the exact same `.curly/environments/*.json` files the CLI reads and writes, so a variable added in the GUI is visible to `curly env show` right away, and vice versa — no caching, every read/save goes straight to disk.

### Sending a request

- **Method + URL bar** — pick a method from the dropdown, type a URL.
- **Send** — click the button, press Enter in the URL field, or press Ctrl+Enter (Cmd+Enter on macOS) from anywhere in the window.
- **Headers** — an editable table: checkbox to enable/disable a header without deleting it, name/value fields, `+ Add header` to add another row. A header with an empty name is ignored even if checked.
- **Body** — a plain multi-line text box, sent as the raw request body (matching the CLI's `-d`/`--data`; no separate form/multipart/binary body modes in the GUI's own editor yet, even though a loaded request's raw body shows here fine).
- **Response pane** — status (color-coded: green 2xx, yellow 3xx, red 4xx/5xx), elapsed time, body size, a collapsible list of response headers, and the body — pretty-printed automatically if it's JSON, shown as-is otherwise.

Sending is asynchronous — the window stays responsive while a request is in flight ("Sending…" on the button), and you can't fire a second request until the first one finishes. This part is completely independent of which project (if any) is open — typing a URL and sending it works the same regardless.

## What's not there yet

Tracked in DESIGN.md's M4 entry:

- Multiple projects open simultaneously (the data model already supports it — see "Projects" above — the interaction doesn't yet)
- A `--var key=value`-style ad-hoc override for a single send, without saving it to an environment (rest of FR-21)
- Extraction rules writing into a session from the GUI — the GUI reads sessions the CLI's `curly run --extract-*` already wrote, but doesn't run extraction itself yet
- Multiple request tabs (FR-19) — loading a new request replaces what's in the editor, there's no "open in a new tab"
- Save/rename/duplicate/move a request from the GUI (FR-20) — for now, build/edit saved requests with the CLI (`curly collections add-request ...`); they show up in the sidebar's tree once you do, and loading one back into the editor works today
- Loading a form/multipart/binary body or Basic auth into the editor (see "Loading a saved request" above) — flagged with a notice, not silently wrong
- Syntax highlighting, in-body search, save-to-file, or image preview for responses (FR-22) — the body view is plain monospace text
- Ctrl/Cmd+S to save, new-tab shortcuts (FR-23) — Ctrl/Cmd+Enter to send already works
- Light/dark theme following the OS (FR-24) — currently whatever `eframe`'s own default is

## A note on this guide's own testing

Automated screenshotting wasn't available in the environment this was built in (no `xdotool`/`grim`/`gnome-screenshot`; the GNOME Shell and xdg-desktop-portal screenshot D-Bus APIs both need interactive consent that never resolves headlessly) — so the code's author could confirm the process launches cleanly but not see the window directly. The person running it, on their own real desktop, has confirmed it six times now: the send/response loop (GET round-tripped, body pretty-printed correctly), opening a real project via the folder picker (label and lists updated correctly), expanding a real project's collection tree and clicking a nested request to confirm it loaded into the editor with the expected unresolved-`{{variable}}` notice, the `{ }` window icon including the GNOME dock/Alt-Tab icon after running `scripts/linux-install-desktop-entry.sh`, the environment switcher end-to-end (loading a real `Auth/login` request against `health-record-backend-spring`'s actual `.curly` project, switching the active environment, and getting a real JWT back from the real local backend after `{{BASE_URL}}`/`{{EMAIL}}`/`{{PASSWORD}}` all resolved correctly), and — most recently — creating/editing/deleting an environment from the sidebar itself. All six are human-confirmed working, not just inferred from clean process exit; the exact visual layout (spacing, precise widget placement) is still code-derived rather than pixel-verified.
