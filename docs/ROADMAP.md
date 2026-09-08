# Curly — Roadmap

Not-yet-committed feature ideas, kept separate from [DESIGN.md](DESIGN.md) §9 (which tracks *decided/shipped* milestones — M1 through M5) so exploratory thinking doesn't get mixed in with settled status. When an idea here gets scoped and built, its outcome moves to DESIGN.md the way every other feature in this project has (a "done, here's what shipped vs. deviated" writeup), and its entry here is removed or marked superseded.

## Scenarios — multi-request orchestration (linear + parallel)

**Problem**: `curly run` executes one saved request at a time. Chaining several (e.g. login → create-patient → add-vitals → verify) currently means separate `curly run` invocations, relying on extraction/sessions (shipped, see DESIGN.md §9's FR-10 entry) to pass variables between them. A "scenario" would let a *sequence* of such steps be defined once and executed as a single command — and the user asked for scenarios that support both **linear** (must-happen-in-order) and **parallel** (can-happen-at-the-same-time) steps.

**Explicitly not this**: REQUIREMENTS.md's non-goals list "automated test-suite runners with assertions" as out of v1 scope, deferred to a possible v2. A pure orchestration feature (run these requests, in this order/parallelism, using the existing session-chaining and `--fail`-style status handling) stays on the right side of that line. Adding per-step assertions ("expect status 200," "expect `body.role == 'admin'`") would cross it into test-framework territory — a real scope decision to make explicitly if/when this gets built, not something to back into.

### Modeling research (2026-09)

Asked whether a standard exists for "some steps sequential, some parallel." The general theoretical model is a **DAG (directed acyclic graph) of tasks**, with **fork-join** as the specific name for "split into parallel branches, then wait for all of them before continuing." Concrete standards/precedents surveyed, heaviest to lightest:

- **BPMN** (ISO/IEC 19510) — the formal standard for business process modeling: sequence flows, parallel/exclusive *gateways*, events, lanes, sub-processes. XML-based, built for visual modeling tools. Far more machinery than an API-testing tool needs.
- **CNCF Serverless Workflow** — a newer JSON/YAML spec aiming at "BPMN but sane." Same spirit as Step Functions below, more modern/community-driven.
- **AWS States Language** (Step Functions) — a plain-JSON state machine spec: `Task`, `Parallel` (with `Branches: [...]`, each its own sub-sequence, joining when all branches finish), `Choice`, `Map`. JSON-native, task-oriented — the closest real-world precedent to what curly needs, and a de facto standard many developers have already seen.
- **CI pipeline DAGs** — GitHub Actions' `needs: [...]` (arbitrary dependency edges between named jobs) and GitLab's `stages` (sequential waves of parallel jobs). The pattern most developers meet daily without calling it a standard.

**Recommendation**: don't adopt any of these wholesale — they carry error-handling/retry/branching semantics curly doesn't need. Borrow the one primitive that actually matters and that all of them converge on for the common case: **nested sequence/parallel composition**. A sequence is an ordered list of steps; a step can itself *be* a parallel block of branches that run concurrently and join before the scenario continues. This covers essentially every realistic API-test scenario without needing arbitrary dependency-edge graphs (Airflow-style DAGs — see "DAG vs. nesting" below for the concrete tradeoff), while staying expressible in curly's own plain, hand-editable JSON — no new file format, no external spec dependency.

### Decided (2026-09): node format

Every scenario node is one of two shapes — a leaf (a single request) or a composite (a `sequence` or `parallel` of child nodes, each itself a leaf or composite). Fully uniform and recursive, rather than special-casing "a parallel branch is a raw array of steps" — the implementation is one recursive function over one node type, and a branch can itself contain a multi-step sequence without inventing a second shape for it:

```json
{
  "request": "collection/path"
}
```
```json
{
  "requests": {
    "type": "sequence",
    "items": [ /* nodes */ ]
  }
}
```
```json
{
  "requests": {
    "type": "parallel",
    "items": [ /* nodes, run concurrently, joined before the parent continues */ ]
  }
}
```

A top-level `scenario.requests` is itself one such node (conventionally a `sequence`).

### Decided (2026-09): conditional branching (`choice` steps)

A third node kind, alongside `request` and `requests`: routes to a different child node based on the *previous* step's response. This is control flow, not test assertions — it never itself produces a pass/fail verdict, so it stays on the right side of REQUIREMENTS.md's "no test-runner with assertions" non-goal (see "Explicitly not this" above); it only decides which node runs next.

```json
{
  "choice": {
    "cases": [
      {
        "when": { "all": [
          { "status": { "eq": 200 } },
          { "body": "mfaRequired", "eq": true }
        ]},
        "then": { "request": "auth/mfa-verify" }
      },
      {
        "when": { "status": { "eq": 200 } },
        "then": { "request": "me/get" }
      }
    ],
    "default": { "request": "handle-login-failure" }
  }
}
```

`when` targets `status`, `header: "<name>"`, or `body: "<path>"` (dot/bracket syntax — reuses `extraction::get_json_path`, already written and tested, rather than a second JSON-path implementation), each with an operator (`eq`, `ne`, `gt`, `lt`, `contains`, `exists`, ...), composable via `all`/`any`/`not`. `then`/`default` are themselves scenario nodes (a `request`, or a nested `requests` sequence/parallel), so a matched case can fan out into its own multi-step sub-scenario, same as any other node.

**`default` is optional. When it's omitted and no `case` matches, the scenario halts with a clear error naming what didn't match** (e.g. `"choice: response status 500 matched no case (defined: 200, 400) and no default was given"`) — not a silent no-op. This was deliberately chosen over "continue past the choice as if it wasn't there," and it's worth recording *why*, since an earlier pass through this reasoning leaned the other way before working through a concrete example changed it: extraction skipping on a non-2xx response is a genuinely different situation — the request already ran either way, extraction is a bonus step layered on top, so skipping it is a true no-op. A `choice` node *is* the thing deciding what happens next; if nothing matches, the scenario genuinely doesn't know what to do, and silently doing nothing there would be exactly the kind of surprise curly refuses elsewhere (`substitution::resolve` errors on an undefined `{{var}}` rather than sending it literally; Save As errors on a path collision rather than silently overwriting). `default` remains available for a genuine, deliberate fallback — including a deliberate no-op, if that's actually wanted, but only by writing it on purpose.

### Worked example: health-record onboarding

A concrete run through the format above, against real endpoints in this project's own `health-record-api` collection (not placeholders) — `auth/register`'s actual behavior, read from `AuthController`/`AuthService`/`GlobalExceptionHandler` rather than assumed: `200` on success, `400` (`"Email already in use"`, not `409`) if the email's already registered.

```json
{
  "scenario": {
    "name": "onboarding",
    "requests": {
      "type": "sequence",
      "items": [
        { "request": "auth/register" },
        {
          "choice": {
            "cases": [
              {
                "when": { "status": { "eq": 200 } },
                "then": {
                  "requests": {
                    "type": "sequence",
                    "items": [
                      { "request": "me/update-preferences" },
                      { "request": "body-measurements/update-preferences" },
                      { "request": "me/update-preferences" },
                      { "request": "notifications/update-preference" }
                    ]
                  }
                }
              },
              {
                "when": { "status": { "eq": 400 } },
                "then": { "request": "auth/login" }
              }
            ]
          }
        }
      ]
    }
  }
}
```

`register` runs first. A `200` fans out into the onboarding wizard (settings → measurements → views → notifications, in order); a `400` (email already registered) logs the user in instead of failing the scenario outright. Per the decision above, any *other* status (a `500`, say) halts the scenario with a clear error, since neither `case` matches and there's no `default`.

Two honesty notes baked into this example, not smoothed over: there's no dedicated `/views` endpoint in this codebase — per the `configurable_views_prefs_bug` work, view visibility is configured through the *same* `me/update-preferences` endpoint as general settings, so "views" here is a second call to that endpoint with a different (unshown) payload, not a distinct route. And the two `me/update-preferences` calls only make sense as written if their request bodies actually differ — this sketch doesn't fill those in.

### Open questions (unresolved — next session should raise these, not assume answers)

1. **DAG vs. nesting, finally confirmed?** — every worked example so far (including the one above) has used nesting without objection, and the recursive node format above has been adopted, but the user hasn't explicitly ruled out needing arbitrary "step D depends on both A and C, not just the previous step" dependency edges for some future scenario. Treat nesting as the working default, not a closed decision, until a real scenario surfaces that it can't express.
2. **Where do scenarios live?** — inside `Collection` (`Collection.scenarios: Vec<Scenario>`, alongside `folders`/`requests`) is the natural fit given everything else lives there, but not yet decided.
3. **General step failure semantics** — distinct from the `choice`-specific decision above, which only covers "no case matched." Does an ordinary `request` node's own 4xx/5xx (or an extraction failure) halt the whole scenario by default (matching `--fail`'s existing spirit and the "fail loud" reasoning used for `choice` above), or is that configurable per-scenario? What happens to a `parallel` block where one branch fails and others are still running — cancel the rest, or let them finish and report all failures together?
4. **Output**: a per-step summary (status/ms/pass-fail, test-report-style) vs. each step's normal `run` output back-to-back vs. something configurable?
5. **CLI shape**: `curly scenario run <collection>/<scenario-name>`, mirroring `curly run`? How are scenarios authored — `collections add-scenario` flags, or JSON-only (nested sequence/parallel/choice trees don't map cleanly onto flat CLI flags the way a single request's headers/body do, so JSON-only may be the more honest answer here even though extraction rules support flags)?
