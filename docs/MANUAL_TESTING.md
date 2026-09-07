# Curly CLI — Manual Testing Scenarios

Hand-run scenarios for exercising `curly` the way an end user would, on top of the automated unit tests (`cargo test -p curly-cli`) and integration tests (`cargo test -p curly-core`). Run these after any change that touches `crates/curly-cli/src/` or `crates/curly-core/src/`, and before tagging a release.

Uses [httpbin.org](https://httpbin.org) as the target — a public HTTP-echo service, no setup required. If it's unreachable, most scenarios also work against `https://postman-echo.com` with minor path adjustments, or a locally-run `python3 -m http.server`/httpbin Docker image for offline testing.

Sections 10–13 (saved requests/collections/environments/history) read and write real files under your user data directory (`~/.local/share/curly` on Linux by default). Set `XDG_DATA_HOME` to a scratch directory first so these scenarios don't touch or get confused with real data:
```sh
export XDG_DATA_HOME=/tmp/curly-manual-test-data
rm -rf "$XDG_DATA_HOME"   # start clean
```

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
| 1.5 | `curly` (no URL, no subcommand) | Fails with "a URL is required (or use a subcommand: ...)", exit code **1** (URL is optional at the clap level so subcommands can coexist with it — this is our own runtime check, not clap's arg-parsing error). |
| 1.6 | `curly --bogus-flag https://example.com` | Fails with clap's own "unexpected argument" usage message, exit code **2** (clap's parse-error exit code — different from 1.5's runtime check). |

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

## 9. Environments — `curly env`

| # | Steps | Expected |
|---|---|---|
| 9.1 | `curly env list` (clean state) | "no environments yet — create one with: ..." |
| 9.2 | `curly env set dev HOST=dev.example.com` | "set HOST in environment \"dev\""; creates the environment. |
| 9.3 | `curly env set dev TOKEN=secret123 --secret` then `curly env show dev` | Output shows `HOST=dev.example.com` and `TOKEN=***` (masked). |
| 9.4 | `curly env set dev HOST=other.example.com` then `curly env show dev` | `HOST` is now `other.example.com` — `set` overwrites an existing key rather than duplicating it. |
| 9.5 | `curly env unset dev HOST` then `curly env show dev` | Only `TOKEN=***` remains. |
| 9.6 | `curly env unset dev NOPE` | Fails with "has no variable named \"NOPE\"". |
| 9.7 | `curly env delete dev` then `curly env show dev` | Delete succeeds; show then fails with "no environment named \"dev\"". |
| 9.8 | `curly env set "my env" X=1` then `curly env show "My Env"` | Same result both ways — names are slugified (`my-env`) for storage, so lookup is case/spacing-insensitive. |

## 10. Collections — `curly collections`

| # | Steps | Expected |
|---|---|---|
| 10.1 | `curly collections list` (clean state) | "no collections yet — create one with: ..." |
| 10.2 | `curly collections create "My API"` then `curly collections create "My API"` again | First succeeds; second fails with "already exists". |
| 10.3 | `curly collections add-request "My API" get-user "https://httpbin.org/get" -Q "id=42"` | "saved \"get-user\" in collection \"My API\"" — also auto-creates a collection that doesn't exist yet if you skip step 10.2. |
| 10.4 | `curly collections add-request "My API" get-user "https://httpbin.org/get"` again (same path) | Fails with "a request already exists at \"get-user\"" (wrapped in "in collection \"My API\" (remove it first with collections remove-request)"). |
| 10.5 | `curly collections show "My API"` | Lists `GET     get-user`. |
| 10.6 | Inspect `$XDG_DATA_HOME/curly/collections/my-api.json` directly | Human-readable JSON; a top-level `"folders": []` plus `"requests"`; the `-Q "id=42"` from 10.3 appears as a `query_params` entry with `"enabled": true`. |
| 10.7 | `curly collections remove-request "My API" get-user` then `show` | "removed ..."; show then says "has no requests yet". |
| 10.8 | `curly collections remove-request "My API" nope` | Fails with "has no request named \"nope\"". |
| 10.9 | `curly collections delete "My API"` then `list` | Delete succeeds; list back to "no collections yet". |
| 10.10 | `curly collections add-request "My API" login https://httpbin.org/post -d '{"u":"a"}' -H "Content-Type: application/json"` | Saves a POST with a JSON body — same body/header flags as one-shot mode. |

### Folders

| # | Steps | Expected |
|---|---|---|
| 10.11 | `curly collections add-request "My API" "Auth/OAuth/login" https://httpbin.org/post` | Creates `Auth` and `Auth/OAuth` folders along the way; "saved \"Auth/OAuth/login\" in collection \"My API\"". |
| 10.12 | `curly collections add-request "My API" "Auth/logout" https://httpbin.org/post` | Succeeds; reuses the existing `Auth` folder rather than creating a duplicate one — confirm via 10.14. |
| 10.13 | `curly run "My API/Auth/OAuth/login"` | Runs the nested request directly — same path syntax works for `run` as for `add-request`/`remove-request`. |
| 10.14 | `curly collections show "My API"` | Tree output: `Auth/` containing `OAuth/` (with `login`) and `logout` as a sibling — one `Auth` folder, not two. |
| 10.15 | `curly collections add-folder "My API" "Empty/Nested"` then `show` | Creates both folder levels even with no request in them; appears in the tree as empty. |
| 10.16 | `curly collections add-folder "My API" "Auth"` again | No error, no duplicate — idempotent. |
| 10.17 | `curly collections remove-folder "My API" "Auth"` (still has `OAuth/login` and `logout` in it) | Fails with "is not empty (pass force to delete it and everything inside)"; nothing removed. |
| 10.18 | `curly collections remove-folder "My API" "Auth" --force` then `show` | Removes `Auth` and everything nested inside it (`OAuth/login`, `logout`) in one shot. |
| 10.19 | `curly collections remove-folder "My API" "NoSuchFolder"` | Fails with "has no folder named \"NoSuchFolder\"". |
| 10.20 | Take the `health-record-backend-spring/.curly/collections/health-record-api.json` committed before folder support existed (no `"folders"` key at all) and run `curly collections show "Health Record API"` against it | Still loads and shows correctly (`folders` defaults to empty on parse) — confirms old collection files aren't broken by this feature. |

## 11. Running a saved request — `curly run`

| # | Steps | Expected |
|---|---|---|
| 11.1 | `curly env set global HOST=httpbin.org` and `curly env set dev USER_ID=99`, then `curly collections add-request "My API" get-user "https://{{HOST}}/get" -Q "id={{USER_ID}}"`, then `curly run "My API/get-user" --env dev` | `args.id == "99"` in the response — confirms `global` (HOST) and `dev` (USER_ID) merge together. |
| 11.2 | `curly run "My API/get-user" --env dev --var USER_ID=777` | `args.id == "777"` — `--var` overrides the environment's value. |
| 11.3 | `curly run "My API/get-user"` (no `--env`) | Fails with "undefined variable(s): USER_ID" — `HOST` resolves from `global` alone, but `USER_ID` was only ever set in `dev`. |
| 11.4 | `curly run "My API/get-user" --env dev -i -v` | `-i`/`-v` behave identically to one-shot mode (status/headers printed, trace on stderr). |
| 11.5 | `curly run "My API/no-such-request" --env dev` | Fails with "collection \"My API\" has no request named \"no-such-request\"". |
| 11.6 | `curly run "No Such Collection/x"` | Fails with "no collection named \"No Such Collection\"". |
| 11.7 | `curly run "no-slash-in-target"` | Fails with "invalid run target (expected \"collection/request-name\")" — before touching storage or the network. |
| 11.8 | After 11.1/11.2/11.4 (3 successful sends), `curly history --limit 3` | All three appear, most recent first. 11.3's failure (undefined variable, never reached execute) does *not* appear. |

## 12. Extraction and sessions

| # | Steps | Expected |
|---|---|---|
| 12.1 | `curly collections add-request "Test" get-echo "https://httpbin.org/get?token=abc123" --extract-body "ORIGIN=origin" --extract-header "CT=Content-Type" --extract-template "GREETING=hello {{ORIGIN}}" --extract-secret ORIGIN` then `curly collections show "Test"` | Saves the rules; show marks the request `[extracts: ORIGIN, CT, GREETING]`. |
| 12.2 | `curly session show` (before running) | "session \"global\" is empty" |
| 12.3 | `curly run "Test/get-echo" -v` | Prints `~ extracted ORIGIN into session "global"` (and CT, GREETING) on stderr; response body still prints normally. |
| 12.4 | `curly session show` | `ORIGIN=***` (masked, `--extract-secret`), `CT=application/json`, `GREETING=hello <the real origin ip>` (template correctly referenced the ORIGIN extracted moments earlier). |
| 12.5 | `curly collections add-request "Test" get-user "https://httpbin.org/get?from={{ORIGIN}}"` then `curly run "Test/get-user"` (no `--var ORIGIN=...` anywhere) | Succeeds — `{{ORIGIN}}` resolves from the session written by 12.3, with no manual `env set` step. This is the actual point of the feature. |
| 12.6 | `curly collections add-request "Test" bad-path "https://httpbin.org/get" --extract-body "X=nonexistent.path"` then `curly run "Test/bad-path"` | The response body prints first, *then* `Error: path "nonexistent.path" not found in response body (extracting "X")`, exit 1 — confirms extraction failure never swallows the response. |
| 12.7 | `curly collections add-request "Test" not-found "https://httpbin.org/status/404" --extract-body "X=nonexistent.path"` then `curly run "Test/not-found"` | Succeeds (exit 0), no extraction error — non-2xx responses skip extraction rules entirely. |
| 12.8 | `curly run "Test/get-echo" --env dev` (with a `dev` environment already created) then `curly session show --env dev` and `curly session show` | `dev`'s session has the extracted variables; the `global` session from 12.3 is untouched — sessions are isolated per environment. |
| 12.9 | `curly session clear` then `curly session show` | Clears the `global` session only; back to "is empty". |
| 12.10 | `curly session clear --all` | Clears every environment's session (verify with `session show --env dev` too). |
| 12.11 | `curly collections add-request "Test" x "https://httpbin.org/get" --extract-secret NOPE` (no matching `--extract-body/--extract-header/--extract-template`) | Fails immediately with "no --extract-body/--extract-header/--extract-template above defines a variable named \"NOPE\"" — before saving anything. |
| 12.12 | Inspect `$XDG_DATA_HOME/curly/session/global.json` directly after 12.3 | Same shape as an environment file (`name`, `variables: [{key, value, secret}]`) — confirms sessions reuse the Environment format. |

## 13. History — `curly history`

| # | Steps | Expected |
|---|---|---|
| 13.1 | `curly history` (clean state) | "no history yet" |
| 13.2 | `curly https://httpbin.org/get`, then `curly -X POST -d '{}' -H "Content-Type: application/json" https://httpbin.org/post`, then `curly history` | Both appear, most recent (POST) first, with correct method/status/URL/elapsed_ms. |
| 13.3 | `curly history --limit 1` | Only the most recent entry. |
| 13.4 | `curly -H "Authorization: Bearer sekrit" https://httpbin.org/get`, then inspect `$XDG_DATA_HOME/curly/history/*.jsonl` directly | The stored `request_headers` entry for `Authorization` reads `"***redacted***"`, not the real token — confirms NFR-4 redaction independent of what `curly history`'s own summary view shows (it doesn't print headers at all). |

## 14. Project-local storage — `curly init`, `--data-dir`

| # | Steps | Expected |
|---|---|---|
| 14.1 | `mkdir -p /tmp/proj/backend/src/main && cd /tmp/proj/backend && curly init` | "initialized curly project at /tmp/proj/backend/.curly"; `.curly/.gitignore` exists containing `environments/`, `session/`, and `history/` (only `collections/` is meant to be committed). |
| 14.2 | Run `curly init` again in the same directory | "curly project already initialized at ..." — idempotent, not an error. |
| 14.3 | `cd /tmp/proj/backend/src/main && curly env set dev X=1` (three levels below where `.curly` was created) | Succeeds; `/tmp/proj/backend/.curly/environments/dev.json` is what got written — confirms auto-detection walks *up* from the current directory, not just checks it. |
| 14.4 | From anywhere outside `/tmp/proj`, `curly --data-dir /tmp/other env list` | Uses `/tmp/other`, not the OS default and not any `.curly` that happens to be an ancestor of the current directory — `--data-dir` wins outright. |
| 14.5 | `CURLY_DATA_DIR=/tmp/other2 curly env list` (run from inside `/tmp/proj/backend`, which has its own `.curly`) | Uses `/tmp/other2` — the env var beats auto-detection, even though a `.curly` is sitting right there. |
| 14.6 | From a directory with no `.curly` anywhere in its ancestry and no `--data-dir`/`CURLY_DATA_DIR` set, `curly env list` | Falls back to the OS default (`~/.local/share/curly` on Linux) — unchanged pre-M2 behavior. |

## 15. Cross-platform sanity (when releasing)

| # | Steps | Expected |
|---|---|---|
| 15.1 | `cargo build --release` on Linux, macOS, and Windows | Each produces a working binary with no OS-specific build errors. |
| 15.2 | Run scenario 1.1 and 4.1 on each OS | Same output shape on all three (path separators in `-o`/`--data-binary`/`--cacert` examples are the main thing to sanity-check on Windows). |
| 15.3 | Run scenario 10.6 (inspect the collection JSON file) on each OS | Confirms the data directory resolves correctly per-OS (`~/.local/share/curly`, `~/Library/Application Support/curly`, `%APPDATA%\curly`). |

## 16. Regression checklist for new flags/subcommands

When adding a new flag or subcommand, add at minimum:
- A unit test covering the pure parsing/building logic — `crates/curly-cli/src/one_shot.rs`, `args.rs`, or `commands/*.rs`'s `#[cfg(test)] mod tests` depending on where the logic lives, plus `crates/curly-core/tests/` for anything storage- or substitution-related.
- One row in the relevant table above exercising it against a real server (or a real temp storage directory for env/collections/history).
- A corresponding entry in [CLI.md](CLI.md) (an example in the relevant section + a reference entry).
