//! Saving/loading the small set of GUI preferences that survive a restart
//! (zoom, theme, open tabs) — the rest of `CurlyApp`'s state (which project
//! is open, environments, sessions, ...) already lives in the project's own
//! `.curly` storage and doesn't need a separate mechanism.
//!
//! Deliberately a plain JSON file of our own, not `eframe`'s built-in
//! `persistence` Cargo feature/`eframe::Storage` — that mechanism is a
//! generic key-value blob store aimed at arbitrary app state; a single
//! explicit struct we control is simpler to reason about and test.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::app::{AuthEditor, HeaderRow, LoadedRequestRef};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum PersistedTheme {
    Dark,
    Light,
}

impl From<egui::Theme> for PersistedTheme {
    fn from(theme: egui::Theme) -> Self {
        match theme {
            egui::Theme::Dark => PersistedTheme::Dark,
            egui::Theme::Light => PersistedTheme::Light,
        }
    }
}

impl From<PersistedTheme> for egui::Theme {
    fn from(theme: PersistedTheme) -> Self {
        match theme {
            PersistedTheme::Dark => egui::Theme::Dark,
            PersistedTheme::Light => egui::Theme::Light,
        }
    }
}

/// The persisted subset of one request tab's editor state — deliberately
/// not the full `RequestTab` (which holds `mpsc::Receiver`s and other
/// runtime-only fields with no meaningful "restore after a restart"
/// behavior). `var_overrides` and `extract_rules` are excluded on purpose:
/// overrides already have a "never written to disk" design intent (see
/// docs/GUI.md's "Ad-hoc variable overrides" section) and extraction rules
/// are the kind of thing worth re-checking after a restart rather than
/// silently carried forward.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PersistedTab {
    pub(crate) method: String,
    pub(crate) url: String,
    pub(crate) headers: Vec<HeaderRow>,
    pub(crate) body: String,
    pub(crate) auth: AuthEditor,
    pub(crate) loaded_request: Option<LoadedRequestRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PersistedState {
    pub(crate) zoom: f32,
    pub(crate) theme: PersistedTheme,
    /// Whether the GUI should keep following the OS light/dark preference
    /// live, or whether the user has manually pinned `theme` by using the
    /// toggle — once pinned, it stays pinned across restarts until toggled
    /// again (matches how a manual light/dark override behaves in most
    /// apps that also support "follow the OS").
    pub(crate) theme_follows_system: bool,
    pub(crate) tabs: Vec<PersistedTab>,
    pub(crate) active_tab_idx: usize,
}

/// `~/.config/curly/gui_state.json` on Linux (via `dirs::config_dir()`) —
/// deliberately separate from `Storage::default_location`'s
/// `dirs::data_dir()`-based project storage: this is app-level GUI
/// preferences, not project data, and shouldn't live inside any project's
/// `.curly` directory.
fn preferences_path() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("curly").join("gui_state.json"))
}

/// `None` covers a first-ever launch (no file yet) and a corrupt/unreadable
/// file equally — both fall back to today's defaults rather than erroring.
pub(crate) fn load() -> Option<PersistedState> {
    let path = preferences_path()?;
    let contents = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&contents).ok()
}

/// Best-effort — there's no UI left to show a failure notice in by the time
/// this runs (called from `on_exit`), so a write failure is logged to
/// stderr rather than surfaced anywhere in the app itself.
pub(crate) fn save(state: &PersistedState) {
    let Some(path) = preferences_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            eprintln!("curly: failed to create GUI preferences directory: {e}");
            return;
        }
    }
    match serde_json::to_string_pretty(state) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&path, json) {
                eprintln!("curly: failed to save GUI preferences: {e}");
            }
        }
        Err(e) => eprintln!("curly: failed to serialize GUI preferences: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persisted_theme_round_trips_through_egui_theme() {
        assert_eq!(egui::Theme::from(PersistedTheme::Dark), egui::Theme::Dark);
        assert_eq!(egui::Theme::from(PersistedTheme::Light), egui::Theme::Light);
        assert_eq!(PersistedTheme::from(egui::Theme::Dark), PersistedTheme::Dark);
        assert_eq!(PersistedTheme::from(egui::Theme::Light), PersistedTheme::Light);
    }

    #[test]
    fn persisted_state_round_trips_through_json() {
        let state = PersistedState {
            zoom: 1.2,
            theme: PersistedTheme::Light,
            theme_follows_system: false,
            tabs: vec![PersistedTab {
                method: "POST".to_string(),
                url: "https://example.com".to_string(),
                headers: vec![HeaderRow {
                    name: "Accept".to_string(),
                    value: "application/json".to_string(),
                    enabled: true,
                }],
                body: "{}".to_string(),
                auth: AuthEditor::default(),
                loaded_request: Some(LoadedRequestRef {
                    collection: "api".to_string(),
                    path: "login".to_string(),
                }),
            }],
            active_tab_idx: 0,
        };

        let json = serde_json::to_string(&state).unwrap();
        let restored: PersistedState = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.zoom, 1.2);
        assert_eq!(restored.theme, PersistedTheme::Light);
        assert!(!restored.theme_follows_system);
        assert_eq!(restored.tabs.len(), 1);
        assert_eq!(restored.tabs[0].method, "POST");
        assert_eq!(restored.tabs[0].headers[0].name, "Accept");
        assert_eq!(
            restored.tabs[0].loaded_request,
            Some(LoadedRequestRef {
                collection: "api".to_string(),
                path: "login".to_string(),
            })
        );
    }
}
