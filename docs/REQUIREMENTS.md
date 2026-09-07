# Curly — Requirements

## 1. Purpose

Curly is a REST API testing tool, positioned as a lighter, self-contained alternative to Postman for people who want a fast native app and a scriptable CLI, without an account, cloud sync, or Electron. Written in Rust; ships as native binaries for Linux, macOS, and Windows.

## 2. Goals

- One core engine, two interfaces: a curl-like CLI and a Postman-like GUI, sharing the same request model, execution engine, and on-disk storage format.
- Fast startup, small binary, no runtime dependencies (no Node/Electron/JVM).
- Everything stored locally in plain files (no forced account, no cloud).
- Good enough to replace Postman for the 80% case: build a request, send it, inspect the response, save it, organize it into collections, reuse variables across environments.

## 3. Non-goals (v1)

- Team collaboration / cloud sync / shared workspaces.
- Mock servers, API design/contract tooling (OpenAPI editing), or automated test-suite runners with assertions (may become a v2 goal).
- GraphQL/gRPC/WebSocket support (HTTP/1.1 and HTTP/2 REST only for v1).
- Plugin/extension system.
- Team/enterprise auth (SSO, RBAC).

## 4. Personas

- **CLI user**: developer scripting requests in a terminal or CI pipeline, wants curl-like ergonomics plus reusable named requests/environments.
- **GUI user**: developer or QA exploring/debugging an API interactively, wants request history, collections, and readable response viewing (JSON/XML/HTML formatting, headers, timing).

## 5. Functional Requirements

### 5.1 Shared core

- FR-1: Build and execute an HTTP request: method, URL, query params, headers, body (raw text, JSON, form-urlencoded, multipart/form-data, binary file).
- FR-2: Support HTTP/1.1 and HTTP/2, TLS (including client certs and custom CA bundles), redirects (follow/don't-follow, configurable max), timeouts, proxy configuration.
- FR-3: Authentication helpers: Basic, Bearer token, API key (header or query param), OAuth2 (authorization code + client credentials grants).
- FR-4: Variables and environments: named key/value sets, `{{variable}}` substitution in URL/headers/body, an active environment selectable per request/run, a global scope plus per-environment overrides.
- FR-5: Collections: named, ordered groups of saved requests, with folders/nesting.
- FR-6: Request history: every sent request (and its response) recorded locally, browsable and re-runnable.
- FR-7: Import: at minimum, Postman Collection v2.1 JSON and curl command strings. Export: Curly's own format at minimum; Postman-compatible export as a stretch goal.
- FR-8: Local persistence in plain files under a user config/data directory (see Design doc) — no external database server required.
- FR-9: Response handling: status, headers, body, timing (DNS/connect/TLS/TTFB/total), size; pretty-print JSON/XML/HTML; raw view toggle.
- FR-10: Post-response variable extraction (done, declaratively — see DESIGN.md §4/§9): pull a value from a response body (JSON path) or header, or compute one from a template, and persist it as a session variable for later requests. No scripting language — "none in v1" won out over embedding Rhai, since a declarative rule set covers the actual use case (chaining an auth token into the next request). Pre-request scripts remain out of scope.

### 5.2 CLI (`curly`)

- FR-11: One-shot mode mirroring curl ergonomics: `curly -X POST https://api.example.com/users -H "Content-Type: application/json" -d '{"name":"a"}'`.
- FR-12: Named/saved request mode: `curly run <collection>/<request-name> [--env <name>] [--var key=value]`.
- FR-13: Collection/environment management subcommands: list, add, edit (via `$EDITOR` or flags), delete, import, export.
- FR-14: Output formats: human-readable (default, colorized when TTY), `--json` machine-readable, `-o/--output` to write body to a file, `-i` to include response headers, `-v/--verbose` for request/response trace.
- FR-15: Exit codes usable in scripts/CI: non-2xx status optionally triggers non-zero exit (`--fail`, curl-style).
- FR-16: Non-interactive by default; safe to run in CI/pipelines with no prompts unless explicitly requested (e.g., interactive OAuth2 login flow).

### 5.3 GUI

- FR-17: Request builder: method/URL bar, tabs for Params/Headers/Body/Auth, send button, response pane (status/time/size, body/headers/timeline tabs).
- FR-18: Sidebar: collections tree (folders + requests), history list, environments switcher.
- FR-19: Multiple requests open in tabs simultaneously.
- FR-20: Save/rename/delete/duplicate/move requests and folders; drag-and-drop reordering (stretch).
- FR-21: Environment editor (key/value table, secret-value masking for tokens/passwords).
- FR-22: Response body viewer: syntax-highlighted JSON/XML/HTML, search within body, copy, save to file, image preview for image responses.
- FR-23: Keyboard shortcuts for send (Ctrl/Cmd+Enter), save (Ctrl/Cmd+S), new tab.
- FR-24: Light/dark theme, following OS preference by default.

## 6. Non-Functional Requirements

- NFR-1: Cross-platform: Linux, macOS (Intel + Apple Silicon), Windows — native binaries, no bundled browser/runtime.
- NFR-2: Cold start under ~300ms for CLI one-shot requests; GUI window visible under ~1s on typical hardware.
- NFR-3: No telemetry/analytics phoning home by default.
- NFR-4: Secrets (tokens, passwords in env vars) stored at rest with basic protection (file permissions at minimum; OS keychain integration as a stretch goal) and never logged in plaintext at verbose/debug levels without an explicit `--reveal-secrets`-style opt-in.
- NFR-5: CLI and GUI must read/write the same on-disk collection/environment files, so switching between them (or scripting around GUI-built collections) works without a conversion step.
- NFR-6: Reasonably testable: core request-building/variable-substitution/import-parsing logic covered by unit tests independent of network I/O.

## 7. Success Criteria (v1 "done")

- Can fully replace Postman + curl for the health-record project's own API testing workflow (JSON REST, Bearer/Basic auth, environments for local/dev/prod).
- A user can import an existing Postman collection and keep working without re-creating requests by hand.
- CLI and GUI operate on the same saved collections interchangeably.
- Single downloadable binary per OS for each of the CLI and GUI (or one binary that behaves as both, TBD in Design doc).

## 8. Open Questions

- One combined binary (`curly` vs `curly gui`) vs. two separate binaries (`curly` + `curly-gui`)? Leaning combined for distribution simplicity — see Design doc §3.
- Pre-request scripts remain unaddressed (post-response is done declaratively, no scripting language — see FR-10). Revisit only if a real need surfaces that the declarative extraction rules can't cover.
- Exact storage format: single JSON/TOML file per collection vs. SQLite. Leaning plain files for git-friendliness and NFR-5; see Design doc.
- OAuth2 flows requiring a local redirect listener — acceptable to spin up a temporary localhost server for the CLI's interactive login too, or GUI-only?
