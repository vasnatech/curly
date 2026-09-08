//! Integration tests for `curly_core::scenario`'s execution engine — the
//! parts that need a real (mocked) HTTP server and real (tempdir) storage,
//! as opposed to `scenario.rs`'s own inline unit tests for pure logic
//! (condition evaluation, reference resolution, JSON shape).

use curly_core::scenario::{
    ChoiceCase, ChoiceNode, Comparison, Condition, RequestsNode, Scenario, ScenarioNode, SequenceKind, StepOutcome,
};
use curly_core::extraction::Extraction;
use curly_core::storage::{Collection, KvPair, SavedRequest, Storage};
use reqwest::Method;
use tempfile::tempdir;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn scenario_with(imports: &[(&str, &str)], requests: RequestsNode) -> Scenario {
    let mut scenario = Scenario::new("test", requests);
    for (alias, collection) in imports {
        scenario.imports.insert(alias.to_string(), collection.to_string());
    }
    scenario
}

fn seq(items: Vec<ScenarioNode>) -> RequestsNode {
    RequestsNode { kind: SequenceKind::Sequence, items }
}

fn parallel(items: Vec<ScenarioNode>) -> RequestsNode {
    RequestsNode { kind: SequenceKind::Parallel, items }
}

fn request_node(path: &str) -> ScenarioNode {
    ScenarioNode::Request { request: path.to_string() }
}

fn status_eq(code: i64) -> Condition {
    Condition::Status { status: Comparison::Eq { eq: code.into() } }
}

#[tokio::test]
async fn sequence_runs_steps_in_order() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/step1")).respond_with(ResponseTemplate::new(200)).mount(&server).await;
    Mock::given(method("GET")).and(path("/step2")).respond_with(ResponseTemplate::new(200)).mount(&server).await;

    let dir = tempdir().unwrap();
    let storage = Storage::new(dir.path());
    let mut collection = Collection::new("api");
    collection
        .add_request("step1", SavedRequest::new("step1", Method::GET, format!("{}/step1", server.uri())))
        .unwrap();
    collection
        .add_request("step2", SavedRequest::new("step2", Method::GET, format!("{}/step2", server.uri())))
        .unwrap();
    storage.save_collection(&collection).unwrap();

    let scenario = scenario_with(&[("default", "api")], seq(vec![request_node("step1"), request_node("step2")]));

    let client = reqwest::Client::new();
    let run = curly_core::scenario::run(&scenario, &storage, &client, None, &[]).await.unwrap();

    assert_eq!(run.outcome, StepOutcome::Ok);
    assert_eq!(run.steps.len(), 2);
    assert_eq!(run.steps[0].reference, "step1");
    assert_eq!(run.steps[1].reference, "step2");
}

#[tokio::test]
async fn request_failure_halts_when_nothing_handles_it() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/broken")).respond_with(ResponseTemplate::new(500)).mount(&server).await;

    let dir = tempdir().unwrap();
    let storage = Storage::new(dir.path());
    let mut collection = Collection::new("api");
    collection
        .add_request("broken", SavedRequest::new("broken", Method::GET, format!("{}/broken", server.uri())))
        .unwrap();
    storage.save_collection(&collection).unwrap();

    let scenario = scenario_with(&[("default", "api")], seq(vec![request_node("broken")]));
    let client = reqwest::Client::new();
    let run = curly_core::scenario::run(&scenario, &storage, &client, None, &[]).await.unwrap();

    assert!(matches!(run.outcome, StepOutcome::Halted(_)));
}

#[tokio::test]
async fn request_failure_is_forgiven_by_a_following_choice_that_handles_it() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/register")).respond_with(ResponseTemplate::new(400)).mount(&server).await;
    Mock::given(method("GET")).and(path("/login")).respond_with(ResponseTemplate::new(200)).mount(&server).await;

    let dir = tempdir().unwrap();
    let storage = Storage::new(dir.path());
    let mut collection = Collection::new("api");
    collection
        .add_request("register", SavedRequest::new("register", Method::GET, format!("{}/register", server.uri())))
        .unwrap();
    collection
        .add_request("login", SavedRequest::new("login", Method::GET, format!("{}/login", server.uri())))
        .unwrap();
    storage.save_collection(&collection).unwrap();

    let scenario = scenario_with(
        &[("default", "api")],
        seq(vec![
            request_node("register"),
            ScenarioNode::Choice {
                choice: ChoiceNode {
                    cases: vec![ChoiceCase { when: status_eq(400), then: Box::new(request_node("login")) }],
                    default: None,
                },
            },
        ]),
    );

    let client = reqwest::Client::new();
    let run = curly_core::scenario::run(&scenario, &storage, &client, None, &[]).await.unwrap();

    assert_eq!(run.outcome, StepOutcome::Ok);
    assert_eq!(run.steps.iter().map(|s| s.reference.as_str()).collect::<Vec<_>>(), vec!["register", "login"]);
}

#[tokio::test]
async fn choice_with_no_matching_case_and_no_default_halts() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/register")).respond_with(ResponseTemplate::new(500)).mount(&server).await;

    let dir = tempdir().unwrap();
    let storage = Storage::new(dir.path());
    let mut collection = Collection::new("api");
    collection
        .add_request("register", SavedRequest::new("register", Method::GET, format!("{}/register", server.uri())))
        .unwrap();
    storage.save_collection(&collection).unwrap();

    let scenario = scenario_with(
        &[("default", "api")],
        seq(vec![
            request_node("register"),
            ScenarioNode::Choice {
                choice: ChoiceNode {
                    cases: vec![
                        ChoiceCase { when: status_eq(200), then: Box::new(request_node("register")) },
                        ChoiceCase { when: status_eq(400), then: Box::new(request_node("register")) },
                    ],
                    default: None,
                },
            },
        ]),
    );

    let client = reqwest::Client::new();
    let run = curly_core::scenario::run(&scenario, &storage, &client, None, &[]).await.unwrap();

    match run.outcome {
        StepOutcome::Halted(reason) => assert!(reason.contains("500"), "reason was: {reason}"),
        other => panic!("expected Halted, got {other:?}"),
    }
}

#[tokio::test]
async fn parallel_picks_the_first_failure_by_declared_order_not_completion_order() {
    let server = MockServer::start().await;
    // Branch 0 ("slow-fail") is deliberately slower than branch 1
    // ("fast-ok"), so branch 1 finishes first in real time -- the decided
    // behavior is still "first by array position," so branch 0's failure
    // must win regardless.
    Mock::given(method("GET"))
        .and(path("/slow-fail"))
        .respond_with(ResponseTemplate::new(500).set_delay(std::time::Duration::from_millis(150)))
        .mount(&server)
        .await;
    Mock::given(method("GET")).and(path("/fast-ok")).respond_with(ResponseTemplate::new(200)).mount(&server).await;

    let dir = tempdir().unwrap();
    let storage = Storage::new(dir.path());
    let mut collection = Collection::new("api");
    collection
        .add_request("slow-fail", SavedRequest::new("slow-fail", Method::GET, format!("{}/slow-fail", server.uri())))
        .unwrap();
    collection
        .add_request("fast-ok", SavedRequest::new("fast-ok", Method::GET, format!("{}/fast-ok", server.uri())))
        .unwrap();
    storage.save_collection(&collection).unwrap();

    let scenario = scenario_with(
        &[("default", "api")],
        seq(vec![ScenarioNode::Requests {
            requests: parallel(vec![request_node("slow-fail"), request_node("fast-ok")]),
        }]),
    );

    let client = reqwest::Client::new();
    let run = curly_core::scenario::run(&scenario, &storage, &client, None, &[]).await.unwrap();

    // Nothing follows the parallel block in this sequence, so its
    // (unforgiven) HttpFailure becomes a Halted at the top level -- what
    // matters for *this* test is which branch's status shows up in the
    // reason: 500 (branch 0, "slow-fail") should win over 200 (branch 1,
    // "fast-ok", which finishes first in real time but is declared second).
    match run.outcome {
        StepOutcome::Halted(reason) => assert!(reason.contains("500"), "reason was: {reason}"),
        other => panic!("expected Halted, got {other:?}"),
    }
    // Both branches still ran to completion (parallel doesn't cancel
    // siblings on failure) -- just the *outcome* reflects branch 0.
    let refs: Vec<&str> = run.steps.iter().map(|s| s.reference.as_str()).collect();
    assert!(refs.contains(&"slow-fail"));
    assert!(refs.contains(&"fast-ok"));
}

#[tokio::test]
async fn a_parallel_blocks_failure_is_forgiven_by_a_following_choice_same_as_a_plain_requests() {
    // ROADMAP.md's failure-semantics decision is explicit that a parallel
    // block's failure is handled "the same way" a plain sequential
    // failure is -- not a special case. This proves a following choice can
    // catch it, exactly like the plain-request case above.
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/a")).respond_with(ResponseTemplate::new(500)).mount(&server).await;
    Mock::given(method("GET")).and(path("/b")).respond_with(ResponseTemplate::new(200)).mount(&server).await;
    Mock::given(method("GET")).and(path("/recover")).respond_with(ResponseTemplate::new(200)).mount(&server).await;

    let dir = tempdir().unwrap();
    let storage = Storage::new(dir.path());
    let mut collection = Collection::new("api");
    collection.add_request("a", SavedRequest::new("a", Method::GET, format!("{}/a", server.uri()))).unwrap();
    collection.add_request("b", SavedRequest::new("b", Method::GET, format!("{}/b", server.uri()))).unwrap();
    collection
        .add_request("recover", SavedRequest::new("recover", Method::GET, format!("{}/recover", server.uri())))
        .unwrap();
    storage.save_collection(&collection).unwrap();

    let scenario = scenario_with(
        &[("default", "api")],
        seq(vec![
            ScenarioNode::Requests { requests: parallel(vec![request_node("a"), request_node("b")]) },
            ScenarioNode::Choice {
                choice: ChoiceNode {
                    cases: vec![ChoiceCase { when: status_eq(500), then: Box::new(request_node("recover")) }],
                    default: None,
                },
            },
        ]),
    );

    let client = reqwest::Client::new();
    let run = curly_core::scenario::run(&scenario, &storage, &client, None, &[]).await.unwrap();

    assert_eq!(run.outcome, StepOutcome::Ok);
    let refs: Vec<&str> = run.steps.iter().map(|s| s.reference.as_str()).collect();
    assert!(refs.contains(&"recover"));
}

#[tokio::test]
async fn parallel_does_not_cancel_siblings_when_one_branch_fails() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/a")).respond_with(ResponseTemplate::new(500)).mount(&server).await;
    Mock::given(method("GET")).and(path("/b")).respond_with(ResponseTemplate::new(200)).mount(&server).await;

    let dir = tempdir().unwrap();
    let storage = Storage::new(dir.path());
    let mut collection = Collection::new("api");
    collection.add_request("a", SavedRequest::new("a", Method::GET, format!("{}/a", server.uri()))).unwrap();
    collection.add_request("b", SavedRequest::new("b", Method::GET, format!("{}/b", server.uri()))).unwrap();
    storage.save_collection(&collection).unwrap();

    let scenario = scenario_with(
        &[("default", "api")],
        seq(vec![ScenarioNode::Requests { requests: parallel(vec![request_node("a"), request_node("b")]) }]),
    );

    let client = reqwest::Client::new();
    let run = curly_core::scenario::run(&scenario, &storage, &client, None, &[]).await.unwrap();

    // Both requests were actually sent -- wiremock's mounted mocks would
    // simply not have been hit for "b" if it had been cancelled, and the
    // step log below confirms both appear.
    assert_eq!(run.steps.len(), 2);
}

#[tokio::test]
async fn network_error_halts_even_with_a_following_choice() {
    let dir = tempdir().unwrap();
    let storage = Storage::new(dir.path());
    let mut collection = Collection::new("api");
    // Port 1 is a real "connection refused" on essentially any machine --
    // nothing ever listens there.
    collection
        .add_request("unreachable", SavedRequest::new("unreachable", Method::GET, "http://127.0.0.1:1/x"))
        .unwrap();
    collection
        .add_request("next", SavedRequest::new("next", Method::GET, "http://127.0.0.1:1/y"))
        .unwrap();
    storage.save_collection(&collection).unwrap();

    let scenario = scenario_with(
        &[("default", "api")],
        seq(vec![
            request_node("unreachable"),
            ScenarioNode::Choice {
                choice: ChoiceNode {
                    cases: vec![ChoiceCase { when: status_eq(500), then: Box::new(request_node("next")) }],
                    default: Some(Box::new(request_node("next"))),
                },
            },
        ]),
    );

    let client = reqwest::Client::new();
    let run = curly_core::scenario::run(&scenario, &storage, &client, None, &[]).await.unwrap();

    match run.outcome {
        StepOutcome::Halted(reason) => assert!(reason.contains("failed to send"), "reason was: {reason}"),
        other => panic!("expected Halted, got {other:?}"),
    }
    // Only the one (failed) request was ever attempted -- a network error
    // isn't "explicitly handled" by a following choice the way a real HTTP
    // response is, so the choice (and its default) never ran.
    assert_eq!(run.steps.len(), 1);
}

#[tokio::test]
async fn imports_route_requests_to_the_right_collection() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/from-default")).respond_with(ResponseTemplate::new(200)).mount(&server).await;
    Mock::given(method("GET")).and(path("/from-billing")).respond_with(ResponseTemplate::new(200)).mount(&server).await;

    let dir = tempdir().unwrap();
    let storage = Storage::new(dir.path());

    let mut api = Collection::new("health-record-api");
    api.add_request("ping", SavedRequest::new("ping", Method::GET, format!("{}/from-default", server.uri())))
        .unwrap();
    storage.save_collection(&api).unwrap();

    let mut billing = Collection::new("billing-api");
    billing
        .add_request("accounts/create", SavedRequest::new("create", Method::GET, format!("{}/from-billing", server.uri())))
        .unwrap();
    storage.save_collection(&billing).unwrap();

    let scenario = scenario_with(
        &[("default", "health-record-api"), ("billing", "billing-api")],
        seq(vec![request_node("ping"), request_node("billing->accounts/create")]),
    );

    let client = reqwest::Client::new();
    let run = curly_core::scenario::run(&scenario, &storage, &client, None, &[]).await.unwrap();

    assert_eq!(run.outcome, StepOutcome::Ok);
    assert_eq!(run.steps.len(), 2);
}

#[tokio::test]
async fn extraction_from_one_step_flows_into_substitution_in_the_next() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/login"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"token": "abc123"})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/me"))
        .and(header("Authorization", "Bearer abc123"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let dir = tempdir().unwrap();
    let storage = Storage::new(dir.path());
    let mut collection = Collection::new("api");

    let mut login = SavedRequest::new("login", Method::GET, format!("{}/login", server.uri()));
    login.extract = vec![Extraction::Body { name: "TOKEN".to_string(), path: "token".to_string(), secret: false }];
    collection.add_request("login", login).unwrap();

    let mut me = SavedRequest::new("me", Method::GET, format!("{}/me", server.uri()));
    me.headers.push(KvPair::new("Authorization", "Bearer {{TOKEN}}"));
    collection.add_request("me", me).unwrap();

    storage.save_collection(&collection).unwrap();

    let scenario = scenario_with(&[("default", "api")], seq(vec![request_node("login"), request_node("me")]));

    let client = reqwest::Client::new();
    let run = curly_core::scenario::run(&scenario, &storage, &client, None, &[]).await.unwrap();

    assert_eq!(run.outcome, StepOutcome::Ok);
    assert_eq!(run.steps[0].extracted, vec!["TOKEN".to_string()]);
}
