# The Ziman implementation

Curly is being reimplemented in [Ziman](https://ziman.dev). The Rust implementation in `rust/`
stays as the **reference** until the port passes parity; the React frontend in `curly-gui-web/`
is **shared and is not being rewritten**.

What that means in practice is that nothing here is finished because its own tests pass. `parity.sh`
asks both implementations the same questions over the same `.curly/` tree and diffs the answers, and
that is what makes "replaces" a claim a test can fail on.

## Running it

The Rust implementation is one native binary. This one is a Ziman program, so it needs the
interpreter — `ziman/bin/curly` wraps that:

```sh
ziman/bin/curly --help
ziman/bin/curly collections list
```

That is all, from any directory, as long as `ziman` is on `PATH`. If it is not, or you want a
particular build, `ZIMAN=/path/to/ziman ziman/bin/curly ...` overrides it. `ZIMAN_MODULE_PATH` is
**not** needed — the interpreter resolves `use core/...` relative to the main script — although the
test runners set it because they run the modules from elsewhere.

The wrapper is not a convenience. `ziman script.zm args` is rejected: the invocation is
`ziman curly.zm -- <args>`, and `os.args()` answers only what follows the `--`. A shebang cannot
hide it either, because the kernel appends the script path *after* the interpreter's own arguments,
so the `--` would land in the wrong place.

**Global flags go after the subcommand**: `curly serve --data-dir DIR --port N`, not
`curly --data-dir DIR serve`. A leading flag makes the first word not a subcommand, which is how
one-shot mode is recognised (`curly https://example.com/` has to work), so `--data-dir` before the
subcommand is read as a one-shot flag and refused.

## What is built

| | |
|---|---|
| `core/` | model, substitution, saved (+ resolve, into_request), exec, extraction, storage, collection, scenario, scenario_run, the Postman and curl importers |
| `cli/` | every subcommand: `init`, `env`, `collections`, `scenario`, `session`, `history`, `run`, one-shot mode |
| `tui/` | `curly ui` — a terminal runner: browse the saved tree, send a request, read the response |
| `server/` | `curly serve` — the HTTP API the React frontend talks to |
| `gui/state.zm` | what the window *is*: every type it holds, and `step`, which takes a state and an input and answers a new one. Pure, and tested |
| `gui/app.zm` | the half that cannot be pure: the window, the font, the filesystem, the network |

Three commands exist here and not in the Rust CLI, and all three are deliberate rather than drift:
`curly ui`, because a terminal interface is this implementation's own; `curly serve`, because the
Rust frontend was a Tauri desktop shell that linked `curly-core` into the app directly — there was
nothing to serve (that shell was deleted on 2026-11-11, and `curly serve` is now how the React
frontend reaches storage at all); and `curly gui`, which **means the opposite of what it means in the Rust CLI**.
There it prints "the graphical interface is a separate application", left behind when the egui crate
was deleted in favour of the Tauri app. Here it opens a window.

`parity.sh` records all three where they would otherwise look like a gap — and `gui` is the only one
where the two implementations answer the *same* command differently, so it is asserted as a
divergence rather than skipped: the Rust side must still print its sentence, and the Ziman side must
not.

## `curly gui`

A desktop REST client in a window: the saved requests in a tree on the left, method and URL across
the top, the editors below them, the response under that, Send and Save on the right.

| | |
|---|---|
| **Edit** | method, URL, headers, query parameters, authorization, extraction rules, a raw body, the environment to resolve `{{variables}}` against |
| **Send** | `core/exec.send` in this process — history written, extractions applied, exactly as `curly ui` does them |
| **Save** | `replace_request` back into the collection, **keeping the request's position** |
| **Save As** | a modal dialog naming a collection and a path, then `add_request`, which **refuses a path already taken** |
| **History** | every past exchange on disk, read back into the same response pane |
| **Browse** | collections and folders as a tree, with the keyboard contract a tree owes |
| **Filter** | a box above the tree narrowing it by path, so a collection of hundreds is navigable |
| **Scroll** | the tree and the response both show a bar saying there is more and where you are |

Save As is how a request is *created*: until it existed this window could only edit what was
already on disk, so a fresh install had an empty sidebar and no way forward. A collection that does
not exist yet is created with it, as `curly collections create` would -- refusing there would mean
leaving the window to run a command and coming back.

Only a raw body is editable: a binary, form or multipart body is shown as what it is and left
alone, rather than flattened into something that would save back wrong. The environment is picked
from the ones on disk, with `(global)` meaning the absence of one rather than an environment named
that.

**The sidebar is a tree** (ziman F282, written for this), where it was a flat list of
`collection/path` strings -- the storage has folders and the window did not show them. Branches
open and close with their mark or with Left and Right, Right steps *into* an open branch, and Left
from a request climbs to the folder holding it. Its shape survives a save: the reload matches rows
**by path, not by index**, because saving under a new name adds a row and moves every index below
it, so carrying the numbers over would reopen the wrong folders and quietly select a different
request.

**A filter box narrows it.** The query matches the whole path rather than a row's own label, so
typing a folder's name shows what is in it and typing a collection's shows all of it; a match
keeps the folders above it, or what is left would be orphans at depths that mean nothing; and a
collection or folder with nothing left in it goes too, rather than standing as an empty heading.
The sidebar is rebuilt per query rather than flagged per row -- the rows, the items, the map back
to a request and the keys are four arrays that have to agree, and filtering them in place is four
chances to disagree. A save while a filter is on re-derives through the query, or it would quietly
widen the sidebar back to everything.

**The flat list had been labelling nested requests wrong and nobody had noticed.** A request
directly in a collection read `demo/missing` while one inside a folder read `local/index` -- the
folder and the name, with the collection missing -- so two collections with a folder of the same
name were indistinguishable in the sidebar. The tree shows where a request is, and `Item.label`
turned out to have no other reader and is gone.

**The editors share a tab strip** -- Headers, Query, Body -- because stacked between the URL and the
response they left the response a sliver, and a request with eight headers pushed it off the window
entirely. **Headers and query parameters are one set of functions**, not two copies of it: both are
a list of `KvPair`, so a table takes the control-name prefix as an argument, which is what keeps
them two tables rather than one (a control is addressed by name, so sharing a prefix would mean
typing in one typed in the other). Each grows and shrinks -- `+ header` or `+ parameter` adds a row,
each row's `x` removes it, and removing one renames the rows below it because their names are their
positions.

**Authorization** is its own tab: a picker of None, Basic and Bearer, and the fields the picked
kind needs. The fields the other kinds need are **not drawn**, rather than drawn disabled -- a
Bearer token has no username, and a greyed-out box labelled Username invites a user to wonder what
it would do; not drawing it also keeps it out of the tab order and out of the accessibility tree,
which "disabled" does not. "None" is one of the three choices rather than an empty state, for the
same reason `(global)` is in the environment picker: a chooser with no way back to the absence of a
value is a trap.

**The secret is a masked field**, and this window is why ziman's `lib/ui` has one (F281). It is
bullets on the screen *and* bullets on the accessibility bus -- read off the real bus, the node is
`password text: "Password" = "••••••••"`, and the password itself appears nowhere in the tree. The
one field is relabelled Token for a Bearer and keeps what was typed into it: losing a secret to a
mis-click in a picker is worse than carrying one across, and either way it is a secret.

**Extraction rules** are the fifth tab: one row per rule, holding its kind (Body, Header or
Template), the variable it defines, where the value comes from, a Secret tick and an `x`. A row
with no variable name is not a rule, which is how one is deleted without the `x`.

Not a pair table with more columns, though it looks like one: a pair is two strings, while a rule
is a tagged union whose third field *means* something different in each variant -- a JSON path, a
header name, a template -- so the row has to say which. **It says so on the screen and not only to
a reader**: each row draws the word beside the box, because every row picks its own kind and a
column header could not name a third column that is a path on one line and a header name on the
next. Declaring it only to the accessibility tree would have left a sighted user looking at three
empty boxes while the screen reader was better informed than the screen.

A rule is applied by the same `core/exec.send` the terminal uses, so a send from this window writes
the session exactly as `curly run` does -- verified by sending against a server answering
`{"data": {"token": "s3cr3t"}}` and reading `token` back out of `session/global.json` with its
`secret` flag set.

Every field of a saved request is now editable here. What is still carried through a save untouched
is anything a future field adds: `edited_request_from` builds on the request it loaded, so a field
with no editor keeps what was on disk rather than being written back empty.

**It depends on no server.** `curly serve` exists so the React frontend can reach the same `.curly`
storage over HTTP; this does not use it. The same collections are read from the same disk, variables
are merged the same way, and `core/exec.send` runs in this process — so there is no port to choose,
nothing to start first, and the window is as offline as `curly run` is. History is written and
extractions are applied exactly as the terminal runner does them.

**It needs a ziman with a window backend.** `lib/ui/desktop/window.zm` keeps X11 behind a compile
flag so an ordinary `ziman` does not depend on a display, so the natives live only in the build
`make gui` produces. `bin/curly` picks `ziman-gui` up automatically when it is on PATH; otherwise
`ZIMAN=/path/to/ziman-gui curly gui`. Run without one it says so, and says what to do.

**It draws at the display's own resolution.** How many device pixels a point is comes from
`CURLY_SCALE` if it is set, then from what the display reports, then 1.0 -- the order
`widget.resolve_density` arbitrates, and the reason the user's setting comes first is that a
display's number is a guess about someone's eyes and their desk from the pixels on a panel. A
`CURLY_SCALE` that is not a number, or is outside `(0, 64]`, is treated as absent rather than
refused: a mistyped setting should leave a usable window, not none.

This was hard-coded to 1.0 until 2026-11-12, on a machine whose display had been reporting 2.0 the
whole time -- so every window the application had ever opened was drawn at half the resolution of
the screen it was on. It looks like a soft font rather than like a bug, which is why it survived
so long. Two traps are worth knowing for anything else built on `lib/ui`: the scale has to reach
**`raster.Options.px_per_pt`**, because `desktop.repaint` rebuilds the painter from the options
whenever the two disagree and a scale set only on `painter_of` is discarded on the first frame;
and the window is opened in **pixels** while everything inside it is laid out in **points**, so a
resize has to be divided back down.

**The font** comes from `CURLY_FONT` if set, then a copy vendored in `vendor/fonts/`, then a short
list of system paths — `lib/graphics` has no notion of a system font, so something has to hand it
bytes, and an application that asked the user to find a TrueType file before it would open is not
standalone.

The response is a **read-only text area**, not a column of labels: Tab reaches it, the arrows read
through it, and a screen reader can be *on* it and read it out. Labels cannot be focused, so before
this the response was unreachable without a mouse.

**History shares that pane**, under a **Response**/**History** tab strip. The strip did not exist
when this needed one -- `widget.Role` had `Tab` and `TabList` since accessibility was built, and no
control made them -- so it shipped as two buttons for an afternoon and `lib/ui` gained
`controls.Tabs` (ziman F280). A screen reader now hears "page tab list, Panes" with "page tab,
Response, selected", which two buttons could never have said. The list is read from disk when the
tab is reached and not every frame, because history is written by sends and this window knows when
it sent. Moving the selection loads the exchange behind the list; Enter, or the Response tab, is
what leaves the list to go and read it -- the requests list on the left switches on either, because
it stays on screen either way, but this list *is* the pane, so switching on a move would throw a
keyboard user out of it on their first Down. A past exchange is rendered by the same text area a
fresh one is, so there is one place a response is read and one way it looks.

**A screen reader can read it.** The window publishes its accessibility tree over AT-SPI every time
something changes and pumps the bridge every frame, so Orca sees the request list, the fields, the
buttons, the combo box and which control has focus. A ziman without the backend, or a session with
no accessibility bus, turns it off and says so once in the status line rather than failing -- a
window that cannot publish is still a window.

**The window is two modules, and the line between them is testability.** `gui/state.zm` holds
every type and every function that turns one value into another -- `step(state, input, laid,
style, metrics)` answers a new state and `view(state, ...)` answers a widget tree -- while
`gui/app.zm` holds the window, the font, the disk and the network. That is the split `tui/`
already has one directory over (`tui/app.zm` reads keys; `tui/rows.zm` and `tui/render.zm` take
values and answer values), and it was arrived at here the same way: there was no way to test a
thousand lines of new window behaviour without it. `src/test/unit/gui_test.zm` lays the **real**
`view` out at a window-sized box and asserts what a gesture does -- the font is the only thing
stubbed.

It is also **the first application built on ziman's `lib/ui`**. `curly ui` predates that layer and
has its own rows-and-cells renderer; every part of the widget stack was until now exercised only by
its own fixtures.

**Four defects in that layer were found here rather than by a wishlist**, which is the argument for
having an application at all, and all four are closed:

1. **No margins.** Nothing had noticed `lib/ui` drew to the edges of its own box, because every
   example did. A window has glass.
2. **No multi-line text.** The request body had to be one line. `controls.text_area_of` is the
   answer, and `edit` needed only line-aware Home/End plus an up and a down to get there -- a
   newline had always been an ordinary character in its string.
3. **A value picker announced itself as a menu.** There was no `ComboBox` role, so a reader would
   have called the environment chooser a list of commands. `controls.choice_of` is a `Menu` that
   announces a value.
4. **Choosing from a menu with a mouse did not work at all.** A press on a row moved the selection
   and nothing turned it into a choice, so a menu could be opened, an item clicked, and the menu
   would sit there with the item highlighted. Every test in that module chose with Enter, so the
   mouse path was uncovered. A release over a row chooses it now.

A fifth was reported here and **was not a gap**: growable rows need no control. The application owns
the array, `push` adds a row and a `Button` removes one. The four fixed header rows were a shortcut,
not a limitation.

## The design files, and the builder that reads them

`curly gui` is not one module per screen but two: `gui/state.zm` is the logic and `gui/request_ui.zm`
is the layout. The layout file is **Ziman, restricted** -- not XML, not JSON, not a brace DSL -- and
the restriction is the whole point: full Ziman needs a Ziman parser, while the subset is about as
hard to read as JSON. A tool can parse it, draw it, map a click on the drawing back into it and
rewrite one node of it without touching the rest.

The dialect, in full: one expression per function; calls to `widget.*`, `controls.*` and sibling
functions in the same file; literals; field paths off a parameter; module-level constants, which may
do arithmetic over other constants. No `if`, no loops, no `switch`, no arithmetic in a body, no
function-local `val`. One root called `screen`; every other `pub fn` is a part it calls. A design
file **declares its own input struct** rather than importing the application's -- the logic module
has to `use` the design module to draw itself, and ziman refuses the cycle, so the dependency runs
outwards and the cycle cannot form.

What the dialect refuses, it refuses on purpose, and twice the refusal found a mistake rather than
a cost. Repetition is a row template plus a list the caller fills, so the loop stays in the logic
file; *which* children appear is the same mechanism, so the dialect needs no conditional. A slot
briefly grew a size parameter and then lost it again: both hand-computed heights turned out to be
spacing written in the wrong place.

### The four tools, in `ziman/tools/`

| | |
|---|---|
| `lint_design.zm` | the dialect, enforced. Textual, and sound only because `--fmt` runs first |
| `read_design.zm` | the text as a tree. Every node carries **the span it came from** |
| `render_design.zm` | the tree as a window, with a placeholder invented for every input |
| `pick_design.zm` | a point on that window as a node, and `--set` to rewrite that node in place |

`read_design` keeps spans because **regenerating a design file from its tree is the wrong way to
save**, which the round trip measured rather than guessed: 7,701 of 16,079 bytes are comments and a
regenerating editor deletes them; `editor_strip_width = tab_width * editor_tabs` comes back as
`500.0`, so changing `tab_width` stops changing the strip; and `pub val` comes back `val`, so the
file does not compile. An editor rewrites the span one node occupies and leaves every other byte
alone. Replacing `heading_height` with `44.0` is a one-line diff with all 96 comments intact.

`pick_design` maps a click back **by name, not by a parallel tree**: `widget.layout` passes a
parent's name down to every child that has none, so naming a node names everything inside it, and
ziman's `widget.chain_at` hands back the whole descent under a point so the lookup can stop at the
innermost node the *file* is responsible for -- a click on a tree row belongs to the tree, because
the row was invented by `controls` while drawing and the file never wrote one.

### Five gates, in `run-all.sh`

Formatting, the dialect (with a fixture proving the lint bites), the parse, the round trip, and
**click to source**: every control in every component is clicked in its own middle, and has to come
back as a `controls.` call that names it *and* survive being rewritten there. The `controls.` half
is load-bearing -- without it a mapping that answered "the component's root" to every point passed,
because a root's source contains every name in the component. Mutation is what said so.

The layout itself is held by a committed baseline, `src/test/resources/window_geometry.txt`: every
named control's bounds in eight states of the window. **The rest of the suite cannot see a control
move**, because every test finds one with `bounds_of(name)` and presses inside it; a change that
shifted the pane switcher twelve points left all 263 of them green.

## `curly serve`

```sh
ziman/bin/curly serve --port 8787 --web-root curly-gui-web/dist
```

| flag | |
|---|---|
| `--port` | default 8787; `0` asks the OS for any free port |
| `--host` | default `127.0.0.1` |
| `--web-root` | the built frontend (`npm run build`'s `dist/`). Omit it to serve the API alone |

Every operation is `POST /api/<command>` carrying the same JSON object of named arguments that
Tauri's `invoke(name, args)` already sends, and answering the serde JSON of the return value. An
argument keeps its **JavaScript** spelling, so `envName` and not `env_name`: Tauri converts a
camelCase key to its snake_case Rust parameter and there is no such layer here, so the wire name is
what `api.ts` already writes.

It is command-shaped and not REST on purpose. Nothing but curly's own frontend will ever call it, it
is not a public contract, and keeping `invoke`'s shape means the frontend's move off the desktop
shell is one function rather than 26 new URLs to design and then get wrong.

A refused command answers **500 with the message as the whole body**, and the status carries no
further meaning. Every one of these operations is `Result<T, String>` on the Rust side — the bridge
has no error kinds, so there is nothing to map onto 404 and 409 but guesswork, and a frontend that
branched on a status this port invented would be coupled to that guess. The message is the answer,
exactly as it is through Tauri, where a rejected `invoke` carries a plain string.

### Two bridge commands have no server equivalent

- **`save_bytes`** writes bytes to a path a native save dialog picked. On a server that is a
  straightforward hole, and in a browser the operation is a download — a `Blob` and an
  `<a download>`, which is the frontend's job.
- **`open_project`** is implemented, but `resolve_default_project` deliberately **reports the root
  the server was started with** rather than re-resolving one. The bridge re-resolves because a
  desktop app has nowhere else to look; this server was handed a root on its command line, and
  `curly`'s own resolution (explicit `--data-dir`, then `CURLY_DATA_DIR`, then the nearest `.curly`
  above the working directory, then the OS default) already ran once, at the right time, in a
  process whose working directory is the one the user chose.

### Reaching it from the frontend

**In development, proxy it — do not enable CORS.** Add to `vite.config.ts`:

```ts
server: { proxy: { "/api": "http://127.0.0.1:8787" } }
```

Then Vite serves the frontend and `/api` arrives from the dev server's own origin. This is the same
arrangement health-record's React frontend uses to reach its Ziman backend.

The reason is not style. These commands read and write the user's files and send requests from
their machine, and the Tauri IPC they replace could not be addressed by a web page at all — so an
HTTP server on the same machine is new risk that the port itself created. Four things answer it:

- the listener binds `127.0.0.1`, not `0.0.0.0`, so nothing off the machine reaches it;
- a command must be `POST` with `Content-Type: application/json`, which is not a CORS *simple
  request* and so cannot be sent cross-origin without a preflight;
- no CORS headers are written and `OPTIONS` is not answered, so that preflight fails;
- an `Origin` header that does not match the request's own `Host` is refused.

Each alone is weak; together they mean a page at `evil.example` cannot reach these commands even
knowing the port. Turning CORS on instead would hand every open tab a file-writing API.

**`Origin` is compared with `Host`, not merely required to be absent** — and the simpler rule was
tried first and is wrong in the direction that breaks everything. A browser appends `Origin` to a
**same-origin** request whose method is not GET or HEAD (Fetch, "append a request `Origin` header"),
so every command the frontend sends carries one, and refusing the header outright refused the whole
frontend. A cross-origin header is what the tests sent and plain `curl` sends none at all, so
nothing caught it until a config question did.

That is also why **the proxy must not rewrite `Host`**: Vite's `changeOrigin` defaults to false and
has to stay that way, or the browser's `Origin` and the forwarded `Host` disagree and every command
is refused.

### The frontend

**Done.** `curly-gui-web/src/api.ts` calls the server, not Tauri: the `invoke` import became a
`post` helper of the same shape, and all 21 wrappers below it are unchanged, because the server kept
`invoke`'s shape precisely so they could be. `vite.config.ts` proxies `/api` (set `CURLY_PORT` to
match a `curly serve --port`).

`saveBytes` is the exception — no endpoint, so it is now a **download**: a `Blob` and a synthesised
`<a download>`, still byte-exact from `body_bytes`. Its first argument is therefore a *name* rather
than a path, and `ResponsePane.tsx` no longer opens a native save dialog to produce one.

**The last Tauri dependency is gone, and the shell with it (2026-11-11).** `App.tsx`'s
"Open Project…" used `@tauri-apps/plugin-dialog`'s folder picker, which a browser has no equivalent
for: the File System Access API answers an opaque handle and `<input webkitdirectory>` gives paths
relative to the chosen directory, neither of which is the absolute path `open_project` needs. Of the
two honest options recorded here — a text field, or dropping the switcher and letting
`curly serve --data-dir` decide — the first was taken: `OpenProjectDialog` asks for the path, which
the server then opens. The server runs on the same machine as the files, so this is less convenient
and no less capable.

With that, nothing reached Tauri and `curly-gui-web/src-tauri/` was deleted — 18 tracked files, a
Rust crate whose `project.rs` the Ziman server had already reimplemented, and four `@tauri-apps`
npm dependencies. **The directory is still called `curly-gui-web` and no longer contains any
Tauri**, which is a rename waiting to happen rather than a statement about the code.

curly's own mark survived the shell: it was the Tauri bundler's icon source and is now the web app's
favicon (`public/curly.svg`), which is its third home — the egui GUI had it first.

## The tests

```sh
./run-all.sh        # both implementations, plus the cross-check
./parity.sh         # the cross-check alone
```

- `ziman/src/test/unit/*.zm` — the pure and storage-level behaviour, including every `server/api`
  command as a function, which needs no socket.
- `ziman/src/test/other/*_test.py` — what only a real socket shows: `core/exec` against a stub
  server, scenario runs, and `curly serve` end to end (a real send writing history and the session,
  the static layer, the cross-origin guards, and a path that tries to climb out of the web root).
- `parity.sh` — the Rust binary and this one, asked the same questions and diffed. Including
  `cargo run --example wire_shapes -p curly-core`, which prints serde's JSON for the shapes the web
  API carries that exist nowhere on disk: a `Duration` is `{"secs","nanos"}`, a `Vec<u8>` is an
  array of numbers, and an enum with no serde attributes is externally tagged. The frontend's
  `types.ts` asserts all three, but it is hand-written and so a second-hand account.
