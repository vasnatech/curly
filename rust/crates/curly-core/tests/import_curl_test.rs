use curly_core::import::curl;
use curly_core::storage::{SavedAuth, SavedBody, SavedMultipartField};
use reqwest::Method;

#[test]
fn imports_simple_get() {
    let req = curl::import_curl("get-example", "curl https://example.com/api/get").unwrap();
    assert_eq!(req.method, "GET");
    assert_eq!(req.url, "https://example.com/api/get");
}

#[test]
fn imports_get_without_leading_curl_word() {
    let req = curl::import_curl("get-example", "https://example.com/api/get").unwrap();
    assert_eq!(req.url, "https://example.com/api/get");
}

#[test]
fn imports_explicit_method() {
    let req = curl::import_curl("del", "curl -X DELETE https://example.com/api/x").unwrap();
    assert_eq!(req.method, "DELETE");
}

#[test]
fn data_flag_defaults_method_to_post() {
    let req = curl::import_curl("post", r#"curl https://example.com -d '{"a":1}'"#).unwrap();
    assert_eq!(req.method, "POST");
    match &req.body {
        Some(SavedBody::Raw { content }) => assert_eq!(content, r#"{"a":1}"#),
        other => panic!("expected Raw body, got {other:?}"),
    }
}

#[test]
fn imports_headers_and_quoted_json_body_browser_style() {
    // Mimics a real "Copy as cURL" export: single-quoted URL, -H repeated,
    // --data-raw with an escaped double-quoted JSON body.
    let command = r#"curl 'https://example.com/api/login' \
  -H 'Content-Type: application/json' \
  -H 'Accept: application/json' \
  --data-raw '{"email":"a@example.com","password":"hunter2"}'"#;
    let req = curl::import_curl("login", command).unwrap();
    assert_eq!(req.url, "https://example.com/api/login");
    assert_eq!(req.method, "POST");
    assert_eq!(req.headers.len(), 2);
    assert_eq!(req.headers[0].name, "Content-Type");
    assert_eq!(req.headers[1].name, "Accept");
    match &req.body {
        Some(SavedBody::Raw { content }) => {
            assert_eq!(content, r#"{"email":"a@example.com","password":"hunter2"}"#);
        }
        other => panic!("expected Raw body, got {other:?}"),
    }
}

#[test]
fn imports_basic_auth() {
    let req = curl::import_curl("x", "curl -u alice:s3cret https://example.com").unwrap();
    match &req.auth {
        Some(SavedAuth::Basic { username, password }) => {
            assert_eq!(username, "alice");
            assert_eq!(password, "s3cret");
        }
        other => panic!("expected Basic auth, got {other:?}"),
    }
}

#[test]
fn malformed_user_flag_errors() {
    assert!(curl::import_curl("x", "curl -u alice https://example.com").is_err());
}

#[test]
fn imports_multipart_form_fields() {
    let req = curl::import_curl(
        "upload",
        "curl -F name=alice -F avatar=@photo.png https://example.com/upload",
    )
    .unwrap();
    match &req.body {
        Some(SavedBody::Multipart { fields }) => {
            assert_eq!(fields.len(), 2);
            assert!(matches!(
                &fields[0],
                SavedMultipartField::Text { name, value } if name == "name" && value == "alice"
            ));
            assert!(matches!(
                &fields[1],
                SavedMultipartField::File { name, path }
                    if name == "avatar" && path.to_str() == Some("photo.png")
            ));
        }
        other => panic!("expected Multipart body, got {other:?}"),
    }
}

#[test]
fn data_binary_with_at_prefix_is_a_file_reference() {
    let req = curl::import_curl("x", "curl --data-binary @payload.bin https://example.com").unwrap();
    match &req.body {
        Some(SavedBody::Binary { path }) => assert_eq!(path.to_str(), Some("payload.bin")),
        other => panic!("expected Binary body, got {other:?}"),
    }
}

#[test]
fn conflicting_body_sources_error() {
    let result = curl::import_curl("x", "curl -d 'a' -F 'b=c' https://example.com");
    assert!(result.is_err());
}

#[test]
fn explicit_url_flag_is_honored() {
    let req = curl::import_curl("x", "curl -X GET --url https://example.com/api -H 'Accept: */*'").unwrap();
    assert_eq!(req.url, "https://example.com/api");
}

#[test]
fn unknown_flag_with_value_does_not_get_mistaken_for_url() {
    let req = curl::import_curl(
        "x",
        "curl --connect-timeout 5 https://example.com/real-url",
    )
    .unwrap();
    assert_eq!(req.url, "https://example.com/real-url");
}

#[test]
fn no_url_errors() {
    assert!(curl::import_curl("x", "curl -X GET").is_err());
}

#[test]
fn double_quoted_body_with_escaped_quotes() {
    let req = curl::import_curl(
        "x",
        r#"curl https://example.com -H "Content-Type: application/json" -d "{\"a\":\"b\"}""#,
    )
    .unwrap();
    match &req.body {
        Some(SavedBody::Raw { content }) => assert_eq!(content, r#"{"a":"b"}"#),
        other => panic!("expected Raw body, got {other:?}"),
    }
}

#[test]
fn method_is_case_insensitive() {
    let req = curl::import_curl("x", "curl -X post https://example.com").unwrap();
    assert_eq!(req.method, Method::POST.as_str());
}
