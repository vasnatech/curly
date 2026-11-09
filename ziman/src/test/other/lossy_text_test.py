#!/usr/bin/env python3
"""Checks core/exec.lossy_text against the reference UTF-8 replacement algorithm.

`send` needs `String::from_utf8_lossy`, which Ziman has no equivalent of -- `bytes_to_string` is
strict -- so it is implemented in `core/exec.zm` and has to follow Unicode's **maximal subpart**
rule, the one Rust follows: a malformed sequence costs exactly one U+FFFD, not one per byte. Python's
`bytes.decode('utf-8', errors='replace')` implements the same rule, which makes it a usable oracle.

This comparison earned its keep twice: it caught the first implementation emitting one replacement
per byte for a truncated sequence, and then found that a Ziman string cannot hold a NUL byte at all.

**The NUL cases are expected to differ** and are asserted to differ, so that the day Ziman allows
NUL in a string this test fails and says so rather than silently passing.

Usage: src/test/other/lossy_text_test.py [path-to-ziman]   Exit 0 = all pass.
"""
import os, random, shutil, subprocess, sys, tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(os.path.dirname(HERE)))
ZIMAN = (sys.argv[1] if len(sys.argv) > 1 else shutil.which("ziman")
         or os.path.expanduser("~/workspace/vasna.tech/repos/ziman/ziman"))
failures = 0

DRIVER = '''use core/exec.lossy_text
use encoding
use os

fn main() {
    val hex = switch os.get_env("CURLY_HEX") {
        Some(h) { h }
        None { "" }
    }
    val none: [byte] = []
    val data = switch encoding.base16_decode(hex) {
        Ok(d) { d }
        Err(_) { none }
    }
    // Printed as hex of the decoded text's UTF-8, so the comparison cannot be confused by whatever
    // a terminal does with a replacement character.
    print(encoding.base16_encode(to_bytes(lossy_text(data))))
}
'''

NAMED = {
    "empty": b"",
    "ascii": b"hello",
    "valid multibyte": "héllo ✓ \U0001d11e".encode(),
    "lone FF": b"a\xffb",
    "truncated 3-byte": b"a\xe2\x82",
    "truncated 2-byte at end": b"ab\xc3",
    "truncated 4-byte": b"\xf0\x9f\x98",
    "overlong C0 AF": b"\xc0\xaf",
    "surrogate ED A0 80": b"\xed\xa0\x80",
    "ED 9F BF is valid": b"\xed\x9f\xbf",
    "beyond U+10FFFF": b"\xf5\x80\x80\x80",
    "F4 90 past last code point": b"\xf4\x90\x80\x80",
    "F4 8F BF BF is the last": b"\xf4\x8f\xbf\xbf",
    "leading continuation": b"\x80\x80a",
    "E0 80 41 maximal subpart": b"\xe0\x80\x41",
    "F0 then ascii": b"\xf0A",
    "png header": bytes([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
}
# A NUL anywhere is the documented divergence.
NUL_CASES = {"nul alone": b"\x00", "nul among text": b"a\x00b", "nul and high": b"\x00\xc2\xa0\xff"}


def check(name, ok, detail=""):
    global failures
    print(("PASS " if ok else "FAIL ") + name + ("" if ok else f"  ({detail})"))
    failures += 0 if ok else 1


def decoded_by_ziman(driver, data):
    env = dict(os.environ, ZIMAN_MODULE_PATH=os.path.join(ROOT, "src", "main", "ziman"),
               CURLY_HEX=data.hex().upper())
    done = subprocess.run([ZIMAN, driver], cwd=ROOT, env=env, capture_output=True, text=True)
    if done.returncode != 0:
        raise SystemExit(f"driver failed: {done.stderr.strip()[:400]}")
    out = done.stdout.strip()
    return bytes.fromhex(out).decode("utf-8") if out else ""


def main():
    tmp = tempfile.mkdtemp()
    driver = os.path.join(tmp, "lossy_driver.zm")
    with open(driver, "w") as f:
        f.write(DRIVER)
    try:
        random.seed(7)
        cases = dict(NAMED)
        for n in range(12):
            cases[f"random {n}"] = bytes(random.randrange(256) for _ in range(random.randrange(1, 14)))

        mismatched = []
        for name, data in cases.items():
            got = decoded_by_ziman(driver, data)
            want = data.decode("utf-8", errors="replace")
            # A random case may contain a NUL; that is the known divergence, not a failure.
            if got != want and b"\x00" in data:
                continue
            if got != want:
                mismatched.append((name, data.hex(), got, want))
        check(f"{len(cases)} inputs decode exactly as the reference algorithm does",
              not mismatched, mismatched[:3])

        for name, data in NUL_CASES.items():
            got = decoded_by_ziman(driver, data)
            want = data.decode("utf-8", errors="replace")
            # Asserted to differ, and to differ in exactly one way: the NUL became U+FFFD and
            # everything else is unchanged.
            check(f"a NUL is replaced rather than carried ({name})",
                  got != want and got == want.replace("\x00", "�"), (repr(got), repr(want)))
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    print(f"\n{'all passed' if failures == 0 else str(failures) + ' failed'}")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
