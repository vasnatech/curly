use std::time::Duration;

use curly_core::exec::ResponseSummary;
use curly_core::model::Request;
use curly_core::storage::{
    find_project_local_root, slugify, Collection, Environment, HistoryEntry, SavedRequest,
    Storage,
};
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

#[test]
fn find_project_local_root_finds_dot_curly_in_start_dir() {
    let dir = tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".curly")).unwrap();
    assert_eq!(
        find_project_local_root(dir.path()),
        Some(dir.path().join(".curly"))
    );
}

#[test]
fn find_project_local_root_walks_up_through_ancestors() {
    let dir = tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".curly")).unwrap();
    let nested = dir.path().join("backend").join("src").join("main");
    std::fs::create_dir_all(&nested).unwrap();

    assert_eq!(
        find_project_local_root(&nested),
        Some(dir.path().join(".curly"))
    );
}

#[test]
fn find_project_local_root_returns_none_when_absent() {
    let dir = tempdir().unwrap();
    let nested = dir.path().join("a").join("b");
    std::fs::create_dir_all(&nested).unwrap();
    assert_eq!(find_project_local_root(&nested), None);
}

#[test]
fn find_project_local_root_ignores_a_file_named_dot_curly() {
    let dir = tempdir().unwrap();
    std::fs::write(dir.path().join(".curly"), "not a directory").unwrap();
    assert_eq!(find_project_local_root(dir.path()), None);
}

#[test]
fn resolve_prefers_explicit_over_everything() {
    let explicit = tempdir().unwrap();
    let cwd_with_local = tempdir().unwrap();
    std::fs::create_dir(cwd_with_local.path().join(".curly")).unwrap();

    let storage = Storage::resolve(
        Some(explicit.path()),
        Some("/some/env/dir"),
        cwd_with_local.path(),
    )
    .unwrap();
    assert_eq!(storage.root(), explicit.path());
}

#[test]
fn resolve_prefers_env_var_over_auto_detected_local_root() {
    let env_dir = tempdir().unwrap();
    let cwd_with_local = tempdir().unwrap();
    std::fs::create_dir(cwd_with_local.path().join(".curly")).unwrap();

    let storage = Storage::resolve(
        None,
        Some(env_dir.path().to_str().unwrap()),
        cwd_with_local.path(),
    )
    .unwrap();
    assert_eq!(storage.root(), env_dir.path());
}

#[test]
fn resolve_falls_back_to_auto_detected_local_root() {
    let cwd_with_local = tempdir().unwrap();
    std::fs::create_dir(cwd_with_local.path().join(".curly")).unwrap();

    let storage = Storage::resolve(None, None, cwd_with_local.path()).unwrap();
    assert_eq!(storage.root(), cwd_with_local.path().join(".curly"));
}

#[test]
fn resolve_falls_back_to_default_location_when_nothing_else_matches() {
    let cwd_without_local = tempdir().unwrap();

    let storage = Storage::resolve(None, None, cwd_without_local.path()).unwrap();
    assert_eq!(storage.root(), Storage::default_location().unwrap().root());
}

#[test]
fn resolve_ignores_blank_env_var() {
    let cwd_with_local = tempdir().unwrap();
    std::fs::create_dir(cwd_with_local.path().join(".curly")).unwrap();

    let storage = Storage::resolve(None, Some("   "), cwd_with_local.path()).unwrap();
    assert_eq!(storage.root(), cwd_with_local.path().join(".curly"));
}

fn sample_request(name: &str) -> SavedRequest {
    SavedRequest::new(name, Method::GET, "https://example.com")
}

#[test]
fn add_request_at_top_level() {
    let mut coll = Collection::new("My API");
    coll.add_request("login", sample_request("login")).unwrap();
    assert!(coll.find_request("login").is_some());
    assert_eq!(coll.requests.len(), 1);
    assert!(coll.folders.is_empty());
}

#[test]
fn add_request_creates_missing_folders() {
    let mut coll = Collection::new("My API");
    coll.add_request("Auth/OAuth/refresh", sample_request("placeholder"))
        .unwrap();

    let auth = coll.folders.iter().find(|f| f.name == "Auth").unwrap();
    let oauth = auth.folders.iter().find(|f| f.name == "OAuth").unwrap();
    assert_eq!(oauth.requests.len(), 1);
    assert_eq!(oauth.requests[0].name, "refresh");

    // find_request resolves the same path
    let found = coll.find_request("Auth/OAuth/refresh").unwrap();
    assert_eq!(found.name, "refresh");
}

#[test]
fn add_request_reuses_existing_folder_rather_than_duplicating() {
    let mut coll = Collection::new("My API");
    coll.add_request("Auth/login", sample_request("login")).unwrap();
    coll.add_request("Auth/logout", sample_request("logout")).unwrap();

    assert_eq!(coll.folders.len(), 1);
    let auth = &coll.folders[0];
    assert_eq!(auth.requests.len(), 2);
}

#[test]
fn add_request_duplicate_path_errors() {
    let mut coll = Collection::new("My API");
    coll.add_request("Auth/login", sample_request("login")).unwrap();
    assert!(coll
        .add_request("Auth/login", sample_request("login"))
        .is_err());
}

#[test]
fn find_request_missing_folder_segment_is_none() {
    let coll = Collection::new("My API");
    assert!(coll.find_request("NoSuchFolder/login").is_none());
}

#[test]
fn remove_request_nested() {
    let mut coll = Collection::new("My API");
    coll.add_request("Auth/login", sample_request("login")).unwrap();

    assert!(coll.remove_request("Auth/login"));
    assert!(coll.find_request("Auth/login").is_none());
    // the (now-empty) Auth folder itself is left in place
    assert_eq!(coll.folders.len(), 1);
}

#[test]
fn remove_request_missing_returns_false() {
    let mut coll = Collection::new("My API");
    assert!(!coll.remove_request("Auth/login"));
}

#[test]
fn add_folder_is_idempotent() {
    let mut coll = Collection::new("My API");
    coll.add_folder("Auth");
    coll.add_folder("Auth");
    assert_eq!(coll.folders.len(), 1);
}

#[test]
fn add_folder_creates_nested_path() {
    let mut coll = Collection::new("My API");
    coll.add_folder("Auth/OAuth");
    let auth = coll.folders.iter().find(|f| f.name == "Auth").unwrap();
    assert!(auth.folders.iter().any(|f| f.name == "OAuth"));
}

#[test]
fn remove_folder_empty_succeeds() {
    let mut coll = Collection::new("My API");
    coll.add_folder("Auth");
    assert!(coll.remove_folder("Auth", false).unwrap());
    assert!(coll.folders.is_empty());
}

#[test]
fn remove_folder_nonempty_without_force_errors() {
    let mut coll = Collection::new("My API");
    coll.add_request("Auth/login", sample_request("login")).unwrap();
    assert!(coll.remove_folder("Auth", false).is_err());
    assert!(coll.find_request("Auth/login").is_some());
}

#[test]
fn remove_folder_nonempty_with_force_succeeds() {
    let mut coll = Collection::new("My API");
    coll.add_request("Auth/login", sample_request("login")).unwrap();
    assert!(coll.remove_folder("Auth", true).unwrap());
    assert!(coll.folders.is_empty());
}

#[test]
fn remove_folder_missing_returns_false() {
    let mut coll = Collection::new("My API");
    assert!(!coll.remove_folder("NoSuchFolder", false).unwrap());
}

#[test]
fn nested_collection_round_trips_through_disk() {
    let dir = tempdir().unwrap();
    let storage = Storage::new(dir.path());

    let mut coll = Collection::new("My API");
    coll.add_request("Auth/login", sample_request("login")).unwrap();
    coll.add_folder("Empty");
    storage.save_collection(&coll).unwrap();

    let loaded = storage.load_collection("My API").unwrap();
    assert!(loaded.find_request("Auth/login").is_some());
    assert!(loaded.folders.iter().any(|f| f.name == "Empty"));
}

#[test]
fn collection_without_folders_key_still_parses() {
    // A collection saved before folder support existed (M2) — no "folders"
    // key at all. Must still load correctly (backward compatibility).
    let old_format = r#"{
        "id": "2de0e066-83db-4252-a018-cd0265fde8de",
        "name": "My API",
        "requests": [
            {
                "id": "86d01615-0d5e-42e4-be08-e1e4b74ce3e6",
                "name": "login",
                "method": "GET",
                "url": "https://example.com",
                "query_params": [],
                "headers": [],
                "body": null,
                "auth": null
            }
        ]
    }"#;
    let collection: Collection = serde_json::from_str(old_format).unwrap();
    assert!(collection.folders.is_empty());
    assert!(collection.find_request("login").is_some());
}
