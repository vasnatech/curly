# Curly GUI — User Guide

> Status: the GUI is now a **Tauri + React + TypeScript** app, living in `curly-gui-tauri/` (a sibling of `rust/`), replacing the earlier Rust/egui implementation (`crates/curly-gui`, **deleted 2026-11-09** after a period frozen and parked — see `DESIGN.md`'s M8 and M9 entries). It reaches the same feature set the M8 design pass specified: a sidebar Project/History switcher, a Collections tree with hover "⋯" menus (rename/duplicate/delete; new-folder/new-request/delete-folder; new-folder/new-request/save-all on the collection root), an Environments list with create/edit/delete, multiple request tabs with dirty `"*"` tracking, an editable breadcrumb for renaming, Save/Save As, Variables (ad-hoc overrides)/Authorization/Headers/Body/Extraction sub-tabs, dedicated Collection/Folder/Environment main tabs plus an "Active environment" selector, and a Response pane with Body/Headers/History sub-tabs, JSON syntax highlighting, and search with match navigation. Like the GUI it replaces, it shares the exact same `.curly` storage the CLI reads and writes (NFR-5) — nothing here is a separate data store.

> **There are two graphical front-ends now, and they are not rivals.** The Tauri app above is the
> React one, and it reaches storage over HTTP through `curly serve`. `curly gui` is a desktop window
> written in Ziman on ziman's own `lib/ui` (`ziman/src/main/ziman/gui/app.zm`), and it depends on no
> server at all — it reads the same `.curly` storage from disk and sends through `core/exec` in
> process. See `docs/ZIMAN.md` for what it does and what it needs. The Rust CLI's `gui` subcommand
> still prints "the graphical interface is a separate application"; the Ziman CLI's opens the window.

## Launching it

Unlike the CLI (a single `curly` binary), the GUI isn't bundled into that binary yet — it's a separate project you run directly:

```sh
cd curly-gui-tauri
npm install        # first time only
npm run tauri dev  # launches the dev build (Vite + the Tauri window together)
```

There's no packaged/installable build documented yet (`npm run tauri build` exists and should produce one, but hasn't been exercised as part of this milestone — treat it as untested until someone runs it through).

### Linux build prerequisites

Tauri v2's Linux webview needs system packages this repo doesn't otherwise require. On Fedora:

```sh
sudo dnf install webkit2gtk4.1-devel openssl-devel curl wget file libappindicator-gtk3-devel librsvg2-devel libxdo-devel
sudo dnf group install "c-development"
```

(Exact package names differ by distro — check Tauri's own prerequisites page for yours.) You'll also want the Tauri CLI available as a cargo subcommand: `cargo install tauri-cli`.

## What's there today

### Opening a project

The GUI auto-opens a project on launch the same way the CLI resolves storage by default: an auto-detected `.curly` walking up from the process's working directory, `CURLY_DATA_DIR`, or the OS-wide default location (labeled **"Default"** in the sidebar).

**"Open Project…"**, top-right of the sidebar's project label, opens a native folder picker. Pick any directory: if it already has a `.curly` in it, that project opens; if not, one is created there on the spot (same as running `curly init` in that folder). Opening a different project resets the active environment selection and closes any open Collection/Folder/Environment tabs (their data belonged to the previous project) — but leaves your request tabs alone, so in-progress edits aren't lost just for switching projects.

### Sidebar: Project / History

A small switcher at the top of the sidebar picks between two views:

- **Project** (the default) — the active project's label/path, then Collections, then Environments.
- **History** — the project's most recent sent requests (FR-6), newest first (method/status/url). Click one to load it back into the editor for a re-run.

### Collections tree and its hover "⋯" menus

Each collection is expandable (click its **arrow** to toggle) into the same folder/request tree `curly collections show` prints, lazily loaded from disk the first time you expand it. Clicking a request loads it into the *active* tab.

Hover any row — a request, a folder, or a collection's own name — to reveal a **⋯** button (invisible otherwise, so the tree stays uncluttered) with that row's actions:

- **Request row**: Rename, Duplicate, Delete.
- **Folder row**: New Folder, New Request, Delete Folder (refuses on a non-empty folder — delete its contents first).
- **Collection's own row**: New Folder, New Request, Save All (saves every open tab with unsaved changes belonging to that collection).

Clicking a folder's or a collection's **name** (not its ⋯, not the arrow) opens it as its own main tab in the tab strip — see "Collection and Folder tabs" below.

Renaming/duplicating opens a small dialog pre-filled with the request's current folder/name; both call the same `curly-core` methods (`rename_or_move_request`/`duplicate_request`) the CLI's `curly collections rename-request`/`duplicate-request` use, so the two stay in sync automatically.

### Loading a saved request

Clicking a request loads its method, URL, headers, body (if it's a plain-text `raw` body — form/multipart/binary bodies aren't yet editable here), and Authorization (Basic/Bearer) into the active tab, plus its extraction rules into the Extraction sub-tab.

### Authorization

A sub-tab with a None/Basic/Bearer selector. Basic takes Username/Password; Bearer takes a single Token. Either can reference `{{variable}}` tokens, resolved the same way as the URL/headers/body when you send.

### History and re-running a request

Clicking a history entry (sidebar History tab, or the Response pane's own History sub-tab — see below) loads it back into the editor. A history entry is already fully resolved (`{{variable}}` substitution happened at send time), so there's nothing to pick an environment for first. Credential-looking headers and Basic/Bearer credentials are never stored in history at all (NFR-4) — those come back blank/dropped, same as the CLI's own redaction. The reloaded request isn't tied to any saved collection entry, so Save falls back to Save As.

### Environments and `{{variable}}` substitution

Sending resolves `{{variable}}` tokens using the same precedence `curly run` uses (low to high): the always-merged-in `global` environment (if one exists), the selected environment, that environment's session (written by extraction rules on an earlier send), then this tab's own ad-hoc overrides (see below). An undefined variable fails the send outright, naming every undefined token, never silently going out as a literal `{{token}}`.

Two separate ways to pick/edit an environment:

- The **sidebar's Environments list** (or its "+ New" field) both selects an environment *and* navigates to the **Environment** main tab to edit it.
- The **tab strip's "Active environment" dropdown**, next to the permanent Environment tab entry, only changes which environment is active for substitution — it does not navigate anywhere, so you can quickly switch environments while staying on whatever request you're editing.

### The Environment tab

Always present in the tab strip (no `✕` — it can't be closed). Shows "Environment: `<name>`" with an editable KEY/value/secret table, **+ Add variable**, **Save**, and **Delete environment**. Nothing is written to disk until you click Save. All of this reads/writes the exact same `.curly/environments/*.json` files the CLI does — a variable added here is visible to `curly env show` immediately, and vice versa.

### Ad-hoc variable overrides (Variables sub-tab)

Enabled-checkbox / KEY / value rows, highest precedence of anything in the merge above — the GUI's equivalent of `curly run --var key=value`. **Never saved anywhere** (not part of dirty-tracking either): closing a tab or restarting the app loses them, same as a CLI `--var` only ever applying to one invocation. Use them for a one-off value you don't want to save into a real environment.

### Extraction and sessions

The Extraction sub-tab lets you author rules directly: a Body/Header/Template selector, a Name field (the session variable to write into), a second field whose label follows the selector, and a secret checkbox (masks the value in `curly session show`). On a 2xx response, rules run and the results are written into the session for whichever environment is active (or `"global"` if none is) — the response includes what got extracted, shown as a notice.

### Saving, renaming, and starting a new request (FR-20)

Above the URL row, a **breadcrumb** shows `<collection> › <folder> › <editable name>` if the tab mirrors a saved request, or a placeholder otherwise. Editing the trailing name and clicking the small **✓** next to it renames the request in place — it doesn't touch the request's content, only its name/location (use plain Save for content).

Below the URL row, two green **Save**/**Save As…** buttons:

- **Save** — if something's loaded, overwrites it in place (same identity — its id doesn't change); if nothing's loaded, falls back to Save As.
- **Save As…** — Collection (existing or new)/Folder (optional)/Name dialog. A name that collides with an existing request shows an error rather than silently overwriting.

To start a blank request, open a new tab (**+**) — there's no separate "New" button; a new tab already gives you a clean slate.

Every tab with unsaved changes shows a leading **`* `** in its label — derived by comparing the live editor fields against a snapshot taken at the last load/save, so it can't drift out of sync with reality. Saving (either button) clears it.

### Collection and Folder tabs

Clicking a collection's or a folder's **name** in the sidebar tree (not its ⋯, not its arrow) opens it as its own main tab, alongside request tabs:

- A **Collection tab** lists its top-level folders/requests, plus New Folder/New Request/Save All.
- A **Folder tab** lists its own sub-folders/requests, plus New Folder/New Request scoped to it.

Clicking the same collection/folder again just focuses the existing tab rather than opening a duplicate. Each has its own `✕` to close it.

### Sending a request

Method dropdown + URL field + **Send** (or press Enter in the URL field). Sending, appending to history, applying extraction, and writing the session all happen as one round trip on the Rust side — the window stays responsive throughout.

### Reading a response

Three sub-tabs:

- **Body** — pretty-printed automatically if the response is JSON, with syntax coloring (strings/numbers/keywords/punctuation in different colors, readable in both light and dark). The **search box** above it highlights every case-insensitive match; once there's at least one, ◀/▶ step through them with a "match X of Y" label, scrolling the current one into view.
- **Headers** — the raw response headers, one per line.
- **History** — every past send of *this exact request* (same method+URL as the active tab), newest first, filtered from the same project-wide history the sidebar shows in full. Click an entry to reload it. Updates live if a new send lands while this sub-tab is already open.

## What's not there yet

- **A frontend automated test suite** (Vitest/Playwright) — this milestone's TypeScript side was verified by `tsc --noEmit` plus manual exercise of every feature in the running window, not automated tests. `curly-core`/`curly-cli` keep their existing `cargo test` coverage untouched. (The parked `curly-gui` kept its own until it was deleted on 2026-11-09 -- see DESIGN.md's M8 entry.)
- **Multiple simultaneously-open projects** (M7 — see `DESIGN.md` §9) — unaffected by this rewrite; still one project open at a time.
- **A GUI scenario editor** — scenarios (`curly scenario`) stay CLI/JSON-file-only, same as before this rewrite.
- Loading a form/multipart/binary body into the editor isn't supported (same limitation the egui GUI had).

## A note on this guide's own testing

Every feature described above was manually exercised by the user in the actual running dev-mode window during the session this was built in, checkpoint by checkpoint as each slice landed — including one real bug they caught and got fixed same-session (the Response History sub-tab not live-refreshing when a send completed while it was already open). Nothing here is describing untested code. What *hasn't* happened yet: a packaged/production build (`npm run tauri build`) has never been run, and there's no automated test suite for the frontend — see "What's not there yet" above.
