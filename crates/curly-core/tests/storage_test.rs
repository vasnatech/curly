use std::time::Duration;

use curly_core::exec::ResponseSummary;
use curly_core::model::Request;
use curly_core::storage::{slugify, Collection, Environment, HistoryEntry, SavedRequest, Storage};
use reqwest::Method;
use tempfile::tempdir;

#[test]
fn slugify_lowercases_and_collapses_separators() {
    assert_eq!(slugify("My API Collection"), "my-api-collection");
    assert_eq!(slugify("  spaced  "), "spaced");
    assert_eq!(slugify("already-slug_ok"), "already-slug_ok");
    assert_eq!(slugify("!!!"), "untitled");
}

#[test]
fn collection_round_trips_through_disk() {
    let dir = tempdir().unwrap();
    let storage = Storage::new(dir.path());

    let mut collection = Collection::new("My API");
    collection
        .requests
        .push(SavedRequest::new("get-user", Method::GET, "https://example.com/users/1"));

    storage.save_collection(&collection).unwrap();
    let loaded = storage.load_collection("My API").unwrap();

    assert_eq!(loaded.name, "My API");
    assert_eq!(loaded.requests.len(), 1);
    assert_eq!(loaded.requests[0].name, "get-user");
    assert_eq!(loaded.requests[0].method, "GET");
}

#[test]
fn collection_lookup_is_case_and_spacing_insensitive_via_slug() {
    let dir = tempdir().unwrap();
    let storage = Storage::new(dir.path());
    storage.save_collection(&Collection::new("My API")).unwrap();

    // Same slug ("my-api") regardless of how the name is typed back.
    assert!(storage.load_collection("my api").is_ok());
    assert!(storage.load_collection("My API").is_ok());
}

#[test]
fn list_collections_returns_sorted_slugs() {
    let dir = tempdir().unwrap();
    let storage = Storage::new(dir.path());
    storage.save_collection(&Collection::new("Zebra")).unwrap();
    storage.save_collection(&Collection::new("Alpha")).unwrap();

    assert_eq!(storage.list_collections().unwrap(), vec!["alpha", "zebra"]);
}

#[test]
fn list_collections_on_missing_directory_is_empty_not_an_error() {
    let dir = tempdir().unwrap();
    let storage = Storage::new(dir.path());
    assert_eq!(storage.list_collections().unwrap(), Vec::<String>::new());
}

#[test]
fn load_missing_collection_errors() {
    let dir = tempdir().unwrap();
    let storage = Storage::new(dir.path());
    assert!(storage.load_collection("nope").is_err());
}

#[test]
fn delete_collection_removes_it() {
    let dir = tempdir().unwrap();
    let storage = Storage::new(dir.path());
    storage.save_collection(&Collection::new("Temp")).unwrap();
    assert!(storage.load_collection("Temp").is_ok());

    storage.delete_collection("Temp").unwrap();
    assert!(storage.load_collection("Temp").is_err());
}

#[test]
fn environment_round_trips_and_set_overwrites_existing_key() {
    let dir = tempdir().unwrap();
    let storage = Storage::new(dir.path());

    let mut env = Environment::new("dev");
    env.set("HOST", "dev.example.com", false);
    env.set("TOKEN", "secret", true);
    env.set("HOST", "dev2.example.com", false); // overwrite

    storage.save_environment(&env).unwrap();
    let loaded = storage.load_environment("dev").unwrap();

    assert_eq!(loaded.variables.len(), 2);
    let host = loaded.variables.iter().find(|v| v.key == "HOST").unwrap();
    assert_eq!(host.value, "dev2.example.com");
    let token = loaded.variables.iter().find(|v| v.key == "TOKEN").unwrap();
    assert!(token.secret);
}

#[test]
fn environment_unset_removes_key() {
    let mut env = Environment::new("dev");
    env.set("HOST", "example.com", false);
    assert!(env.unset("HOST"));
    assert!(!env.unset("HOST")); // already gone
    assert!(env.variables.is_empty());
}

#[test]
fn load_environment_opt_returns_none_when_missing() {
    let dir = tempdir().unwrap();
    let storage = Storage::new(dir.path());
    assert!(storage.load_environment_opt("global").unwrap().is_none());
}

#[test]
fn history_entries_are_appended_and_listed_most_recent_first() {
    let dir = tempdir().unwrap();
    let storage = Storage::new(dir.path());

    let request = Request::new(Method::GET, "https://example.com/a");
    let response_a = ResponseSummary {
        status: 200,
        headers: vec![],
        body: "a".to_string(),
        elapsed: Duration::from_millis(1),
    };
    let response_b = ResponseSummary {
        status: 201,
        headers: vec![],
        body: "b".to_string(),
        elapsed: Duration::from_millis(2),
    };

    storage.append_history(&HistoryEntry::new(&request, &response_a)).unwrap();
    storage.append_history(&HistoryEntry::new(&request, &response_b)).unwrap();

    let entries = storage.list_history(10).unwrap();
    assert_eq!(entries.len(), 2);
    // Most recently appended (status 201) should come first.
    assert_eq!(entries[0].status, 201);
    assert_eq!(entries[1].status, 200);
}

#[test]
fn history_entry_redacts_authorization_header() {
    let request = Request::new(Method::GET, "https://example.com")
        .with_header("Authorization", "Bearer super-secret")
        .with_header("Accept", "application/json");
    let response = ResponseSummary {
        status: 200,
        headers: vec![],
        body: String::new(),
        elapsed: Duration::from_millis(1),
    };

    let entry = HistoryEntry::new(&request, &response);
    let auth_header = entry
        .request_headers
        .iter()
        .find(|(name, _)| name == "Authorization")
        .unwrap();
    assert_eq!(auth_header.1, "***redacted***");
    let accept_header = entry
        .request_headers
        .iter()
        .find(|(name, _)| name == "Accept")
        .unwrap();
    assert_eq!(accept_header.1, "application/json");
}

#[test]
fn history_entry_truncates_large_bodies() {
    let request = Request::new(Method::GET, "https://example.com");
    let big_body = "x".repeat(curly_core::storage::MAX_HISTORY_BODY_BYTES + 100);
    let response = ResponseSummary {
        status: 200,
        headers: vec![],
        body: big_body,
        elapsed: Duration::from_millis(1),
    };

    let entry = HistoryEntry::new(&request, &response);
    assert!(entry.response_body_truncated);
    assert!(entry.response_body.len() <= curly_core::storage::MAX_HISTORY_BODY_BYTES);
}

#[test]
fn list_history_on_missing_directory_is_empty_not_an_error() {
    let dir = tempdir().unwrap();
    let storage = Storage::new(dir.path());
    assert_eq!(storage.list_history(10).unwrap().len(), 0);
}
