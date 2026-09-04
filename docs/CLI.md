# Curly CLI — User Guide

`curly` is the command-line face of the Curly project: a curl-like tool for sending HTTP requests from a terminal. This guide covers everything the CLI currently supports. If you already know curl, the flags will feel familiar on purpose.

> Status: §1–5 cover one-shot mode (send a single request, see the response). §6–7 cover saved requests, collections, environments, and history. Postman/curl import and OAuth2 are planned but not implemented yet — see [DESIGN.md](DESIGN.md) for the roadmap.

## 1. Installing / Running

Curly isn't published to a package registry yet. For now, build it from source.

**Requirements:** [Rust and Cargo](https://www.rust-lang.org/tools/install) (any recent stable toolchain).

```sh
git clone git@github.com:vasnatech/curly.git
cd curly
cargo build --release
```

The compiled binary is at `target/release/curly` (`target\release\curly.exe` on Windows). Either run it by full path, or copy/symlink it somewhere on your `PATH` (e.g. `~/.local/bin` on Linux/macOS) so you can just type `curly`.

While developing, you can also run it directly without a separate build step:

```sh
cargo run -- <curly arguments here>
```

(Note the `--`: everything after it is passed to `curly` itself, not to `cargo`.)

## 2. Basic Usage

```
curly [OPTIONS] <URL>
```

`<URL>` is the only required argument. With no other flags, this sends a `GET` request and prints the response body to stdout:

```sh
curly https://httpbin.org/get
```

## 3. Common Examples

**GET a URL, see just the body:**
```sh
curly https://httpbin.org/get
```

**GET, but also show the status line and response headers:**
```sh
curly -i https://httpbin.org/get
```

**Add query parameters without hand-building the URL:**
```sh
curly -Q "id=42" -Q "active=true" https://httpbin.org/get
```

**POST a JSON body** (method defaults to `POST` automatically whenever a body flag like `-d` is used):
```sh
curly -H "Content-Type: application/json" -d '{"name":"a"}' https://httpbin.org/post
```

**POST a URL-encoded form** (each `--data-urlencode` adds one field; `Content-Type` is set automatically):
```sh
curly --data-urlencode "name=a b" --data-urlencode "id=1" https://httpbin.org/post
```

**POST a multipart form, including a file upload** (`name=value` for text, `name=@path` to upload a file):
```sh
curly -F "name=alice" -F "avatar=@photo.png" https://httpbin.org/post
```

**Send the raw contents of a file as the body** (e.g. binary data, or a large payload you don't want to inline):
```sh
curly --data-binary payload.bin https://httpbin.org/post
```

**Explicit method** (needed for methods that don't imply a body, like `DELETE`):
```sh
curly -X DELETE https://httpbin.org/delete
```

**Basic auth:**
```sh
curly -u "alice:s3cret" https://httpbin.org/basic-auth/alice/s3cret
```

**Bearer token auth:**
```sh
curly --bearer "eyJhbGciOi..." https://httpbin.org/bearer
```

**Multiple headers** (repeat `-H` as many times as you need — also how you'd send an API key expected in a header):
```sh
curly -H "Content-Type: application/json" -H "X-Api-Key: abc123" https://httpbin.org/get
```

**Save the response body to a file instead of printing it:**
```sh
curly -o response.json https://httpbin.org/get
```

**Pretty-print a JSON response body:**
```sh
curly -p https://httpbin.org/get
```

**Get the whole response — status, headers, body, timing — as one JSON object, for scripting:**
```sh
curly --json https://httpbin.org/get | jq .status
```

**Follow redirects** (off by default, matching curl rather than a browser):
```sh
curly -L https://httpbin.org/redirect/1
```

**See exactly what was sent and received** (request/response trace goes to stderr, so it doesn't pollute piped output):
```sh
curly -v https://httpbin.org/get > body-only.json
```

**Treat an HTTP error status as a failure** (useful in scripts/CI — see §5 Exit Codes):
```sh
curly --fail https://httpbin.org/status/404 || echo "request failed"
```

**Abort a slow request:**
```sh
curly --max-time 5 https://httpbin.org/delay/10
```

**Route through a proxy:**
```sh
curly -x http://localhost:8080 https://httpbin.org/get
```

**Skip TLS certificate verification** (self-signed certs on a local/dev server — avoid on anything else):
```sh
curly -k https://localhost:8443/api/health
```

**Trust a specific self-signed CA instead of disabling verification entirely** (safer than `-k` when you control the CA):
```sh
curly --cacert ./dev-ca.crt https://localhost:8443/api/health
```

**Mutual TLS (present a client certificate):**
```sh
curly --cacert ./dev-ca.crt --cert ./client.crt --key ./client.key https://localhost:8443/api/health
```

## 4. Argument Reference

### `<URL>` (required, positional)

The full URL to request, including scheme (`http://`/`https://`). Query parameters can be appended directly to the URL, or added via `-Q` (see below) — either works; `-Q` is easier to compose across a script since you don't have to worry about `&`/`?` quoting yourself.

### `-X, --request <METHOD>`

Sets the HTTP method (`GET`, `POST`, `PUT`, `PATCH`, `DELETE`, `HEAD`, `OPTIONS`, or any other verb the server accepts). Case-insensitive on input.

- If omitted **and** a body flag is given (`-d`, `--data-urlencode`, `--data-binary`, or `-F`), the method defaults to `POST` (matches curl's behavior).
- If omitted and there's no body flag, it defaults to `GET`.

```sh
curly -X PUT -d '{"status":"done"}' https://httpbin.org/put
```

### `-H, --header <"Name: value">`

Adds a request header. Repeatable — pass `-H` once per header. The value must be in `Name: value` form (colon-separated); leading/trailing whitespace around the value is trimmed. Also how you send a header-based API key (`-H "X-Api-Key: ..."`) — there's no separate API-key flag since this already covers it.

```sh
curly -H "Accept: application/json" -H "X-Request-Id: abc123" https://httpbin.org/get
```

### `-Q, --query <"key=value">`

Adds a query parameter, URL-encoded and appended to the request URL. Repeatable.

```sh
curly -Q "id=42" -Q "active=true" https://httpbin.org/get
```

### Body flags: `-d`, `--data-urlencode`, `--data-binary`, `-F`

These four are **mutually exclusive** — use at most one per request (mixing them is an error, since only one body can actually be sent).

#### `-d, --data <DATA>`

Sets the request body to the given string, sent as-is (no encoding/escaping beyond what your shell does). Pair with `-H "Content-Type: ..."` to tell the server what format the body is in — Curly does not infer or set a content type for you with this flag.

```sh
curly -H "Content-Type: application/json" -d '{"id":1}' https://httpbin.org/post
```

To send the contents of a file as JSON, use shell substitution: `curly -H "Content-Type: application/json" -d "$(cat payload.json)" https://httpbin.org/post`.

#### `--data-urlencode <"key=value">`

Adds one `application/x-www-form-urlencoded` field. Repeatable — each occurrence adds another field. The `Content-Type` header is set automatically; don't also set it via `-H`.

```sh
curly --data-urlencode "name=a" --data-urlencode "id=1" https://httpbin.org/post
```

#### `--data-binary <PATH>`

Reads the given file and sends its raw bytes as the body, unmodified — for binary payloads or anything you don't want to pass as a shell string. A leading `@` (out of curl habit) is accepted and stripped, but isn't required — `--data-binary payload.bin` and `--data-binary @payload.bin` do the same thing. *(Unlike curl's `--data-binary`, this always takes a file path — it does not accept a literal data string.)*

```sh
curly --data-binary payload.bin https://httpbin.org/post
```

#### `-F, --form <"name=value" | "name=@path">`

Adds a `multipart/form-data` field. Repeatable. `name=value` sends a text field; `name=@path` uploads the file at `path`, using its filename as the part's filename.

```sh
curly -F "name=alice" -F "avatar=@photo.png" https://httpbin.org/post
```

### `-u, --user <"user:password">`

Sends HTTP Basic authentication. Both parts are required (unlike curl, Curly won't prompt for a missing password — it's non-interactive by design).

```sh
curly -u "alice:s3cret" https://httpbin.org/basic-auth/alice/s3cret
```

### `--bearer <TOKEN>`

Sends `Authorization: Bearer <TOKEN>`. Mutually exclusive with `-u`.

```sh
curly --bearer "eyJhbGciOi..." https://httpbin.org/bearer
```

*(An API key expected in a header or query string doesn't need a dedicated flag — use `-H`/`-Q`. OAuth2 authorization-code/client-credentials flows are planned but not implemented yet.)*

### `-o, --output <PATH>`

Writes the response body (or, with `--json`, the whole JSON object) to the given file path instead of printing it to stdout. Overwrites the file if it already exists. Status/headers printed via `-i` still go to your terminal even when `-o` is set — only the body goes to the file.

```sh
curly -o out.json https://httpbin.org/get
```

### `-i, --include`

Prints the response status line and headers to stdout before the body. Ignored when `--json` is set, since the JSON object already carries status and headers.

```sh
curly -i https://httpbin.org/get
```

### `--json`

Prints one machine-readable JSON object instead of the plain-text output — `{"status": ..., "elapsed_ms": ..., "headers": {...}, "body": "..."}` — handy for piping into `jq` or parsing in a script. Takes precedence over `-i`. `-o` still works, writing the JSON object to the file instead of stdout.

```sh
curly --json https://httpbin.org/get | jq -r .body
```

### `-p, --pretty`

If the response body parses as JSON, pretty-prints it with 2-space indentation. Non-JSON bodies (or malformed JSON) are left unchanged. Ignored when `--json` is set (the `body` field inside `--json`'s output is always the raw response text).

```sh
curly -p https://httpbin.org/get
```

### `-v, --verbose`

Prints a request/response trace — the method, URL, and headers sent, followed by the status, elapsed time, and headers received — to **stderr**. The response body still goes to stdout (or the `-o` file) as usual, so verbose tracing never mixes into output you're piping or redirecting.

```sh
curly -v https://httpbin.org/get > body-only.json
```

### `-k, --insecure`

Disables TLS certificate verification entirely for this request. Use only for local/dev servers with self-signed certificates you don't have the CA for — never against a server you don't control, since it removes protection against man-in-the-middle attacks. Prefer `--cacert` when you do have the CA certificate.

### `--cacert <PEM_PATH>`

Trusts an additional root CA certificate (PEM file) when verifying the server, on top of the normal system trust store. Safer than `-k` for a self-signed dev server, since it only trusts that one CA rather than disabling verification altogether.

```sh
curly --cacert ./dev-ca.crt https://localhost:8443/api/health
```

### `--cert <PEM_PATH>` / `--key <PEM_PATH>`

Present a client certificate for mutual TLS (mTLS). Both are required together — Curly errors if only one is given. `--cert` is the certificate, `--key` is its matching private key, both PEM-encoded.

```sh
curly --cacert ./dev-ca.crt --cert ./client.crt --key ./client.key https://localhost:8443/api/health
```

### `-L, --location`

Follow HTTP redirects. **Off by default** (this matches curl, not a browser — a bare `curly` request will show you a 3xx response rather than silently following it).

```sh
curly -L https://httpbin.org/redirect/1
```

### `--max-redirects <N>`

Caps how many redirects `-L`/`--location` will follow before giving up (default 10). Has no effect without `-L`.

```sh
curly -L --max-redirects 3 https://httpbin.org/redirect/5
```

### `--max-time <SECONDS>`

Aborts the whole request (connect + send + receive) if it takes longer than this many seconds. Fractional values are allowed (e.g. `--max-time 0.5`).

```sh
curly --max-time 5 https://httpbin.org/delay/10
```

### `-x, --proxy <URL>`

Routes the request through an HTTP or HTTPS proxy.

```sh
curly -x http://localhost:8080 https://httpbin.org/get
```

### `--fail`

If the response status is 400 or higher, `curly` exits with a non-zero status (see §5). Without this flag, HTTP error responses are treated the same as any other response — printed normally, exit code 0.

### `-h, --help`

Prints the built-in usage summary and exits.

### `-V, --version`

Prints the installed `curly` version and exits.

## 5. Exit Codes

| Code | Meaning |
|---|---|
| `0` | Request completed and, if `--fail` was given, the response status was under 400. |
| `1` | Something went wrong: a network/connection error, an invalid argument (e.g. malformed header/query/form value, conflicting body flags, bad method, `--cert` without `--key`, unwritable output path), a bare `curly` with no URL and no subcommand, or — if `--fail` was passed — an HTTP status of 400 or higher. |
| `2` | clap couldn't parse the command line at all (unknown flag, missing value for a flag that requires one). |

Errors are printed to stderr with an `Error:` prefix.

## 6. Saved Requests, Collections, and Environments

Beyond one-shot mode, `curly` can save requests into named **collections**, parameterize them with `{{variable}}` tokens, and resolve those against named **environments** — all stored as plain JSON under your user data directory (`~/.local/share/curly` on Linux, `~/Library/Application Support/curly` on macOS, `%APPDATA%\curly` on Windows), so `curly env list`/`curly collections list` etc. work identically regardless of which project directory you're in.

### Environments — `curly env`

An environment is a named set of key/value variables. `{{variable}}` tokens in a saved request's URL, header values, query values, or body are substituted against the active environment when you `run` it (see below).

There's one special environment name: **`global`**. If it exists, its variables are always merged in underneath whichever environment you select with `--env` — so put anything shared across all your environments (e.g. an API version) in `global`, and per-environment values (host, credentials) in `dev`/`staging`/`prod`/etc.

```sh
curly env set global API_VERSION=v2
curly env set dev HOST=dev.example.com
curly env set dev TOKEN=dev-secret-token --secret
curly env list                  # dev
curly env show dev              # HOST=dev.example.com \n TOKEN=***
curly env unset dev HOST
curly env delete dev
```

- `curly env list` — list environment names.
- `curly env show <name>` — print its variables; values marked `--secret` show as `***` rather than their real value.
- `curly env set <name> KEY=VALUE [--secret]` — create the environment if it doesn't exist yet, then set (or overwrite) one variable. `--secret` masks it in `show` — it's still stored as plain JSON on disk, though, so treat the whole storage directory as sensitive if you keep real credentials in it (there's no OS-keychain integration yet, see DESIGN.md §2).
- `curly env unset <name> KEY` — remove one variable.
- `curly env delete <name>` — delete the environment entirely.

### Collections — `curly collections`

A collection is a named, ordered group of saved requests, stored as one JSON file (`curly collections show <name>` to see what's in it; the file itself lives at `<data dir>/curly/collections/<slug>.json` if you want to inspect or hand-edit it, or check it into a project's git repo).

```sh
curly collections create "My API"
curly collections add-request "My API" get-user "https://{{HOST}}/users/{{USER_ID}}" \
  -H "Authorization: Bearer {{TOKEN}}"
curly collections show "My API"
curly collections remove-request "My API" get-user
curly collections delete "My API"
```

- `curly collections list` — list collection names.
- `curly collections create <name>` — create an empty collection. Errors if one with that name already exists.
- `curly collections delete <name>` — delete a collection and everything saved in it.
- `curly collections show <name>` — list its saved requests (method, name, URL).
- `curly collections add-request <collection> <request-name> <url> [flags...]` — save a request. Creates the collection first if it doesn't exist yet. The flags are **exactly the same body/header/auth flags as one-shot mode** (`-X`, `-H`, `-Q`, `-d`, `--data-urlencode`, `--data-binary`, `-F`, `-u`, `--bearer`) — so the easiest workflow is: get a request working with plain `curly <url> ...`, then re-run the same flags under `collections add-request <collection> <name> <url> ...` to save it. `{{variable}}` tokens are stored literally — they're not resolved until `run`. Errors if the collection already has a request with that name (remove it first).
- `curly collections remove-request <collection> <request-name>` — delete one saved request from a collection.

### Running a saved request — `curly run`

```sh
curly run "My API/get-user" --env dev
curly run "My API/get-user" --env dev --var USER_ID=42
```

`curly run <collection>/<request-name>` resolves `{{variable}}` tokens against the merged variable scope (`global` + `--env`'s environment + any `--var key=value` overrides, in that precedence order — later wins) and sends the result. **Any variable left unresolved is a hard error** naming every undefined variable found, not a silently-sent literal `{{var}}` — that's a deliberate difference from Postman's GUI behavior, since a CLI/CI tool sending garbage to a server is almost always a bug worth catching immediately.

`run` accepts the same connection/output flags as one-shot mode — `-i`, `-v`, `-k`, `--fail`, `-o`, `--json`, `-p`, `-L`, `--max-redirects`, `--max-time`, `-x`, `--cacert`, `--cert`, `--key` — everything from [§4](#4-argument-reference) except the request-building flags (those come from the saved request, not the command line).

```sh
curly run "My API/get-user" --env dev -i -v --fail
```

## 7. History — `curly history`

Every request sent by either one-shot mode or `run` is automatically recorded — no separate opt-in. `curly history [--limit N]` lists the most recent ones (default 20), newest first:

```sh
curly history
curly history --limit 5
```

Each line shows timestamp, method, status, URL, and elapsed time. Recorded entries redact `Authorization`/`X-Api-Key`/`Cookie` header values and cap the stored response body at 8KB — see [DESIGN.md](DESIGN.md) §4 for the exact format if you want to read the underlying JSONL files directly (`<data dir>/curly/history/<yyyy-mm-dd>.jsonl`). There's no `curly history show <id>`/re-run-from-history yet — see §8.

## 8. What's Not Here Yet

These are on the roadmap (see [DESIGN.md](DESIGN.md) §9) but don't exist in the CLI yet — using them will just fail as an unrecognized flag or bare argument for now:

- OAuth2 authorization-code/client-credentials auth helpers
- Importing curl commands or Postman collections; exporting collections in Postman's format
- Re-running or viewing the full detail of a single history entry (only the summary list exists)
- Folder nesting within a collection (collections are a flat, ordered list of requests)
- Editing a saved request in place (currently: remove-request, then add-request again)
- Colorized output when connected to a TTY
- Pretty-printing for non-JSON bodies (XML/HTML)
- OS-keychain storage for `--secret` environment variables (currently plain JSON on disk)
