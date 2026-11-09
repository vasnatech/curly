#!/usr/bin/env python3
"""Checks core/exec.send against a stub server that echoes what it received.

Mirrors Rust's `crates/curly-core/tests/exec_test.rs`, which does the same against wiremock: these
are the parts of `send` that only a real socket can test -- how query parameters land on the URL,
which Content-Type a body implies, what an Authorization header looks like, and that `body_bytes`
is the server's exact bytes rather than a re-encoding of the lossy `body` string.

Four of these cases have no Rust counterpart because reqwest handled them and `client` does not:
query appending onto a URL that already has one, two parameters of the same name, an explicit
Content-Type beating the one a form body implies, and a hand-assembled multipart body that a real
multipart parser has to accept.

Usage: src/test/other/exec_test.py [path-to-ziman]   Exit 0 = all pass.
"""
import json, os, shutil, subprocess, sys, tempfile, threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(os.path.dirname(HERE)))
DRIVER = os.path.join(HERE, "exec_driver.zm")
ZIMAN = (sys.argv[1] if len(sys.argv) > 1 else shutil.which("ziman")
         or os.path.expanduser("~/workspace/vasna.tech/repos/ziman/ziman"))
PORT = 18731
# Deliberately not valid UTF-8: a lone 0xFF can never appear in a UTF-8 sequence, so `body` must
# come back with a replacement character while `body_bytes` keeps the byte.
BINARY = bytes([0x89, 0x50, 0x4E, 0x47, 0xFF, 0xFE, 0x00, 0x41])
failures = 0


def check(name, ok, detail=""):
    global failures
    print(("PASS " if ok else "FAIL ") + name + ("" if ok else f"  ({detail})"))
    failures += 0 if ok else 1


class Echo(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def _send(self, status, payload, content_type="application/json"):
        body = payload if isinstance(payload, bytes) else json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("X-Stub", "yes")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _handle(self):
        parsed = urlparse(self.path)
        length = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(length) if length else b""
        if parsed.path == "/hello":
            return self._send(200, b"world", "text/plain")
        if parsed.path == "/binary":
            return self._send(200, BINARY, "application/octet-stream")
        if parsed.path == "/boom":
            return self._send(500, {"error": "boom"})
        return self._send(200, {
            "method": self.command,
            "path": parsed.path,
            "query": parsed.query,
            "headers": {k.lower(): v for k, v in self.headers.items()},
            # Hex, so a multipart body with arbitrary bytes survives the round trip to the test.
            "body_hex": raw.hex(),
        })

    do_GET = do_POST = do_PUT = do_DELETE = _handle

    def log_message(self, *args):
        pass


def run(case, env_extra=None):
    env = dict(os.environ, ZIMAN_MODULE_PATH=os.path.join(ROOT, "src", "main", "ziman"),
               CURLY_CASE=case, CURLY_BASE=f"http://127.0.0.1:{PORT}")
    env.update(env_extra or {})
    done = subprocess.run([ZIMAN, DRIVER], cwd=ROOT, env=env, capture_output=True, text=True)
    if done.returncode != 0:
        raise SystemExit(f"driver failed for {case}: {done.stderr or done.stdout}")
    return json.loads(done.stdout.strip().splitlines()[-1])


def echoed(result):
    """The stub's JSON, as the driver saw it in the response body."""
    return json.loads(result["body"])


def main():
    server = ThreadingHTTPServer(("127.0.0.1", PORT), Echo)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    tmp = tempfile.mkdtemp()
    try:
        # -- the basics, as Rust's first two tests --
        r = run("get")
        check("a GET returns the status and the body", r["ok"] and r["status"] == 200 and r["body"] == "world", r)
        check("the elapsed time is recorded and plausible", r["elapsed_is_sane"], r)
        check("response headers come back", any(h[0].lower() == "x-stub" for h in r["headers"]), r["headers"])

        r = run("headers")
        check("a custom header is sent", echoed(r)["headers"].get("x-test") == "abc", echoed(r)["headers"])

        # -- query parameters: appended, ordered, duplicated, encoded --
        q = echoed(run("query"))["query"]
        check("query parameters are sent in order, duplicates kept",
              q == "a=1&b=two+words%26x%3Dy&a=2", q)
        check("a space encodes as + and a separator is escaped", "two+words%26x%3Dy" in q, q)
        q2 = echoed(run("query-appends"))["query"]
        check("parameters append to a URL that already has a query",
              q2 == "already=here&added=yes", q2)

        # -- bodies --
        e = echoed(run("raw"))
        check("a raw body is sent verbatim", bytes.fromhex(e["body_hex"]) == b'{"a":1}', e["body_hex"])
        check("an explicit Content-Type is kept", e["headers"].get("content-type") == "application/json", e["headers"])
        e = echoed(run("raw-no-content-type"))
        check("a raw body implies no Content-Type, as reqwest's .body() does",
              "content-type" not in e["headers"], e["headers"])
        e = echoed(run("form"))
        check("a form body is urlencoded with + for space",
              bytes.fromhex(e["body_hex"]) == b"grant_type=password&note=a+b", e["body_hex"])
        check("a form body sets the urlencoded Content-Type",
              e["headers"].get("content-type") == "application/x-www-form-urlencoded", e["headers"])
        e = echoed(run("form-explicit-content-type"))
        check("an explicit Content-Type beats the one the body implies",
              e["headers"].get("content-type") == "application/x-custom", e["headers"])

        # -- auth --
        e = echoed(run("basic"))
        check("basic auth is base64 of user:password",
              e["headers"].get("authorization") == "Basic YW5uOnMzY3JldA==", e["headers"])
        e = echoed(run("bearer"))
        check("bearer auth is the token after Bearer",
              e["headers"].get("authorization") == "Bearer abc123", e["headers"])

        # -- multipart, hand-assembled because `client` has no builder --
        upload = os.path.join(tmp, "photo.png")
        with open(upload, "wb") as f:
            f.write(b"\x89PNG\r\n\x1a\nfake")
        e = echoed(run("multipart", {"CURLY_UPLOAD": upload}))
        ctype = e["headers"].get("content-type", "")
        body = bytes.fromhex(e["body_hex"])
        check("multipart sets its Content-Type with a boundary",
              ctype.startswith("multipart/form-data; boundary=curlyFormBoundary"), ctype)
        boundary = ctype.split("boundary=", 1)[1].encode() if "boundary=" in ctype else b"?"
        check("the declared boundary is the one used in the body",
              body.startswith(b"--" + boundary + b"\r\n"), body[:80])
        check("the body ends with the closing boundary",
              body.endswith(b"--" + boundary + b"--\r\n"), body[-60:])
        check("a text part carries its name", b'name="caption"' in body and b"a photo" in body, body[:200])
        check("a file part carries name and filename",
              b'name="file"' in body and b'filename="photo.png"' in body, body[:400])
        check("the file's exact bytes are in the body", b"\x89PNG\r\n\x1a\nfake" in body, body[-120:])
        # The real proof: a standards-compliant parser accepts it.
        check("a real multipart parser accepts the assembled body",
              *parses_as_multipart(body, ctype))

        # -- a non-2xx is an answer, not an error --
        r = run("status500")
        check("a 500 is returned as a status, not an error", r["ok"] and r["status"] == 500, r)

        # -- lossy body vs exact bytes, as Rust's last two tests --
        r = run("binary")
        check("body_bytes keeps the exact non-UTF-8 bytes",
              r["body_byte_count"] == len(BINARY) and r["body_byte_sum"] == sum(BINARY),
              (r["body_byte_count"], r["body_byte_sum"]))
        check("body decodes lossily rather than failing",
              "�" in r["body"] and r["body"].endswith("A"), repr(r["body"]))

        # -- a failure to exchange anything is an error --
        r = run("unreachable")
        check("an unreachable host is an error with a reason", not r["ok"] and r["error"], r)
    finally:
        server.shutdown()
        shutil.rmtree(tmp, ignore_errors=True)

    print(f"\n{'all passed' if failures == 0 else str(failures) + ' failed'}")
    return 1 if failures else 0


def parses_as_multipart(body, content_type):
    """Feed the assembled body to Python's own multipart parser."""
    try:
        from email.parser import BytesParser
        head = f"Content-Type: {content_type}\r\nMIME-Version: 1.0\r\n\r\n".encode()
        message = BytesParser().parsebytes(head + body)
        if not message.is_multipart():
            return False, "parser did not see a multipart message"
        names = []
        for part in message.get_payload():
            disposition = part.get("Content-Disposition", "")
            names.append(disposition)
        return (len(message.get_payload()) == 2, names)
    except Exception as e:  # noqa: BLE001
        return False, f"parser raised {e}"


if __name__ == "__main__":
    sys.exit(main())
