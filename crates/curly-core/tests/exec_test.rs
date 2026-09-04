use curly_core::exec;
use curly_core::model::{Auth, Body, Request};
use reqwest::Method;
use wiremock::matchers::{body_string, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn get_request_returns_status_and_body() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/hello"))
        .respond_with(ResponseTemplate::new(200).set_body_string("world"))
        .mount(&server)
        .await;

    let client = reqwest::Client::new();
    let request = Request::new(Method::GET, format!("{}/hello", server.uri()));
    let response = exec::send(&client, &request).await.unwrap();

    assert_eq!(response.status, 200);
    assert_eq!(response.body, "world");
}

#[tokio::test]
async fn sends_custom_headers() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(header("X-Test", "abc"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let client = reqwest::Client::new();
    let request = Request::new(Method::GET, server.uri()).with_header("X-Test", "abc");
    let response = exec::send(&client, &request).await.unwrap();

    assert_eq!(response.status, 200);
}

#[tokio::test]
async fn sends_query_params() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(query_param("id", "42"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let client = reqwest::Client::new();
    let request = Request::new(Method::GET, server.uri()).with_query("id", "42");
    let response = exec::send(&client, &request).await.unwrap();

    assert_eq!(response.status, 200);
}

#[tokio::test]
async fn sends_raw_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_string("hello"))
        .respond_with(ResponseTemplate::new(201))
        .mount(&server)
        .await;

    let client = reqwest::Client::new();
    let request =
        Request::new(Method::POST, server.uri()).with_body(Body::Raw("hello".to_string()));
    let response = exec::send(&client, &request).await.unwrap();

    assert_eq!(response.status, 201);
}

#[tokio::test]
async fn sends_form_urlencoded_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(header("Content-Type", "application/x-www-form-urlencoded"))
        .and(body_string("name=alice&id=1"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let client = reqwest::Client::new();
    let request = Request::new(Method::POST, server.uri()).with_body(Body::Form(vec![
        ("name".to_string(), "alice".to_string()),
        ("id".to_string(), "1".to_string()),
    ]));
    let response = exec::send(&client, &request).await.unwrap();

    assert_eq!(response.status, 200);
}

#[tokio::test]
async fn sends_basic_auth() {
    let server = MockServer::start().await;
    // base64("alice:s3cret") = YWxpY2U6czNjcmV0
    Mock::given(method("GET"))
        .and(header("Authorization", "Basic YWxpY2U6czNjcmV0"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let client = reqwest::Client::new();
    let request = Request::new(Method::GET, server.uri()).with_auth(Auth::Basic {
        username: "alice".to_string(),
        password: "s3cret".to_string(),
    });
    let response = exec::send(&client, &request).await.unwrap();

    assert_eq!(response.status, 200);
}

#[tokio::test]
async fn sends_bearer_auth() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(header("Authorization", "Bearer tok123"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let client = reqwest::Client::new();
    let request = Request::new(Method::GET, server.uri()).with_auth(Auth::Bearer {
        token: "tok123".to_string(),
    });
    let response = exec::send(&client, &request).await.unwrap();

    assert_eq!(response.status, 200);
}

#[tokio::test]
async fn returns_response_headers() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).insert_header("X-Reply", "yes"))
        .mount(&server)
        .await;

    let client = reqwest::Client::new();
    let request = Request::new(Method::GET, server.uri());
    let response = exec::send(&client, &request).await.unwrap();

    assert!(response
        .headers
        .iter()
        .any(|(name, value)| name.eq_ignore_ascii_case("X-Reply") && value == "yes"));
}
