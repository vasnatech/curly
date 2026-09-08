//! The GUI's first two slices: FR-17's core loop (method/URL bar, Send,
//! Headers/Body panels, a response pane) plus the beginning of FR-18 — a
//! project picker and a read-only listing of the active project's
//! collections/environments. Not yet built: browsing into a collection's
//! requests, loading one into the editor, an environment switcher, or
//! multiple simultaneously-open projects (see project.rs's doc comment for
//! why the data model already supports the last one).

use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use curly_core::exec::{self, ResponseSummary};
use curly_core::model::{Body, Request};
use curly_core::storage::Storage;
use reqwest::Method;

use crate::project::Project;

const METHODS: [&str; 7] = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];

struct HeaderRow {
    name: String,
    value: String,
    enabled: bool,
}

impl HeaderRow {
    fn empty() -> Self {
        Self {
            name: String::new(),
            value: String::new(),
            enabled: true,
        }
    }
}

enum SendOutcome {
    Success(ResponseSummary),
    Error(String),
}

pub struct CurlyApp {
    method: String,
    url: String,
    headers: Vec<HeaderRow>,
    body: String,

    response: Option<Result<ResponseSummary, String>>,
    in_flight: bool,
    rx: Option<mpsc::Receiver<SendOutcome>>,

    runtime: tokio::runtime::Runtime,
    client: reqwest::Client,

    /// A `Vec` (not `Option<Project>`), and an index rather than always
    /// "the last one" — deliberately, even though today opening a project
    /// always replaces this vec's single entry. See project.rs's doc
    /// comment: real multi-project support later (tabs, switching, closing
    /// one without losing the others) becomes an additive UI change against
    /// this same data shape, not a migration.
    projects: Vec<Project>,
    active_project_idx: Option<usize>,
    project_open_error: Option<String>,
    /// Cached listing of the active project's collections/environments —
    /// re-read from disk whenever the active project changes. Not yet
    /// clickable (that's the next slice, browsing into a collection).
    collections: Vec<String>,
    environments: Vec<String>,
}

impl CurlyApp {
    /// The real eframe entry point. Auto-opens a project the same way the
    /// CLI resolves storage by default: an auto-detected `./.curly` walking
    /// up from the current directory, `CURLY_DATA_DIR`, or the OS-wide
    /// default (no `--data-dir` equivalent — the GUI has no CLI flags of its
    /// own). This touches the real filesystem/cwd, which is exactly why it's
    /// kept out of `default_state` below — unit tests build on that instead,
    /// so they stay hermetic regardless of what directory `cargo test` runs
    /// from or what `.curly` might exist somewhere above it.
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let mut app = Self::default_state();
        match Storage::resolve_default(None) {
            Ok(storage) => app.set_active_project(storage),
            Err(e) => app.project_open_error = Some(e.to_string()),
        }
        app
    }

    /// A blank app with no project open — deliberately untouched by the
    /// real filesystem/cwd (see `new`'s doc comment), so unit tests are
    /// hermetic. Also doesn't need a real `eframe::CreationContext`.
    fn default_state() -> Self {
        Self {
            method: "GET".to_string(),
            url: String::new(),
            headers: vec![HeaderRow::empty()],
            body: String::new(),
            response: None,
            in_flight: false,
            rx: None,
            runtime: tokio::runtime::Runtime::new().expect("failed to start async runtime"),
            client: reqwest::Client::new(),
            projects: Vec::new(),
            active_project_idx: None,
            project_open_error: None,
            collections: Vec::new(),
            environments: Vec::new(),
        }
    }

    /// Open (creating if needed, via `Storage::init_project_local`) the
    /// project rooted at `dir` — used by the "Open Project" folder picker.
    /// Unlike `Storage::resolve`'s CLI-facing walk-up search, this operates
    /// directly on the picked directory: a folder dialog already lets the
    /// user navigate to exactly the directory they want, so searching its
    /// ancestors too could surprise them by opening a *different* project
    /// than the one they picked.
    fn open_project_at(&mut self, dir: &Path) {
        match Storage::init_project_local(dir) {
            Ok((storage, _created)) => self.set_active_project(storage),
            Err(e) => self.project_open_error = Some(e.to_string()),
        }
    }

    fn set_active_project(&mut self, storage: Storage) {
        self.projects = vec![Project::from_storage(storage)];
        self.active_project_idx = Some(0);
        self.project_open_error = None;
        self.refresh_project_lists();
    }

    fn refresh_project_lists(&mut self) {
        let listing = self.active_project_idx.and_then(|i| self.projects.get(i)).map(|p| {
            (
                p.storage.list_collections().unwrap_or_default(),
                p.storage.list_environments().unwrap_or_default(),
            )
        });
        match listing {
            Some((collections, environments)) => {
                self.collections = collections;
                self.environments = environments;
            }
            None => {
                self.collections.clear();
                self.environments.clear();
            }
        }
    }

    fn active_project(&self) -> Option<&Project> {
        self.active_project_idx.and_then(|i| self.projects.get(i))
    }

    fn build_request(&self) -> anyhow::Result<Request> {
        let method = Method::from_bytes(self.method.to_uppercase().as_bytes())?;
        let mut request = Request::new(method, self.url.clone());
        for h in &self.headers {
            if h.enabled && !h.name.trim().is_empty() {
                request = request.with_header(h.name.clone(), h.value.clone());
            }
        }
        if !self.body.is_empty() {
            request = request.with_body(Body::Raw(self.body.clone()));
        }
        Ok(request)
    }

    /// Fire the request on the background runtime; the response comes back
    /// through `self.rx`, polled each frame in `App::ui`. `ctx.request_repaint()`
    /// wakes the UI thread immediately when it arrives, rather than waiting
    /// on the next input-driven repaint.
    fn send(&mut self, ctx: &egui::Context) {
        let request = match self.build_request() {
            Ok(r) => r,
            Err(e) => {
                self.response = Some(Err(e.to_string()));
                return;
            }
        };

        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        self.in_flight = true;
        self.response = None;

        let client = self.client.clone();
        let ctx = ctx.clone();
        self.runtime.spawn(async move {
            let outcome = match exec::send(&client, &request).await {
                Ok(resp) => SendOutcome::Success(resp),
                Err(e) => SendOutcome::Error(e.to_string()),
            };
            let _ = tx.send(outcome);
            ctx.request_repaint();
        });
    }

    fn poll_response(&mut self) {
        let Some(rx) = &self.rx else { return };
        if let Ok(outcome) = rx.try_recv() {
            self.in_flight = false;
            self.response = Some(match outcome {
                SendOutcome::Success(r) => Ok(r),
                SendOutcome::Error(e) => Err(e),
            });
            self.rx = None;
        }
    }
}

impl eframe::App for CurlyApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll_response();

        egui::Panel::left("project_sidebar")
            .resizable(true)
            .default_size(220.0)
            .show(ui, |ui| {
                ui.heading("Project");

                match self.active_project() {
                    Some(project) => {
                        ui.strong(&project.label);
                        ui.weak(project.root.display().to_string());
                    }
                    None => {
                        ui.weak("No project open");
                    }
                }

                if let Some(err) = &self.project_open_error {
                    ui.colored_label(egui::Color32::from_rgb(220, 80, 80), err);
                }

                if ui.button("Open Project…").clicked() {
                    if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                        self.open_project_at(&dir);
                    }
                }

                ui.separator();

                ui.label(format!("Collections ({})", self.collections.len()));
                if self.collections.is_empty() {
                    ui.weak("  (none yet)");
                }
                for name in &self.collections {
                    ui.label(format!("  {name}"));
                }

                ui.separator();

                ui.label(format!("Environments ({})", self.environments.len()));
                if self.environments.is_empty() {
                    ui.weak("  (none yet)");
                }
                for name in &self.environments {
                    ui.label(format!("  {name}"));
                }
            });

        egui::CentralPanel::default().show(ui, |ui| {
            let ctx = ui.ctx().clone();
            let mut should_send = false;

            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("method")
                    .selected_text(self.method.clone())
                    .show_ui(ui, |ui| {
                        for m in METHODS {
                            ui.selectable_value(&mut self.method, m.to_string(), m);
                        }
                    });

                let url_response = ui.add(
                    egui::TextEdit::singleline(&mut self.url)
                        .desired_width(ui.available_width() - 80.0)
                        .hint_text("https://api.example.com/..."),
                );
                let enter_in_url = url_response.lost_focus()
                    && ui.input(|i| i.key_pressed(egui::Key::Enter));

                let send_clicked = ui
                    .add_enabled(
                        !self.in_flight,
                        egui::Button::new(if self.in_flight { "Sending…" } else { "Send" }),
                    )
                    .clicked();

                let ctrl_enter =
                    ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter));

                if !self.in_flight
                    && !self.url.trim().is_empty()
                    && (send_clicked || enter_in_url || ctrl_enter)
                {
                    should_send = true;
                }
            });

            ui.separator();

            egui::CollapsingHeader::new("Headers")
                .default_open(true)
                .show(ui, |ui| {
                    let mut remove_idx = None;
                    for (i, h) in self.headers.iter_mut().enumerate() {
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut h.enabled, "");
                            ui.add(
                                egui::TextEdit::singleline(&mut h.name)
                                    .hint_text("Header")
                                    .desired_width(160.0),
                            );
                            ui.add(
                                egui::TextEdit::singleline(&mut h.value)
                                    .hint_text("value")
                                    .desired_width(260.0),
                            );
                            if ui.small_button("✕").clicked() {
                                remove_idx = Some(i);
                            }
                        });
                    }
                    if let Some(i) = remove_idx {
                        self.headers.remove(i);
                    }
                    if ui.button("+ Add header").clicked() {
                        self.headers.push(HeaderRow::empty());
                    }
                });

            egui::CollapsingHeader::new("Body")
                .default_open(true)
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut self.body)
                            .desired_rows(6)
                            .desired_width(f32::INFINITY)
                            .font(egui::TextStyle::Monospace),
                    );
                });

            ui.separator();
            ui.heading("Response");

            match &self.response {
                None if self.in_flight => {
                    ui.label("Sending…");
                }
                None => {
                    ui.weak("No response yet.");
                }
                Some(Err(e)) => {
                    ui.colored_label(egui::Color32::from_rgb(220, 80, 80), format!("Error: {e}"));
                }
                Some(Ok(resp)) => {
                    ui.horizontal(|ui| {
                        let color = if resp.status < 300 {
                            egui::Color32::from_rgb(90, 200, 100)
                        } else if resp.status < 400 {
                            egui::Color32::from_rgb(230, 190, 60)
                        } else {
                            egui::Color32::from_rgb(220, 80, 80)
                        };
                        ui.colored_label(color, format!("{}", resp.status));
                        ui.label(format!("{:?}", resp.elapsed));
                        ui.label(format!("{} bytes", resp.body.len()));
                    });

                    egui::CollapsingHeader::new("Response Headers").show(ui, |ui| {
                        for (k, v) in &resp.headers {
                            ui.label(format!("{k}: {v}"));
                        }
                    });

                    let mut pretty = serde_json::from_str::<serde_json::Value>(&resp.body)
                        .ok()
                        .and_then(|v| serde_json::to_string_pretty(&v).ok())
                        .unwrap_or_else(|| resp.body.clone());

                    egui::ScrollArea::vertical().show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut pretty)
                                .font(egui::TextStyle::Monospace)
                                .desired_width(f32::INFINITY),
                        );
                    });
                }
            }

            if should_send {
                self.send(&ctx);
            }
        });

        if self.in_flight {
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_get() {
        let app = CurlyApp::default_state();
        let request = app.build_request().unwrap();
        assert_eq!(request.method, Method::GET);
    }

    #[test]
    fn method_field_is_used_and_case_insensitive() {
        let mut app = CurlyApp::default_state();
        app.method = "post".to_string();
        let request = app.build_request().unwrap();
        assert_eq!(request.method, Method::POST);
    }

    #[test]
    fn invalid_method_errors() {
        let mut app = CurlyApp::default_state();
        app.method = "NOT A METHOD".to_string();
        assert!(app.build_request().is_err());
    }

    #[test]
    fn url_is_carried_through() {
        let mut app = CurlyApp::default_state();
        app.url = "https://example.com/get".to_string();
        let request = app.build_request().unwrap();
        assert_eq!(request.url, "https://example.com/get");
    }

    #[test]
    fn enabled_named_headers_are_included() {
        let mut app = CurlyApp::default_state();
        app.headers = vec![HeaderRow {
            name: "Content-Type".to_string(),
            value: "application/json".to_string(),
            enabled: true,
        }];
        let request = app.build_request().unwrap();
        assert_eq!(
            request.headers,
            vec![("Content-Type".to_string(), "application/json".to_string())]
        );
    }

    #[test]
    fn disabled_headers_are_excluded() {
        let mut app = CurlyApp::default_state();
        app.headers = vec![HeaderRow {
            name: "X-Skip".to_string(),
            value: "x".to_string(),
            enabled: false,
        }];
        let request = app.build_request().unwrap();
        assert!(request.headers.is_empty());
    }

    #[test]
    fn headers_with_blank_name_are_excluded() {
        let mut app = CurlyApp::default_state();
        app.headers = vec![HeaderRow {
            name: "   ".to_string(),
            value: "x".to_string(),
            enabled: true,
        }];
        let request = app.build_request().unwrap();
        assert!(request.headers.is_empty());
    }

    #[test]
    fn empty_body_produces_no_body() {
        let app = CurlyApp::default_state();
        let request = app.build_request().unwrap();
        assert!(request.body.is_none());
    }

    #[test]
    fn non_empty_body_is_sent_as_raw() {
        let mut app = CurlyApp::default_state();
        app.body = r#"{"a":1}"#.to_string();
        let request = app.build_request().unwrap();
        assert!(matches!(request.body, Some(Body::Raw(ref s)) if s == r#"{"a":1}"#));
    }

    #[test]
    fn default_state_opens_no_project() {
        let app = CurlyApp::default_state();
        assert!(app.active_project().is_none());
        assert!(app.projects.is_empty());
    }

    #[test]
    fn open_project_at_creates_and_activates_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = CurlyApp::default_state();

        app.open_project_at(dir.path());

        let project = app.active_project().unwrap();
        assert_eq!(project.root, dir.path().join(".curly"));
        assert!(dir.path().join(".curly").is_dir());
        assert!(app.project_open_error.is_none());
    }

    #[test]
    fn open_project_at_reopens_an_existing_project_without_clearing_it() {
        let dir = tempfile::tempdir().unwrap();
        curly_core::storage::Storage::init_project_local(dir.path()).unwrap();
        let storage = curly_core::storage::Storage::new(dir.path().join(".curly"));
        storage
            .save_collection(&curly_core::storage::Collection::new("Existing"))
            .unwrap();

        let mut app = CurlyApp::default_state();
        app.open_project_at(dir.path());

        assert_eq!(app.collections, vec!["existing".to_string()]);
    }

    #[test]
    fn opening_a_second_project_replaces_the_first() {
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        let mut app = CurlyApp::default_state();

        app.open_project_at(dir_a.path());
        app.open_project_at(dir_b.path());

        // Single-active-project today (see project.rs's doc comment) — opening
        // a second project replaces the vec's one entry rather than appending.
        assert_eq!(app.projects.len(), 1);
        assert_eq!(app.active_project().unwrap().root, dir_b.path().join(".curly"));
    }

    #[test]
    fn refresh_project_lists_reflects_the_active_projects_storage() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = CurlyApp::default_state();
        app.open_project_at(dir.path());

        let storage = &app.active_project().unwrap().storage;
        storage
            .save_collection(&curly_core::storage::Collection::new("My API"))
            .unwrap();
        storage
            .save_environment(&curly_core::storage::Environment::new("dev"))
            .unwrap();

        app.refresh_project_lists();

        assert_eq!(app.collections, vec!["my-api".to_string()]);
        assert_eq!(app.environments, vec!["dev".to_string()]);
    }
}
