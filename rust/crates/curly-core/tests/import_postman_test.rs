use curly_core::import::postman;
use curly_core::storage::{SavedAuth, SavedBody, SavedMultipartField};

const SAMPLE_COLLECTION: &str = r#"
{
  "info": {
    "_postman_id": "abc-123",
    "name": "My API",
    "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"
  },
  "item": [
    {
      "name": "login",
      "request": {
        "method": "POST",
        "header": [
          { "key": "Content-Type", "value": "application/json" },
          { "key": "X-Disabled", "value": "nope", "disabled": true }
        ],
        "body": {
          "mode": "raw",
          "raw": "{\"email\":\"{{EMAIL}}\",\"password\":\"{{PASSWORD}}\"}",
          "options": { "raw": { "language": "json" } }
        },
        "url": {
          "raw": "{{BASE_URL}}/api/auth/login",
          "host": ["{{BASE_URL}}"],
          "path": ["api", "auth", "login"]
        }
      },
      "response": []
    },
    {
      "name": "Auth",
      "item": [
        {
          "name": "get-me",
          "request": {
            "method": "GET",
            "url": "{{BASE_URL}}/api/patients/me",
            "auth": {
              "type": "bearer",
              "bearer": [{ "key": "token", "value": "{{TOKEN}}", "type": "string" }]
            }
          }
        },
        {
          "name": "OAuth",
          "item": [
            {
              "name": "basic-auth-example",
              "request": {
                "method": "GET",
                "url": "{{BASE_URL}}/api/secure",
                "auth": {
                  "type": "basic",
                  "basic": [
                    { "key": "username", "value": "alice", "type": "string" },
                    { "key": "password", "value": "s3cret", "type": "string" }
                  ]
                }
              }
            }
          ]
        }
      ]
    },
    {
      "name": "create-form",
      "request": {
        "method": "POST",
        "url": "{{BASE_URL}}/api/upload",
        "body": {
          "mode": "urlencoded",
          "urlencoded": [
            { "key": "name", "value": "alice" },
            { "key": "skip", "value": "me", "disabled": true }
          ]
        }
      }
    },
    {
      "name": "multipart-upload",
      "request": {
        "method": "POST",
        "url": "{{BASE_URL}}/api/files",
        "body": {
          "mode": "formdata",
          "formdata": [
            { "key": "caption", "value": "hello", "type": "text" },
            { "key": "file", "src": "/tmp/photo.png", "type": "file" }
          ]
        }
      }
    }
  ]
}
"#;

#[test]
fn imports_collection_name() {
    let collection = postman::import_collection(SAMPLE_COLLECTION).unwrap();
    assert_eq!(collection.name, "My API");
}

#[test]
fn imports_top_level_request() {
    let collection = postman::import_collection(SAMPLE_COLLECTION).unwrap();
    let login = collection.find_request("login").unwrap();
    assert_eq!(login.method, "POST");
    assert_eq!(login.url, "{{BASE_URL}}/api/auth/login");
    match &login.body {
        Some(SavedBody::Raw { content }) => {
            assert_eq!(content, r#"{"email":"{{EMAIL}}","password":"{{PASSWORD}}"}"#);
        }
        other => panic!("expected Raw body, got {other:?}"),
    }
}

#[test]
fn disabled_headers_are_dropped() {
    let collection = postman::import_collection(SAMPLE_COLLECTION).unwrap();
    let login = collection.find_request("login").unwrap();
    assert_eq!(login.headers.len(), 1);
    assert_eq!(login.headers[0].name, "Content-Type");
}

#[test]
fn imports_nested_folders() {
    let collection = postman::import_collection(SAMPLE_COLLECTION).unwrap();
    let get_me = collection.find_request("Auth/get-me").unwrap();
    assert_eq!(get_me.method, "GET");
    match &get_me.auth {
        Some(SavedAuth::Bearer { token }) => assert_eq!(token, "{{TOKEN}}"),
        other => panic!("expected Bearer auth, got {other:?}"),
    }
}

#[test]
fn imports_doubly_nested_folders_and_basic_auth() {
    let collection = postman::import_collection(SAMPLE_COLLECTION).unwrap();
    let req = collection.find_request("Auth/OAuth/basic-auth-example").unwrap();
    match &req.auth {
        Some(SavedAuth::Basic { username, password }) => {
            assert_eq!(username, "alice");
            assert_eq!(password, "s3cret");
        }
        other => panic!("expected Basic auth, got {other:?}"),
    }
}

#[test]
fn imports_urlencoded_form_body_and_drops_disabled_fields() {
    let collection = postman::import_collection(SAMPLE_COLLECTION).unwrap();
    let req = collection.find_request("create-form").unwrap();
    match &req.body {
        Some(SavedBody::Form { fields }) => {
            assert_eq!(fields, &vec![("name".to_string(), "alice".to_string())]);
        }
        other => panic!("expected Form body, got {other:?}"),
    }
}

#[test]
fn imports_multipart_form_with_text_and_file_fields() {
    let collection = postman::import_collection(SAMPLE_COLLECTION).unwrap();
    let req = collection.find_request("multipart-upload").unwrap();
    match &req.body {
        Some(SavedBody::Multipart { fields }) => {
            assert_eq!(fields.len(), 2);
            assert!(matches!(
                &fields[0],
                SavedMultipartField::Text { name, value }
                    if name == "caption" && value == "hello"
            ));
            assert!(matches!(
                &fields[1],
                SavedMultipartField::File { name, path }
                    if name == "file" && path.to_str() == Some("/tmp/photo.png")
            ));
        }
        other => panic!("expected Multipart body, got {other:?}"),
    }
}

#[test]
fn rejects_invalid_json() {
    assert!(postman::import_collection("not json").is_err());
}

#[test]
fn get_request_with_no_method_defaults_missing_field_gracefully() {
    // Postman always includes "method", but be defensive: absence should
    // default to GET rather than erroring.
    let json = r#"{
        "info": { "name": "X" },
        "item": [
            { "name": "r", "request": { "url": "https://example.com" } }
        ]
    }"#;
    let collection = postman::import_collection(json).unwrap();
    assert_eq!(collection.find_request("r").unwrap().method, "GET");
}

const SAMPLE_ENVIRONMENT: &str = r#"
{
  "id": "env-1",
  "name": "Dev",
  "values": [
    { "key": "BASE_URL", "value": "http://localhost:8080", "type": "default", "enabled": true },
    { "key": "TOKEN", "value": "secret-token", "type": "secret", "enabled": true },
    { "key": "SKIPPED", "value": "should not appear", "type": "default", "enabled": false }
  ],
  "_postman_variable_scope": "environment"
}
"#;

const SAMPLE_GLOBALS: &str = r#"
{
  "id": "glob-1",
  "name": "Globals",
  "values": [
    { "key": "API_VERSION", "value": "v2", "type": "default", "enabled": true }
  ],
  "_postman_variable_scope": "globals"
}
"#;

#[test]
fn imports_environment_with_name_and_secret_flag() {
    let env = postman::import_environment(SAMPLE_ENVIRONMENT, None).unwrap();
    assert_eq!(env.name, "Dev");
    assert_eq!(env.variables.len(), 2); // SKIPPED (disabled) dropped

    let base_url = env.variables.iter().find(|v| v.key == "BASE_URL").unwrap();
    assert_eq!(base_url.value, "http://localhost:8080");
    assert!(!base_url.secret);

    let token = env.variables.iter().find(|v| v.key == "TOKEN").unwrap();
    assert!(token.secret);
}

#[test]
fn environment_name_override_wins() {
    let env = postman::import_environment(SAMPLE_ENVIRONMENT, Some("local")).unwrap();
    assert_eq!(env.name, "local");
}

#[test]
fn globals_map_to_curly_global_environment_name() {
    let env = postman::import_environment(SAMPLE_GLOBALS, None).unwrap();
    assert_eq!(env.name, "global");
    assert_eq!(env.variables[0].key, "API_VERSION");
}

#[test]
fn rejects_invalid_environment_json() {
    assert!(postman::import_environment("not json", None).is_err());
}
