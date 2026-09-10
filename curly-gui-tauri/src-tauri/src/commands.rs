//! Every `curly-core` operation the frontend needs, exposed as
//! `#[tauri::command]`s. Grouped by the `curly-core` module each wraps.
//! Almost all of these are thin serialize-in/serialize-out shims around an
//! already-existing `Storage`/`Collection` method (load-mutate-save on the
//! Rust side, so the frontend calls one command per user action, not
//! three). `send_request` is the one genuinely new piece of orchestration —
//! see its own doc comment.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;

use curly_core::exec::{self, ResponseSummary};
use curly_core::extraction::{self, Extracted};
use curly_core::scenario::{self, Scenario, ScenarioRun};
use curly_core::storage::{self, Collection, Environment, HistoryEntry, SavedRequest, Storage};
use curly_core::substitution;
use serde::Serialize;
use tauri::State;

use crate::project::{self, ProjectInfo};

/// The currently-open project's storage, plus a shared HTTP client (one per
/// app instance, matching `curly-cli`'s and the egui GUI's own single-
/// `reqwest::Client` reuse). `Storage` is cheap to `Clone` (it's just a root
/// path), so commands lock only long enough to clone it out, then operate
/// on their own copy — no I/O happens while the lock is held.
pub struct AppState {
    pub storage: Mutex<Storage>,
    pub client: reqwest::Client,
}

fn current_storage(state: &State<AppState>) -> Storage {
    state.storage.lock().expect("storage mutex poisoned").clone()
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

// --- Project ---

#[tauri::command]
pub fn open_project(state: State<AppState>, path: String) -> Result<ProjectInfo, String> {
    let (storage, _created) = Storage::init_project_local(Path::new(&path)).map_err(err)?;
    let info = project::info_for(&storage);
    *state.storage.lock().expect("storage mutex poisoned") = storage;
    Ok(info)
}

#[tauri::command]
pub fn resolve_default_project(state: State<AppState>) -> Result<ProjectInfo, String> {
    let storage = Storage::resolve_default(None).map_err(err)?;
    let info = project::info_for(&storage);
    *state.storage.lock().expect("storage mutex poisoned") = storage;
    Ok(info)
}

// --- Collections ---

#[tauri::command]
pub fn list_collections(state: State<AppState>) -> Result<Vec<String>, String> {
    current_storage(&state).list_collections().map_err(err)
}

#[tauri::command]
pub fn load_collection(state: State<AppState>, name: String) -> Result<Collection, String> {
    current_storage(&state).load_collection(&name).map_err(err)
}

#[tauri::command]
pub fn save_collection(state: State<AppState>, collection: Collection) -> Result<(), String> {
    current_storage(&state).save_collection(&collection).map_err(err)
}

#[tauri::command]
pub fn delete_collection(state: State<AppState>, name: String) -> Result<(), String> {
    current_storage(&state).delete_collection(&name).map_err(err)
}

#[tauri::command]
pub fn add_request(
    state: State<AppState>,
    collection: String,
    path: String,
    request: SavedRequest,
) -> Result<(), String> {
    let storage = current_storage(&state);
    let mut coll = storage
        .load_collection_opt(&collection)
        .map_err(err)?
        .unwrap_or_else(|| Collection::new(&collection));
    coll.add_request(&path, request).map_err(err)?;
    storage.save_collection(&coll).map_err(err)
}

#[tauri::command]
pub fn rename_or_move_request(
    state: State<AppState>,
    collection: String,
    old_path: String,
    new_path: String,
) -> Result<(), String> {
    let storage = current_storage(&state);
    let mut coll = storage.load_collection(&collection).map_err(err)?;
    coll.rename_or_move_request(&old_path, &new_path).map_err(err)?;
    storage.save_collection(&coll).map_err(err)
}

#[tauri::command]
pub fn duplicate_request(
    state: State<AppState>,
    collection: String,
    path: String,
    new_path: String,
) -> Result<(), String> {
    let storage = current_storage(&state);
    let mut coll = storage.load_collection(&collection).map_err(err)?;
    coll.duplicate_request(&path, &new_path).map_err(err)?;
    storage.save_collection(&coll).map_err(err)
}

#[tauri::command]
pub fn remove_request(state: State<AppState>, collection: String, path: String) -> Result<bool, String> {
    let storage = current_storage(&state);
    let mut coll = storage.load_collection(&collection).map_err(err)?;
    let removed = coll.remove_request(&path);
    storage.save_collection(&coll).map_err(err)?;
    Ok(removed)
}

#[tauri::command]
pub fn add_folder(state: State<AppState>, collection: String, path: String) -> Result<(), String> {
    let storage = current_storage(&state);
    let mut coll = storage
        .load_collection_opt(&collection)
        .map_err(err)?
        .unwrap_or_else(|| Collection::new(&collection));
    coll.add_folder(&path);
    storage.save_collection(&coll).map_err(err)
}

#[tauri::command]
pub fn remove_folder(
    state: State<AppState>,
    collection: String,
    path: String,
    force: bool,
) -> Result<bool, String> {
    let storage = current_storage(&state);
    let mut coll = storage.load_collection(&collection).map_err(err)?;
    let removed = coll.remove_folder(&path, force).map_err(err)?;
    storage.save_collection(&coll).map_err(err)?;
    Ok(removed)
}

// --- Environments ---

#[tauri::command]
pub fn list_environments(state: State<AppState>) -> Result<Vec<String>, String> {
    current_storage(&state).list_environments().map_err(err)
}

#[tauri::command]
pub fn load_environment(state: State<AppState>, name: String) -> Result<Environment, String> {
    current_storage(&state).load_environment(&name).map_err(err)
}

#[tauri::command]
pub fn save_environment(state: State<AppState>, environment: Environment) -> Result<(), String> {
    current_storage(&state).save_environment(&environment).map_err(err)
}

#[tauri::command]
pub fn delete_environment(state: State<AppState>, name: String) -> Result<(), String> {
    current_storage(&state).delete_environment(&name).map_err(err)
}

// --- Session ---

#[tauri::command]
pub fn load_session(state: State<AppState>, env_name: String) -> Result<Environment, String> {
    current_storage(&state).load_session(&env_name).map_err(err)
}

#[tauri::command]
pub fn save_session(state: State<AppState>, session: Environment) -> Result<(), String> {
    current_storage(&state).save_session(&session).map_err(err)
}

#[tauri::command]
pub fn clear_session(state: State<AppState>, env_name: String) -> Result<(), String> {
    current_storage(&state).clear_session(&env_name).map_err(err)
}

// --- History ---

#[tauri::command]
pub fn list_history(state: State<AppState>, limit: usize) -> Result<Vec<HistoryEntry>, String> {
    current_storage(&state).list_history(limit).map_err(err)
}

// --- Scenarios ---

#[tauri::command]
pub fn list_scenarios(state: State<AppState>) -> Result<Vec<String>, String> {
    current_storage(&state).list_scenarios().map_err(err)
}

#[tauri::command]
pub fn load_scenario(state: State<AppState>, name: String) -> Result<Scenario, String> {
    current_storage(&state).load_scenario(&name).map_err(err)
}

#[tauri::command]
pub fn save_scenario(state: State<AppState>, scenario: Scenario) -> Result<(), String> {
    current_storage(&state).save_scenario(&scenario).map_err(err)
}

#[tauri::command]
pub fn delete_scenario(state: State<AppState>, name: String) -> Result<(), String> {
    current_storage(&state).delete_scenario(&name).map_err(err)
}

#[tauri::command]
pub async fn run_scenario(
    state: State<'_, AppState>,
    name: String,
    env_name: Option<String>,
    overrides: Vec<(String, String)>,
) -> Result<ScenarioRun, String> {
    let storage = current_storage(&state);
    let scenario = storage.load_scenario(&name).map_err(err)?;
    let client = state.client.clone();
    scenario::run(&scenario, &storage, &client, env_name.as_deref(), &overrides)
        .await
        .map_err(err)
}

// --- Send ---

#[derive(Debug, Serialize)]
pub struct SendOutcome {
    pub response: ResponseSummary,
    pub extracted: Vec<Extracted>,
}

/// Merge variables the same way `curly run` does (low to high precedence,
/// see `curly-cli`'s `commands::run::merged_variables`, mirrored here since
/// that function is CLI-arg-shaped and not reusable as-is): the
/// always-merged-in "global" environment, the selected environment (if
/// any), that environment's (or "global"'s) session, then ad-hoc overrides.
fn merged_variables(
    storage: &Storage,
    env_name: Option<&str>,
    overrides: &[(String, String)],
) -> anyhow::Result<BTreeMap<String, String>> {
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

/// Resolve variables, send, append to history, and — on a 2xx response with
/// extraction rules — apply them and write the session, all in one round
/// trip. `saved` is either a real saved request (loaded by the frontend via
/// `load_collection`/`find_request` client-side) or an ad-hoc one built
/// from scratch in the editor — this command doesn't care which; nothing
/// here touches collection storage. This is the one place this scoping
/// pass's "step 2" needed genuinely new orchestration rather than a 1:1
/// wrap: the egui GUI's equivalent (`send`/`poll_response`/
/// `apply_pending_extraction`) was split across polling an `mpsc::Receiver`
/// every frame because egui is immediate-mode; here it's a single `async
/// fn` a webview can just `await`.
#[tauri::command]
pub async fn send_request(
    state: State<'_, AppState>,
    saved: SavedRequest,
    env_name: Option<String>,
    overrides: Vec<(String, String)>,
) -> Result<SendOutcome, String> {
    let storage = current_storage(&state);
    let variables = merged_variables(&storage, env_name.as_deref(), &overrides).map_err(err)?;
    let resolved = substitution::resolve(&saved, &variables).map_err(err)?;
    let request = storage::into_request(&resolved).map_err(err)?;

    let response = exec::send(&state.client, &request).await.map_err(err)?;
    let _ = storage.append_history(&HistoryEntry::new(&request, &response));

    let mut extracted = Vec::new();
    if (200..300).contains(&response.status) && !saved.extract.is_empty() {
        extracted = extraction::apply(&saved.extract, &response, &variables).map_err(err)?;
        let session_scope = env_name.as_deref().unwrap_or("global");
        let mut session = storage.load_session(session_scope).map_err(err)?;
        for var in &extracted {
            session.set(var.name.as_str(), var.value.as_str(), var.secret);
        }
        storage.save_session(&session).map_err(err)?;
    }

    Ok(SendOutcome { response, extracted })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn storage() -> (tempfile::TempDir, Storage) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".curly");
        (dir, Storage::new(root))
    }

    #[test]
    fn merged_variables_none_selected_uses_only_global_and_its_session() {
        let (_dir, storage) = storage();
        let mut global = Environment::new("global");
        global.set("BASE_URL", "https://global.example.com", false);
        storage.save_environment(&global).unwrap();
        let mut session = storage.load_session("global").unwrap();
        session.set("TOKEN", "from-session", false);
        storage.save_session(&session).unwrap();

        let vars = merged_variables(&storage, None, &[]).unwrap();

        assert_eq!(vars.get("BASE_URL").unwrap(), "https://global.example.com");
        assert_eq!(vars.get("TOKEN").unwrap(), "from-session");
    }

    #[test]
    fn merged_variables_selected_environment_overrides_global() {
        let (_dir, storage) = storage();
        let mut global = Environment::new("global");
        global.set("BASE_URL", "https://global.example.com", false);
        storage.save_environment(&global).unwrap();
        let mut local = Environment::new("local");
        local.set("BASE_URL", "https://local.example.com", false);
        storage.save_environment(&local).unwrap();

        let vars = merged_variables(&storage, Some("local"), &[]).unwrap();

        assert_eq!(vars.get("BASE_URL").unwrap(), "https://local.example.com");
    }

    #[test]
    fn merged_variables_session_is_scoped_to_the_selected_environment() {
        let (_dir, storage) = storage();
        storage.save_environment(&Environment::new("local")).unwrap();
        let mut global_session = storage.load_session("global").unwrap();
        global_session.set("TOKEN", "global-token", false);
        storage.save_session(&global_session).unwrap();
        let mut local_session = storage.load_session("local").unwrap();
        local_session.set("TOKEN", "local-token", false);
        storage.save_session(&local_session).unwrap();

        let vars = merged_variables(&storage, Some("local"), &[]).unwrap();

        assert_eq!(vars.get("TOKEN").unwrap(), "local-token");
    }

    #[test]
    fn merged_variables_overrides_win_over_everything_else() {
        let (_dir, storage) = storage();
        let mut global = Environment::new("global");
        global.set("BASE_URL", "https://global.example.com", false);
        storage.save_environment(&global).unwrap();

        let vars = merged_variables(
            &storage,
            None,
            &[("BASE_URL".to_string(), "https://override.example.com".to_string())],
        )
        .unwrap();

        assert_eq!(vars.get("BASE_URL").unwrap(), "https://override.example.com");
    }

    #[test]
    fn merged_variables_with_no_data_at_all_is_empty() {
        let (_dir, storage) = storage();
        let vars = merged_variables(&storage, None, &[]).unwrap();
        assert!(vars.is_empty());
    }
}
