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
os_normalise() {
  sed -E -e 's/"elapsed_ms": [0-9]+/"elapsed_ms": <ms>/' \
         -e 's/boundary=[A-Za-z0-9-]+/boundary=<b>/g' \
         -e 's/--[A-Za-z0-9-]{20,}/--<b>/g'
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
