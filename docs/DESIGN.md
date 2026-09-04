# Curly — Design

Status: draft, pre-implementation. Companion to [REQUIREMENTS.md](REQUIREMENTS.md).

## 1. Architecture Overview

A Cargo workspace with one shared library crate and two thin binary crates:

```
curly/
├── Cargo.toml                  # workspace
├── crates/
│   ├── curly-core/             # lib: model, HTTP execution, storage, import/export, var substitution
│   ├── curly-cli/               # bin "curly": clap-based CLI, calls curly-core
│   └── curly-gui/               # bin "curly-gui" (or feature-gated mode of curly): egui app, calls curly-core
├── docs/
│   ├── REQUIREMENTS.md
│   └── DESIGN.md
└── .github/workflows/          # CI: build+test on linux/mac/windows, release binaries
```

Neither binary crate contains business logic — they're presentation layers over `curly-core`. This is what makes NFR-5 (CLI and GUI share files/behavior) fall out naturally instead of needing to be maintained by hand.

## 2. Technology Choices

| Concern | Choice | Why |
|---|---|---|
| HTTP client | `reqwest` (with `rustls-tls`, not native-tls) | Batteries-included, async, HTTP/2, proxy/redirect/cert support out of the box; `rustls` avoids linking OpenSSL per-platform (simplifies Windows/macOS builds). |
| Async runtime | `tokio` | Required by `reqwest`; GUI wraps calls via a background task + channel so the UI thread never blocks. |
| CLI parsing | `clap` (derive) | De facto standard, generates `--help`, shell completion. |
| GUI framework | `egui` + `eframe` | Pure Rust, immediate-mode, compiles to a single native binary per OS with no embedded webview/JS runtime — matches NFR-1 and keeps packaging simple. Trade-off: less polished widget aesthetics than a web-tech UI (Tauri); acceptable for v1's functional-over-pretty goal. |
| Serialization | `serde` + `serde_json` (+ `toml` for config) | Standard; collections/environments/history stored as JSON. |
| Storage | Plain files on disk (see §4), no embedded DB in v1 | Git-friendly (diffable, so a collection dir can itself be a project's repo artifact), matches how Postman collections are already shared as JSON, avoids a SQLite dependency for v1's data volumes. Revisit only if history/search performance demands it. |
| Variable substitution | Hand-rolled `{{var}}` scanner in `curly-core` | Requirement is simple (literal token replace against active environment); no templating engine needed. |
| Import (Postman v2.1) | `serde_json` + custom mapping | Postman's schema is public JSON; write a `PostmanCollection` deserialize target and map into Curly's model. |
| curl-string import | Hand-rolled tokenizer for common curl flags (`-X -H -d --data -u -F`) | Full curl flag coverage is out of scope; support the common subset. |
| Secrets at rest | OS keychain via `keyring` crate for tokens marked "secret"; fallback to file with `0600` perms where no keychain is available (e.g. some Linux CI containers) | Meets NFR-4 without inventing crypto. |
| Logging | `tracing` | Structured, works for both `-v` CLI trace output and future debugging. |

## 3. One Binary or Two?

Decision: **one binary**, `curly`, that runs as CLI by default and launches the GUI when invoked with no subcommand/args (or `curly gui`). Rationale: simpler release/download story (one artifact per OS, matches curl's own single-binary model), and `curly-core`/`curly-cli` stay usable as a lean CLI-only build via a Cargo feature flag (`--no-default-features --features cli`) for anyone who wants a smaller binary without GUI deps. `curly-gui` crate still exists internally for code organization; it's linked into the same final binary rather than shipped separately.

Revisit if GUI dependencies meaningfully bloat the CLI-only use case (e.g., CI environments pulling a binary that links a windowing toolkit); in that case fall back to two published binaries built from the same crates.

## 4. Data Model & Storage Layout

Root data directory (`dirs::data_dir()` — e.g. `~/.local/share/curly` on Linux, `~/Library/Application Support/curly` on macOS, `%APPDATA%\curly` on Windows):

```
curly/
├── collections/
│   └── <collection-slug>/
│       ├── collection.json      # metadata + folder tree + request refs
│       └── requests/
│           └── <request-id>.json
├── environments/
│   └── <env-slug>.json
├── history/
│   └── <yyyy-mm-dd>.jsonl       # append-only, one JSON object per sent request+response
└── config.toml                  # app-level settings: theme, default timeout, active environment
```

Core types (`curly-core::model`), sketched:

```rust
struct Request {
    id: Uuid,
    name: String,
    method: Method,          // GET/POST/PUT/PATCH/DELETE/HEAD/OPTIONS
    url: String,              // may contain {{vars}}
    query_params: Vec<(String, String, bool /* enabled */)>,
    headers: Vec<(String, String, bool)>,
    body: Body,                // enum: None, Raw(String, ContentType), Form(Vec<(String,String)>), Multipart(...), Binary(PathBuf)
    auth: Auth,                 // enum: None, Basic{..}, Bearer{..}, ApiKey{..}, OAuth2{..}
}

struct Collection {
    id: Uuid,
    name: String,
    folders: Vec<Folder>,       // nested
    requests: Vec<RequestRef>,  // ids, ordered
}

struct Environment {
    id: Uuid,
    name: String,
    variables: Vec<(String, String, bool /* secret */)>,
}

struct HistoryEntry {
    request_snapshot: Request,  // resolved (post-substitution) request actually sent
    response: ResponseSummary,  // status, headers, body (truncated/hashed if too large), timing
    sent_at: DateTime<Utc>,
}
```

`curly-core::exec` owns building the `reqwest::Request` from a `Request` + active `Environment` (substituting variables), sending it, and producing a `ResponseSummary`. This is the one function both CLI and GUI call.

## 5. CLI Design

Mirrors curl for one-shot use, adds subcommands for saved-request/collection management:

```
curly -X POST https://api.example.com/users -H "Content-Type: application/json" -d '{"name":"a"}'
curly run my-api/get-user --env dev --var id=42
curly collections list
curly collections import postman ./exported.json
curly env list / env show dev / env set dev API_KEY=... [--secret]
curly history [--limit 20]
```

Flags on the one-shot form follow curl naming where they overlap (`-X`, `-H`, `-d`/`--data`, `-u`, `-F`, `-o`, `-i`, `-v`, `-k`/`--insecure`) so existing muscle memory transfers; `--env`/`--var` are Curly additions for saved-request runs.

## 6. GUI Design (v1 scope)

Single window, `egui` immediate-mode layout:

- Left sidebar: tabs for **Collections** (tree view) / **History** / **Environments**, with an environment dropdown pinned at the top.
- Main area: tab strip of open requests; each tab = method dropdown + URL bar + Send button, then sub-tabs (Params / Headers / Body / Auth), then a response pane below (Status/Time/Size line, then Body/Headers/Timing sub-tabs).
- Response body viewer: use a JSON pretty-printer + a simple token-based syntax highlighter (candidate crate: `syntect` if binary size is acceptable, else hand-rolled minimal JSON highlighter to avoid pulling in Sublime-syntax grammars).
- Async sends: UI dispatches to a `tokio` task via channel, keeps a per-tab "in flight" flag, never blocks the render loop.

## 7. Cross-Platform Build & Release

- `Cargo.toml` workspace, no OS-specific code beyond what `reqwest`(`rustls`)/`eframe`/`keyring`/`dirs` already abstract.
- GitHub Actions matrix build (`ubuntu-latest`, `macos-latest`, `windows-latest`) on tag push: `cargo build --release`, upload artifacts, attach to a GitHub Release. macOS: build both `x86_64-apple-darwin` and `aarch64-apple-darwin`, optionally lipo into a universal binary.
- No installer/package-manager distribution in v1 (no Homebrew tap, no MSI, no .deb) — raw binaries on GitHub Releases is enough to start; revisit once there are users beyond the author.

## 8. Testing Strategy

- `curly-core`: unit tests for variable substitution, request-building (headers/body/auth assembly), Postman import mapping, curl-string parsing — all pure functions, no network needed.
- Integration tests against a local mock server (`wiremock` or a minimal `axum` test server) for the actual send/receive path.
- CLI: snapshot-style tests on stdout formatting for a few canonical requests.
- GUI: manual testing only for v1 (immediate-mode UI testing tooling in Rust is immature); revisit if a regression-prone area emerges.

## 9. Milestones (maps to REQUIREMENTS §5, rough only — no dates yet)

1. **M1 — core + CLI one-shot** ✅ **done**: `curly-core` model/exec, CLI one-shot mode (FR-1..3, FR-9, FR-11, FR-14..16). No storage yet — a working curl replacement.
   - FR-1 (method/URL/query/headers/body — raw, form-urlencoded, multipart incl. file upload, binary file): done.
   - FR-2 (HTTP/1.1+2 via reqwest, TLS incl. `--cacert`/`--cert`/`--key`, redirects via `-L`/`--max-redirects`, `--max-time`, `-x`/`--proxy`): done.
   - FR-3 (Basic via `-u`, Bearer via `--bearer`; API key covered by existing `-H`/`-Q`): done. OAuth2 explicitly deferred to M5 (interactive flow, out of one-shot-CLI scope).
   - FR-9 (status/headers/body/timing; JSON pretty-print via `-p`): done. DNS/connect/TLS/TTFB timing *breakdown* (vs. total elapsed) not implemented — would need low-level `hyper` connection hooks; not pursued for CLI v1.
   - FR-11, FR-14 (`--json`), FR-15 (`--fail`), FR-16: done.
   - NFR-6: `curly-cli`'s request/client-building logic is pure and unit tested (24 tests); `curly-core::exec` has `wiremock`-based integration tests (8 tests) per §8.
   - Not done / explicitly out of scope for M1: colorized TTY output, XML/HTML pretty-printing (JSON only).
2. **M2 — storage + saved requests/collections/environments** ✅ **done**: FR-4..8, FR-12..13 CLI subcommands.
   - FR-4 (variables/environments, `{{var}}` substitution, global + per-environment scope): done via `curly-core::substitution` + `curly env`. Deliberate deviation from Postman: substitution errors on any undefined variable (naming all of them) rather than silently leaving `{{var}}` in place — see substitution.rs's module doc for rationale.
   - FR-5 (named, ordered collections of saved requests, folders/nesting): done except folder nesting — collections are a flat, ordered list for M2 (deferred until the GUI needs the tree view; `Folder`/`RequestRef` from the original DESIGN.md sketch aren't implemented).
   - FR-6 (history, browsable): done via `curly history`. Not done: re-running from history, viewing one entry's full detail (only the summary list exists).
   - FR-8 (local plain-file persistence): done, but the on-disk layout deviates from this doc's original §4 sketch — see storage.rs's module doc: one `collections/<slug>.json` file per collection with requests embedded inline, instead of `collection.json` + a `requests/<id>.json` per request. Simpler to implement correctly, still git-diffable as a whole, still satisfies NFR-5.
   - FR-12 (`curly run <collection>/<request-name> --env --var`): done.
   - FR-13 (collection/environment management subcommands): done for list/create-or-set/delete/show; `import`/`export` subcommands from this doc's §5 CLI sketch are M3's job (Postman import) and not yet implemented. No `$EDITOR`-based editing — editing a saved request is currently remove-request + add-request again.
   - NFR-4 (secrets not logged in plaintext): `env set --secret` masks values in `env show`; history entries redact `Authorization`/`X-Api-Key`/`Cookie` header values. Not done: OS-keychain storage (`--secret` values are still plain JSON on disk) — deferred to M5 per this doc's tech-choices table.
   - NFR-6: 57 tests total — `curly-core` unit+integration (32: substitution, storage via `tempfile`, `wiremock` exec) + `curly-cli` unit (25: request/client building, collections add-request body/auth parsing).
   - Not done / explicitly out of scope for M2: Postman/curl import (M3), folder nesting, editing saved requests in place, re-running from history, OS keychain.
3. **M3 — Postman import**: FR-7.
4. **M4 — GUI v1**: FR-17..24 against the by-then-stable core.
5. **M5 — polish**: OAuth2 flows, secret storage via keychain, packaging/CI release pipeline.

## 10. Open Design Questions

- `syntect` vs. hand-rolled highlighter for response body — decide once GUI work starts and binary size is measured.
- Whether `curly-cli` needs its own crate boundary at all vs. folding directly into the top-level binary crate — kept separate for now since it costs little and keeps `curly-gui` swappable/optional per §3.
