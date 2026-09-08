# Curly — Roadmap

Not-yet-committed feature ideas, kept separate from [DESIGN.md](DESIGN.md) §9 (which tracks *decided/shipped* milestones — M1 through M6, though M6 itself is still just a placeholder pointing back here until it's actually built) so exploratory thinking doesn't get mixed in with settled status. When an idea here gets scoped and built, its outcome moves to DESIGN.md the way every other feature in this project has (a "done, here's what shipped vs. deviated" writeup), and its entry here is removed or marked superseded.

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
  "request": "path"
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
          { "body": { "path": "mfaRequired", "op": { "eq": true } } }
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

`when` targets `status` (`{"eq": ...}` directly, nothing to name), `header` (`{"name": "...", "op": {"eq": ...}}`), or `body` (`{"path": "...", "op": {"eq": ...}}`, dot/bracket syntax — reuses `extraction::get_json_path`, already written and tested, rather than a second JSON-path implementation), each `op`/`status` an operator object (`eq`, `ne`, `gt`, `lt`, `contains`, `exists`, ...), composable via `all`/`any`/`not`. `then`/`default` are themselves scenario nodes (a `request`, or a nested `requests` sequence/parallel), so a matched case can fan out into its own multi-step sub-scenario, same as any other node.
>
> **Implementation note (post-build):** an earlier draft of this sketch showed `header`/`body`'s operator as a sibling key (`{"body": "path", "eq": value}`) rather than nested the way `status` always was (`{"status": {"eq": value}}`) — an accidental asymmetry from an example that was never meant to be a field-name commitment. The shipped shape (above) nests all three consistently, since a variable-key sibling (the operator's own name varies) isn't something a derived `Deserialize` expresses cleanly. See DESIGN.md's M6 entry for the full reasoning.

**`default` is optional. When it's omitted and no `case` matches, the scenario halts with a clear error naming what didn't match** (e.g. `"choice: response status 500 matched no case (defined: 200, 400) and no default was given"`) — not a silent no-op. This was deliberately chosen over "continue past the choice as if it wasn't there," and it's worth recording *why*, since an earlier pass through this reasoning leaned the other way before working through a concrete example changed it: extraction skipping on a non-2xx response is a genuinely different situation — the request already ran either way, extraction is a bonus step layered on top, so skipping it is a true no-op. A `choice` node *is* the thing deciding what happens next; if nothing matches, the scenario genuinely doesn't know what to do, and silently doing nothing there would be exactly the kind of surprise curly refuses elsewhere (`substitution::resolve` errors on an undefined `{{var}}` rather than sending it literally; Save As errors on a path collision rather than silently overwriting). `default` remains available for a genuine, deliberate fallback — including a deliberate no-op, if that's actually wanted, but only by writing it on purpose.

### Decided (2026-09): storage location, and cross-collection references via `imports`

**Scenarios get their own file**, `.curly/scenarios/<slug>.json`, one per scenario — mirroring collections exactly (`.curly/collections/<slug>.json`, `Storage::collections_dir`/`collection_path`), not nested inside a `Collection`. Same pattern of new `Storage` methods: `scenarios_dir`/`scenario_path`/`list_scenarios`/`load_scenario`/`load_scenario_opt`/`save_scenario`/`delete_scenario`, no new machinery invented. This was a reversal of the earlier "inside `Collection`" lean, once the real need — a scenario spanning *more than one* collection — came up; a request path is only unambiguous within a single collection's own file, so a scenario that isn't itself scoped to one collection can't live inside one.

A scenario file declares which collections it needs, and under what local name, via `imports` — a map from **alias** to real collection name:

```json
{
  "name": "onboarding",
  "imports": {
    "default": "health-record-api"
  },
  "requests": { "type": "sequence", "items": [ /* ... */ ] }
}
```

A request reference is resolved one of two ways:
- **`"alias->path"`** — split once on the first `->`; the left side must be a key in `imports`, the right side is handed to that collection's `Collection::find_request` exactly as-is (still `/`-nested for folders, unchanged). Deliberately `->` and not another `/` — request paths already use `/` for folder nesting (`"Auth/OAuth/login"`), and reusing it again for "cross into a different collection" would make the same character mean two different things depending on where you are in the string; the CLI's own `curly run <collection>/<path>` also uses `/`, but there it's the *real* collection name doing the splitting, not an alias, so visually reusing `/` for the alias case would look identical to that while actually meaning something different.
- **A bare path with no `->`** — resolved against whichever import is named `"default"`. If no import is named `"default"`, this is an undefined-reference error, not a silent guess at which collection was meant.

`"default"` is not a keyword with special parsing — it's simply the alias name this convention singles out; nothing stops also writing `"default->auth/register"` explicitly, and both forms resolve identically. A single-collection scenario (the common case) names its one import `"default"` and every reference stays a bare path, same ergonomics as before `imports` existed; a scenario spanning multiple collections only needs the explicit `alias->path` form for references into the *non-default* ones.

### Worked example: health-record onboarding

A concrete run through the format above, against real endpoints in this project's own `health-record-api` collection (not placeholders) — `auth/register`'s actual behavior, read from `AuthController`/`AuthService`/`GlobalExceptionHandler` rather than assumed: `200` on success, `400` (`"Email already in use"`, not `409`) if the email's already registered. This is the full contents of what `.curly/scenarios/onboarding.json` would hold — no outer wrapper key, matching how a collection's own file *is* the `Collection` object directly rather than `{"collection": {...}}`:

```json
{
  "name": "onboarding",
  "imports": {
    "default": "health-record-api"
  },
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
```

`register` runs first. A `200` fans out into the onboarding wizard (settings → measurements → views → notifications, in order); a `400` (email already registered) logs the user in instead of failing the scenario outright. Per the decision above, any *other* status (a `500`, say) halts the scenario with a clear error, since neither `case` matches and there's no `default` in the `choice` — a different, coincidentally-same-named thing from the `imports` alias called `"default"` above; the two `default`s don't interact.

Two honesty notes baked into this example, not smoothed over: there's no dedicated `/views` endpoint in this codebase — per the `configurable_views_prefs_bug` work, view visibility is configured through the *same* `me/update-preferences` endpoint as general settings, so "views" here is a second call to that endpoint with a different (unshown) payload, not a distinct route. And the two `me/update-preferences` calls only make sense as written if their request bodies actually differ — this sketch doesn't fill those in.

If a second collection joined in (say a separate `billing-api`), only `imports` and the specific steps that need it change — everything else, including every existing bare reference, is untouched:

```json
"imports": { "default": "health-record-api", "billing": "billing-api" },
...
{ "request": "billing->accounts/create" },
{ "request": "me/get" }
```

### Decided (2026-09): failure semantics

An ordinary `request` node's 4xx/5xx (or an extraction failure) **halts its own branch by default — unless it's explicitly handled**. "Explicitly handled" has a precise meaning, not a vibe: if the very next node in the same sequence is a `choice`, that `choice` gets first look at the response regardless of status — a matching `case` (or a `default`) means the failure was exactly what the `choice` was there for, so nothing halts; only a status that `choice` *also* doesn't handle (no case, no default — already decided above) halts. A halt at the scenario's own top level ends the whole scenario; a halt inside a `parallel` branch ends only that branch.

**In a `parallel` block, one branch failing does not cancel its siblings** — every branch runs to completion (or its own halt) independently. Whatever comes after the block (typically a `choice`) sees the failed branch's response the same way it would see a plain sequential failure — a `parallel` block's failure is handled through the exact same mechanism as any other, not a special case bolted on.

**Decided: what "the next node" sees when *more than one* branch fails.** Six options were weighed (first-failure by real-time completion order; first-failure by declared/array order; named branches with per-branch targeting; unnamed aggregate conditions like `any_failure`; a synthesized "worst-of" pseudo-response; treating 2+ simultaneous failures as an automatic hard halt) — full writeup of all six kept in this session's history, not reproduced here since only the outcome matters going forward. **Chosen: first failure by declared order** — among the branches that failed, the one appearing earliest in the `parallel` block's own `items` array is what a following `choice` (or any other consumer) sees, regardless of which one actually finished first in real time. Deterministic and needs no schema change (no branch naming), at the acknowledged cost of being an arbitrary pick when it matters most — "branch 0 over branch 1" isn't meaningful, just consistent run to run.

**Named branches with explicit per-branch targeting** (give a branch an optional `id`; `choice.when` gains `{"branch": "doctor-track", "status": {"eq": 500}}` plus aggregate `any_branch_failed`/`all_branches_failed` conditions) **is an accepted future extension, not built now** — worth adding once a real scenario needs to distinguish *which* branch failed rather than accepting "whichever came first in the array." Revisit if/when that need actually shows up; don't build it speculatively ahead of one.

### Decided (2026-09): authoring shape

**JSON-only for v1** — hand-edit `.curly/scenarios/<name>.json` directly. No CLI flag-based scenario-authoring subcommand: nested `sequence`/`parallel`/`choice` trees plus `imports` don't map onto flat CLI flags the way a single request's headers/body do (where `curly collections add-request` flags work fine). `curly scenario run <name>` is the only CLI surface M6 needs to *execute* a scenario; `curly scenario list`/`show` (mirroring `collections list`/`show`) are natural to add alongside for parity, but creating/editing a scenario stays JSON-only.

A **GUI scenario editor/designer** (a visual sequence/parallel/choice builder) is a real idea for later — raised by the user, explicitly not part of M6, not scoped or committed. Worth its own design pass whenever it's picked up, not backed into as a side effect of building the execution engine first.

### Open questions (unresolved — next session should raise these, not assume answers)

1. **DAG vs. nesting, finally confirmed?** — every worked example so far has used nesting without objection, and the recursive node format above has been adopted, but the user hasn't explicitly ruled out needing arbitrary "step D depends on both A and C, not just the previous step" dependency edges for some future scenario. Treat nesting as the working default, not a closed decision, until a real scenario surfaces that it can't express.
2. **Output**: a per-step summary (status/ms/pass-fail, test-report-style) vs. each step's normal `run` output back-to-back vs. something configurable?
