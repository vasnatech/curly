# Curly GUI — User Guide

> Status: **early, several slices so far, FR-21 now complete** (M4, still in progress — see [DESIGN.md](DESIGN.md) §6/§9). The GUI can open a project, browse its collections as a tree, click a saved request to load it (including its extraction rules), create/edit/delete environments and switch between them (plus ad-hoc `--var`-style overrides) to resolve `{{variable}}` tokens, build a request from scratch, save it into a collection (new or overwriting what's loaded) or delete it, send it, show the response, run a loaded request's extraction rules on success and write the results into a session (viewable read-only in the sidebar), and follow (or manually toggle) light/dark theme. It does **not** yet open more than one project at once, offer multiple tabs, or offer syntax-highlighted/searchable responses, and there's no rename/duplicate/move for an existing saved request yet — those are still FR-19/20 (partly)/22/23. If you need history today, use [the CLI](CLI.md) — GUI and CLI share the same underlying storage, so anything you build or extract with one is already usable by the other.

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
- **"Open Project…"** — a native folder picker, run asynchronously so the window stays responsive while it's open (reads "Choosing…" and disables itself for the moment it's up — see "A note on the folder picker" below for why this matters). Pick any directory: if it already has a `.curly` in it, that project opens; if not, one is created there on the spot (same as running `curly init` in that folder), then opens. Unlike the CLI's own auto-detection, this does **not** search parent directories — a folder dialog already lets you navigate to exactly the directory you want, so opening `Documents/my-api` won't accidentally reopen some unrelated project three levels up.
- **Collections** — the active project's collections, each expandable into the same folder/request tree `curly collections show` prints. **Click any request to load it into the editor**, or its **✕** to delete it (see "Saving, deleting, and starting a new request" below).
- **Environments** — click a name to make it the active environment and open its variable editor, or **"(none selected)"** to go back to none (see "Environments and `{{variable}}` substitution" below).

**Only one project is open at a time right now** — picking a new one replaces the current one, it doesn't add a tab. This is deliberate groundwork, not a missing feature that slipped through: the underlying state is already a list (so a future "open several projects, switch between them" doesn't need a data-model change, just a UI for it), but only single-project interaction is wired up so far.

### Loading a saved request

Clicking a request in the sidebar's Collections tree loads it into the editor: method, URL, and headers always carry over. Beyond that:

- A plain text (`raw`) body loads into the Body box as-is.
- A `Bearer` token auth folds into an `Authorization: Bearer <token>` header (the editor has no separate auth concept yet, so this is the closest equivalent).
- Anything the editor **can't** represent — a form/multipart/binary body, `Basic` auth, or a `{{variable}}` token the *currently selected environment* doesn't define — shows as a yellow notice above the editor instead of being silently dropped or silently sent wrong. If you load a request that uses `{{BASE_URL}}` and no environment (or the wrong one) is selected, you'll see the literal `{{BASE_URL}}` in the URL/header fields plus a notice naming exactly which variables are undefined; pick the right environment in the sidebar (or add the missing variable to it) and the notice clears on its own — no need to reload the request.
- If the request has extraction rules (added via `curly collections add-request --extract-*`), they come along too — see "Extraction and sessions" below.

### Environments and `{{variable}}` substitution

Clicking a name in the sidebar's Environments list makes it the **active environment** for the editor — every `{{variable}}` token in the URL, headers, and body resolves against it when you hit Send, using the exact same precedence `curly run` uses (low to high): the always-merged-in `global` environment (if one exists), then the selected environment, then that environment's session (variables written by `curly run --extract-*` on a previous CLI run — the GUI doesn't do extraction itself yet, but reads sessions the CLI already wrote). Click **"(none selected)"** to go back to no environment — only `global` and the `global`-scoped session still apply then, matching what `curly run` does when `--env` is omitted.

An undefined variable fails the send outright with a clear error naming every undefined token (not just the first) — never silently goes out as a literal `{{token}}`, and never silently picks a value from the wrong environment.

### Creating and editing environments

- **"new environment" field + `+ New`** — type a name and click it (or it's disabled while the field is empty) to create an empty environment; it's selected immediately, ready for you to add variables to.
- Selecting any environment (new or existing) opens an **"Edit `<name>`"** panel below the list: each variable is a row of KEY / value / a `secret` checkbox (mirrors `curly env set ... --secret` — masks nothing in this editor itself, since you need to see the value to edit it, but is saved the same way and still masked by `curly env show`) / a `✕` to remove the row, plus **`+ Add variable`**.
- Nothing is written to disk until you click **Save** — a half-typed row never leaks into what a Send resolves against, since `Save` (not the in-memory editor) is what `merged_variables` reads back from. A row with a blank key is silently skipped on save, same rule `build_request` already applies to a blank-name header.
- **Delete environment** removes it immediately — no confirmation dialog, matching `curly env delete`'s own behavior. Deleting the currently-active one also clears the active-environment selection.
- All of this writes to the exact same `.curly/environments/*.json` files the CLI reads and writes, so a variable added in the GUI is visible to `curly env show` right away, and vice versa — no caching, every read/save goes straight to disk.

### Ad-hoc variable overrides (`--var`)

A collapsible **"Variable overrides (N)"** section sits between the URL bar and Headers (collapsed by default — it's a less-common need than headers, so it stays out of the way until you open it). Each row is a checkbox / KEY / value / `✕`-to-remove, plus **`+ Add override`** — the same shape as the Headers table.

These are the highest-precedence variable source: an enabled override with a matching key wins over the selected environment, its session, and even `global`, for sends made from this editor. Unlike environment variables, they're **never written to disk** — closing the app or switching projects loses them, exactly like `curly run --var key=value` only ever applies to that one invocation. Use them for a one-off value you don't want to save anywhere (a temporary test token, trying a different host without touching your real environment file). Unchecking a row's checkbox disables it without deleting it — same behavior as a header's checkbox.

### Extraction and sessions

If a loaded request has extraction rules (defined via `curly collections add-request --extract-body/--extract-header/--extract-template` — the GUI has no rule *editor* yet, only support for running rules already defined), you'll see a note above the editor: *"On a successful send, extracts TOKEN into session "\<scope\>"."* On a 2xx response, those rules run and the results are written into the session for whichever environment is selected (or `"global"` if none is) — a green confirmation notice names what got extracted. A non-2xx response skips extraction silently, same as `curly run` (an unmet rule on a failed request is an expected absence, not a bug); an actual extraction failure (e.g. a JSON path that doesn't exist) or a save failure shows a red-ish notice instead.

**Sessions are a separate store from environments** — the same distinction `curly env show` vs. `curly session show` draws on the CLI. A **"Session "\<scope\>" (N)"** panel in the sidebar, below the environment editor, shows that distinction: it's read-only (secret values masked as `***`, same as `curly session show`), reflects whatever `session_scope()` currently is (the selected environment's name, or `"global"`), and refreshes automatically when you switch environments or right after an extraction lands — no manual reload needed. A **"Clear session"** button mirrors `curly session clear`. If you extract a variable and don't see it where you'd expect, check here, not the environment editor above it — the environment editor only shows the environment *file*, which extraction never touches.

### Saving, deleting, and starting a new request (FR-20)

Below the URL/Send row, a small label shows what "Save" would do — `Editing "<collection>/<path>"` if the editor mirrors a saved request, or `New request (not saved)` otherwise — plus three buttons:

- **New** — clears the editor to a blank request (method resets to GET, headers/body/extraction rules all clear) without touching the active project, environment, or overrides. Use it before building something unrelated to what's currently loaded — otherwise a later "Save" would overwrite the wrong thing.
- **Save** (also **Ctrl/Cmd+S**) — if something's loaded, overwrites it in place (same collection, same path, same identity — its position in the tree doesn't move) with the editor's current method/URL/headers/body and whatever extraction rules it already had (there's no rule *editor*, only carry-through — see "Extraction and sessions" above). If nothing's loaded, falls back to Save As.
- **Save As…** — opens a small window: **Collection** (existing or new — typing a name that doesn't exist yet creates it, same as `curly collections add-request`'s own create-if-missing behavior), **Folder** (optional, e.g. `Auth/OAuth`), and **Name**. Pre-filled from whatever's loaded, so changing just the name or folder and clicking Save produces a copy at the new location rather than overwriting the original. A name that collides with an existing request at that exact path shows an error in the dialog rather than silently overwriting it — delete the existing one first, or use plain Save if you actually meant to overwrite it.

Deleting a request (the tree's **✕**) takes effect immediately, no confirmation — same pattern as environment/override deletion elsewhere in this app. Deleting the request that's currently loaded also clears the "Editing…" state, so a stray Save afterward can't try to write to something that no longer exists.

**Not yet built**: renaming, duplicating, or moving an *existing* tree entry directly (Save As from the editor gets you most of the way to "duplicate" — load it, Save As under a new name/folder — but there's no dedicated action for it, and no auth beyond whatever ends up in a header, since the editor still has no separate auth concept).

### Theme (FR-24)

A ☀/🌙 toggle sits next to the "Project" heading at the top of the sidebar — click it to switch between light and dark instantly. On launch, curly tries to match your OS's preference: on GNOME it reads `gsettings get org.gnome.desktop.interface color-scheme` once at startup (see "A note on Linux theme detection" below for why that's necessary at all); elsewhere, or if that lookup fails, it falls back to egui's own default (dark). This is a one-time check at launch, not a live watch — changing your OS theme while curly is already running won't update it; use the toggle, or restart the app.

### A note on Linux theme detection

egui has built-in support for "follow the OS light/dark setting" (`ThemePreference::System`, the default), but on Linux it doesn't actually work out of the box: `winit` (the windowing library egui/eframe build on) unconditionally returns `None` from its system-theme query on both X11 and Wayland, so egui's own "follow the system" silently falls back to a hardcoded dark theme regardless of your real setting. Curly works around this by checking GNOME's own `gsettings` schema directly at startup — a GNOME-specific heuristic, since that's what this project's development machine runs; other desktop environments (KDE, etc.) aren't specifically detected yet and will get the dark fallback until someone adds a heuristic for them (the toggle always works regardless, as an immediate workaround).

### A note on the folder picker

Native folder/file dialogs on Linux are commonly implemented as *blocking* calls — the calling thread just waits until the OS dialog closes. Calling one directly from `egui`'s UI thread stalls the whole event loop for as long as the dialog is open: no redraws, no input handling, nothing — which is exactly what a desktop compositor (GNOME's included) interprets as "this application has stopped responding", even though the app isn't actually hung. "Open Project…" runs the picker asynchronously instead (`rfd::AsyncFileDialog` on the same background runtime the HTTP client uses, polled back like a response), so the window keeps responding to the compositor the whole time the dialog is up.

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
- Defining/editing extraction rules from the GUI — only running rules a saved request already has (defined via `curly collections add-request --extract-*`) is supported
- Multiple request tabs (FR-19) — loading a new request replaces what's in the editor, there's no "open in a new tab"
- Renaming, duplicating, or moving an existing saved request directly (rest of FR-20) — Save/Save As/Delete are done; see "Saving, deleting, and starting a new request" above
- Loading a form/multipart/binary body or Basic auth into the editor (see "Loading a saved request" above) — flagged with a notice, not silently wrong
- Syntax highlighting, in-body search, save-to-file, or image preview for responses (FR-22) — the body view is plain monospace text
- A new-tab shortcut (FR-23) — Ctrl/Cmd+S to save and Ctrl/Cmd+Enter to send both already work
- Live-following an OS theme change while curly is already running, and non-GNOME OS-theme detection (rest of FR-24) — the manual toggle always works regardless

## A note on this guide's own testing

Automated screenshotting wasn't available in the environment this was built in (no `xdotool`/`grim`/`gnome-screenshot`; the GNOME Shell and xdg-desktop-portal screenshot D-Bus APIs both need interactive consent that never resolves headlessly) — so the code's author could confirm the process launches cleanly but not see the window directly. The person running it, on their own real desktop, has confirmed it thirteen times now: the send/response loop, opening a real project via the folder picker, expanding a real project's collection tree and clicking a nested request, the `{ }` window icon including the GNOME dock/Alt-Tab icon, the environment switcher end-to-end against a real backend, creating/editing/deleting an environment, the ad-hoc `--var` overrides section and its checkbox, extraction writing a real `TOKEN` into a real session followed by the read-only Session panel that surfaces it, the FR-20 save/delete slice and, separately, a real regression it surfaced ("Open Project…" freezing the window under GNOME, fixed by switching to an async file dialog), and — most recently — the light/dark toggle. All thirteen are human-confirmed working, not just inferred from clean process exit; the exact visual layout (spacing, precise widget placement) is still code-derived rather than pixel-verified.
