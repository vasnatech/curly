"""Checks core/scenario_run against a stub server and a fresh storage root.

Ports `crates/curly-core/tests/scenario_test.rs` -- the parts of the engine that need a real socket
and real files, as against `scenario.zm`'s pure logic which `src/test/unit/scenario_test.zm` covers.

The semantics worth defending, and what each would break if it drifted:

  * A non-2xx **halts the run** unless a `choice` comes next to handle it. Without that, a scenario
    carries on silently past a failed request and reports success.
  * A `choice` that matches nothing and has no `default` **halts**, rather than falling through.
  * A `parallel` block reports the first failure **by declared order**, and runs every branch.
  * A send that never got a response halts **even with a choice following**, because there is no
    response for the choice to inspect.
  * An extraction in one step is visible to `{{substitution}}` in the next -- the whole reason the
    variable scope is re-derived per request rather than once per scenario.

Usage: src/test/other/scenario_run_test.py [path-to-ziman]   Exit 0 = all pass.
"""
import json, os, shutil, subprocess, sys, tempfile, threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(os.path.dirname(HERE)))
DRIVER = os.path.join(HERE, "scenario_driver.zm")
ZIMAN = (sys.argv[1] if len(sys.argv) > 1 else shutil.which("ziman")
         or os.path.expanduser("~/workspace/vasna.tech/repos/ziman/ziman"))
PORT = 18741
failures = 0
seen = []          # every path the stub was asked for, in order
echoed = []        # the X-Token header value /echo received, per request
seen_lock = threading.Lock()


def check(name, ok, detail=""):
    global failures
    print(("PASS " if ok else "FAIL ") + name + ("" if ok else f"  ({detail})"))
    failures += 0 if ok else 1


class Stub(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_GET(self):
        path = urlparse(self.path).path
        with seen_lock:
            seen.append(path)
        if path in ("/ok", "/ok2", "/other-ok"):
            return self._send(200, {"ok": True})
        if path == "/created":
            return self._send(201, {"created": True})
        if path in ("/bad", "/bad2"):
            return self._send(500, {"error": path})
        if path == "/no-such-path":
            return self._send(404, {"error": "missing"})
        if path == "/token":
            return self._send(200, {"token": "abc123"})
        if path == "/echo":
            # **What this endpoint is for**: recording the header it received, so the test can
            # assert that substitution in a later step saw an earlier step's extraction rather
            # than sending the unresolved `{{TOKEN}}` or nothing at all.
            value = self.headers.get("X-Token", "(absent)")
            with seen_lock:
                echoed.append(value)
            return self._send(200, {"seen": value})
        return self._send(404, {"error": "no such path"})

    def _send(self, status, payload):
        body = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


def run_case(case):
    """One scenario run, in its own storage root so no case can see another's session."""
    with seen_lock:
        seen.clear()
        echoed.clear()
    root = tempfile.mkdtemp()
    try:
        env = dict(os.environ, ZIMAN_MODULE_PATH=os.path.join(ROOT, "src", "main", "ziman"),
                   CURLY_CASE=case, CURLY_ROOT=root, CURLY_BASE=f"http://127.0.0.1:{PORT}")
        done = subprocess.run([ZIMAN, DRIVER], cwd=ROOT, env=env, capture_output=True, text=True)
        if done.returncode != 0:
            raise SystemExit(f"driver failed for {case}: {done.stderr or done.stdout}")
        return json.loads(done.stdout.strip().splitlines()[-1])
    finally:
        shutil.rmtree(root, ignore_errors=True)


def refs(result):
    return [s["reference"] for s in result["steps"]]


def main():
    server = ThreadingHTTPServer(("127.0.0.1", PORT), Stub)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        # -- a plain sequence --
        r = run_case("sequence")
        check("a sequence runs every step in order and completes",
              r["outcome"] == "ok" and refs(r) == ["ok", "ok2", "created"], r)
        check("a 2xx that is not 200 is still a success", r["steps"][2]["status"] == 201, r["steps"][2])

        # -- an unhandled failure stops the run --
        r = run_case("failure-halts")
        check("an unhandled non-2xx halts the run",
              r["outcome"] == "halted" and refs(r) == ["ok", "bad"], r)
        check("the halt names the request and the status",
              '"bad"' in r["halt_reason"] and "500" in r["halt_reason"], r["halt_reason"])
        check("nothing after the failed step was sent", "/ok2" not in seen, seen)

        # -- a following choice forgives it --
        r = run_case("failure-handled-by-choice")
        check("a non-2xx handled by a following choice does not halt",
              r["outcome"] == "ok" and refs(r) == ["bad", "ok", "ok2"], r)

        # -- a choice that matches nothing --
        r = run_case("choice-no-match-no-default")
        check("a choice matching no case with no default halts",
              r["outcome"] == "halted" and "no default" in r["halt_reason"], r)
        r = run_case("choice-no-match-with-default")
        check("a default is taken when no case matches",
              r["outcome"] == "ok" and refs(r) == ["ok", "created"], r)
        r = run_case("choice-without-a-preceding-request")
        check("a choice with no preceding response halts rather than guessing",
              r["outcome"] == "halted" and "no response" in r["halt_reason"], r)

        # -- parallel --
        r = run_case("parallel-all-ok")
        check("a parallel block of successes completes",
              r["outcome"] == "ok" and sorted(refs(r)) == ["ok", "ok2"], r)
        # **Declared-order selection, made observable.** Two branches fail with different statuses
        # and a choice on 500 follows: it matches only if the block propagated the first failure by
        # declared order. Asserting the halt message instead would prove nothing -- a failing
        # parallel block is a `Requests` node, so the message is the generic "a step returned ..."
        # with no branch named, exactly as Rust's own `_ =>` arm writes it.
        r = run_case("parallel-first-failure-wins")
        check("the first failure by declared order is the one propagated (500 before 404)",
              r["outcome"] == "ok" and refs(r)[-1] == "ok", r)
        check("and both branches still ran", "/bad" in seen and "/no-such-path" in seen, seen)
        r = run_case("parallel-first-failure-wins-reversed")
        check("reversing the branches propagates the other failure, so the choice does not match",
              r["outcome"] == "halted" and "404" in r["halt_reason"], r)
        r = run_case("parallel-later-failure-is-reported")
        check("when only a later branch fails, its response is the one propagated",
              r["outcome"] == "ok" and refs(r)[-1] == "ok2", r)
        r = run_case("parallel-runs-every-branch")
        check("a failing branch does not stop its siblings from running",
              sorted(seen) == ["/bad", "/ok", "/ok2"], seen)
        check("and every branch's step is logged", sorted(refs(r)) == ["bad", "ok", "ok2"], refs(r))
        r = run_case("parallel-failure-handled-by-choice")
        check("a parallel block's failure is forgiven by a following choice, as a plain step is",
              r["outcome"] == "ok" and refs(r)[-1] == "ok2", r)
        r = run_case("parallel-empty")
        check("an empty parallel block completes and sends nothing",
              r["outcome"] == "ok" and r["steps"] == [] and seen == [], (r, seen))

        # -- a send that never landed --
        r = run_case("network-error-halts-despite-a-choice")
        check("a network error halts even with a choice following",
              r["outcome"] == "halted" and "failed to send" in r["halt_reason"], r)
        check("the step records the attempt with no status and an error",
              r["steps"][0]["status"] is None and r["steps"][0]["error"], r["steps"][0])
        check("the choice after it never ran", len(r["steps"]) == 1, r["steps"])

        # -- imports --
        r = run_case("imports-route-by-alias")
        check("a bare reference and an aliased one route to different collections",
              r["outcome"] == "ok" and seen == ["/ok", "/other-ok"], seen)
        r = run_case("unknown-alias")
        check("an alias the scenario does not import is an error naming it",
              "error" in r and "nope" in r["error"], r)
        r = run_case("missing-request")
        check("a reference to a request that is not in the collection names both",
              "error" in r and "not-a-request" in r["error"] and "main" in r["error"], r)

        # -- extraction flowing forward --
        r = run_case("extraction-flows-into-the-next-step")
        check("an extraction is recorded on the step that produced it",
              r["outcome"] == "ok" and r["steps"][0]["extracted"] == ["TOKEN"], r["steps"][0])
        # **The decisive assertion**: the stub recorded the header it was sent. An extraction that
        # did not reach the session, or a scope re-derived once per scenario instead of per request,
        # would send the literal `{{TOKEN}}` or fail substitution outright -- and either way this
        # value would not be `abc123`.
        check("the next step's {{substitution}} really carried the extracted value",
              echoed == ["abc123"], echoed)
    finally:
        server.shutdown()

    print(f"\n{'all passed' if failures == 0 else str(failures) + ' failed'}")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
