# The Ziman implementation

Curly is being reimplemented in [Ziman](https://ziman.dev). The Rust implementation in `rust/`
stays as the **reference** until the port passes parity; the React frontend in `curly-gui-tauri/`
is **shared and is not being rewritten**.

What that means in practice is that nothing here is finished because its own tests pass. `parity.sh`
asks both implementations the same questions over the same `.curly/` tree and diffs the answers, and
that is what makes "replaces" a claim a test can fail on.

## Running it

The Rust implementation is one native binary. This one is a Ziman program, so it needs the
interpreter — `ziman/bin/curly` wraps that:

```sh
ZIMAN=/path/to/ziman ziman/bin/curly --help
ZIMAN=/path/to/ziman ziman/bin/curly collections list
```

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

Two commands exist here and not in the Rust CLI, and both are deliberate rather than drift:
`curly ui`, because a terminal interface is this implementation's own, and `curly serve`, because
the Rust frontend is a Tauri desktop shell that linked `curly-core` into the app directly — there
was nothing to serve. `parity.sh` records both where it would otherwise look like a gap.

## `curly serve`

```sh
ziman/bin/curly serve --port 8787 --web-root curly-gui-tauri/dist
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

**Done.** `curly-gui-tauri/src/api.ts` calls the server, not Tauri: the `invoke` import became a
`post` helper of the same shape, and all 21 wrappers below it are unchanged, because the server kept
`invoke`'s shape precisely so they could be. `vite.config.ts` proxies `/api` (set `CURLY_PORT` to
match a `curly serve --port`).

`saveBytes` is the exception — no endpoint, so it is now a **download**: a `Blob` and a synthesised
`<a download>`, still byte-exact from `body_bytes`. Its first argument is therefore a *name* rather
than a path, and `ResponsePane.tsx` no longer opens a native save dialog to produce one.

**One Tauri dependency is left, and it is a product decision rather than an oversight**: `App.tsx`'s
"Open Project…" uses `@tauri-apps/plugin-dialog`'s folder picker. A browser has no equivalent that
yields an absolute path — `<input webkitdirectory>` gives paths relative to the chosen directory —
so the honest options are a text field for a path (the app's existing `PathDialog` is the pattern),
or dropping the in-app switcher and letting `curly serve --data-dir` decide. The `open_project`
command itself works over HTTP; only the picker does not.

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
