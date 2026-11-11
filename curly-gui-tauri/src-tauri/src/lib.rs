mod commands;
mod project;

use commands::AppState;
use curly_core::storage::Storage;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Auto-open a project the same way the CLI/egui GUI both already do by
    // default: an auto-detected `.curly` walking up from the process's cwd,
    // `CURLY_DATA_DIR`, or the OS-wide default. `resolve_default` only
    // fails if the OS-wide default location itself can't be determined
    // (e.g. no home directory) — a real desktop always has one, so this
    // mirrors the egui GUI's own "just resolve at startup" behavior rather
    // than adding a recovery path for a case that shouldn't occur here.
    let storage = Storage::resolve_default(None).expect("failed to resolve default storage location");

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState {
            storage: std::sync::Mutex::new(storage),
            client: reqwest::Client::new(),
        })
        .invoke_handler(tauri::generate_handler![
            commands::open_project,
            commands::resolve_default_project,
            commands::list_collections,
            commands::load_collection,
            commands::save_collection,
            commands::delete_collection,
            commands::add_request,
            commands::rename_or_move_request,
            commands::duplicate_request,
            commands::remove_request,
            commands::add_folder,
            commands::remove_folder,
            commands::list_environments,
            commands::load_environment,
            commands::save_environment,
            commands::delete_environment,
            commands::load_session,
            commands::save_session,
            commands::clear_session,
            commands::list_history,
            commands::list_scenarios,
            commands::load_scenario,
            commands::save_scenario,
            commands::delete_scenario,
            commands::run_scenario,
            commands::send_request,
            commands::save_bytes,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
