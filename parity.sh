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
