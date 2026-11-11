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
| `gui/` | `curly gui` — a desktop window, built on ziman's `lib/ui`, talking to nothing but the network |

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

A desktop REST client in a window: the saved requests on the left, method and URL across the top,
headers, body and environment below them, the response under that, Send and Save on the right.

| | |
|---|---|
| **Edit** | method, URL, four headers, a raw body, the environment to resolve `{{variables}}` against |
| **Send** | `core/exec.send` in this process — history written, extractions applied, exactly as `curly ui` does them |
| **Save** | `replace_request` back into the collection, **keeping the request's position** |

Only a raw body is editable: a binary, form or multipart body is shown as what it is and left
alone, rather than flattened into something that would save back wrong. Headers grow and shrink --
`+ header` adds a row and each row's `x` removes it -- and the environment is picked from the ones
on disk, with `(global)` meaning the absence of one rather than an environment named that.

**It depends on no server.** `curly serve` exists so the React frontend can reach the same `.curly`
storage over HTTP; this does not use it. The same collections are read from the same disk, variables
are merged the same way, and `core/exec.send` runs in this process — so there is no port to choose,
nothing to start first, and the window is as offline as `curly run` is. History is written and
extractions are applied exactly as the terminal runner does them.

**It needs a ziman with a window backend.** `lib/ui/desktop/window.zm` keeps X11 behind a compile
flag so an ordinary `ziman` does not depend on a display, so the natives live only in the build
`make gui` produces. `bin/curly` picks `ziman-gui` up automatically when it is on PATH; otherwise
`ZIMAN=/path/to/ziman-gui curly gui`. Run without one it says so, and says what to do.

**The font** comes from `CURLY_FONT` if set, then a copy vendored in `vendor/fonts/`, then a short
list of system paths — `lib/graphics` has no notion of a system font, so something has to hand it
bytes, and an application that asked the user to find a TrueType file before it would open is not
standalone.

**A screen reader can read it.** The window publishes its accessibility tree over AT-SPI every time
something changes and pumps the bridge every frame, so Orca sees the request list, the fields, the
buttons, the combo box and which control has focus. A ziman without the backend, or a session with
no accessibility bus, turns it off and says so once in the status line rather than failing -- a
window that cannot publish is still a window.

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
