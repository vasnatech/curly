# Curly GUI — User Guide

> Status: **M4 (GUI v1) is complete** except multiple simultaneously-open projects, which has been carved out into its own future milestone (M7 — see [DESIGN.md](DESIGN.md) §9). The GUI has a toolbar (open a project, toggle light/dark, zoom the whole UI — all three remembered across restarts), supports multiple request tabs open at once (with a "response ready" indicator on a background tab), can open a project, browse its collections as a tree, click a saved request to load it into a tab (including its extraction rules and Basic/Bearer authorization), rename/move or duplicate an existing saved request, create/edit/delete environments and switch between them (plus ad-hoc `--var`-style overrides) to resolve `{{variable}}` tokens, build a request from scratch (including its Authorization and extraction rules — both fully editable now), save it into a collection (new or overwriting what's loaded) or delete it, send it, browse recent history in the sidebar and click an entry to load it back into the editor for a re-run, show the response with JSON syntax highlighting, in-body search with find-next/previous, an image preview for image responses, save the response to a file (byte-exact, including images), and run a loaded request's extraction rules on success and write the results into a session (viewable read-only in the sidebar). GUI and CLI share the same underlying storage, so anything you build, send, or extract with one is already usable by the other (`curly history` still works too, and sees exactly the same entries).

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

### Toolbar

A single row across the top of the window holds every "global" action — not scoped to any particular project, request, or panel:

- **Open Project…** — see "Projects" below.
- **☀/🌙** — light/dark theme toggle; see "Theme" below.
- **Zoom: − 100% +  Reset** — scales the *entire* UI (fonts, spacing, every widget) up or down. **100% is the baseline this app is laid out for**, at a 1024×768 window — if everything reads small on a high-DPI/high-resolution display, this is the fix, not a font-size setting buried somewhere. Click `−`/`+` (each click is a ~10% step), or use **Ctrl/Cmd + minus/plus/equals** (`+` and `=` both work, since `+` needs Shift on most keyboards), or **Ctrl/Cmd + scroll wheel** anywhere in the window (egui's own built-in gesture, wired up to actually apply here — see DESIGN.md for why that's a separate step from the gesture itself), or **Ctrl/Cmd+0** to reset to exactly 100%. The percentage shown always reflects the real current value. Zoom (along with theme and open tabs) is remembered across restarts — see "Remembering zoom, theme, and open tabs" below.

### Request tabs (FR-19)

A row of tabs sits above the method/URL bar — each one is a fully independent request: its own method, URL, headers, body, response, search box, extraction rules, and ad-hoc `--var` overrides. Nothing about one tab affects another.

- **Click a tab** to switch to it; the toolbar/sidebar (project, environments, collections) stay the same across all tabs — only the request editor and response pane are per-tab.
- **✕** on a tab closes it immediately, no confirmation — closing the *last* remaining tab resets it to blank instead of leaving zero tabs open, the same way a browser won't let you close its only tab.
- **+** (or **Ctrl/Cmd+T**) opens a new blank tab and switches to it.
- Each tab's label shows the loaded request's name if it has one, otherwise `METHOD url`, otherwise "New Request" — long labels are truncated.
- Environments/overrides/extraction rules are **per-tab**, not global — load two different requests into two tabs and each keeps its own `--var` overrides and its own "will extract..." note independently.
- **Sending from a tab you then switch away from keeps running** — the request doesn't get cancelled, and a small **●** appears before that tab's label once the response (or error) actually lands, so you know it's ready without needing to check every tab by hand. Switching to that tab clears the marker; extraction rules (if any) run the same way they would if you'd stayed on the tab the whole time.
- Opening the Save dialog, then switching tabs, cancels the dialog rather than risk saving the *new* active tab's content under the old tab's pre-filled name/collection.

### Projects

A **project** is exactly what the CLI already calls a project-local `.curly` directory (`curly init`) — the GUI didn't invent a new concept, it's the same `Storage` root. On launch, the GUI auto-opens a project the same way the CLI resolves storage by default: an auto-detected `.curly` walking up from wherever the GUI process's working directory happens to be, `CURLY_DATA_DIR`, or the OS-wide default location (labeled **"Default"** in the sidebar) if none of those find anything.

The left sidebar shows:
- The active project's label (the folder name a `.curly` lives in, or "Default") and its full path. To switch projects, use the toolbar's **"Open Project…"** — a native folder picker, run asynchronously so the window stays responsive while it's open (reads "Choosing…" and disables itself for the moment it's up — see "A note on the folder picker" below for why this matters). Pick any directory: if it already has a `.curly` in it, that project opens; if not, one is created there on the spot (same as running `curly init` in that folder), then opens. Unlike the CLI's own auto-detection, this does **not** search parent directories — a folder dialog already lets you navigate to exactly the directory you want, so opening `Documents/my-api` won't accidentally reopen some unrelated project three levels up.
- **Collections** — the active project's collections, each expandable into the same folder/request tree `curly collections show` prints. **Click any request to load it into the editor**, its **✎** to rename or move it, its **⧉** to duplicate it, or its **✕** to delete it (see "Saving, deleting, and starting a new request" below).
- **History** — the project's most recent sent requests (FR-6), newest first, same data `curly history` prints. Click one to load it back into the editor (see "History and re-running a request" below).
- **Environments** — click a name to make it the active environment and open its variable editor, or **"(none selected)"** to go back to none (see "Environments and `{{variable}}` substitution" below).

**Only one project is open at a time right now** — picking a new one replaces the current one. This is deliberate groundwork, not a missing feature that slipped through: the underlying state is already a list (so a future "open several projects, switch between them" doesn't need a data-model change, just a UI for it), but only single-project interaction is wired up so far — multi-project support is its own future milestone (M7, see DESIGN.md §9), not part of the GUI's current scope. This is a different, independent axis from request tabs above — one project, multiple request tabs into it, today.

### Loading a saved request

Clicking a request in the sidebar's Collections tree loads it into the editor: method, URL, and headers always carry over. Beyond that:

- A plain text (`raw`) body loads into the Body box as-is.
- `Basic` and `Bearer` auth both load straight into the Authorization section (see "Authorization" below) — no more folding a Bearer token into a header or flagging Basic as unrepresentable, both are first-class now.
- Anything the editor **can't** represent — a form/multipart/binary body, or a `{{variable}}` token the *currently selected environment* doesn't define — shows as a yellow notice above the editor instead of being silently dropped or silently sent wrong. If you load a request that uses `{{BASE_URL}}` and no environment (or the wrong one) is selected, you'll see the literal `{{BASE_URL}}` in the URL/header fields plus a notice naming exactly which variables are undefined; pick the right environment in the sidebar (or add the missing variable to it) and the notice clears on its own — no need to reload the request.
- If the request has extraction rules (added via `curly collections add-request --extract-*`), they come along too — see "Extraction and sessions" below.

### Authorization

A collapsible **Authorization** section sits between Variable overrides and Headers, with a None/Basic/Bearer selector:

- **Basic** — Username and Password fields.
- **Bearer** — a single Token field.

Either can reference `{{variable}}` tokens, resolved the same way as the URL/headers/body when you Send. Saving a request carries the Authorization section over as `SavedAuth` (the same thing `curly collections add-request -u`/`--bearer` write), and loading a saved request populates it straight back — round-trips exactly, in both directions, with the CLI.

### History and re-running a request

Clicking an entry in the sidebar's **History** list loads it back into the editor, ready to send again — a lightweight re-run, not a live replay of the original response. A history entry is already fully resolved (the CLI/GUI record it *after* `{{variable}}` substitution), so there's nothing to pick an environment for first. A few things intentionally don't come back byte-for-byte, each flagged with a yellow notice when it applies:

- **Redacted headers** (`Authorization`, `X-Api-Key`, `Cookie` — the same NFR-4 redaction `curly history` already applies) are dropped rather than loaded as the literal `***redacted***` placeholder — re-add them by hand if the request needs them.
- **Basic/Bearer credentials are never stored in history at all**, only *which kind* of auth was used — loading a history entry that used one sets the Authorization section's mode accordingly but leaves the username/password/token blank; re-enter them before sending. This is deliberate: storing real credentials in history to make re-run fully automatic would undo the same NFR-4 guarantee the redacted-headers behavior above protects.
- The request body **is** recorded and reloads as-is, but only for a plain text body — a request sent with a form/multipart/binary body (not something the editor can build anyway) has no body recorded for it.

Method, URL, and everything else recorded (headers, body) reload exactly as sent. The reloaded request isn't tied to any saved collection entry, so **Save** falls back to **Save As…**, same as any other from-scratch request.

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

A collapsible **"Extraction rules (N)"** section (collapsed by default) lets you author extraction rules directly in the editor — no need to drop to `curly collections add-request --extract-*` anymore, though that still works exactly the same and both write the same `SavedRequest.extract` data. Each row has a Body/Header/Template selector, a Name field (the session variable to write into), a second field whose label follows the selector (JSON path / header name / template text), and a `secret` checkbox (masks the value in the read-only Session panel below, same as `curly session show`); **`+ Add rule`** adds a blank one, **✕** removes a row. Switching a row's kind keeps its name/secret and clears the kind-specific field. A row with a blank name is dropped on Save, same rule headers already follow.

If the editor has any rules (freshly authored or carried over from a loaded request), you'll see a note above the editor: *"On a successful send, extracts TOKEN into session "\<scope\>"."* On a 2xx response, those rules run and the results are written into the session for whichever environment is selected (or `"global"` if none is) — a green confirmation notice names what got extracted. A non-2xx response skips extraction silently, same as `curly run` (an unmet rule on a failed request is an expected absence, not a bug); an actual extraction failure (e.g. a JSON path that doesn't exist) or a save failure shows a red-ish notice instead. This also now works for a send from a **background tab** — switch away right after sending and the extraction still runs (and the session panel still updates) the moment the response lands, not only once you switch back.

**Sessions are a separate store from environments** — the same distinction `curly env show` vs. `curly session show` draws on the CLI. A **"Session "\<scope\>" (N)"** panel in the sidebar, below the environment editor, shows that distinction: it's read-only (secret values masked as `***`, same as `curly session show`), reflects whatever `session_scope()` currently is (the selected environment's name, or `"global"`), and refreshes automatically when you switch environments or right after an extraction lands — no manual reload needed. A **"Clear session"** button mirrors `curly session clear`. If you extract a variable and don't see it where you'd expect, check here, not the environment editor above it — the environment editor only shows the environment *file*, which extraction never touches.

### Saving, deleting, and starting a new request (FR-20)

Below the URL/Send row, a small label shows what "Save" would do — `Editing "<collection>/<path>"` if the editor mirrors a saved request, or `New request (not saved)` otherwise — plus three buttons:

- **New** — resets the *current tab* to a fully blank request (method back to GET, headers/body/extraction rules/overrides/search all cleared) without touching the active project or environment selection. Equivalent to opening a new tab (Ctrl/Cmd+T) except it reuses the current tab instead of adding one — use whichever fits; both leave you with a blank slate so a later "Save" can't overwrite the wrong thing.
- **Save** (also **Ctrl/Cmd+S**) — if something's loaded, overwrites it in place (same collection, same path, same identity — its position in the tree doesn't move) with the editor's current method/URL/headers/body and whatever extraction rules it already had (there's no rule *editor*, only carry-through — see "Extraction and sessions" above). If nothing's loaded, falls back to Save As.
- **Save As…** — opens a small window: **Collection** (existing or new — typing a name that doesn't exist yet creates it, same as `curly collections add-request`'s own create-if-missing behavior), **Folder** (optional, e.g. `Auth/OAuth`), and **Name**. Pre-filled from whatever's loaded, so changing just the name or folder and clicking Save produces a copy at the new location rather than overwriting the original. A name that collides with an existing request at that exact path shows an error in the dialog rather than silently overwriting it — delete the existing one first, or use plain Save if you actually meant to overwrite it.

Deleting a request (the tree's **✕**) takes effect immediately, no confirmation — same pattern as environment/override deletion elsewhere in this app. Deleting the request that's currently loaded also clears the "Editing…" state, so a stray Save afterward can't try to write to something that no longer exists.

**Renaming/moving and duplicating** (the tree's **✎**/**⧉**) open a small window pre-filled with the request's current folder/name — edit either field and confirm:

- **Rename / Move** changes the request's path in place (same identity — its `id`, body, auth, and extraction rules all carry over unchanged); if the request being renamed is the one currently loaded, "Editing…" updates to point at its new location automatically. Errors (shown in the dialog, not closing it) if something already exists at the new path.
- **Duplicate** copies the request to a new path with a fresh identity (a new id, same as what "Save As…" produces) — the original is untouched and stays loaded if it was.

Both are also available from the CLI as `curly collections rename-request`/`duplicate-request`, for the same NFR-5 (GUI/CLI share storage) reason every other collection operation is.

### Theme (FR-24)

The toolbar's ☀/🌙 toggle switches between light and dark instantly. On launch, curly tries to match your OS's preference — see "A note on Linux theme detection" below for how — and then **keeps following it live**: while you haven't manually toggled (this session or a past one — see "Remembering zoom, theme, and open tabs" below), curly re-checks the OS setting roughly every 2 seconds and switches automatically if it changed, no restart needed.

Clicking the toggle yourself takes over: from that point on, your choice sticks (including across restarts) and curly stops live-following the OS until you toggle again. This is deliberate — a manual choice should stay put, not get silently overridden by the next automatic re-check.

### A note on Linux theme detection

egui has built-in support for "follow the OS light/dark setting" (`ThemePreference::System`, the default), but on Linux it doesn't actually work out of the box: `winit` (the windowing library egui/eframe build on) unconditionally returns `None` from its system-theme query on both X11 and Wayland, so egui's own "follow the system" silently falls back to a hardcoded dark theme regardless of your real setting. Curly works around this with two desktop-specific heuristics, picked based on `$XDG_CURRENT_DESKTOP`:

- **GNOME** — reads `gsettings get org.gnome.desktop.interface color-scheme`, the same source GTK/GNOME apps themselves read.
- **KDE** — reads `~/.config/kdeglobals`'s `[General]` section for a `ColorScheme=` value, treating a scheme name containing "dark" (e.g. `BreezeDark`) as dark — no shelling out to `kreadconfig5`/`kreadconfig6`, since the binary name differs across KDE versions and may not even be installed, whereas the config file itself always exists for a KDE user.

Any other desktop environment isn't specifically detected and gets whatever theme was already set (the manual toggle always works regardless, as an immediate workaround, and sticks across restarts once used — see "Theme" above).

### Remembering zoom, theme, and open tabs

Quitting curly saves your current zoom level, theme (and whether it's still following the OS or was manually pinned — see "Theme" above), and every open request tab's method/URL/headers/body/Authorization/which-saved-request-it-mirrors to `~/.config/curly/gui_state.json` (Linux; the OS-appropriate config directory elsewhere) — a small preferences file, separate from any project's `.curly` data. Launching again restores all of it, so your workspace looks the way you left it.

A few things are deliberately **not** remembered, matching this app's existing "never written to disk" stance for anything ad-hoc/ephemeral elsewhere: a tab's variable overrides, its response/response-search state, and its extraction rules. For a tab that mirrors a saved request, "Editing…" itself does come back after a restart, but its extraction rules don't — if it had any, re-click the request in Collections to reload them (they're always safe on disk regardless; this is only about what a restored tab shows before you do). If curly's process is killed rather than closed normally (a crash, `kill -9`, etc.), nothing from that session is saved — the file is only written on a normal exit.

### A note on the folder picker

Native folder/file dialogs on Linux are commonly implemented as *blocking* calls — the calling thread just waits until the OS dialog closes. Calling one directly from `egui`'s UI thread stalls the whole event loop for as long as the dialog is open: no redraws, no input handling, nothing — which is exactly what a desktop compositor (GNOME's included) interprets as "this application has stopped responding", even though the app isn't actually hung. The toolbar's "Open Project…" (and the Response pane's "Save Response…") run the picker asynchronously instead (`rfd::AsyncFileDialog` on the same background runtime the HTTP client uses, polled back like a response), so the window keeps responding to the compositor the whole time the dialog is up.

### Sending a request

- **Method + URL bar** — pick a method from the dropdown, type a URL.
- **Send** — click the button, press Enter in the URL field, or press Ctrl+Enter (Cmd+Enter on macOS) from anywhere in the window.
- **Authorization** — None/Basic/Bearer, see "Authorization" above.
- **Headers** — an editable table: checkbox to enable/disable a header without deleting it, name/value fields, `+ Add header` to add another row. A header with an empty name is ignored even if checked.
- **Body** — a plain multi-line text box, sent as the raw request body (matching the CLI's `-d`/`--data`; no separate form/multipart/binary body modes in the GUI's own editor yet, even though a loaded request's raw body shows here fine).
- **Response pane** — status (color-coded: green 2xx, yellow 3xx, red 4xx/5xx), elapsed time, body size, a collapsible list of response headers, and the body — pretty-printed automatically if it's JSON, shown as-is otherwise. See "Reading a response" below for the search/highlighting/save-to-file that comes with it.

Sending is asynchronous — the window stays responsive while a request is in flight ("Sending…" on the button), and you can't fire a second request until the first one finishes. This part is completely independent of which project (if any) is open — typing a URL and sending it works the same regardless. Every request that actually gets a response (successful or not — a 404 counts, a network error doesn't) is recorded into the project's history, same as the CLI (see "History and re-running a request" above) — this needs a project open (falls back to "Default" if none was ever picked) since history is stored per-project, same as collections/environments.

### Reading a response (FR-22)

- **JSON syntax highlighting** — a response body that parses as JSON gets its strings, numbers, `true`/`false`/`null`, and punctuation colored differently (colors chosen to work in both light and dark theme). A non-JSON body (HTML, XML, plain text, ...) still displays fine, just without the coloring.
- **Search** — the "Search:" box above the body highlights every case-insensitive match with a yellow background. Once there's at least one match, **◀**/**▶** buttons appear next to a "match X of N" label — click either to move which match is "current," shown in orange instead of yellow so it's visually distinct from the rest, and scrolls it into view. Wraps around at either end. Works on any response body, JSON or not.
- **Image preview** — if the response's `Content-Type` starts with `image/` and the body decodes as one of PNG/JPEG/GIF/WebP/BMP, it renders as an image instead of text (scaled to fit the pane's width). A `Content-Type` that looks like an image but doesn't actually decode shows a red notice rather than a blank pane or a crash.
- **Save Response…** — opens a native "Save As" dialog (async, same responsiveness fix as "Open Project…" below) and writes the response's exact raw bytes to wherever you pick — byte-for-byte, including for an image response (this also fixed a latent correctness gap for *any* binary body: saving used to go through the lossy UTF-8-decoded text copy, which would have silently corrupted anything that wasn't valid text). Defaults the suggested filename to `response.json`/`response.txt` for text, or `response.<ext>` guessed from the image's Content-Type (e.g. `response.png`) — you can change either the name or the extension in the dialog. A green notice confirms the path once it's written; a cancelled dialog shows nothing.

## What's not there yet

M4 (GUI v1) is otherwise complete. What's left is scoped as its own future milestone, not an M4 gap:

- **Multiple projects open simultaneously** (M7 — see DESIGN.md §9) — the data model already supports it (`CurlyApp::projects: Vec<Project>`, see "Projects" above), but the interaction (a project switcher, and deciding how it relates to the existing request-tab strip) isn't built yet.

A few narrower, deliberate limitations remain within what *is* built, each already called out where it applies above: loading a form/multipart/binary body into the editor (flagged with a notice, not silently wrong — Basic/Bearer auth both load fine); re-running a history entry that used a form/multipart/binary body (never recorded, only raw text bodies are) or Basic/Bearer auth (credentials are never stored in history, by design — see "History and re-running a request"); and a restored tab after a restart not showing its extraction rules until you re-click the request in Collections (see "Remembering zoom, theme, and open tabs").

## A note on this guide's own testing

Automated screenshotting wasn't available in the environment this was built in (no `xdotool`/`grim`/`gnome-screenshot`; the GNOME Shell and xdg-desktop-portal screenshot D-Bus APIs both need interactive consent that never resolves headlessly) — so the code's author could confirm the process launches cleanly but not see the window directly. The person running it, on their own real desktop, has confirmed it sixteen times now: the send/response loop, opening a real project via the folder picker, expanding a real project's collection tree and clicking a nested request, the `{ }` window icon including the GNOME dock/Alt-Tab icon, the environment switcher end-to-end against a real backend, creating/editing/deleting an environment, the ad-hoc `--var` overrides section and its checkbox, extraction writing a real `TOKEN` into a real session followed by the read-only Session panel that surfaces it, the FR-20 save/delete slice and, separately, a real regression it surfaced ("Open Project…" freezing the window under GNOME, fixed by switching to an async file dialog), the light/dark toggle, JSON syntax highlighting plus search on a real response, the toolbar and zoom, and — most recently — opening multiple request tabs, loading different requests into each, and switching between them with nothing bleeding across tabs. All sixteen are human-confirmed working, not just inferred from clean process exit; the exact visual layout (spacing, precise widget placement) is still code-derived rather than pixel-verified.
