//! Scenarios (M6, FR-25): multi-request orchestration — a named, nested
//! sequence/parallel composition of saved requests, potentially spanning
//! more than one collection, with conditional branching (`choice` nodes) on
//! a response's status/header/body. Pure orchestration, no assertions (see
//! REQUIREMENTS.md's non-goal on test-suite runners) — a `choice` never
//! produces a pass/fail verdict, it only decides which node runs next.
//!
//! The full design — node format, `choice` semantics, storage location and
//! cross-collection `imports`, and failure semantics (including what
//! happens when more than one `parallel` branch fails) — was worked through
//! in `docs/ROADMAP.md` before any of this was written; this module is that
//! design, not a reinterpretation of it. Read the ROADMAP section first if
//! something here looks arbitrary — it probably has a documented reason.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::exec::{self, ResponseSummary};
use crate::extraction::{self, get_json_path};
use crate::storage::{Collection, Storage};
use crate::substitution;

// --- Data model ---

/// A named scenario, loaded from its own file (`.curly/scenarios/<slug>.json`
/// — see `Storage::load_scenario`), not nested inside a `Collection`: a
/// scenario's `imports` can name more than one collection, so it can't live
/// inside any single one of them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scenario {
    #[serde(default = "Uuid::new_v4")]
    pub id: Uuid,
    pub name: String,
    /// Alias → real collection name. A request reference either names an
    /// alias explicitly (`"alias->path"`) or, if it has no `"->"`, resolves
    /// against whichever import is named `"default"` — see
    /// `resolve_request_ref`.
    #[serde(default)]
    pub imports: BTreeMap<String, String>,
    /// Always a `sequence`/`parallel` container, not a bare `ScenarioNode`
    /// — the struct field is already named `requests`, so its value is a
    /// `RequestsNode` directly (`{"type": ..., "items": [...]}`), not a
    /// second `{"requests": {...}}` wrapper around that. A scenario's top
    /// level is always "a sequence (or parallel block) of steps," never a
    /// single bare request or a bare `choice`.
    pub requests: RequestsNode,
}

impl Scenario {
    pub fn new(name: impl Into<String>, requests: RequestsNode) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            imports: BTreeMap::new(),
            requests,
        }
    }
}

/// One node in a scenario's tree — a leaf (`request`) or a composite
/// (`requests`: a `sequence` or `parallel` of child nodes, or `choice`:
/// routes to a different child based on the previous response). Untagged
/// rather than internally tagged: each variant is keyed by its own distinct
/// JSON field name (`"request"`/`"requests"`/`"choice"`), so there's no
/// ambiguity for serde to resolve despite no explicit `"type"` discriminator.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ScenarioNode {
    Request { request: String },
    Requests { requests: RequestsNode },
    Choice { choice: ChoiceNode },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestsNode {
    #[serde(rename = "type")]
    pub kind: SequenceKind,
    pub items: Vec<ScenarioNode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SequenceKind {
    Sequence,
    Parallel,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChoiceNode {
    pub cases: Vec<ChoiceCase>,
    /// Optional — an unmatched response with no `default` halts the
    /// scenario with a clear error rather than silently continuing. See
    /// this module's own doc comment / ROADMAP.md for why that's
    /// deliberate, not an oversight.
    #[serde(default)]
    pub default: Option<Box<ScenarioNode>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChoiceCase {
    pub when: Condition,
    pub then: Box<ScenarioNode>,
}

/// A condition evaluated against a response's status, a header, or a body
/// JSON path — composable via `all`/`any`/`not`. Untagged for the same
/// reason as `ScenarioNode`: each variant has its own distinct key.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Condition {
    All { all: Vec<Condition> },
    Any { any: Vec<Condition> },
    Not { not: Box<Condition> },
    Status { status: Comparison },
    Header { header: NamedComparison },
    Body { body: PathComparison },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamedComparison {
    pub name: String,
    pub op: Comparison,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathComparison {
    pub path: String,
    pub op: Comparison,
}

/// A single comparison operator + operand. `exists` is the only operator
/// that makes sense without a response actually having the header/body
/// path at all — the others simply don't match (not error) against an
/// absent value, same "false rather than a crash" spirit as
/// `Collection::find_request` returning `None` rather than panicking.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Comparison {
    Eq { eq: Value },
    Ne { ne: Value },
    Gt { gt: Value },
    Lt { lt: Value },
    Contains { contains: Value },
    Exists { exists: bool },
}

// --- Pure logic: condition evaluation ---

/// Evaluate `condition` against a response's status/headers/body. Never
/// errors — an absent header/body path just makes that specific leaf
/// condition `false` (except `exists: false`, which is exactly the
/// condition that's satisfied by absence).
pub fn evaluate_condition(condition: &Condition, status: u16, headers: &[(String, String)], body: &str) -> bool {
    match condition {
        Condition::All { all } => all.iter().all(|c| evaluate_condition(c, status, headers, body)),
        Condition::Any { any } => any.iter().any(|c| evaluate_condition(c, status, headers, body)),
        Condition::Not { not } => !evaluate_condition(not, status, headers, body),
        Condition::Status { status: cmp } => compare(&Value::from(status), cmp),
        Condition::Header { header } => {
            let found = headers
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(&header.name))
                .map(|(_, value)| value.clone());
            match (&header.op, found) {
                (Comparison::Exists { exists }, found) => found.is_some() == *exists,
                (op, Some(value)) => compare(&Value::String(value), op),
                (_, None) => false,
            }
        }
        Condition::Body { body: target } => {
            let parsed: Option<Value> = serde_json::from_str(body).ok();
            let found = parsed.as_ref().and_then(|v| get_json_path(v, &target.path));
            match (&target.op, found) {
                (Comparison::Exists { exists }, found) => found.is_some() == *exists,
                (op, Some(value)) => compare(value, op),
                (_, None) => false,
            }
        }
    }
}

fn compare(actual: &Value, cmp: &Comparison) -> bool {
    match cmp {
        Comparison::Eq { eq } => actual == eq,
        Comparison::Ne { ne } => actual != ne,
        Comparison::Gt { gt } => actual.as_f64().zip(gt.as_f64()).is_some_and(|(a, b)| a > b),
        Comparison::Lt { lt } => actual.as_f64().zip(lt.as_f64()).is_some_and(|(a, b)| a < b),
        Comparison::Contains { contains } => match actual {
            Value::String(s) => contains.as_str().is_some_and(|needle| s.contains(needle)),
            Value::Array(items) => items.contains(contains),
            _ => false,
        },
        Comparison::Exists { .. } => true,
    }
}

// --- Pure logic: cross-collection request references ---

/// Split a scenario's request reference into `(alias, path-within-that-
/// collection)` — `"alias->path"` explicitly, splitting once on the first
/// `"->"`, or a bare `"path"` (no `"->"`) resolved against whichever import
/// is named `"default"`. Errors if the resolved alias isn't a key in
/// `known_aliases` — including the bare-path case with no `"default"`
/// import, which is an undefined reference, not a silent guess. Generic
/// over the map's value type since callers have two different things keyed
/// by alias: `Scenario.imports` (alias → collection *name*, unresolved) and
/// `ExecDeps.collections` (alias → the already-*loaded* `Collection`) — this
/// only ever needs the key set, not the value.
pub fn resolve_request_ref<'a, V>(reference: &'a str, known_aliases: &BTreeMap<String, V>) -> Result<(&'a str, &'a str)> {
    let (alias, path) = match reference.split_once("->") {
        Some((alias, path)) => (alias, path),
        None => ("default", reference),
    };
    if !known_aliases.contains_key(alias) {
        let known = known_aliases.keys().cloned().collect::<Vec<_>>().join(", ");
        if alias == "default" && !reference.contains("->") {
            bail!("request \"{reference}\" has no \"->\" and no import is named \"default\" (imports: {known})");
        }
        bail!("request \"{reference}\" references undefined import \"{alias}\" (imports: {known})");
    }
    Ok((alias, path))
}

// --- Execution ---

/// How one node finished — `Ok` (nothing to see here), `HttpFailure` (a
/// `request` node got a real non-2xx response — *provisionally* a failure;
/// `execute_sequence` may still forgive it if the very next sibling is a
/// `choice`), or `Halted` (final — a network-level send error, an
/// unhandled `HttpFailure`, or a `choice` that matched nothing and had no
/// `default`). Only a plain `request` node ever produces `HttpFailure`;
/// every composite node either propagates a child's `Halted` or resolves to
/// `Ok`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepOutcome {
    Ok,
    HttpFailure,
    Halted(String),
}

/// One `request` node's outcome, in the order it completed — the flat
/// "back-to-back" log this module produces isn't a final decision on
/// output format (still open, see ROADMAP.md), just a first pass simple
/// enough to build on.
#[derive(Debug, Clone)]
pub struct StepLog {
    pub reference: String,
    pub method: String,
    pub url: String,
    /// `None` only when the request never got a response at all (a
    /// network-level send failure — see `error`).
    pub status: Option<u16>,
    pub error: Option<String>,
    pub extracted: Vec<String>,
}

pub struct ScenarioRun {
    pub outcome: StepOutcome,
    pub steps: Vec<StepLog>,
}

struct ExecDeps<'a> {
    storage: &'a Storage,
    client: &'a reqwest::Client,
    /// Every imported collection, pre-loaded once and keyed by *alias* (not
    /// real collection name) — so `resolve_request_ref`'s alias is a direct
    /// map key, and a scenario's collections don't need re-reading from
    /// disk on every request node.
    collections: &'a BTreeMap<String, Collection>,
    env_name: Option<&'a str>,
    var_overrides: &'a [(String, String)],
}

struct NodeResult {
    outcome: StepOutcome,
    /// The response this node hands to whatever comes next (typically a
    /// `choice`) — carried forward even on `HttpFailure`, since that's
    /// exactly what a following `choice` needs to inspect.
    response: Option<ResponseSummary>,
    steps: Vec<StepLog>,
}

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Run a scenario to completion. Every imported collection is loaded once
/// up front (erroring immediately if one doesn't exist, rather than partway
/// through execution); `env_name`/`var_overrides` mirror `curly run`'s own
/// `--env`/`--var` precedence exactly (see this module's private
/// `merged_variables`) and apply to every request in the scenario, not just
/// the first.
pub async fn run(
    scenario: &Scenario,
    storage: &Storage,
    client: &reqwest::Client,
    env_name: Option<&str>,
    var_overrides: &[(String, String)],
) -> Result<ScenarioRun> {
    let mut collections = BTreeMap::new();
    for (alias, name) in &scenario.imports {
        let collection = storage.load_collection(name).with_context(|| {
            format!(
                "scenario \"{}\": import \"{alias}\" -> collection \"{name}\"",
                scenario.name
            )
        })?;
        collections.insert(alias.clone(), collection);
    }

    let deps = ExecDeps {
        storage,
        client,
        collections: &collections,
        env_name,
        var_overrides,
    };
    let result = match scenario.requests.kind {
        SequenceKind::Sequence => execute_sequence(&scenario.requests.items, &deps, None).await?,
        SequenceKind::Parallel => execute_parallel(&scenario.requests.items, &deps, None).await?,
    };
    Ok(ScenarioRun {
        outcome: result.outcome,
        steps: result.steps,
    })
}

fn execute_node<'a>(
    node: &'a ScenarioNode,
    deps: &'a ExecDeps<'a>,
    incoming: Option<ResponseSummary>,
) -> BoxFuture<'a, Result<NodeResult>> {
    Box::pin(async move {
        match node {
            ScenarioNode::Request { request } => execute_request(request, deps).await,
            ScenarioNode::Requests { requests } => match requests.kind {
                SequenceKind::Sequence => execute_sequence(&requests.items, deps, incoming).await,
                SequenceKind::Parallel => execute_parallel(&requests.items, deps, incoming).await,
            },
            ScenarioNode::Choice { choice } => execute_choice(choice, deps, incoming).await,
        }
    })
}

async fn execute_request(reference: &str, deps: &ExecDeps<'_>) -> Result<NodeResult> {
    let (alias, path) = resolve_request_ref(reference, deps.collections)?;
    let collection = &deps.collections[alias];
    let saved = collection.find_request(path).ok_or_else(|| {
        anyhow!(
            "request \"{reference}\": no request named \"{path}\" in collection \"{}\" (imported as \"{alias}\")",
            collection.name
        )
    })?;

    let variables = merged_variables(deps.storage, deps.env_name, deps.var_overrides)?;
    let resolved = substitution::resolve(saved, &variables).with_context(|| format!("request \"{reference}\""))?;
    let request = crate::storage::into_request(&resolved)?;

    let method = request.method.to_string();
    let url = request.url.clone();

    match exec::send(deps.client, &request).await {
        Err(e) => Ok(NodeResult {
            outcome: StepOutcome::Halted(format!("request \"{reference}\" failed to send: {e}")),
            response: None,
            steps: vec![StepLog {
                reference: reference.to_string(),
                method,
                url,
                status: None,
                error: Some(e.to_string()),
                extracted: Vec::new(),
            }],
        }),
        Ok(response) => {
            let mut extracted_names = Vec::new();
            if (200..300).contains(&response.status) && !saved.extract.is_empty() {
                let extracted = extraction::apply(&saved.extract, &response, &variables)
                    .with_context(|| format!("request \"{reference}\": extraction"))?;
                if !extracted.is_empty() {
                    let session_scope = deps.env_name.unwrap_or("global");
                    let mut session = deps.storage.load_session(session_scope)?;
                    for var in &extracted {
                        session.set(var.name.as_str(), var.value.as_str(), var.secret);
                        extracted_names.push(var.name.clone());
                    }
                    deps.storage.save_session(&session)?;
                }
            }

            let outcome = if (200..300).contains(&response.status) {
                StepOutcome::Ok
            } else {
                StepOutcome::HttpFailure
            };

            Ok(NodeResult {
                outcome,
                steps: vec![StepLog {
                    reference: reference.to_string(),
                    method,
                    url,
                    status: Some(response.status),
                    error: None,
                    extracted: extracted_names,
                }],
                response: Some(response),
            })
        }
    }
}

async fn execute_sequence<'a>(
    items: &'a [ScenarioNode],
    deps: &'a ExecDeps<'a>,
    incoming: Option<ResponseSummary>,
) -> Result<NodeResult> {
    let mut current = incoming;
    let mut all_steps = Vec::new();

    for (i, item) in items.iter().enumerate() {
        let mut result = execute_node(item, deps, current.clone()).await?;
        all_steps.append(&mut result.steps);

        match result.outcome {
            StepOutcome::Ok => {
                current = result.response;
            }
            StepOutcome::Halted(reason) => {
                return Ok(NodeResult {
                    outcome: StepOutcome::Halted(reason),
                    response: result.response,
                    steps: all_steps,
                });
            }
            StepOutcome::HttpFailure => {
                current = result.response;
                let next_is_choice = matches!(items.get(i + 1), Some(ScenarioNode::Choice { .. }));
                if next_is_choice {
                    continue;
                }
                let status = current.as_ref().map(|r| r.status).unwrap_or(0);
                let reason = match item {
                    ScenarioNode::Request { request } => format!(
                        "request \"{request}\" returned {status} and was not explicitly handled by a following choice"
                    ),
                    _ => format!("a step returned {status} and was not explicitly handled by a following choice"),
                };
                return Ok(NodeResult {
                    outcome: StepOutcome::Halted(reason),
                    response: current,
                    steps: all_steps,
                });
            }
        }
    }

    Ok(NodeResult {
        outcome: StepOutcome::Ok,
        response: current,
        steps: all_steps,
    })
}

async fn execute_parallel<'a>(
    items: &'a [ScenarioNode],
    deps: &'a ExecDeps<'a>,
    incoming: Option<ResponseSummary>,
) -> Result<NodeResult> {
    if items.is_empty() {
        return Ok(NodeResult {
            outcome: StepOutcome::Ok,
            response: incoming,
            steps: Vec::new(),
        });
    }

    let futures = items.iter().map(|item| execute_node(item, deps, incoming.clone()));
    let joined = futures::future::join_all(futures).await;

    let mut branch_results = Vec::with_capacity(joined.len());
    for r in joined {
        branch_results.push(r?);
    }

    let mut all_steps = Vec::new();
    for branch in &mut branch_results {
        all_steps.append(&mut branch.steps);
    }

    // First failure by declared (array) order — see ROADMAP.md's "Decided:
    // what 'the next node' sees when more than one branch fails" for why
    // this, and not completion-time order (nondeterministic) or named
    // per-branch targeting (a deferred future extension).
    let winner = branch_results
        .iter()
        .position(|b| !matches!(b.outcome, StepOutcome::Ok))
        .unwrap_or(0);

    Ok(NodeResult {
        outcome: branch_results[winner].outcome.clone(),
        response: branch_results[winner].response.clone(),
        steps: all_steps,
    })
}

async fn execute_choice<'a>(
    choice: &'a ChoiceNode,
    deps: &'a ExecDeps<'a>,
    incoming: Option<ResponseSummary>,
) -> Result<NodeResult> {
    let Some(response) = &incoming else {
        return Ok(NodeResult {
            outcome: StepOutcome::Halted("choice: no response available to evaluate (no preceding request)".to_string()),
            response: None,
            steps: Vec::new(),
        });
    };

    for case in &choice.cases {
        if evaluate_condition(&case.when, response.status, &response.headers, &response.body) {
            return execute_node(&case.then, deps, incoming.clone()).await;
        }
    }

    if let Some(default) = &choice.default {
        return execute_node(default, deps, incoming.clone()).await;
    }

    Ok(NodeResult {
        outcome: StepOutcome::Halted(format!(
            "choice: response status {} matched no case and no default was given",
            response.status
        )),
        response: Some(response.clone()),
        steps: Vec::new(),
    })
}

/// Mirrors `curly-cli`'s own `run.rs::merged_variables` precedence exactly
/// (global environment → selected environment → that environment's
/// session → `--var` overrides), minus the CLI-specific `RunArgs` plumbing.
/// Deliberately a small, separate copy rather than a shared extraction —
/// re-derived fresh before *every* request node (not once per scenario),
/// since an earlier step's extraction can write into the session mid-run
/// and a later step's `{{substitution}}` needs to see that.
fn merged_variables(storage: &Storage, env_name: Option<&str>, overrides: &[(String, String)]) -> Result<BTreeMap<String, String>> {
    let mut variables = BTreeMap::new();

    if let Some(global) = storage.load_environment_opt("global")? {
        for var in global.variables {
            variables.insert(var.key, var.value);
        }
    }
    if let Some(name) = env_name {
        let env = storage.load_environment(name)?;
        for var in env.variables {
            variables.insert(var.key, var.value);
        }
    }
    let session = storage.load_session(env_name.unwrap_or("global"))?;
    for var in session.variables {
        variables.insert(var.key, var.value);
    }
    for (key, value) in overrides {
        variables.insert(key.clone(), value.clone());
    }

    Ok(variables)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- evaluate_condition ---

    fn cmp_eq(v: impl Into<Value>) -> Comparison {
        Comparison::Eq { eq: v.into() }
    }

    #[test]
    fn status_eq_matches() {
        let c = Condition::Status { status: cmp_eq(200) };
        assert!(evaluate_condition(&c, 200, &[], ""));
        assert!(!evaluate_condition(&c, 404, &[], ""));
    }

    #[test]
    fn status_ne_gt_lt() {
        assert!(evaluate_condition(&Condition::Status { status: Comparison::Ne { ne: 200.into() } }, 404, &[], ""));
        assert!(evaluate_condition(&Condition::Status { status: Comparison::Gt { gt: 199.into() } }, 200, &[], ""));
        assert!(evaluate_condition(&Condition::Status { status: Comparison::Lt { lt: 500.into() } }, 200, &[], ""));
        assert!(!evaluate_condition(&Condition::Status { status: Comparison::Gt { gt: 200.into() } }, 200, &[], ""));
    }

    #[test]
    fn header_eq_is_case_insensitive_on_name() {
        let c = Condition::Header {
            header: NamedComparison { name: "x-foo".to_string(), op: cmp_eq("bar") },
        };
        assert!(evaluate_condition(&c, 200, &[("X-Foo".to_string(), "bar".to_string())], ""));
        assert!(!evaluate_condition(&c, 200, &[("X-Foo".to_string(), "baz".to_string())], ""));
    }

    #[test]
    fn header_exists_true_and_false() {
        let exists = Condition::Header {
            header: NamedComparison { name: "X-Foo".to_string(), op: Comparison::Exists { exists: true } },
        };
        let not_exists = Condition::Header {
            header: NamedComparison { name: "X-Foo".to_string(), op: Comparison::Exists { exists: false } },
        };
        let headers = [("X-Foo".to_string(), "bar".to_string())];
        assert!(evaluate_condition(&exists, 200, &headers, ""));
        assert!(!evaluate_condition(&not_exists, 200, &headers, ""));
        assert!(!evaluate_condition(&exists, 200, &[], ""));
        assert!(evaluate_condition(&not_exists, 200, &[], ""));
    }

    #[test]
    fn header_op_other_than_exists_is_false_when_header_absent() {
        let c = Condition::Header {
            header: NamedComparison { name: "X-Foo".to_string(), op: cmp_eq("bar") },
        };
        assert!(!evaluate_condition(&c, 200, &[], ""));
    }

    #[test]
    fn body_path_eq_true() {
        let c = Condition::Body {
            body: PathComparison { path: "mfaRequired".to_string(), op: cmp_eq(true) },
        };
        assert!(evaluate_condition(&c, 200, &[], r#"{"mfaRequired":true}"#));
        assert!(!evaluate_condition(&c, 200, &[], r#"{"mfaRequired":false}"#));
    }

    #[test]
    fn body_path_missing_is_false_for_eq_but_satisfies_exists_false() {
        let eq = Condition::Body {
            body: PathComparison { path: "nope".to_string(), op: cmp_eq(true) },
        };
        let not_exists = Condition::Body {
            body: PathComparison { path: "nope".to_string(), op: Comparison::Exists { exists: false } },
        };
        assert!(!evaluate_condition(&eq, 200, &[], r#"{"a":1}"#));
        assert!(evaluate_condition(&not_exists, 200, &[], r#"{"a":1}"#));
    }

    #[test]
    fn body_non_json_never_matches_anything_but_exists_false() {
        let eq = Condition::Body {
            body: PathComparison { path: "a".to_string(), op: cmp_eq(1) },
        };
        let not_exists = Condition::Body {
            body: PathComparison { path: "a".to_string(), op: Comparison::Exists { exists: false } },
        };
        assert!(!evaluate_condition(&eq, 200, &[], "not json"));
        assert!(evaluate_condition(&not_exists, 200, &[], "not json"));
    }

    #[test]
    fn body_contains_on_string_and_array() {
        let contains_str = Condition::Body {
            body: PathComparison {
                path: "name".to_string(),
                op: Comparison::Contains { contains: "curly".into() },
            },
        };
        assert!(evaluate_condition(&contains_str, 200, &[], r#"{"name":"curly-cli"}"#));
        assert!(!evaluate_condition(&contains_str, 200, &[], r#"{"name":"postman"}"#));

        let contains_item = Condition::Body {
            body: PathComparison {
                path: "roles".to_string(),
                op: Comparison::Contains { contains: "admin".into() },
            },
        };
        assert!(evaluate_condition(&contains_item, 200, &[], r#"{"roles":["user","admin"]}"#));
        assert!(!evaluate_condition(&contains_item, 200, &[], r#"{"roles":["user"]}"#));
    }

    #[test]
    fn all_any_not_composition() {
        let status_200 = Condition::Status { status: cmp_eq(200) };
        let status_404 = Condition::Status { status: cmp_eq(404) };

        let all = Condition::All { all: vec![status_200.clone(), status_404.clone()] };
        assert!(!evaluate_condition(&all, 200, &[], ""));

        let any = Condition::Any { any: vec![status_200.clone(), status_404.clone()] };
        assert!(evaluate_condition(&any, 200, &[], ""));

        let not = Condition::Not { not: Box::new(status_404) };
        assert!(evaluate_condition(&not, 200, &[], ""));
    }

    // --- resolve_request_ref ---

    fn imports(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn bare_path_resolves_against_default() {
        let imports = imports(&[("default", "health-record-api")]);
        let (alias, path) = resolve_request_ref("auth/register", &imports).unwrap();
        assert_eq!(alias, "default");
        assert_eq!(path, "auth/register");
    }

    #[test]
    fn explicit_alias_arrow_path_splits_once() {
        let imports = imports(&[("default", "health-record-api"), ("billing", "billing-api")]);
        let (alias, path) = resolve_request_ref("billing->accounts/create", &imports).unwrap();
        assert_eq!(alias, "billing");
        assert_eq!(path, "accounts/create");
    }

    #[test]
    fn bare_path_with_no_default_import_errors() {
        let imports = imports(&[("billing", "billing-api")]);
        let err = resolve_request_ref("auth/register", &imports).unwrap_err();
        assert!(err.to_string().contains("default"));
    }

    #[test]
    fn explicit_undefined_alias_errors() {
        let imports = imports(&[("default", "health-record-api")]);
        let err = resolve_request_ref("nope->auth/register", &imports).unwrap_err();
        assert!(err.to_string().contains("nope"));
    }

    #[test]
    fn path_can_itself_contain_folder_slashes_after_the_arrow() {
        let imports = imports(&[("default", "health-record-api")]);
        let (alias, path) = resolve_request_ref("default->Auth/OAuth/login", &imports).unwrap();
        assert_eq!(alias, "default");
        assert_eq!(path, "Auth/OAuth/login");
    }

    // --- JSON shape (deserializing the format decided in ROADMAP.md) ---

    #[test]
    fn deserializes_a_bare_request_node() {
        let node: ScenarioNode = serde_json::from_str(r#"{"request":"auth/register"}"#).unwrap();
        assert!(matches!(node, ScenarioNode::Request { request } if request == "auth/register"));
    }

    #[test]
    fn deserializes_a_sequence_node() {
        let node: ScenarioNode = serde_json::from_str(
            r#"{"requests":{"type":"sequence","items":[{"request":"a"},{"request":"b"}]}}"#,
        )
        .unwrap();
        match node {
            ScenarioNode::Requests { requests } => {
                assert_eq!(requests.kind, SequenceKind::Sequence);
                assert_eq!(requests.items.len(), 2);
            }
            other => panic!("expected Requests, got {other:?}"),
        }
    }

    #[test]
    fn deserializes_a_parallel_node() {
        let node: ScenarioNode =
            serde_json::from_str(r#"{"requests":{"type":"parallel","items":[{"request":"a"}]}}"#).unwrap();
        match node {
            ScenarioNode::Requests { requests } => assert_eq!(requests.kind, SequenceKind::Parallel),
            other => panic!("expected Requests, got {other:?}"),
        }
    }

    #[test]
    fn deserializes_the_full_worked_onboarding_example() {
        // The exact JSON from docs/ROADMAP.md's worked example -- kept in
        // sync deliberately, not just "close enough".
        let json = r#"
        {
          "name": "onboarding",
          "imports": { "default": "health-record-api" },
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
        "#;

        let scenario: Scenario = serde_json::from_str(json).unwrap();
        assert_eq!(scenario.name, "onboarding");
        assert_eq!(scenario.imports.get("default"), Some(&"health-record-api".to_string()));

        let requests = &scenario.requests;
        assert_eq!(requests.kind, SequenceKind::Sequence);
        assert_eq!(requests.items.len(), 2);
        assert!(matches!(&requests.items[0], ScenarioNode::Request { request } if request == "auth/register"));

        let ScenarioNode::Choice { choice } = &requests.items[1] else {
            panic!("expected the second item to be a choice");
        };
        assert_eq!(choice.cases.len(), 2);
        assert!(choice.default.is_none());
        assert!(matches!(
            &choice.cases[0].when,
            Condition::Status { status: Comparison::Eq { eq } } if *eq == Value::from(200)
        ));
    }

    #[test]
    fn round_trips_through_serialize_and_deserialize() {
        let scenario = Scenario::new(
            "smoke",
            RequestsNode {
                kind: SequenceKind::Parallel,
                items: vec![ScenarioNode::Request { request: "a".to_string() }],
            },
        );
        let json = serde_json::to_string(&scenario).unwrap();
        let back: Scenario = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, "smoke");
        assert_eq!(back.id, scenario.id);
    }
}
