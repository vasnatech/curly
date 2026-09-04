# Curly CLI — Manual Testing Scenarios

Hand-run scenarios for exercising `curly` the way an end user would, on top of the automated unit tests (`cargo test -p curly-cli`) and integration tests (`cargo test -p curly-core`). Run these after any change that touches `crates/curly-cli/src/main.rs` or `crates/curly-core/src/exec.rs`, and before tagging a release.

Uses [httpbin.org](https://httpbin.org) as the target — a public HTTP-echo service, no setup required. If it's unreachable, most scenarios also work against `https://postman-echo.com` with minor path adjustments, or a locally-run `python3 -m http.server`/httpbin Docker image for offline testing.

Build first:
```sh
cargo build --release
alias curly=./target/release/curly   # or use cargo run -- ... directly
```

## 1. Basic requests

| # | Steps | Expected |
|---|---|---|
| 1.1 | `curly https://httpbin.org/get` | Prints the JSON body only, exit code 0. |
| 1.2 | `curly -i https://httpbin.org/get` | Status line + response headers printed above the body. |
| 1.3 | `curly -X DELETE https://httpbin.org/delete` | Body echoes back the request as a DELETE; no error. |
| 1.4 | `curly not-a-real-host.invalid` | Fails with a network/DNS error, exit code 1. |
| 1.5 | `curly` (no URL) | Fails with clap's "required argument" usage message, exit code 2 (clap's own exit code for arg-parsing errors — not `--fail`-related). |

## 2. Query parameters

| # | Steps | Expected |
|---|---|---|
| 2.1 | `curly -Q "id=42" -Q "active=true" https://httpbin.org/get` | Response `args` object contains both `id: "42"` and `active: "true"`. |
| 2.2 | `curly "https://httpbin.org/get?foo=bar"` (query baked into the URL, no `-Q`) | Response `args` contains `foo: "bar"` — confirms plain URL query strings still work without `-Q`. |
| 2.3 | `curly -Q "bad"` (no `=`) | Fails with a clear "invalid -Q/--query value" error, exit code 1. |

## 3. Request bodies

| # | Steps | Expected |
|---|---|---|
| 3.1 | `curly -H "Content-Type: application/json" -d '{"name":"a"}' https://httpbin.org/post` | Method auto-defaults to POST; response `json` field shows `{"name":"a"}`. |
| 3.2 | `curly --data-urlencode "name=a b" --data-urlencode "id=1" https://httpbin.org/post` | Response `form` object has `name: "a b"` (space decoded) and `id: "1"`; request `Content-Type` header was `application/x-www-form-urlencoded`. |
| 3.3 | `curly -F "name=alice" https://httpbin.org/post` | Response `form.name == "alice"`. |
| 3.4 | `printf 'hi' > /tmp/f.txt && curly -F "file=@/tmp/f.txt" https://httpbin.org/post` | Response `files.file == "hi"`. |
| 3.5 | `printf 'raw bytes' > /tmp/f.bin && curly --data-binary /tmp/f.bin https://httpbin.org/post` | Response `data == "raw bytes"`. |
| 3.6 | `curly --data-binary @/tmp/f.bin https://httpbin.org/post` (leading `@`, curl habit) | Same as 3.5 — `@` is accepted and stripped. |
| 3.7 | `curly -d 'x' -F 'a=b' https://httpbin.org/post` (two body sources at once) | Fails immediately with "only one of -d/--data-urlencode/--data-binary/-F may be used", no request sent, exit code 1. |
| 3.8 | `curly --data-binary /nonexistent/file https://httpbin.org/post` | Fails with a "failed to read --data-binary file" error, exit code 1. |

## 4. Authentication

| # | Steps | Expected |
|---|---|---|
| 4.1 | `curly -u "alice:s3cret" https://httpbin.org/basic-auth/alice/s3cret` | `{"authenticated": true, "user": "alice"}`, exit 0. |
| 4.2 | `curly -i -u "alice:wrong" https://httpbin.org/basic-auth/alice/s3cret` | `HTTP 401` in the output. |
| 4.3 | `curly --bearer "tok123" https://httpbin.org/bearer` | `{"authenticated": true, "token": "tok123"}`. |
| 4.4 | `curly -u "alice" https://httpbin.org/get` (no colon) | Fails with an "invalid -u/--user value" error. |
| 4.5 | `curly -u "alice:pw" --bearer "tok" https://httpbin.org/get` | Fails with "only one of -u/--user or --bearer may be used". |
| 4.6 | `curly -H "X-Api-Key: secret123" https://httpbin.org/get` | Confirms header-based API keys work via plain `-H` (no dedicated flag exists or is needed). |

## 5. Output shaping

| # | Steps | Expected |
|---|---|---|
| 5.1 | `curly -o /tmp/out.json https://httpbin.org/get && cat /tmp/out.json` | Nothing printed to stdout by curly itself; file contains the response body. |
| 5.2 | `curly -i -o /tmp/out.json https://httpbin.org/get` | Status/headers print to your terminal even though the body goes to the file. |
| 5.3 | `curly -p https://httpbin.org/get` | Body is indented/pretty JSON. |
| 5.4 | `curly -p https://httpbin.org/robots.txt` (non-JSON body) | Body prints unchanged (no error, no mangling) since it isn't valid JSON. |
| 5.5 | `curly --json https://httpbin.org/get \| python3 -m json.tool` | Valid JSON with `status`, `elapsed_ms`, `headers`, `body` keys. |
| 5.6 | `curly --json -i https://httpbin.org/get` | `-i` is silently ignored — output is still the single JSON object, not status+headers-then-JSON. |
| 5.7 | `curly -v https://httpbin.org/get 1>/tmp/body.txt` | Trace lines (`>`/`<` prefixed) appear on your terminal (stderr), while `/tmp/body.txt` contains only the response body. |

## 6. Redirects, timeouts, proxy

| # | Steps | Expected |
|---|---|---|
| 6.1 | `curly -i https://httpbin.org/redirect/1` | `HTTP 302` — not followed by default. |
| 6.2 | `curly -i -L https://httpbin.org/redirect/1` | `HTTP 200` — followed. |
| 6.3 | `curly -L --max-redirects 1 https://httpbin.org/redirect/3` | Fails with "too many redirects". |
| 6.4 | `curly --max-time 1 https://httpbin.org/delay/5` | Fails with "operation timed out" in well under 5s. |
| 6.5 | `curly -x http://127.0.0.1:1 https://httpbin.org/get` (nothing listening on that port) | Fails with a proxy-connect error — confirms `-x` actually routes through the given address rather than being silently ignored. |
| 6.6 | `curly -x "not a url" https://httpbin.org/get` | Fails immediately with "invalid --proxy URL", before attempting any connection. |

## 7. TLS

| # | Steps | Expected |
|---|---|---|
| 7.1 | `curly https://self-signed.badssl.com/` | Fails with a certificate trust error (e.g. `UnknownIssuer`/`invalid peer certificate`). |
| 7.2 | `curly -k https://self-signed.badssl.com/` | Succeeds (verification skipped). |
| 7.3 | `curly --cert /tmp/c.pem https://httpbin.org/get` (no `--key`) | Fails with "--cert and --key must be used together" before attempting a connection. |
| 7.4 | `curly --cacert /nonexistent.pem https://httpbin.org/get` | Fails with "failed to read --cacert file". |
| 7.5 (optional, needs a local mTLS server) | Generate a CA/server/client cert trio with `openssl req`, run a local HTTPS server requiring client certs, then `curly --cacert ca.crt --cert client.crt --key client.key https://localhost:<port>/`. | Request succeeds; omitting `--cert`/`--key` against the same server fails with a `CertificateRequired` TLS alert. |

## 8. Exit codes / `--fail`

| # | Steps | Expected |
|---|---|---|
| 8.1 | `curly https://httpbin.org/status/500; echo $?` | Body/status printed normally, exit code **0** (no `--fail`). |
| 8.2 | `curly --fail https://httpbin.org/status/500; echo $?` | Exit code **1**. |
| 8.3 | `curly --fail https://httpbin.org/status/200; echo $?` | Exit code **0**. |

## 9. Cross-platform sanity (when releasing)

| # | Steps | Expected |
|---|---|---|
| 9.1 | `cargo build --release` on Linux, macOS, and Windows | Each produces a working binary with no OS-specific build errors. |
| 9.2 | Run scenario 1.1 and 4.1 on each OS | Same output shape on all three (path separators in `-o`/`--data-binary`/`--cacert` examples are the main thing to sanity-check on Windows). |

## 10. Regression checklist for new flags

When adding a new flag, add at minimum:
- A unit test in `crates/curly-cli/src/main.rs`'s `#[cfg(test)] mod tests` covering the pure parsing/building logic.
- One row in the relevant table above exercising it against a real server.
- A corresponding entry in [CLI.md](CLI.md) (§3 example + §4 reference entry).
