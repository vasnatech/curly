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

**Recommendation**: don't adopt any of these wholesale — they carry error-handling/retry/branching semantics curly doesn't need. Borrow the one primitive that actually matters and that all of them converge on for the common case: **nested sequence/parallel composition**. A sequence is an ordered list of steps; a step can itself *be* a parallel block (a list of branches, each its own sequence, that run concurrently and join before the scenario continues). This covers essentially every realistic API-test scenario without needing arbitrary dependency-edge graphs (Airflow-style), while staying expressible in curly's own plain, hand-editable JSON — no new file format, no external spec dependency.

Sketch (loosely Step-Functions-flavored, not a commitment to exact field names):

```json
"scenario": {
  "name": "onboarding",
  "steps": [
    { "request": "auth/login" },
    { "parallel": [
        [ { "request": "vitals/create" } ],
        [ { "request": "medications/create" } ]
    ]},
    { "request": "me/get" }
  ]
}
```

### Open questions (unresolved — next session should raise these, not assume answers)

1. **Is nested sequence/parallel expressive enough?** — or does real usage need arbitrary "step C depends on steps A and B specifically" dependency edges (a true DAG, not just nesting)? Posed to the user, not yet answered.
2. **Where do scenarios live?** — inside `Collection` (`Collection.scenarios: Vec<Scenario>`, alongside `folders`/`requests`) is the natural fit given everything else lives there, but not yet decided.
3. **Assertions**: pure orchestration (as scoped above) vs. crossing into REQUIREMENTS.md's deferred "test-runner with assertions" territory — explicit call needed before implementation, not implicit.
4. **Failure semantics**: does one step's 4xx/5xx (or extraction failure) halt the whole scenario (matches `--fail`'s existing spirit) by default? What happens to a parallel block where one branch fails and others are still running — cancel the rest, or let them finish and report all failures together?
5. **Output**: a per-step summary (status/ms/pass-fail, test-report-style) vs. each step's normal `run` output back-to-back vs. something configurable?
6. **CLI shape**: `curly scenario run <collection>/<scenario-name>`, mirroring `curly run`? How are scenarios authored — `collections add-scenario` flags, or JSON-only (like extraction rules currently support both)?
