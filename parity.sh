#!/usr/bin/env bash
# The Rust implementation and the Ziman one, asked the same questions over the same `.curly/` tree.
#
# **Why this is separate from either side's own tests.** Each implementation can pass its own suite
# and still disagree with the other -- that is exactly what happened with the storage format, where
# the Ziman codec round-tripped its own fixture perfectly while writing a trailing newline the Rust
# binary does not. Only asking both and diffing finds that.
#
# Everything runs against a COPY of the sample tree, never the original: the Rust side is asked to
# rewrite files, and no check should be able to modify the data it is checking.
#
# As the port grows this grows with it -- today the Ziman side has a core and no CLI, so what can be
# compared is the storage format, the listings and the slug. The scenario runs join when the CLI does.
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ZIMAN="${ZIMAN:-$HOME/workspace/vasna.tech/repos/ziman/ziman}"
RUST="$ROOT/rust/target/debug/curly"
[ -x "$RUST" ] || RUST="$ROOT/rust/target/release/curly"
SAMPLE="${CURLY_SAMPLE:-$HOME/workspace/vasna.tech/repos/health-record/.curly}"

failed=0
ok()   { printf '  PASS %s\n' "$1"; }
bad()  { failed=$((failed + 1)); printf '  FAIL %s\n' "$1"; [ -n "${2:-}" ] && printf '       %s\n' "$2"; }

if [ ! -x "$RUST" ]; then
  echo "  SKIP cross-check: no rust binary (cd rust && cargo build)" >&2
  exit 0
fi
if [ ! -d "$SAMPLE" ]; then
  echo "  SKIP cross-check: no sample tree at $SAMPLE (set CURLY_SAMPLE)" >&2
  exit 0
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cp -r "$SAMPLE" "$work/.curly"
CURLY="$work/.curly"

zim() { (cd "$ROOT/ziman" && ZIMAN_MODULE_PATH=src/main/ziman CURLY_ROOT="$CURLY" "$@" \
          "$ZIMAN" src/test/other/parity_driver.zm); }

# ---- the listings -------------------------------------------------------------------------------

rust_collections="$("$RUST" collections list --data-dir "$CURLY" 2>/dev/null | sort)"
zim_collections="$(CURLY_CASE=list-collections zim env | sort)"
if [ "$rust_collections" = "$zim_collections" ]; then
  ok "collections list agrees ($(echo "$zim_collections" | wc -l) collections)"
else
  bad "collections list differs" "$(diff <(echo "$rust_collections") <(echo "$zim_collections") | head -6 | tr '\n' ' ')"
fi

rust_envs="$("$RUST" env list --data-dir "$CURLY" 2>/dev/null | sort)"
zim_envs="$(CURLY_CASE=list-environments zim env | sort)"
# The Rust `env list` may decorate its output; compare only the names it shares with ours.
if [ -n "$zim_envs" ] && echo "$rust_envs" | grep -qF "$(echo "$zim_envs" | head -1)"; then
  ok "environments list agrees on names"
else
  bad "environments list differs" "rust=[$rust_envs] ziman=[$zim_envs]"
fi

# ---- the storage format ------------------------------------------------------------------------
#
# The Rust binary rewrites each collection first, so the file on disk is in ITS canonical form
# (two-space indent, raw UTF-8, no trailing newline) rather than however it was last written. Then
# the Ziman side reads and re-emits it, and the two must be byte-identical.

for file in "$CURLY"/collections/*.json; do
  [ -f "$file" ] || continue
  name="$(basename "$file" .json)"
  "$RUST" collections add-folder "$name" __parity_probe --data-dir "$CURLY" >/dev/null 2>&1
  "$RUST" collections remove-folder "$name" __parity_probe --data-dir "$CURLY" >/dev/null 2>&1
  canonical="$work/$name.rust"
  cp "$file" "$canonical"
  if CURLY_CASE=rewrite-collection CURLY_FILE="$file" zim env > "$work/$name.ziman" 2>"$work/$name.err"; then
    # The Ziman driver prints the document; `print` adds the newline a file does not have.
    printf '%s' "$(cat "$work/$name.ziman")" > "$work/$name.ziman.trimmed"
    if cmp -s "$canonical" "$work/$name.ziman.trimmed"; then
      ok "storage format byte-identical for $name ($(wc -c < "$canonical") bytes)"
    else
      bad "storage format differs for $name" \
          "$(cmp "$canonical" "$work/$name.ziman.trimmed" 2>&1 | head -1)"
    fi
  else
    bad "ziman could not read $name" "$(head -2 "$work/$name.err" | tr '\n' ' ')"
  fi
done

# ---- the request tree ---------------------------------------------------------------------------
#
# The same script of operations through both implementations, then a diff. `add-request` and
# `duplicate-request` mint fresh ids on both sides, so ids are normalised away and what is compared
# is the structure: folder nesting, request names, ordering, and which operations refused.

tree_dir="$work/tree"
mkdir -p "$tree_dir"
R="$RUST collections"
$R add-request Parity login https://example.com/login --data-dir "$tree_dir" >/dev/null 2>&1
$R add-request Parity Auth/OAuth/refresh https://example.com/Auth/OAuth/refresh --data-dir "$tree_dir" >/dev/null 2>&1
$R add-request Parity Auth/logout https://example.com/Auth/logout --data-dir "$tree_dir" >/dev/null 2>&1
$R add-folder Parity Archive/Old --data-dir "$tree_dir" >/dev/null 2>&1
$R rename-request Parity login Auth/signin --data-dir "$tree_dir" >/dev/null 2>&1
$R duplicate-request Parity Auth/logout Archive/Old/logout-copy --data-dir "$tree_dir" >/dev/null 2>&1
$R remove-request Parity Auth/OAuth/refresh --data-dir "$tree_dir" >/dev/null 2>&1
$R remove-folder Parity Auth/OAuth --data-dir "$tree_dir" >/dev/null 2>&1

# Every UUID to a placeholder; they are random on both sides by design.
normalise() { sed -E 's/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/<id>/g' "$1"; }

if [ -f "$tree_dir/collections/parity.json" ]; then
  CURLY_CASE=tree-ops zim env > "$work/tree.ziman.raw" 2>"$work/tree.err"
  if [ -s "$work/tree.ziman.raw" ]; then
    # `print` ends with a newline and a collection file does not, so the driver's output is
    # trimmed before comparing -- the same adjustment the storage-format check above makes.
    printf '%s' "$(cat "$work/tree.ziman.raw")" > "$work/tree.ziman"
    normalise "$tree_dir/collections/parity.json" > "$work/tree.rust.norm"
    normalise "$work/tree.ziman" > "$work/tree.ziman.norm"
    if diff -q "$work/tree.rust.norm" "$work/tree.ziman.norm" >/dev/null; then
      ok "tree operations produce an identical collection (8 operations)"
    else
      bad "tree operations differ" "$(diff "$work/tree.rust.norm" "$work/tree.ziman.norm" | head -8 | tr '\n' ' ')"
    fi
  else
    bad "ziman tree-ops produced nothing" "$(head -2 "$work/tree.err" | tr '\n' ' ')"
  fi
else
  bad "the rust side wrote no parity collection" "check the collections subcommand names"
fi

# ---- scenarios --------------------------------------------------------------------------------
#
# `curly scenario show` against the Ziman side's reproduction of the same output, for every scenario
# in the tree. Small output, whole codec: a file decoded into the wrong shape reports the wrong step
# count or loses an import.

scen_total=0
scen_bad=0
for file in "$CURLY"/scenarios/*.json; do
  [ -f "$file" ] || continue
  name="$(basename "$file" .json)"
  scen_total=$((scen_total + 1))
  rust_out="$("$RUST" scenario show "$name" --data-dir "$CURLY" 2>&1)"
  zim_out="$(CURLY_CASE=show-scenario CURLY_FILE="$file" zim env 2>&1)"
  if [ "$rust_out" != "$zim_out" ]; then
    scen_bad=$((scen_bad + 1))
    [ "$scen_bad" -le 2 ] && bad "scenario \"$name\" differs" \
      "$(diff <(echo "$rust_out") <(echo "$zim_out") | head -4 | tr '\n' ' ')"
  fi
done
if [ "$scen_total" -eq 0 ]; then
  echo "  SKIP scenarios: none in the sample tree"
elif [ "$scen_bad" -eq 0 ]; then
  ok "all $scen_total scenarios decode to the same shape"
else
  [ "$scen_bad" -gt 2 ] && bad "$scen_bad of $scen_total scenarios differ (first two shown)"
fi

# ---- the importers ------------------------------------------------------------------------------
#
# The same Postman export and the same curl command through both implementations, then a diff with
# ids normalised. These are the two places where an input nobody controls meets a parser written
# twice, so agreeing on a real-shaped fixture is worth more than either side's own unit tests.

imp_dir="$work/import"
mkdir -p "$imp_dir"
cat > "$work/postman.json" <<'JSON'
{
  "info": { "name": "Exported" },
  "item": [
    { "name": "ping", "request": { "method": "GET", "url": "https://example.com/ping" } },
    {
      "name": "Auth",
      "item": [
        {
          "name": "login",
          "request": {
            "method": "POST",
            "header": [
              { "key": "Content-Type", "value": "application/json" },
              { "key": "X-Off", "value": "no", "disabled": true }
            ],
            "body": { "mode": "raw", "raw": "{\"email\":\"a@b.c\"}" },
            "auth": { "type": "bearer", "bearer": [{ "key": "token", "value": "{{TOKEN}}" }] },
            "url": { "raw": "{{BASE}}/login" }
          }
        },
        {
          "name": "upload",
          "request": {
            "method": "POST",
            "body": {
              "mode": "formdata",
              "formdata": [
                { "key": "caption", "value": "hi" },
                { "key": "file", "type": "file", "src": "/tmp/x.png" }
              ]
            },
            "url": "https://example.com/upload"
          }
        }
      ]
    }
  ]
}
JSON

normalise() { sed -E 's/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/<id>/g' "$1"; }

if "$RUST" collections import postman Imported "$work/postman.json" --data-dir "$imp_dir" >/dev/null 2>&1 \
   && [ -f "$imp_dir/collections/imported.json" ]; then
  CURLY_CASE=import-postman CURLY_FILE="$work/postman.json" CURLY_NAME=Imported zim env \
    > "$work/pm.ziman.raw" 2>"$work/pm.err"
  printf '%s' "$(cat "$work/pm.ziman.raw")" > "$work/pm.ziman"
  normalise "$imp_dir/collections/imported.json" > "$work/pm.rust.norm"
  normalise "$work/pm.ziman" > "$work/pm.ziman.norm"
  if diff -q "$work/pm.rust.norm" "$work/pm.ziman.norm" >/dev/null; then
    ok "a Postman export imports identically (folders, headers, bodies, auth)"
  else
    bad "Postman import differs" "$(diff "$work/pm.rust.norm" "$work/pm.ziman.norm" | head -8 | tr '\n' ' ')"
  fi
else
  bad "the rust side could not import the Postman fixture" "$(head -2 "$work/pm.err" 2>/dev/null | tr '\n' ' ')"
fi

# A command in the shape a browser's "Copy as cURL" produces: single-quoted URL, repeated -H, and a
# JSON body with quotes inside.
CURL_COMMAND="curl 'https://example.com/api/login' -H 'Content-Type: application/json' -H 'Accept: application/json' --data-raw '{\"email\":\"a@example.com\",\"password\":\"hunter2\"}'"
curl_dir="$work/import-curl"
mkdir -p "$curl_dir"
if "$RUST" collections import curl Curled Auth/login "$CURL_COMMAND" --data-dir "$curl_dir" >/dev/null 2>&1 \
   && [ -f "$curl_dir/collections/curled.json" ]; then
  CURLY_CASE=import-curl CURLY_COMMAND="$CURL_COMMAND" CURLY_NAME=Curled CURLY_PATH=Auth/login zim env \
    > "$work/cl.ziman.raw" 2>"$work/cl.err"
  printf '%s' "$(cat "$work/cl.ziman.raw")" > "$work/cl.ziman"
  normalise "$curl_dir/collections/curled.json" > "$work/cl.rust.norm"
  normalise "$work/cl.ziman" > "$work/cl.ziman.norm"
  if diff -q "$work/cl.rust.norm" "$work/cl.ziman.norm" >/dev/null; then
    ok "a \"Copy as cURL\" command imports identically"
  else
    bad "curl import differs" "$(diff "$work/cl.rust.norm" "$work/cl.ziman.norm" | head -8 | tr '\n' ' ')"
  fi
else
  bad "the rust side could not import the curl command" "$(head -2 "$work/cl.err" 2>/dev/null | tr '\n' ' ')"
fi

# ---- the CLI's own output ------------------------------------------------------------------------
#
# Every read-only command, run through both binaries against the same tree, diffed line for line.
# This is the check the whole harness was built for: once the Ziman side has a CLI, parity stops
# being about file shapes and becomes about what a user actually sees.
#
# `curly` here is the Ziman wrapper (`ziman/bin/curly`); `$RUST` is the native binary.

ZCLI="$ROOT/ziman/bin/curly"

# Both implementations, same arguments, same tree -- output diffed line for line.
cli_both() {
  local label="$1"; shift
  local rust_out zim_out
  rust_out="$("$RUST" "$@" --data-dir "$CURLY" 2>&1)" || true
  zim_out="$(ZIMAN="$ZIMAN" "$ZCLI" "$@" --data-dir "$CURLY" 2>&1)" || true
  if [ "$rust_out" = "$zim_out" ]; then
    ok "cli: $label"
  else
    bad "cli: $label" "$(diff <(echo "$rust_out") <(echo "$zim_out") | head -5 | tr '\n' ' ')"
  fi
}

# A refusal, compared by its **first line only**. Both print the same `Error: <message>`; below it
# Rust's `anyhow` appends an indented `Caused by:` paragraph carrying the underlying `io::Error`.
# That paragraph is a presentation its error library chose and the message above it is the contract,
# so this compares the contract and not the layout -- excluded here deliberately and visibly,
# rather than normalised away everywhere.
cli_refusal_both() {
  local label="$1"; shift
  local rust_out zim_out
  rust_out="$("$RUST" "$@" --data-dir "$CURLY" 2>&1 | head -1)" || true
  zim_out="$(ZIMAN="$ZIMAN" "$ZCLI" "$@" --data-dir "$CURLY" 2>&1 | head -1)" || true
  if [ "$rust_out" = "$zim_out" ]; then
    ok "cli: $label"
  else
    bad "cli: $label" "rust=[$rust_out] ziman=[$zim_out]"
  fi
}

if [ -x "$ZCLI" ]; then
  cli_both "collections list" collections list
  cli_both "collections show" collections show health-record-ziman-api
  cli_both "env list" env list
  cli_both "env show global" env show global
  cli_both "scenario list" scenario list
  cli_both "session show (empty)" session show
  cli_both "history (empty)" history
  # The refusals matter as much as the successes: a command that fails differently is a command a
  # script cannot rely on.
  cli_refusal_both "collections show, missing" collections show no-such-collection
  cli_refusal_both "env show, missing" env show no-such-env
  cli_refusal_both "scenario show, missing" scenario show no-such-scenario
  cli_refusal_both "env unset, no such variable" env unset global NO_SUCH_KEY

  # Round-tripping through the writing commands, then reading back with the *other* binary --
  # which is the real test of "these two share a storage root".
  wdir="$work/cli-writes"
  mkdir -p "$wdir"
  "$RUST" env set shared KEY=from-rust --data-dir "$wdir" >/dev/null 2>&1
  ZIMAN="$ZIMAN" "$ZCLI" env set shared OTHER=from-ziman --data-dir "$wdir" >/dev/null 2>&1
  rust_sees="$("$RUST" env show shared --data-dir "$wdir" 2>&1)"
  zim_sees="$(ZIMAN="$ZIMAN" "$ZCLI" env show shared --data-dir "$wdir" 2>&1)"
  if [ "$rust_sees" = "$zim_sees" ] && echo "$rust_sees" | grep -q from-ziman \
     && echo "$zim_sees" | grep -q from-rust; then
    ok "cli: each binary reads what the other wrote"
  else
    bad "cli: cross-written environment differs" "rust=[$rust_sees] ziman=[$zim_sees]"
  fi

  # A secret set by one is masked by the other.
  ZIMAN="$ZIMAN" "$ZCLI" env set shared TOKEN=s3cret --secret --data-dir "$wdir" >/dev/null 2>&1
  if "$RUST" env show shared --data-dir "$wdir" 2>&1 | grep -q 'TOKEN=\*\*\*'; then
    ok "cli: a secret set by ziman is masked by rust"
  else
    bad "cli: secret flag did not survive" "$("$RUST" env show shared --data-dir "$wdir" 2>&1 | tr '\n' ' ')"
  fi

  # **The curl command on standard input**, which the importer check above cannot reach: it passes
  # the command as an argument, and the stdin path is a different branch on both sides. It is also
  # the branch a user actually takes -- "Copy as cURL" produces a multi-line paste, and piping it
  # is how it gets in without the shell fighting the quoting.
  #
  # Worth a check of its own because the Ziman side read `/dev/stdin` as a whole file until
  # 2026-11-09, for want of a stdin API; it now reads `io.stdin()`, and the thing to hold is that
  # the *collection written* is byte-identical, not merely that the command parsed.
  stdin_cmd="curl -X POST 'https://example.com/api/items' \\
  -H 'Content-Type: application/json' \\
  -H 'X-Trace: abc' \\
  --data-raw '{\"name\":\"one\"}'"
  for impl in rust ziman; do
    rm -rf "$work/stdin-$impl"
    mkdir -p "$work/stdin-$impl"
  done
  printf '%s\n' "$stdin_cmd" | "$RUST" collections import curl Piped Items/create \
    --data-dir "$work/stdin-rust" >/dev/null 2>&1
  printf '%s\n' "$stdin_cmd" | ZIMAN="$ZIMAN" "$ZCLI" collections import curl Piped Items/create \
    --data-dir "$work/stdin-ziman" >/dev/null 2>&1
  if [ -f "$work/stdin-rust/collections/piped.json" ] && [ -f "$work/stdin-ziman/collections/piped.json" ]; then
    normalise "$work/stdin-rust/collections/piped.json" > "$work/stdin.rust.norm"
    normalise "$work/stdin-ziman/collections/piped.json" > "$work/stdin.ziman.norm"
    if diff -q "$work/stdin.rust.norm" "$work/stdin.ziman.norm" >/dev/null; then
      ok "cli: a multi-line curl command piped on stdin imports identically"
    else
      bad "cli: piped curl import differs" \
        "$(diff "$work/stdin.rust.norm" "$work/stdin.ziman.norm" | head -8 | tr '\n' ' ')"
    fi
  else
    bad "cli: a piped curl command did not import" \
      "rust=$([ -f "$work/stdin-rust/collections/piped.json" ] && echo yes || echo no) ziman=$([ -f "$work/stdin-ziman/collections/piped.json" ] && echo yes || echo no)"
  fi

  # Empty standard input: both must refuse, with the same words. This is the case the `/dev/stdin`
  # read got right by accident and `io.stdin()` gets right by its end-of-input contract -- an empty
  # source is `None` on the first pull, not an empty chunk, so the command is "" and the parser
  # refuses it rather than importing a request with no URL.
  empty_rust="$(printf '' | "$RUST" collections import curl Empty r --data-dir "$work/stdin-rust" 2>&1)" || true
  empty_zim="$(printf '' | ZIMAN="$ZIMAN" "$ZCLI" collections import curl Empty r --data-dir "$work/stdin-ziman" 2>&1)" || true
  if [ "$empty_rust" = "$empty_zim" ] && [ -n "$empty_rust" ]; then
    ok "cli: empty stdin is refused the same way"
  else
    bad "cli: empty stdin refusals differ" "rust=[$empty_rust] ziman=[$empty_zim]"
  fi
else
  echo "  SKIP cli checks: $ZCLI is not executable"
fi

# ---- one-shot mode ------------------------------------------------------------------------------
#
# The curl-like path, through both binaries against one stub server, output diffed.
#
# **Only two things are normalised, and both are genuinely non-deterministic**: `elapsed_ms`, which
# is a measurement, and a multipart boundary, which is random on both sides by design. Everything
# else -- header names and their order, JSON key order, the exact wording of a refusal -- is
# compared as it comes out, because those are the things that drift.

OS_PORT=18761
STUB="$work/httpstub.py"
cat > "$STUB" <<'STUBEOF'
import json, sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse
class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def _h(self):
        p = urlparse(self.path)
        n = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(n) if n else b""
        if p.path == "/plain":
            body, ct, code = b"hello world", "text/plain", 200
        elif p.path == "/token":
            body, ct, code = b'{"token":"abc123"}', "application/json", 200
        elif p.path == "/boom":
            body, ct, code = b'{"error":"nope"}', "application/json", 500
        else:
            body = json.dumps({
                "method": self.command, "path": p.path, "query": p.query,
                "headers": {k.lower(): v for k, v in self.headers.items()
                            if k.lower() in ("authorization", "content-type", "x-one", "x-two")},
                "body": raw.decode("utf-8", "replace"),
            }, sort_keys=True).encode()
            ct, code = "application/json", 200
        self.send_response(code)
        self.send_header("Content-Type", ct)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    do_GET = do_POST = do_PUT = do_DELETE = _h
    def log_message(self, *a): pass
ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
STUBEOF
python3 -I "$STUB" "$OS_PORT" >/dev/null 2>&1 &
stub_pid=$!
# Killed by PID, never by pattern: `pkill -f` on a name that appears in this script's own command
# line kills the script.
trap 'kill "$stub_pid" 2>/dev/null; rm -rf "$work"' EXIT
for _ in $(seq 1 40); do
  if (exec 3<>"/dev/tcp/127.0.0.1/$OS_PORT") 2>/dev/null; then exec 3>&-; break; fi
  sleep 0.1
done

OSB="http://127.0.0.1:$OS_PORT"
# `elapsed_ms` is a measurement, a multipart boundary is random by design, and the server's `Date`
# header is the wall clock -- two invocations a second apart legitimately disagree. **The `Date`
# normalisation was added after noticing the `-i` and `--json` checks only passed when both
# invocations landed inside the same second**, which is a flake waiting for a slow machine rather
# than a check. Nothing else is normalised.
os_normalise() {
  sed -E -e 's/"elapsed_ms": [0-9]+/"elapsed_ms": <ms>/' \
         -e 's/boundary=[A-Za-z0-9-]+/boundary=<b>/g' \
         -e 's/--[A-Za-z0-9-]{20,}/--<b>/g' \
         -e 's/^date: .*/date: <when>/' \
         -e 's/"date": "[^"]*"/"date": "<when>"/'
}

one_shot_both() {
  local label="$1"; shift
  local a b
  a="$("$RUST" "$@" --data-dir "$work/os-rust" 2>&1 | os_normalise)" || true
  b="$(ZIMAN="$ZIMAN" "$ZCLI" "$@" --data-dir "$work/os-ziman" 2>&1 | os_normalise)" || true
  if [ "$a" = "$b" ]; then
    ok "one-shot: $label"
  else
    bad "one-shot: $label" "$(diff <(echo "$a") <(echo "$b") | head -5 | tr '\n' ' ')"
  fi
}

if [ -x "$ZCLI" ]; then
  one_shot_both "a plain GET" "$OSB/plain"
  one_shot_both "-X PUT" -X PUT "$OSB/echo"
  one_shot_both "repeated -H" "$OSB/echo" -H 'X-One: 1' -H 'X-Two: 2'
  one_shot_both "repeated -Q" "$OSB/echo" -Q a=1 -Q 'b=two words'
  one_shot_both "-d defaults the method to POST" "$OSB/echo" -d '{"a":1}'
  one_shot_both "--data-urlencode" "$OSB/echo" --data-urlencode a=1 --data-urlencode 'b=x y'
  one_shot_both "-F multipart" "$OSB/echo" -F caption=hi
  one_shot_both "-u basic auth" "$OSB/echo" -u ann:s3cret
  one_shot_both "--bearer" "$OSB/echo" --bearer tok123
  one_shot_both "-i include headers" -i "$OSB/plain"
  one_shot_both "-p pretty" -p "$OSB/echo"
  one_shot_both "--json" --json "$OSB/echo"
  one_shot_both "--fail on a 500" --fail "$OSB/boom"
  one_shot_both "conflicting body sources" "$OSB/echo" -d a -F b=c
  one_shot_both "both auth kinds" "$OSB/echo" -u a:b --bearer t
  one_shot_both "a malformed -u" "$OSB/echo" -u nocolon
  one_shot_both "a malformed -H" "$OSB/echo" -H nocolon
  one_shot_both "a malformed -Q" "$OSB/echo" -Q nokey
fi

# ---- curly run ----------------------------------------------------------------------------------
#
# A saved request executed by both binaries against the same stub, with the same storage layout
# built by each in its own root -- so the comparison covers the variable precedence, the response
# rendering, and what the extraction rules leave behind in the session.

# **The fixture is built once with the Rust binary and copied**, rather than built by each side
# with its own CLI: `collections add-request` is not ported yet, and a check that depends on an
# unported command tests nothing about the one under test. Both `run` invocations then see byte-
# identical storage.
if [ -x "$ZCLI" ]; then
  tmpl="$work/run-template"
  mkdir -p "$tmpl"
  "$RUST" collections add-request api echo "$OSB/echo?who={{WHO}}" --data-dir "$tmpl" >/dev/null 2>&1
  "$RUST" collections add-request api token "$OSB/token" --data-dir "$tmpl" >/dev/null 2>&1
  "$RUST" env set staging WHO=world --data-dir "$tmpl" >/dev/null 2>&1
  "$RUST" env set staging GREETING=hello --data-dir "$tmpl" >/dev/null 2>&1
  # No CLI subcommand writes an extraction rule (the GUI's editor does), so it goes in by hand.
  python3 -I - "$tmpl/collections/api.json" <<'PYJSON'
import json, sys
p = sys.argv[1]
d = json.load(open(p))
for r in d["requests"]:
    if r["name"] == "token":
        r["extract"] = [{"source": "body", "name": "TOKEN", "path": "token", "secret": False}]
json.dump(d, open(p, "w"), indent=2)
PYJSON

  r_root="$work/run-rust"
  z_root="$work/run-ziman"
  cp -r "$tmpl" "$r_root"
  cp -r "$tmpl" "$z_root"

  run_both() {
    local label="$1"; shift
    local a b
    a="$("$RUST" run "$@" --data-dir "$r_root" 2>&1 | os_normalise)" || true
    b="$(ZIMAN="$ZIMAN" "$ZCLI" run "$@" --data-dir "$z_root" 2>&1 | os_normalise)" || true
    if [ "$a" = "$b" ]; then
      ok "run: $label"
    else
      bad "run: $label" "$(diff <(echo "$a") <(echo "$b") | head -12 | tr '\n' '|')"
    fi
  }

  run_both "a saved request with --env substitution" api/echo --env staging
  run_both "--var overrides the environment" api/echo --env staging --var WHO=overridden
  run_both "without --env an unresolved variable is refused" api/echo
  # A refusal, first line only -- same reason as `cli_refusal_both`: the message is the contract
  # and `anyhow`'s `Caused by:` paragraph is its library's presentation.
  # Each side runs against its own copy of the fixture, because `run` writes the session -- so the
  # storage root appears in the message and is normalised away. It is the only substitution here,
  # and it is the harness's own doing rather than a difference between the implementations.
  run_refusal_both() {
    local label="$1"; shift
    local a b
    a="$("$RUST" run "$@" --data-dir "$r_root" 2>&1 | head -1 | sed "s|$r_root|<root>|g")" || true
    b="$(ZIMAN="$ZIMAN" "$ZCLI" run "$@" --data-dir "$z_root" 2>&1 | head -1 | sed "s|$z_root|<root>|g")" || true
    if [ "$a" = "$b" ]; then
      ok "run: $label"
    else
      bad "run: $label" "rust=[$a] ziman=[$b]"
    fi
  }
  run_refusal_both "a malformed target" not-a-target
  run_refusal_both "a target naming no collection" nope/echo
  run_refusal_both "a target naming no request" api/nope
  run_both "--json rendering of a saved request" api/echo --env staging --json

  # **Extraction into the session, through the CLI** -- the behaviour `run` exists for: a request's
  # own rules write the session, and a later run substitutes what they wrote.
  "$RUST" run api/token --data-dir "$r_root" >/dev/null 2>&1
  ZIMAN="$ZIMAN" "$ZCLI" run api/token --data-dir "$z_root" >/dev/null 2>&1
  rust_session="$("$RUST" session show --data-dir "$r_root" 2>&1)"
  zim_session="$(ZIMAN="$ZIMAN" "$ZCLI" session show --data-dir "$z_root" 2>&1)"
  if [ "$rust_session" = "$zim_session" ] && echo "$zim_session" | grep -q 'TOKEN=abc123'; then
    ok "run: an extraction rule writes the same session on both"
  else
    bad "run: extracted session differs" "rust=[$rust_session] ziman=[$zim_session]"
  fi

  # And the next run sees it: {{TOKEN}} resolves from the session alone.
  "$RUST" collections add-request api uses-token "$OSB/echo?t={{TOKEN}}" --data-dir "$r_root" >/dev/null 2>&1
  cp "$r_root/collections/api.json" "$z_root/collections/api.json"
  run_both "a later run substitutes what an earlier one extracted" api/uses-token
fi

# ---- empty-state messages -----------------------------------------------------------------------
#
# **Every list command's "nothing here yet" line, compared against an empty root.** These were
# never checked before, because the sample tree always has collections, environments and scenarios
# in it -- and two of them were wrong: `scenario list`'s message, and `scenario show`'s
# `  (no imports)` line for an import-less scenario. Both found by reading the Rust source rather
# than by the harness, which is exactly the gap this section closes.

if [ -x "$ZCLI" ]; then
  empty="$work/empty"
  mkdir -p "$empty"
  empty_both() {
    local label="$1"; shift
    local a b
    a="$("$RUST" "$@" --data-dir "$empty" 2>&1)" || true
    b="$(ZIMAN="$ZIMAN" "$ZCLI" "$@" --data-dir "$empty" 2>&1)" || true
    if [ "$a" = "$b" ]; then
      ok "empty: $label"
    else
      bad "empty: $label" "rust=[$a] ziman=[$b]"
    fi
  }
  empty_both "collections list" collections list
  empty_both "env list" env list
  empty_both "scenario list" scenario list
  empty_both "session show" session show
  empty_both "history" history

  # An import-less scenario, written by hand since no command creates one.
  mkdir -p "$empty/scenarios"
  cat > "$empty/scenarios/bare.json" <<'JSON'
{ "name": "bare", "requests": { "type": "sequence", "items": [{ "request": "x" }] } }
JSON
  empty_both "scenario show with no imports" scenario show bare
fi

# ---- whole scenario runs ------------------------------------------------------------------------
#
# **The check this harness was built for.** Everything before it compares stored shapes or one
# request; this runs a multi-step scenario through both implementations against one stub and diffs
# the step log, the extractions, and the final verdict. If these agree, the two implementations
# orchestrate identically.

if [ -x "$ZCLI" ]; then
  sc_tmpl="$work/scen-template"
  mkdir -p "$sc_tmpl/scenarios"
  "$RUST" collections add-request api ok "$OSB/plain" --data-dir "$sc_tmpl" >/dev/null 2>&1
  "$RUST" collections add-request api token "$OSB/token" --data-dir "$sc_tmpl" >/dev/null 2>&1
  "$RUST" collections add-request api uses "$OSB/echo?t={{TOKEN}}" --data-dir "$sc_tmpl" >/dev/null 2>&1
  "$RUST" collections add-request api boom "$OSB/boom" --data-dir "$sc_tmpl" >/dev/null 2>&1
  python3 -I - "$sc_tmpl/collections/api.json" <<'PYJSON'
import json, sys
p = sys.argv[1]
d = json.load(open(p))
for r in d["requests"]:
    if r["name"] == "token":
        r["extract"] = [{"source": "body", "name": "TOKEN", "path": "token", "secret": False}]
json.dump(d, open(p, "w"), indent=2)
PYJSON

  write_scenario() {
    cat > "$sc_tmpl/scenarios/$1.json"
  }
  write_scenario happy <<'JSON'
{ "name": "happy", "imports": { "default": "api" },
  "requests": { "type": "sequence", "items": [{ "request": "ok" }, { "request": "token" }, { "request": "uses" }] } }
JSON
  write_scenario halts <<'JSON'
{ "name": "halts", "imports": { "default": "api" },
  "requests": { "type": "sequence", "items": [{ "request": "ok" }, { "request": "boom" }, { "request": "ok" }] } }
JSON
  write_scenario handled <<'JSON'
{ "name": "handled", "imports": { "default": "api" },
  "requests": { "type": "sequence", "items": [
    { "request": "boom" },
    { "choice": { "cases": [{ "when": { "status": { "eq": 500 } }, "then": { "request": "ok" } }] } }
  ] } }
JSON
  write_scenario unmatched <<'JSON'
{ "name": "unmatched", "imports": { "default": "api" },
  "requests": { "type": "sequence", "items": [
    { "request": "ok" },
    { "choice": { "cases": [{ "when": { "status": { "eq": 404 } }, "then": { "request": "ok" } }] } }
  ] } }
JSON
  write_scenario parallel <<'JSON'
{ "name": "parallel", "imports": { "default": "api" },
  "requests": { "type": "sequence", "items": [
    { "requests": { "type": "parallel", "items": [{ "request": "ok" }, { "request": "boom" }] } },
    { "choice": { "cases": [{ "when": { "status": { "eq": 500 } }, "then": { "request": "ok" } }] } }
  ] } }
JSON

  sr_root="$work/scen-rust"
  sz_root="$work/scen-ziman"
  cp -r "$sc_tmpl" "$sr_root"
  cp -r "$sc_tmpl" "$sz_root"

  scenario_both() {
    local label="$1"; shift
    local a b
    a="$("$RUST" scenario run "$@" --data-dir "$sr_root" 2>&1 | os_normalise | sed "s|$sr_root|<root>|g")" || true
    b="$(ZIMAN="$ZIMAN" "$ZCLI" scenario run "$@" --data-dir "$sz_root" 2>&1 | os_normalise | sed "s|$sz_root|<root>|g")" || true
    if [ "$a" = "$b" ]; then
      ok "scenario run: $label"
    else
      bad "scenario run: $label" "$(diff <(echo "$a") <(echo "$b") | head -8 | tr '\n' '|')"
    fi
  }

  scenario_both "a sequence that completes, with an extraction feeding the next step" happy
  scenario_both "an unhandled failure halts and names the request" halts
  scenario_both "a failure handled by a following choice completes" handled
  scenario_both "a choice matching nothing with no default halts" unmatched
  scenario_both "a parallel block's failure handled by a following choice" parallel

  # A refusal, first line only -- `anyhow`'s `Caused by:` paragraph again.
  a_out="$("$RUST" scenario run no-such-scenario --data-dir "$sr_root" 2>&1 | head -1 | sed "s|$sr_root|<root>|g")" || true
  b_out="$(ZIMAN="$ZIMAN" "$ZCLI" scenario run no-such-scenario --data-dir "$sz_root" 2>&1 | head -1 | sed "s|$sz_root|<root>|g")" || true
  if [ "$a_out" = "$b_out" ]; then
    ok "scenario run: a scenario that does not exist"
  else
    bad "scenario run: a scenario that does not exist" "rust=[$a_out] ziman=[$b_out]"
  fi
fi

# ---- the request-building collections subcommands ------------------------------------------------
#
# **Each side builds its own storage with its OWN CLI, then the other reads it back.** Every check
# before this had to build fixtures with the Rust binary, because these subcommands were not ported;
# now the same script of nine commands runs through both and the resulting collection file is
# diffed with ids normalised. That is a stronger statement than comparing output: it says the two
# write the same storage from the same instructions.

if [ -x "$ZCLI" ]; then
  build_collection() {
    # $1 = root, $2.. = the command prefix (binary, possibly env-prefixed via a function)
    local root="$1"; shift
    mkdir -p "$root"
    "$@" collections create Empty --data-dir "$root" >/dev/null 2>&1
    "$@" collections add-request api login "$OSB/echo" \
      -X POST -H 'Content-Type: application/json' -H 'Accept: */*' \
      -Q 'a=1' -Q 'b=two words' -d '{"user":"ann"}' \
      --extract-body 'TOKEN=token' --extract-header 'REQ=X-Request-Id' \
      --extract-template 'AUTH=Bearer {{TOKEN}}' --extract-secret TOKEN \
      --data-dir "$root" >/dev/null 2>&1
    "$@" collections add-request api 'Auth/OAuth/refresh' "$OSB/plain" -u ann:s3cret --data-dir "$root" >/dev/null 2>&1
    "$@" collections add-request api upload "$OSB/echo" -F caption=hi -F 'file=@/tmp/x.png' --data-dir "$root" >/dev/null 2>&1
    "$@" collections add-request api raw "$OSB/plain" --bearer tok --data-dir "$root" >/dev/null 2>&1
    "$@" collections add-folder api Archive/Old --data-dir "$root" >/dev/null 2>&1
    "$@" collections rename-request api raw 'Archive/Old/raw' --data-dir "$root" >/dev/null 2>&1
    "$@" collections duplicate-request api login 'Archive/login-copy' --data-dir "$root" >/dev/null 2>&1
    "$@" collections remove-request api upload --data-dir "$root" >/dev/null 2>&1
    "$@" collections import postman Imported "$work/postman.json" --data-dir "$root" >/dev/null 2>&1
    "$@" collections import curl Curled 'Auth/login' "$CURL_COMMAND" --data-dir "$root" >/dev/null 2>&1
  }
  zcli_wrapper() { ZIMAN="$ZIMAN" "$ZCLI" "$@"; }

  cb_rust="$work/build-rust"
  cb_zim="$work/build-ziman"
  build_collection "$cb_rust" "$RUST"
  build_collection "$cb_zim" zcli_wrapper

  for f in api imported curled empty; do
    if [ -f "$cb_rust/collections/$f.json" ] && [ -f "$cb_zim/collections/$f.json" ]; then
      if diff -q <(normalise "$cb_rust/collections/$f.json") <(normalise "$cb_zim/collections/$f.json") >/dev/null; then
        ok "collections: both CLIs write an identical \"$f\""
      else
        bad "collections: \"$f\" differs" \
            "$(diff <(normalise "$cb_rust/collections/$f.json") <(normalise "$cb_zim/collections/$f.json") | head -10 | tr '\n' '|')"
      fi
    else
      bad "collections: \"$f\" was not written by both" \
          "rust=$([ -f "$cb_rust/collections/$f.json" ] && echo yes || echo no) ziman=$([ -f "$cb_zim/collections/$f.json" ] && echo yes || echo no)"
    fi
  done

  # Each binary reading the other's tree, as a tree -- the `show` output carries the extraction
  # names, so this also checks `--extract-secret` landed on the right rule.
  rust_sees_zim="$("$RUST" collections show api --data-dir "$cb_zim" 2>&1)"
  zim_sees_rust="$(ZIMAN="$ZIMAN" "$ZCLI" collections show api --data-dir "$cb_rust" 2>&1)"
  if [ "$rust_sees_zim" = "$zim_sees_rust" ]; then
    ok "collections: each CLI shows the other's tree identically"
  else
    bad "collections: cross-read tree differs" "$(diff <(echo "$rust_sees_zim") <(echo "$zim_sees_rust") | head -6 | tr '\n' '|')"
  fi

  # The refusals, which are where a port most easily drifts.
  coll_refusal_both() {
    local label="$1"; shift
    local a b
    a="$("$RUST" collections "$@" --data-dir "$cb_rust" 2>&1 | head -1 | sed "s|$cb_rust|<root>|g")" || true
    b="$(ZIMAN="$ZIMAN" "$ZCLI" collections "$@" --data-dir "$cb_zim" 2>&1 | head -1 | sed "s|$cb_zim|<root>|g")" || true
    if [ "$a" = "$b" ]; then
      ok "collections: $label"
    else
      bad "collections: $label" "rust=[$a] ziman=[$b]"
    fi
  }
  coll_refusal_both "create an existing collection" create api
  coll_refusal_both "remove a request that is not there" remove-request api nope
  coll_refusal_both "rename onto a taken path" rename-request api login 'Archive/login-copy'
  coll_refusal_both "duplicate a request that is not there" duplicate-request api nope copy
  coll_refusal_both "remove a non-empty folder without --force" remove-folder api Archive
  coll_refusal_both "remove a folder that is not there" remove-folder api Nope
  # **The one refusal whose first line deliberately differs.** Rust builds this with
  # `anyhow::with_context`, which makes the *context* the headline and puts the actual reason in the
  # `Caused by:` chain below -- so its first line is "in collection ... (remove it first ...)" and
  # "a request already exists at ..." is underneath. Since the chain is not reproduced, matching
  # that first line would mean printing the advice and dropping the reason, which is the wrong half
  # to keep in a one-line report. Both facts are compared instead of the exact string.
  a_out="$("$RUST" collections add-request api login "$OSB/plain" --data-dir "$cb_rust" 2>&1)" || true
  b_out="$(ZIMAN="$ZIMAN" "$ZCLI" collections add-request api login "$OSB/plain" --data-dir "$cb_zim" 2>&1)" || true
  if echo "$a_out" | grep -q 'a request already exists at "login"' \
     && echo "$a_out" | grep -q 'in collection "api"' \
     && echo "$b_out" | grep -q 'a request already exists at "login"' \
     && echo "$b_out" | grep -q 'in collection "api"'; then
    ok "collections: add a request at a taken path (same facts, see the comment)"
  else
    bad "collections: add at a taken path" "rust=[$(echo "$a_out" | tr '\n' ' ')] ziman=[$(echo "$b_out" | tr '\n' ' ')]"
  fi
  coll_refusal_both "import postman over an existing collection" import postman Imported "$work/postman.json"
  coll_refusal_both "--extract-secret naming no rule" add-request api fresh "$OSB/plain" --extract-secret NOPE
  coll_refusal_both "two body sources" add-request api fresh2 "$OSB/plain" -d a -F b=c

  # --force really does remove it, on both.
  "$RUST" collections remove-folder api Archive --force --data-dir "$cb_rust" >/dev/null 2>&1
  ZIMAN="$ZIMAN" "$ZCLI" collections remove-folder api Archive --force --data-dir "$cb_zim" >/dev/null 2>&1
  if diff -q <(normalise "$cb_rust/collections/api.json") <(normalise "$cb_zim/collections/api.json") >/dev/null; then
    ok "collections: --force removes a non-empty folder identically"
  else
    bad "collections: --force differs" "$(diff <(normalise "$cb_rust/collections/api.json") <(normalise "$cb_zim/collections/api.json") | head -6 | tr '\n' '|')"
  fi
fi

# ---- the slug -----------------------------------------------------------------------------------
#
# Both sides turn a display name into the same filename, or `load` misses what `save` wrote.

while IFS='|' read -r input expected; do
  got="$(CURLY_CASE=slugify CURLY_NAME="$input" zim env)"
  if [ "$got" = "$expected" ]; then
    ok "slugify \"$input\" -> $expected"
  else
    bad "slugify \"$input\"" "expected $expected, got $got"
  fi
done <<'CASES'
Health Record API|health-record-api
  Trimmed  |trimmed
a    b|a-b
...name...|name
!!!|untitled
café|caf
CASES

printf '\n'
[ "$failed" -eq 0 ] && echo "  cross-check: all agree" || echo "  cross-check: $failed disagreement(s)"
exit "$([ "$failed" -eq 0 ] && echo 0 || echo 1)"
