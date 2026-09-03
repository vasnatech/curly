# Curly CLI — User Guide

`curly` is the command-line face of the Curly project: a curl-like tool for sending HTTP requests from a terminal. This guide covers everything the CLI currently supports. If you already know curl, the flags will feel familiar on purpose.

> Status: this guide documents the CLI's **one-shot mode** (send a single request, see the response). Saved requests, collections, environments, and authentication helpers are planned but not implemented yet — see [DESIGN.md](DESIGN.md) for the roadmap.

## 1. Installing / Running

Curly isn't published to a package registry yet. For now, build it from source.

**Requirements:** [Rust and Cargo](https://www.rust-lang.org/tools/install) (any recent stable toolchain).

```sh
git clone <this-repo-url> curly
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

**POST a JSON body** (method defaults to `POST` automatically whenever `-d` is used):
```sh
curly -H "Content-Type: application/json" -d '{"name":"a"}' https://httpbin.org/post
```

**Explicit method** (needed for methods that don't imply a body, like `DELETE`):
```sh
curly -X DELETE https://httpbin.org/delete
```

**Multiple headers** (repeat `-H` as many times as you need):
```sh
curly -H "Content-Type: application/json" -H "Authorization: Bearer sometoken" https://httpbin.org/get
```

**Save the response body to a file instead of printing it:**
```sh
curly -o response.json https://httpbin.org/get
```

**See exactly what was sent and received** (request/response trace goes to stderr, so it doesn't pollute piped output):
```sh
curly -v https://httpbin.org/get
```

**Treat an HTTP error status as a failure** (useful in scripts/CI — see §5 Exit Codes):
```sh
curly --fail https://httpbin.org/status/404 || echo "request failed"
```

**Skip TLS certificate verification** (self-signed certs on a local/dev server — avoid on anything else):
```sh
curly -k https://localhost:8443/api/health
```

## 4. Argument Reference

### `<URL>` (required, positional)

The full URL to request, including scheme (`http://`/`https://`). Query parameters aren't yet a separate flag — append them directly to the URL: `curly "https://httpbin.org/get?id=42&active=true"`. Quote the URL if it contains `&` or other shell-special characters.

### `-X, --request <METHOD>`

Sets the HTTP method (`GET`, `POST`, `PUT`, `PATCH`, `DELETE`, `HEAD`, `OPTIONS`, or any other verb the server accepts). Case-insensitive on input.

- If omitted **and** `-d`/`--data` is given, the method defaults to `POST` (matches curl's behavior).
- If omitted and there's no `-d`, it defaults to `GET`.

```sh
curly -X PUT -d '{"status":"done"}' https://httpbin.org/put
```

### `-H, --header <"Name: value">`

Adds a request header. Repeatable — pass `-H` once per header. The value must be in `Name: value` form (colon-separated); everything is passed through as-is (leading/trailing whitespace around the value is trimmed).

```sh
curly -H "Accept: application/json" -H "X-Request-Id: abc123" https://httpbin.org/get
```

### `-d, --data <DATA>`

Sets the request body to the given string, sent as-is (no encoding/escaping is applied beyond what your shell does). Pair with `-H "Content-Type: ..."` to tell the server what format the body is in — Curly does not infer or set a content type for you.

```sh
curly -H "Content-Type: application/json" -d '{"id":1}' https://httpbin.org/post
```

To send the contents of a file as the body, use your shell's substitution:
```sh
curly -H "Content-Type: application/json" -d "$(cat payload.json)" https://httpbin.org/post
```

*(A dedicated `-d @file` shorthand, like curl's, is planned but not implemented yet.)*

### `-o, --output <PATH>`

Writes the response body to the given file path instead of printing it to stdout. Overwrites the file if it already exists. Combine with `-i` if you also want the status/headers printed to your terminal while the body goes to disk.

```sh
curly -o out.json https://httpbin.org/get
```

### `-i, --include`

Prints the response status line and headers to stdout before the body (or before writing the body, if `-o` is also set — the status/headers still go to your terminal, only the body goes to the file).

```sh
curly -i https://httpbin.org/get
```

### `-v, --verbose`

Prints a request/response trace — the method, URL, and headers sent, followed by the status, elapsed time, and headers received — to **stderr**. The response body still goes to stdout (or the `-o` file) as usual, so verbose tracing never mixes into output you're piping or redirecting.

```sh
curly -v https://httpbin.org/get > body-only.json
```

### `-k, --insecure`

Disables TLS certificate verification for this request. Use only for local/dev servers with self-signed certificates — never against a server you don't control, since it removes protection against man-in-the-middle attacks.

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
| `1` | Something went wrong: a network/connection error, an invalid argument (e.g. malformed header, bad method, unwritable output path), or — if `--fail` was passed — an HTTP status of 400 or higher. |

Errors are printed to stderr with an `Error:` prefix.

## 6. What's Not Here Yet

These are on the roadmap (see [DESIGN.md](DESIGN.md) §9) but don't exist in the CLI yet — using them will just fail as an unrecognized flag or bare argument for now:

- Saved/named requests and collections (`curly run <name>`)
- Environments and `{{variable}}` substitution
- Built-in auth helpers (Basic, Bearer, API key, OAuth2) — for now, set the `Authorization` header yourself via `-H`
- Request history (`curly history`)
- Importing curl commands or Postman collections
- A dedicated query-parameter flag (append params to the URL directly for now)
- Machine-readable `--json` output mode
