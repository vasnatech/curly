#!/usr/bin/env python3
"""Checks `curly serve` -- the HTTP API the React frontend talks to -- against a real socket.

`src/test/unit/server_api_test.zm` covers every command as a function, which needs no server. This
covers what only a server can show: that the transport works, that a send really sends and really
writes history and the session, that the static layer serves a built frontend and refuses a path
that climbs out of it, and that the three cross-origin guards hold. The Tauri bridge this replaces
could not be reached by a web page at all, so the guards are new risk that the port itself created
and they are checked here rather than argued about.

Usage: src/test/other/server_test.py [path-to-ziman]   Exit 0 = all pass.
"""
import json, os, shutil, socket, subprocess, sys, tempfile, threading, time, urllib.error, urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

HERE = os.path.dirname(os.path.abspath(__file__))
ZIMAN_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(HERE)))
CURLY = os.path.join(ZIMAN_ROOT, "bin", "curly")
MODULES = os.path.join(ZIMAN_ROOT, "src", "main", "ziman")
ZIMAN = (sys.argv[1] if len(sys.argv) > 1 else shutil.which("ziman")
         or os.path.expanduser("~/workspace/vasna.tech/repos/ziman/ziman"))
failures = 0


def check(name, ok, detail=""):
    global failures
    print(("PASS " if ok else "FAIL ") + name + ("" if ok else f"  ({detail})"))
    failures += 0 if ok else 1


def free_port():
    """A port nothing holds right now.

    Asked for rather than hard-coded: these run alongside the other suites, and a fixed port makes
    a second run inside the TIME_WAIT window fail for a reason that has nothing to do with curly.
    """
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


# ------------------------------------------------------------------ the target a send is aimed at

class Target(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_POST(self):  # noqa: N802
        length = int(self.headers.get("Content-Length") or 0)
        self.rfile.read(length) if length else b""
        body = json.dumps({"token": "tok-from-the-target", "id": 7}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):  # noqa: N802
        self.send_response(404)
        self.send_header("Content-Length", "0")
        self.end_headers()

    def log_message(self, *args):
        pass


# ------------------------------------------------------------------ talking to the API

def call(base, name, args=None, content_type="application/json", origin=None, method="POST"):
    """One command. Answers (status, body-text)."""
    data = None if args is None else json.dumps(args).encode()
    request = urllib.request.Request(f"{base}/api/{name}", data=data, method=method)
    if content_type is not None:
        request.add_header("Content-Type", content_type)
    if origin is not None:
        request.add_header("Origin", origin)
    try:
        with urllib.request.urlopen(request, timeout=10) as answer:
            return answer.status, answer.read().decode("utf-8", "replace")
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode("utf-8", "replace")


def get(base, path):
    """One static request, with the path sent exactly as written -- no client-side normalising,
    which is the whole point for a path that tries to climb out of the web root."""
    try:
        with urllib.request.urlopen(f"{base}{path}", timeout=10) as answer:
            return answer.status, answer.headers.get("Content-Type", ""), answer.read()
    except urllib.error.HTTPError as e:
        return e.code, e.headers.get("Content-Type", ""), e.read()


def answered(base, name, args=None):
    status, text = call(base, name, args)
    if status != 200:
        raise AssertionError(f"{name} answered {status}: {text.strip()}")
    return json.loads(text)


def wait_until_listening(port, process, seconds=20):
    """Waits on the port, not on a line of output: a readiness check that greps stdout passes as
    soon as the message is printed, which is before the listener exists."""
    deadline = time.time() + seconds
    while time.time() < deadline:
        if process.poll() is not None:
            return False
        with socket.socket() as s:
            s.settimeout(0.3)
            if s.connect_ex(("127.0.0.1", port)) == 0:
                return True
        time.sleep(0.1)
    return False


def start_server(root, port, web_root=None):
    argv = [CURLY, "serve", "--data-dir", root, "--port", str(port)]
    if web_root is not None:
        argv += ["--web-root", web_root]
    env = dict(os.environ, ZIMAN=ZIMAN, ZIMAN_MODULE_PATH=MODULES)
    process = subprocess.Popen(argv, cwd=ZIMAN_ROOT, env=env,
                               stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    if not wait_until_listening(port, process):
        output = ""
        if process.poll() is None:
            process.kill()
        try:
            output = process.stdout.read().decode("utf-8", "replace")
        except Exception:  # noqa: BLE001
            pass
        raise AssertionError(f"curly serve never listened on {port}: {output.strip()}")
    return process


def stop(process):
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()


A_REQUEST = {
    "id": "11111111-1111-1111-1111-111111111111",
    "name": "Login",
    "method": "POST",
    "url": "",  # filled in with the stub target's address
    "query_params": [],
    "headers": [],
    "body": {"type": "raw", "content": "{\"user\":\"someone\"}"},
    "auth": None,
    "extract": [{"source": "body", "name": "TOKEN", "path": "token", "secret": True}],
}


def main():
    tmp = tempfile.mkdtemp(prefix="curly-serve-test-")
    root = os.path.join(tmp, ".curly")
    web_root = os.path.join(tmp, "dist")
    os.makedirs(os.path.join(web_root, "assets"))
    with open(os.path.join(web_root, "index.html"), "w") as f:
        f.write("<!doctype html><title>curly</title><div id=root></div>")
    with open(os.path.join(web_root, "assets", "app-a1b2c3.js"), "w") as f:
        f.write('console.log("app")')
    # Outside the web root, which is the file a traversal would be after.
    with open(os.path.join(tmp, "secret.txt"), "w") as f:
        f.write("TOP SECRET")

    target_port = free_port()
    target = ThreadingHTTPServer(("127.0.0.1", target_port), Target)
    threading.Thread(target=target.serve_forever, daemon=True).start()

    port = free_port()
    process = None
    try:
        process = start_server(root, port, web_root)
        base = f"http://127.0.0.1:{port}"

        # ---------------------------------------------------------- the transport

        check("an empty project lists no collections", answered(base, "list_collections", {}) == [])

        # An argument-less command called with no body at all, which is `invoke(name)` with no
        # second argument.
        status, text = call(base, "list_collections", None)
        check("a command with no body at all is accepted", (status, text.strip()) == (200, "[]"),
              f"{status} {text.strip()}")

        info = answered(base, "resolve_default_project", {})
        check("resolve_default_project reports the --data-dir it was started with",
              info.get("root") == root, info)

        status, text = call(base, "frobnicate", {})
        check("an unknown command is refused by name",
              status == 500 and text.strip() == "no such command: frobnicate", f"{status} {text}")

        status, _ = call(base, "list_collections", {}, method="GET")
        check("a command sent as GET is refused", status == 405, status)

        status, _ = call(base, "list_collections", {}, content_type="text/plain")
        check("a command sent as text/plain is refused", status == 415, status)

        status, _ = call(base, "list_collections", {}, content_type=None)
        check("a command sent with no Content-Type is refused", status == 415, status)

        status, text = call(base, "list_collections", {}, origin="http://evil.example")
        check("a command carrying an Origin header is refused",
              status == 403 and "evil.example" in text, f"{status} {text}")

        status, text = call(base, "load_collection", {"name": "nope"})
        check("a refused command answers its message as the whole body",
              status == 500 and text.startswith('no collection named "nope"'), f"{status} {text}")

        # ---------------------------------------------------------- a real send

        saved = dict(A_REQUEST, url=f"http://127.0.0.1:{target_port}/login")
        outcome = answered(base, "send_request", {"saved": saved, "envName": None, "overrides": []})
        response = outcome["response"]
        check("send_request answers the target's status", response["status"] == 200, response["status"])
        check("send_request answers the exact bytes the target sent",
              bytes(response["body_bytes"]).decode() == response["body"], response["body"])
        check("send_request's elapsed is serde's {secs, nanos}",
              sorted(response["elapsed"].keys()) == ["nanos", "secs"], response["elapsed"])
        check("a 2xx with extraction rules extracts",
              outcome["extracted"] == [{"name": "TOKEN", "value": "tok-from-the-target", "secret": True}],
              outcome["extracted"])

        session = answered(base, "load_session", {"envName": "global"})
        check("what was extracted is written to the session",
              session["variables"] == [{"key": "TOKEN", "value": "tok-from-the-target", "secret": True}],
              session)

        history = answered(base, "list_history", {"limit": 10})
        check("the exchange is recorded in history",
              len(history) == 1 and history[0]["status"] == 200 and history[0]["method"] == "POST",
              history)

        # A send that cannot reach anything fails with a message rather than an empty 200.
        unreachable = dict(A_REQUEST, url=f"http://127.0.0.1:{free_port()}/login")
        status, text = call(base, "send_request",
                            {"saved": unreachable, "envName": None, "overrides": []})
        check("a send that cannot connect is refused with a reason",
              status == 500 and text.strip() != "", f"{status} {text}")

        # ---------------------------------------------------------- substitution through the API

        answered(base, "save_environment", {"environment": {
            "name": "local",
            "variables": [{"key": "HOST", "value": f"127.0.0.1:{target_port}", "secret": False}],
        }})
        templated = dict(A_REQUEST, url="http://{{HOST}}/login", extract=[])
        outcome = answered(base, "send_request",
                           {"saved": templated, "envName": "local", "overrides": []})
        check("a {{variable}} in the URL is resolved from the named environment",
              outcome["response"]["status"] == 200, outcome["response"]["status"])

        overridden = dict(A_REQUEST, url="http://{{HOST}}/login", extract=[])
        status, text = call(base, "send_request", {
            "saved": overridden, "envName": "local",
            "overrides": [["HOST", f"127.0.0.1:{free_port()}"]],
        })
        check("an override beats the environment it was sent with", status == 500, f"{status} {text}")

        # ---------------------------------------------------------- the built frontend

        status, kind, body = get(base, "/")
        check("/ serves index.html", status == 200 and "text/html" in kind and b"<div id=root>" in body,
              f"{status} {kind}")
        status, kind, _ = get(base, "/assets/app-a1b2c3.js")
        check("an asset is served with its own media type",
              status == 200 and "javascript" in kind, f"{status} {kind}")
        status, _, body = get(base, "/collections")
        check("a route with no extension falls back to index.html",
              status == 200 and b"<div id=root>" in body, status)
        status, _, _ = get(base, "/assets/missing-deadbeef.js")
        check("a missing FILE is a 404 and not a page of HTML", status == 404, status)

        # The guard, with the path sent exactly as written. The encoded forms matter most: a check
        # made before percent-decoding passes `%2e%2e`, and one made on a whole decoded path lets
        # `%2f` become a separator.
        for climb in ["/../secret.txt", "/%2e%2e/secret.txt", "/..%2fsecret.txt",
                      "/assets/../../secret.txt", "/%2e%2e%2f%2e%2e%2fsecret.txt"]:
            status, _, body = get(base, climb)
            check(f"a path climbing out of the web root is refused: {climb}",
                  status == 404 and b"SECRET" not in body, f"{status} {body[:40]!r}")

        # `/api/` with nothing after it: a bug in the frontend's own URL building, not a command.
        status, _ = call(base, "", {})
        check("the api prefix with no command is a 404", status == 404, status)
    finally:
        if process is not None:
            stop(process)
        target.shutdown()
        shutil.rmtree(tmp, ignore_errors=True)

    print(f"\n{'all passed' if failures == 0 else str(failures) + ' failed'}")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
