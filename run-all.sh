#!/usr/bin/env bash
# Every check in this repository, for both implementations, plus the cross-check between them.
#
# **The cross-check is the point.** Curly is being reimplemented in Ziman to replace the Rust
# version, and "replace" is only true if the two behave the same -- so the Ziman side is not
# measured against its own tests alone but against the Rust binary's actual output over the same
# `.curly/` tree. A module that passes its unit tests and disagrees with the reference is a module
# that is not done.
#
# Usage: ./run-all.sh [--quick]
#   --quick   skip the Rust build and test phases (useful while iterating on the Ziman side)
#
# Exit 0 only if every phase passed. **Read this script's own exit status**, not the per-phase
# tallies: a pre-flight or a formatting phase can fail while every test phase reads "0 failed".
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ZIMAN="${ZIMAN:-$HOME/workspace/vasna.tech/repos/ziman/ziman}"
QUICK=0
[ "${1:-}" = "--quick" ] && QUICK=1

failed=0
phase() { printf '\n== %s ==\n' "$1"; }
note()  { printf '  %s\n' "$1"; }
fail()  { failed=$((failed + 1)); printf '  FAIL %s\n' "$1"; }

if [ ! -x "$ZIMAN" ]; then
  echo "ziman not found at $ZIMAN (set ZIMAN=/path/to/ziman)" >&2
  exit 2
fi

# ---------------------------------------------------------------- the Ziman implementation

phase "ziman formatting"
# `--fmt --check` reports an unformatted file by PRINTING ITS NAME and still exiting 0, so the
# output is what has to be checked, not the status.
unformatted="$(cd "$ROOT/ziman" && ZIMAN_MODULE_PATH=src/main/ziman "$ZIMAN" --fmt --check \
  src/main/ziman/core/*.zm src/test/unit/*.zm src/test/other/*.zm 2>&1)"
if [ -n "$unformatted" ]; then
  fail "unformatted: $unformatted"
else
  note "all formatted"
fi

phase "ziman unit tests"
if (cd "$ROOT/ziman" && ZIMAN_MODULE_PATH=src/main/ziman ZIMAN_RESOURCE_PATH=src/test/resources \
      "$ZIMAN" test src/test/unit/*.zm) | tail -1; then
  :
else
  fail "ziman unit tests"
fi

phase "ziman integration tests"
for t in "$ROOT"/ziman/src/test/other/*_test.py; do
  [ -f "$t" ] || continue
  name="$(basename "$t")"
  if (cd "$ROOT/ziman" && python3 "$t" "$ZIMAN") > "/tmp/curly-$name.$$.out" 2>&1; then
    note "PASS $name ($(grep -c '^PASS' "/tmp/curly-$name.$$.out") checks)"
  else
    fail "$name"
    grep -v '^PASS' "/tmp/curly-$name.$$.out" | head -12 | sed 's/^/       /'
  fi
  rm -f "/tmp/curly-$name.$$.out"
done

# ---------------------------------------------------------------- the Rust reference

phase "rust tests"
if [ "$QUICK" = 0 ]; then
  # **`cargo test` runs one binary per target and prints a `test result:` line for each**, so the
  # tail of its output is the LAST target's tally -- which is an empty one here, reading
  # "running 0 tests". Summing the lines and reading cargo's own exit status is the only honest
  # report; the first version of this script printed "0 tests" while 180 were passing.
  log="$(mktemp)"
  (cd "$ROOT/rust" && cargo test --offline -q) > "$log" 2>&1
  rc=$?
  total=$(awk '/^test result:/ { n += $4 } END { print n + 0 }' "$log")
  suites=$(grep -c '^test result:' "$log")
  if [ "$rc" -eq 0 ]; then
    note "PASS $total tests across $suites suites"
  else
    fail "cargo test (exit $rc)"
    grep -E '^(error|test result: FAILED|---- .* stdout)' "$log" | head -12 | sed 's/^/       /'
  fi
  rm -f "$log"
else
  note "SKIPPED (--quick)"
fi

# ---------------------------------------------------------------- the two against each other

phase "cross-check: both implementations over one .curly tree"
if [ "$QUICK" = 0 ] || [ -x "$ROOT/rust/target/debug/curly" ]; then
  if (cd "$ROOT" && ZIMAN="$ZIMAN" ./parity.sh); then
    :
  else
    fail "cross-check"
  fi
else
  note "SKIPPED (no rust binary built)"
fi

printf '\n'
if [ "$failed" -eq 0 ]; then
  echo "all phases passed"
else
  echo "$failed phase(s) failed"
fi
exit "$([ "$failed" -eq 0 ] && echo 0 || echo 1)"
