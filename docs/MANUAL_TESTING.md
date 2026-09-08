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

## 13. Import — Postman and curl

Fixture files, create once at the top of this section:

```sh
cat > /tmp/sample-collection.json <<'EOF'
{
  "info": { "name": "Sample API" },
  "item": [
    { "name": "login", "request": { "method": "POST", "header": [{"key":"Content-Type","value":"application/json"}], "body": {"mode":"raw","raw":"{\"email\":\"{{EMAIL}}\"}"}, "url": "{{BASE_URL}}/post" } },
    { "name": "Users", "item": [
        { "name": "get-me", "request": { "method": "GET", "url": "{{BASE_URL}}/get", "auth": {"type":"bearer","bearer":[{"key":"token","value":"{{TOKEN}}"}]} } }
    ]}
  ]
}
EOF
cat > /tmp/sample-env.json <<'EOF'
{ "name": "Dev", "values": [
    {"key":"BASE_URL","value":"https://httpbin.org","type":"default","enabled":true},
    {"key":"TOKEN","value":"abc123","type":"secret","enabled":true},
    {"key":"SKIP_ME","value":"x","type":"default","enabled":false}
]}
EOF
cat > /tmp/sample-globals.json <<'EOF'
{ "name": "Globals", "values": [{"key":"API_VERSION","value":"v2","type":"default","enabled":true}], "_postman_variable_scope": "globals" }
EOF
```

| # | Steps | Expected |
|---|---|---|
| 13.1 | `curly collections import postman "Sample API" /tmp/sample-collection.json` | "imported 2 request(s) into collection \"Sample API\"". |
| 13.2 | `curly collections show "Sample API"` | Tree shows `POST login` at top level and `Users/` containing `GET get-me` — nested folders imported correctly. |
| 13.3 | `curly run "Sample API/login" --env Dev` (after 13.4's env import) | Resolves `{{BASE_URL}}`/`{{EMAIL}}` — succeeds against httpbin.org (note: `{{EMAIL}}` isn't in the sample env, so this actually errors with "undefined variable(s): EMAIL" unless you `env set Dev EMAIL=...` first — confirms imported `{{variable}}` tokens behave exactly like hand-authored ones). |
| 13.4 | `curly env import-postman /tmp/sample-env.json` then `curly env show Dev` | "imported 2 variable(s)" (not 3 — `SKIP_ME` is disabled and dropped); `TOKEN` shows as `***` (imported as secret from `"type":"secret"`). |
| 13.5 | `curly env import-postman /tmp/sample-globals.json` then `curly env list` | Imports as environment **`global`**, not "Globals" — confirms `_postman_variable_scope: "globals"` auto-maps to curly's always-merged-in scope. |
| 13.6 | `curly env import-postman /tmp/sample-env.json --as staging` | Imports as `staging` instead of `Dev` — `--as` overrides the file's own name. |
| 13.7 | `curly collections import postman "Sample API" /tmp/sample-collection.json` again | Fails with "collection \"Sample API\" already exists (delete it first to re-import)". |
| 13.8 | `curly env import-postman /tmp/sample-env.json` again (still named `Dev`) | Fails with "environment \"Dev\" already exists (delete it first to re-import)". |
| 13.9 | `curly collections import postman "X" /tmp/does-not-exist.json` | Fails with a clear file-not-found error, not a panic. |
| 13.10 | `curly collections import curl "Sample API" "Users/list" "curl 'https://httpbin.org/get?x=1' -H 'Accept: application/json'"` then `curly run "Sample API/Users/list"` | Saves and runs correctly — `args.x == "1"` in the response. |
| 13.11 | `echo "curl 'https://httpbin.org/get?via=stdin'" \| curly collections import curl "Sample API" "via-stdin"` (no command argument — reads stdin) then `curly run "Sample API/via-stdin"` | `args.via == "stdin"` — confirms the stdin path works, important for pasting real multi-line "Copy as cURL" exports. |
| 13.12 | `curly collections import curl "Sample API" bad-user "curl -u alice https://httpbin.org/get"` (no colon in `-u` value) | Fails with "invalid -u/--user value (expected \"user:password\")". |
| 13.13 | `curly collections import curl "Sample API" both-bodies "curl -d 'a' -F 'b=c' https://httpbin.org/post"` | Fails with "only one of -d/--data* or -F/--form may be used per curl command". |
| 13.14 | `curly collections import curl "Sample API" via-timeout "curl --connect-timeout 5 https://httpbin.org/get"` then inspect the saved request's URL | URL is `https://httpbin.org/get`, not `5` — confirms an unrecognized flag's own value isn't mistaken for the URL. |

## 14. History — `curly history`

| # | Steps | Expected |
|---|---|---|
| 14.1 | `curly history` (clean state) | "no history yet" |
| 14.2 | `curly https://httpbin.org/get`, then `curly -X POST -d '{}' -H "Content-Type: application/json" https://httpbin.org/post`, then `curly history` | Both appear, most recent (POST) first, with correct method/status/URL/elapsed_ms. |
| 14.3 | `curly history --limit 1` | Only the most recent entry. |
| 14.4 | `curly -H "Authorization: Bearer sekrit" https://httpbin.org/get`, then inspect `$XDG_DATA_HOME/curly/history/*.jsonl` directly | The stored `request_headers` entry for `Authorization` reads `"***redacted***"`, not the real token — confirms NFR-4 redaction independent of what `curly history`'s own summary view shows (it doesn't print headers at all). |

## 15. Project-local storage — `curly init`, `--data-dir`

| # | Steps | Expected |
|---|---|---|
| 15.1 | `mkdir -p /tmp/proj/backend/src/main && cd /tmp/proj/backend && curly init` | "initialized curly project at /tmp/proj/backend/.curly"; `.curly/.gitignore` exists containing `environments/`, `session/`, and `history/` (only `collections/` is meant to be committed). |
| 15.2 | Run `curly init` again in the same directory | "curly project already initialized at ..." — idempotent, not an error. |
| 15.3 | `cd /tmp/proj/backend/src/main && curly env set dev X=1` (three levels below where `.curly` was created) | Succeeds; `/tmp/proj/backend/.curly/environments/dev.json` is what got written — confirms auto-detection walks *up* from the current directory, not just checks it. |
| 15.4 | From anywhere outside `/tmp/proj`, `curly --data-dir /tmp/other env list` | Uses `/tmp/other`, not the OS default and not any `.curly` that happens to be an ancestor of the current directory — `--data-dir` wins outright. |
| 15.5 | `CURLY_DATA_DIR=/tmp/other2 curly env list` (run from inside `/tmp/proj/backend`, which has its own `.curly`) | Uses `/tmp/other2` — the env var beats auto-detection, even though a `.curly` is sitting right there. |
| 15.6 | From a directory with no `.curly` anywhere in its ancestry and no `--data-dir`/`CURLY_DATA_DIR` set, `curly env list` | Falls back to the OS default (`~/.local/share/curly` on Linux) — unchanged pre-M2 behavior. |

## 16. GUI (M4, first slice — see GUI.md)

Requires an actual display (X11 or Wayland) — not scriptable the way the rest of this doc is; run these by hand. `cargo build --release` first.

| # | Steps | Expected |
|---|---|---|
| 16.1 | `curly` (no arguments) | GUI window opens (not a CLI error). |
| 16.2 | `curly gui` | Same window opens. |
| 16.3 | `curly -v` (a flag, no URL, no subcommand) | Does **not** open the GUI — prints the usual "a URL is required" CLI error. Confirms the zero-args check doesn't over-trigger on a flag-only invocation. |
| 16.4 | `curly https://httpbin.org/get` | Still one-shots normally on stdout — GUI mode didn't hijack normal CLI usage. |
| 16.5 | In the GUI: method dropdown → GET, URL → `https://httpbin.org/get`, click **Send** | Status turns up green (2xx), elapsed time and body size shown, body pretty-printed as JSON. |
| 16.6 | Press **Enter** while focused in the URL field (instead of clicking Send) | Same as clicking Send. |
| 16.7 | Press **Ctrl+Enter** (Cmd+Enter on macOS) anywhere in the window | Same as clicking Send. |
| 16.8 | Click Send, then immediately try clicking it again before the response arrives | Button reads "Sending…" and is disabled — can't fire a second overlapping request; window stays responsive (not frozen) while waiting. |
| 16.9 | Add a header row (`+ Add header`), set name `X-Test` / value `abc`, send to `https://httpbin.org/get` | Response's `headers.X-Test` echoes `abc`. |
| 16.10 | Uncheck that header's enabled box, send again | `X-Test` no longer appears in the echoed headers — disabled means excluded, not deleted. |
| 16.11 | Add a header row with a blank name, leave it checked, send | No error — a blank-name header is silently excluded (matches `build_request`'s unit-tested behavior). |
| 16.12 | Method → POST, Body → `{"a":1}`, send to `https://httpbin.org/post` | Response's `data`/`json` field shows `{"a": 1}` — raw body sent correctly. |
| 16.13 | Send to an invalid URL (e.g. `not-a-url`) | Error shown in red in the response pane — window doesn't crash or freeze. |
| 16.14 | Collapse/expand the "Headers", "Body", and "Response Headers" sections | Each toggles independently, state persists while the window stays open. |
| 16.15 | `cargo build -p curly-cli --no-default-features --release` then run `./target/release/curly` (zero args) and `curly gui` | Both print "this build of curly was compiled without the gui feature..." and exit 1 — no window, no panic. |
| 16.16 | Launch from a directory with no `.curly` anywhere above it (e.g. `cd /tmp && curly gui`) | Sidebar shows project label **"Default"**; Collections/Environments lists are empty unless the OS-wide default already has some. |
| 16.17 | Launch from inside a project directory that already has a `.curly` (e.g. `cd health-record-backend-spring && curly gui`) | Sidebar auto-opens *that* project (label = the folder's name, e.g. "health-record-backend-spring") — same auto-detection the CLI uses, not "Default". |
| 16.18 | Click **"Open Project…"**, pick a folder that already has a `.curly` in it (e.g. `health-record-backend-spring`) | Sidebar's label updates to that folder's name; Collections list shows `health-record-api` (or whatever's actually saved there); Environments list shows the real environment names. |
| 16.19 | Click **"Open Project…"**, pick an empty folder with no `.curly` | A new `.curly` is created there on the spot (check with `ls <folder>/.curly` afterward) — same as running `curly init` in that folder; sidebar opens it (Collections/Environments both empty). |
| 16.20 | With a project already open, click **"Open Project…"** again and pick a *different* folder | The sidebar switches entirely to the new project — old one's Collections/Environments are gone from view (single-project-at-a-time, not a second tab). |
| 16.21 | Cancel the folder-picker dialog (Escape or its own Cancel button) instead of picking anything | Nothing changes — the previously active project (if any) stays active, no error shown. |
| 16.22 | With `health-record-backend-spring`'s project open, click **"Health Record API"** in the sidebar | Expands into a tree — `Auth/`, `Me/`, `Admin/`, etc. as sub-folders, each expandable further, with method+name shown for each leaf request. |
| 16.23 | With no environment selected, click a top-level request that has `{{variable}}` tokens (e.g. `{{BASE_URL}}`) | Method/URL/headers load into the editor as literal `{{BASE_URL}}` text; a yellow notice appears above the editor naming the request and every undefined variable, and saying no environment is selected. |
| 16.24 | Click a nested request, e.g. `Auth/login` | Same as 16.23, but confirms clicking works at nested depth too, not just top-level. |
| 16.25 | Click a request whose saved auth is `Bearer` (e.g. one you've added with `--extract-secret`/`--bearer` via the CLI, or hand-edit a collection JSON to add `"auth": {"type": "bearer", ...}`) | An `Authorization: Bearer <token>` header appears in the headers table — no separate auth UI, folded into headers. |
| 16.26 | Click a request whose saved body is `form`/`multipart` (e.g. one added via `--data-urlencode`/`-F` through the CLI) | Body box stays empty; notice mentions the body "isn't plain text" and wasn't loaded. |
| 16.27 | Load a request (any with `{{variable}}` tokens), replace the tokens in the URL/header fields by hand with real values, then Send | Works normally — the notice is informational only, it doesn't block sending; nothing stops you from fixing the values yourself. |
| 16.28 | Load one request, then load a different one without sending in between | Editor fully replaces its contents with the second request's data; any stale response from a previous send is cleared (no leftover response pane content from before). |
| 16.29 | Launch `curly gui` and look at the window itself (title bar / task switcher preview, whichever your WM shows) | A `{ }` mark icon is visible — not blank, not a generic placeholder. Works out of the box on X11, Windows, and macOS with no extra steps. |
| 16.30 | On GNOME/Wayland specifically: launch `curly gui` *before* running the install script | Dock/Alt-Tab/Activities show a generic fallback icon, not `{ }` — expected, since nothing has told GNOME which `.desktop` entry this window belongs to yet. |
| 16.31 | Run `./scripts/linux-install-desktop-entry.sh`, then quit and relaunch `curly gui` | Script prints the paths it installed and exits 0; dock/Alt-Tab/Activities now show the `{ }` icon. |
| 16.32 | Run `./scripts/linux-install-desktop-entry.sh` a second time in a row | Same output, no error — overwriting its own previously-installed files is safe (idempotent). |
| 16.33 | With `health-record-backend-spring`'s project open (has `global`/`local` environments, e.g. via `curly env set local EMAIL=you@example.com`), load `Auth/login`, then click **`global`** in the Environments list | The undefined-variable notice updates or clears depending on what `global` actually defines (e.g. still lists `EMAIL`/`PASSWORD` as undefined if only `BASE_URL` is in `global`, since `local` hasn't been selected). |
| 16.34 | With `Auth/login` still loaded, click **`local`** in the Environments list | Notice clears once `local` (plus the always-merged `global`) defines every variable the request references; if it still doesn't, the notice updates to name exactly what's still missing. |
| 16.35 | With the notice clear, click **Send** | Request goes out with real values substituted (verify via a response, or by checking a proxy/echo endpoint) — not the literal `{{token}}` text. |
| 16.36 | With an environment selected and a variable resolved successfully, click **`(none selected)`** and Send again the same request | Now fails — response pane shows a red error naming the undefined variable(s); confirms selecting no environment genuinely means no environment (aside from `global`), not "keep the last one". |
| 16.37 | Switch between two environments that both define the same variable name with different values (e.g. `local`'s `BASE_URL` vs `global`'s), then Send after each switch | The request actually goes to the value from whichever environment is currently selected — proves the switch changes what's substituted, not just what's displayed. |
| 16.38 | Open a *different* project (via "Open Project…") while an environment is selected | The active environment resets to **"(none selected)"** — the old project's environment name may not even exist in the new one. |
| 16.39 | Send a request whose URL/headers/body reference the same undefined variable twice (e.g. `{{TOKEN}}` in both a header and the body) with no environment resolving it | The error names `TOKEN` once, not twice — undefined variables are deduplicated in the message. |
| 16.40 | Type a name into the "new environment" field with nothing entered, check the `+ New` button | Button is disabled (greyed out) — an empty name can't be submitted. |
| 16.41 | Type `staging` into the field and click `+ New` | `staging` appears in the Environments list, is immediately selected, an "Edit \"staging\"" panel opens below (empty), the text field clears itself, and a green "created environment..." notice appears. Confirm with `curly env list` from a terminal — `staging` shows up there too. |
| 16.42 | In the `staging` editor, click `+ Add variable`, type a key/value (e.g. `FOO`/`bar`), leave "secret" unchecked, click **Save** | A green "saved 1 variable(s)..." notice appears. Confirm with `curly env show staging` from a terminal — shows `FOO=bar`. |
| 16.43 | Add another variable, check its "secret" box, type a key/value, click **Save** | `curly env show staging` shows the secret one masked as `***`, the non-secret one still in plain text — same masking `env show` always did, now reachable from data the GUI wrote. |
| 16.44 | Add a variable row, leave its key blank, fill in a value, click **Save** | The blank-key row is silently dropped — `curly env show staging` doesn't show a nameless entry, and the notice's count doesn't include it. |
| 16.45 | With `staging` selected and defining `{{FOO}}`, type `{{FOO}}` into the URL field's path and Send (to any reachable base) | Resolves to `bar` (or whatever you set) — confirms a GUI-created/edited environment is immediately usable for substitution, no restart needed. |
| 16.46 | Click a variable row's `✕` in the editor, then **Save** | That variable is gone from `curly env show staging` afterward — removal via the GUI persists the same as an addition does. |
| 16.47 | Click **Delete environment** on `staging` | `staging` disappears from the sidebar's Environments list immediately, the editor panel closes, a green "deleted environment..." notice appears, and `curly env list` from a terminal no longer lists it. |
| 16.48 | Click a different, pre-existing environment (e.g. `health-record-backend-spring`'s `global`), edit one of its real variable values in the editor, click **Save**, then click **`(none selected)`** without reloading anything, then re-click `global` | Editor reloads from disk showing your saved edit — confirms Save actually persisted rather than just updating the in-memory row. |

## 17. Cross-platform sanity (when releasing)

| # | Steps | Expected |
|---|---|---|
| 17.1 | `cargo build --release` on Linux, macOS, and Windows | Each produces a working binary with no OS-specific build errors. |
| 17.2 | Run scenario 1.1 and 4.1 on each OS | Same output shape on all three (path separators in `-o`/`--data-binary`/`--cacert` examples are the main thing to sanity-check on Windows). |
| 17.3 | Run scenario 10.6 (inspect the collection JSON file) on each OS | Confirms the data directory resolves correctly per-OS (`~/.local/share/curly`, `~/Library/Application Support/curly`, `%APPDATA%\curly`). |

## 18. Regression checklist for new flags/subcommands

When adding a new flag or subcommand, add at minimum:
- A unit test covering the pure parsing/building logic — `crates/curly-cli/src/one_shot.rs`, `args.rs`, or `commands/*.rs`'s `#[cfg(test)] mod tests` depending on where the logic lives, plus `crates/curly-core/tests/` for anything storage- or substitution-related.
- One row in the relevant table above exercising it against a real server (or a real temp storage directory for env/collections/history).
- A corresponding entry in [CLI.md](CLI.md) (an example in the relevant section + a reference entry).
