//! The GUI's slices so far: FR-17's core loop (method/URL bar, Send,
//! Headers/Body panels, a response pane), the start of FR-18 (a project
//! picker plus browsing a collection's tree and clicking a request to load
//! it into the editor), an environment switcher with real `{{variable}}`
//! substitution when sending, and now creating/editing/deleting
//! environments and their variables from the GUI itself (all FR-21). Still
//! not built: `--var key=value`-style ad-hoc overrides, extraction rules
//! writing into a session, saving requests, or multiple simultaneously-open
//! projects (see project.rs's doc comment for why the data model already
//! supports the last one).

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use curly_core::exec::{self, ResponseSummary};
use curly_core::model::{Body, Request};
use curly_core::storage::{
    Collection, Environment, Folder, KvPair, SavedAuth, SavedBody, SavedRequest, Storage, Variable,
};
use curly_core::substitution;
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

struct VariableRow {
    key: String,
    value: String,
    secret: bool,
}

impl VariableRow {
    fn empty() -> Self {
        Self {
            key: String::new(),
            value: String::new(),
            secret: false,
        }
    }
}

impl From<Variable> for VariableRow {
    fn from(v: Variable) -> Self {
        Self {
            key: v.key,
            value: v.value,
            secret: v.secret,
        }
    }
}

/// A single ad-hoc `key=value` override, the GUI's equivalent of `curly
/// run`'s `--var key=value` — applies only to sends made from this editor,
/// never written to disk (no `secret` field: there's nothing to mask when
/// nothing's persisted).
struct OverrideRow {
    key: String,
    value: String,
    enabled: bool,
}

impl OverrideRow {
    fn empty() -> Self {
        Self {
            key: String::new(),
            value: String::new(),
            enabled: true,
        }
    }
}

enum SendOutcome {
    Success(ResponseSummary),
    Error(String),
}

/// A click in the sidebar's Environments section, applied after the
/// rendering loop finishes — same reason `show_collection_tree` defers its
/// click, and `load_saved_request`'s callers do too: acting immediately
/// would borrow `self` mutably while the loop still holds an immutable
/// borrow of `self.environments` to iterate over.
enum EnvAction {
    Select(Option<String>),
    Create(String),
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
    /// Cached listing of the active project's collections (loaded in full —
    /// small enough for now to just read all of them upfront rather than
    /// build a lazy-loading scheme — and environment names, re-read from
    /// disk whenever the active project changes.
    collections: Vec<Collection>,
    environments: Vec<String>,
    /// The environment `{{variable}}` tokens resolve against when sending —
    /// `None` means no environment is selected (only the always-merged-in
    /// "global" environment and the "global"-scoped session apply, matching
    /// `curly run`'s own default when `--env` is omitted). Reset to `None`
    /// whenever the active project changes, since the previous selection's
    /// name may not even exist in the new project.
    active_environment: Option<String>,
    /// The active environment's variables, editable in the sidebar — loaded
    /// from disk whenever `active_environment` changes, written back to
    /// disk only when the user clicks Save (so half-edited rows never leak
    /// into what a Send actually resolves against; `merged_variables`
    /// always reads the saved file, not this buffer).
    env_editor_variables: Vec<VariableRow>,
    /// The "+ New Environment" text field's current contents.
    new_environment_name: String,
    /// Feedback from creating/saving/deleting an environment (success or
    /// error) — separate from `load_notice`, which is about the request
    /// editor, not environment management.
    env_notice: Option<String>,
    /// Ad-hoc `{{variable}}` overrides for this editor's sends only — the
    /// GUI's equivalent of `curly run --var key=value`. Highest precedence
    /// in `merged_variables`, never written to disk, not reset when the
    /// active project or environment changes (same as headers/body — it's
    /// editor state, not project state).
    var_overrides: Vec<OverrideRow>,
    /// Set after loading a saved request whose body/auth couldn't be fully
    /// represented in the editor, or that references a variable undefined
    /// in the currently selected environment — shown once as a notice near
    /// the editor, replaced (or cleared) on the next load.
    load_notice: Option<String>,
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
            active_environment: None,
            env_editor_variables: Vec::new(),
            new_environment_name: String::new(),
            env_notice: None,
            var_overrides: Vec::new(),
            load_notice: None,
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
        self.active_environment = None;
        self.env_editor_variables.clear();
        self.env_notice = None;
        self.refresh_project_lists();
    }

    fn refresh_project_lists(&mut self) {
        let listing = self.active_project_idx.and_then(|i| self.projects.get(i)).map(|p| {
            let collections = p
                .storage
                .list_collections()
                .unwrap_or_default()
                .iter()
                .filter_map(|name| p.storage.load_collection(name).ok())
                .collect::<Vec<_>>();
            let environments = p.storage.list_environments().unwrap_or_default();
            (collections, environments)
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

    /// Change which environment is active — for both `{{variable}}`
    /// resolution (`merged_variables`) and the sidebar's variable editor —
    /// loading its variables from disk into `env_editor_variables`, or
    /// clearing them if `name` is `None`.
    fn select_environment(&mut self, name: Option<String>) {
        self.env_notice = None;
        match &name {
            Some(n) => self.load_environment_editor(n),
            None => self.env_editor_variables.clear(),
        }
        self.active_environment = name;
    }

    fn load_environment_editor(&mut self, name: &str) {
        self.env_editor_variables = match self.active_project() {
            Some(project) => project
                .storage
                .load_environment_opt(name)
                .ok()
                .flatten()
                .map(|env| env.variables.into_iter().map(VariableRow::from).collect())
                .unwrap_or_default(),
            None => Vec::new(),
        };
    }

    /// Create `name` as a new, empty environment, then select it (loading
    /// its — empty — variables into the editor, same as any other
    /// selection). Mirrors `curly env set`'s create-if-missing behavior
    /// (here with zero variables, since there's no KEY=VALUE to seed it
    /// with yet).
    fn create_environment(&mut self, name: String) {
        let result = match self.active_project() {
            Some(project) => project.storage.save_environment(&Environment::new(&name)),
            None => return,
        };
        match result {
            Ok(()) => {
                self.new_environment_name.clear();
                self.refresh_project_lists();
                // select_environment (below) clears env_notice as part of
                // switching selection, so this has to be set after it, not
                // before.
                self.select_environment(Some(name.clone()));
                self.env_notice = Some(format!("created environment \"{name}\""));
            }
            Err(e) => self.env_notice = Some(format!("failed to create \"{name}\": {e}")),
        }
    }

    /// Write `env_editor_variables` to disk as the active environment —
    /// rows with a blank key are skipped (same "blank name is ignored" rule
    /// `build_request` already applies to headers), so an in-progress
    /// half-typed row never gets persisted.
    fn save_active_environment(&mut self) {
        let Some(name) = self.active_environment.clone() else {
            return;
        };
        let mut env = Environment::new(&name);
        for v in &self.env_editor_variables {
            if v.key.trim().is_empty() {
                continue;
            }
            env.variables.push(Variable {
                key: v.key.trim().to_string(),
                value: v.value.clone(),
                secret: v.secret,
            });
        }
        let count = env.variables.len();
        let result = match self.active_project() {
            Some(project) => project.storage.save_environment(&env),
            None => return,
        };
        self.env_notice = Some(match result {
            Ok(()) => format!("saved {count} variable(s) to \"{name}\""),
            Err(e) => format!("failed to save \"{name}\": {e}"),
        });
    }

    /// Delete the active environment entirely and deselect it — mirrors
    /// `curly env delete`, including that it takes effect immediately with
    /// no separate confirmation step.
    fn delete_active_environment(&mut self) {
        let Some(name) = self.active_environment.clone() else {
            return;
        };
        let result = match self.active_project() {
            Some(project) => project.storage.delete_environment(&name),
            None => return,
        };
        match result {
            Ok(()) => {
                self.refresh_project_lists();
                self.active_environment = None;
                self.env_editor_variables.clear();
                self.env_notice = Some(format!("deleted environment \"{name}\""));
            }
            Err(e) => self.env_notice = Some(format!("failed to delete \"{name}\": {e}")),
        }
    }

    /// The variable scope `{{variable}}` tokens resolve against, mirroring
    /// `curly run`'s own precedence exactly (see
    /// `curly-cli/src/commands/run.rs`'s `merged_variables`), low to high:
    /// the always-merged-in "global" environment, the selected environment
    /// (if any), that environment's session (or the "global"-scoped session
    /// if none is selected — the same default `curly run` uses when
    /// `--env` is omitted), then `var_overrides` — the GUI's equivalent of
    /// `--var key=value`, highest precedence, applied whether or not a
    /// project is open. A disabled or blank-key override row is ignored,
    /// same rules `build_request` already applies to headers.
    fn merged_variables(&self) -> BTreeMap<String, String> {
        let mut variables = BTreeMap::new();

        if let Some(project) = self.active_project() {
            if let Ok(Some(global)) = project.storage.load_environment_opt("global") {
                for var in global.variables {
                    variables.insert(var.key, var.value);
                }
            }
            if let Some(name) = &self.active_environment {
                if let Ok(env) = project.storage.load_environment(name) {
                    for var in env.variables {
                        variables.insert(var.key, var.value);
                    }
                }
            }
            let session_scope = self.active_environment.as_deref().unwrap_or("global");
            if let Ok(session) = project.storage.load_session(session_scope) {
                for var in session.variables {
                    variables.insert(var.key, var.value);
                }
            }
        }

        for o in &self.var_overrides {
            let key = o.key.trim();
            if o.enabled && !key.is_empty() {
                variables.insert(key.to_string(), o.value.clone());
            }
        }

        variables
    }

    /// Load a saved request's method/URL/headers into the editor, plus its
    /// body if it's a simple raw text body (the only kind the GUI's editor
    /// can represent so far) and its auth if it's a Bearer token (folded
    /// into an `Authorization` header — the editor has no separate auth
    /// concept yet). Anything that can't be represented, or any
    /// `{{variable}}` token that the currently selected environment doesn't
    /// define, surfaces as `load_notice` rather than being silently dropped
    /// or silently sent literally.
    fn load_saved_request(&mut self, saved: &SavedRequest) {
        self.method = saved.method.clone();
        self.url = saved.url.clone();
        self.headers = saved
            .headers
            .iter()
            .map(|kv: &KvPair| HeaderRow {
                name: kv.name.clone(),
                value: kv.value.clone(),
                enabled: kv.enabled,
            })
            .collect();

        let mut notices: Vec<String> = Vec::new();

        self.body = match &saved.body {
            Some(SavedBody::Raw { content }) => content.clone(),
            Some(_) => {
                notices.push(
                    "its body isn't plain text (form/multipart/binary) — not loaded, \
                     the editor only supports raw text bodies so far"
                        .to_string(),
                );
                String::new()
            }
            None => String::new(),
        };

        match &saved.auth {
            Some(SavedAuth::Bearer { token }) => {
                self.headers
                    .retain(|h| !h.name.eq_ignore_ascii_case("authorization"));
                self.headers.insert(
                    0,
                    HeaderRow {
                        name: "Authorization".to_string(),
                        value: format!("Bearer {token}"),
                        enabled: true,
                    },
                );
            }
            Some(SavedAuth::Basic { .. }) => {
                notices.push(
                    "its Basic auth isn't loaded — add an Authorization header manually if needed"
                        .to_string(),
                );
            }
            None => {}
        }

        if self.headers.is_empty() {
            self.headers.push(HeaderRow::empty());
        }

        // One combined substitution attempt (url + all header values + body,
        // newline-joined) rather than three separate ones, so a request with
        // several undefined variables gets a single notice naming all of
        // them (substitute() already reports every undefined name, not just
        // the first) instead of three overlapping ones.
        let combined = format!(
            "{}\n{}\n{}",
            self.url,
            self.headers.iter().map(|h| h.value.as_str()).collect::<Vec<_>>().join("\n"),
            self.body
        );
        if let Err(e) = substitution::substitute(&combined, &self.merged_variables()) {
            let env_desc = self
                .active_environment
                .as_deref()
                .unwrap_or("no environment selected");
            notices.push(format!(
                "it references variables the current environment ({env_desc}) doesn't define ({e}) — \
                 pick a different environment in the sidebar, or edit the fields by hand, before sending"
            ));
        }

        self.load_notice = if notices.is_empty() {
            None
        } else {
            Some(format!("Loaded \"{}\", but {}.", saved.name, notices.join("; and ")))
        };
        self.response = None;
    }

    fn active_project(&self) -> Option<&Project> {
        self.active_project_idx.and_then(|i| self.projects.get(i))
    }

    /// Builds the request to send, substituting `{{variable}}` tokens
    /// against `merged_variables()` first (FR-21) — same as `curly run`,
    /// an undefined variable fails the whole send with a clear error rather
    /// than going out with a literal `{{token}}` in it.
    fn build_request(&self) -> anyhow::Result<Request> {
        let variables = self.merged_variables();
        let method = Method::from_bytes(self.method.to_uppercase().as_bytes())?;
        let url = substitution::substitute(&self.url, &variables)?;
        let mut request = Request::new(method, url);
        for h in &self.headers {
            if h.enabled && !h.name.trim().is_empty() {
                let value = substitution::substitute(&h.value, &variables)?;
                request = request.with_header(h.name.clone(), value);
            }
        }
        if !self.body.is_empty() {
            let body = substitution::substitute(&self.body, &variables)?;
            request = request.with_body(Body::Raw(body));
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
                let mut clicked_request: Option<SavedRequest> = None;
                for collection in &self.collections {
                    egui::CollapsingHeader::new(&collection.name)
                        .id_salt(collection.id)
                        .show(ui, |ui| {
                            if let Some(r) =
                                show_collection_tree(ui, &collection.folders, &collection.requests)
                            {
                                clicked_request = Some(r);
                            }
                        });
                }
                if let Some(saved) = clicked_request {
                    self.load_saved_request(&saved);
                }

                ui.separator();

                ui.label(format!("Environments ({})", self.environments.len()));

                let mut env_action: Option<EnvAction> = None;
                if ui
                    .selectable_label(self.active_environment.is_none(), "  (none selected)")
                    .clicked()
                {
                    env_action = Some(EnvAction::Select(None));
                }
                for name in &self.environments {
                    let selected = self.active_environment.as_deref() == Some(name.as_str());
                    if ui.selectable_label(selected, format!("  {name}")).clicked() {
                        env_action = Some(EnvAction::Select(Some(name.clone())));
                    }
                }

                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.new_environment_name)
                            .hint_text("new environment")
                            .desired_width(120.0),
                    );
                    let name = self.new_environment_name.trim().to_string();
                    if ui.add_enabled(!name.is_empty(), egui::Button::new("+ New")).clicked() {
                        env_action = Some(EnvAction::Create(name));
                    }
                });

                if let Some(action) = env_action {
                    match action {
                        EnvAction::Select(name) => self.select_environment(name),
                        EnvAction::Create(name) => self.create_environment(name),
                    }
                }

                if let Some(notice) = &self.env_notice {
                    ui.colored_label(egui::Color32::from_rgb(120, 190, 130), notice);
                }

                if let Some(name) = self.active_environment.clone() {
                    egui::CollapsingHeader::new(format!("Edit \"{name}\""))
                        .default_open(true)
                        .show(ui, |ui| {
                            let mut remove_idx = None;
                            for (i, v) in self.env_editor_variables.iter_mut().enumerate() {
                                ui.horizontal(|ui| {
                                    ui.add(
                                        egui::TextEdit::singleline(&mut v.key)
                                            .hint_text("KEY")
                                            .desired_width(70.0),
                                    );
                                    ui.add(
                                        egui::TextEdit::singleline(&mut v.value)
                                            .hint_text("value")
                                            .desired_width(80.0),
                                    );
                                    ui.checkbox(&mut v.secret, "secret");
                                    if ui.small_button("✕").clicked() {
                                        remove_idx = Some(i);
                                    }
                                });
                            }
                            if let Some(i) = remove_idx {
                                self.env_editor_variables.remove(i);
                            }
                            if ui.button("+ Add variable").clicked() {
                                self.env_editor_variables.push(VariableRow::empty());
                            }
                            ui.horizontal(|ui| {
                                if ui.button("Save").clicked() {
                                    self.save_active_environment();
                                }
                                if ui.button("Delete environment").clicked() {
                                    self.delete_active_environment();
                                }
                            });
                        });
                }
            });

        egui::CentralPanel::default().show(ui, |ui| {
            let ctx = ui.ctx().clone();
            let mut should_send = false;

            if let Some(notice) = &self.load_notice {
                ui.colored_label(egui::Color32::from_rgb(230, 190, 60), notice);
            }

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

            let override_count = self
                .var_overrides
                .iter()
                .filter(|o| o.enabled && !o.key.trim().is_empty())
                .count();
            egui::CollapsingHeader::new(format!("Variable overrides ({override_count})"))
                .default_open(false)
                .show(ui, |ui| {
                    ui.weak(
                        "Highest precedence — the GUI's equivalent of `curly run --var key=value`. \
                         Overrides the selected environment and its session for this send only; never saved to disk.",
                    );
                    let mut remove_idx = None;
                    for (i, o) in self.var_overrides.iter_mut().enumerate() {
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut o.enabled, "");
                            ui.add(
                                egui::TextEdit::singleline(&mut o.key)
                                    .hint_text("KEY")
                                    .desired_width(140.0),
                            );
                            ui.add(
                                egui::TextEdit::singleline(&mut o.value)
                                    .hint_text("value")
                                    .desired_width(240.0),
                            );
                            if ui.small_button("✕").clicked() {
                                remove_idx = Some(i);
                            }
                        });
                    }
                    if let Some(i) = remove_idx {
                        self.var_overrides.remove(i);
                    }
                    if ui.button("+ Add override").clicked() {
                        self.var_overrides.push(OverrideRow::empty());
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

/// Render a collection's (or folder's) sub-folders and requests, recursing
/// into each sub-folder as its own collapsing section. Returns the request
/// clicked this frame, if any — a free function rather than a method since
/// it only needs the tree data, not the rest of `CurlyApp`.
fn show_collection_tree(
    ui: &mut egui::Ui,
    folders: &[Folder],
    requests: &[SavedRequest],
) -> Option<SavedRequest> {
    let mut clicked = None;

    for folder in folders {
        egui::CollapsingHeader::new(format!("{}/", folder.name))
            .id_salt(folder.id)
            .show(ui, |ui| {
                if let Some(r) = show_collection_tree(ui, &folder.folders, &folder.requests) {
                    clicked = Some(r);
                }
            });
    }

    for request in requests {
        let label = format!("{:<7} {}", request.method, request.name);
        if ui.selectable_label(false, label).clicked() {
            clicked = Some(request.clone());
        }
    }

    clicked
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

        assert_eq!(app.collections.len(), 1);
        assert_eq!(app.collections[0].name, "Existing");
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

        assert_eq!(app.collections.len(), 1);
        assert_eq!(app.collections[0].name, "My API");
        assert_eq!(app.environments, vec!["dev".to_string()]);
    }

    #[test]
    fn load_saved_request_carries_method_url_headers() {
        let mut app = CurlyApp::default_state();
        let mut saved = SavedRequest::new("get-me", Method::GET, "https://example.com/me");
        saved.headers.push(KvPair::new("Accept", "application/json"));

        app.load_saved_request(&saved);

        assert_eq!(app.method, "GET");
        assert_eq!(app.url, "https://example.com/me");
        assert_eq!(app.headers.len(), 1);
        assert_eq!(app.headers[0].name, "Accept");
        assert_eq!(app.headers[0].value, "application/json");
        assert!(app.load_notice.is_none());
    }

    #[test]
    fn load_saved_request_loads_raw_body() {
        let mut app = CurlyApp::default_state();
        let mut saved = SavedRequest::new("create", Method::POST, "https://example.com");
        saved.body = Some(SavedBody::Raw {
            content: r#"{"a":1}"#.to_string(),
        });

        app.load_saved_request(&saved);

        assert_eq!(app.body, r#"{"a":1}"#);
        assert!(app.load_notice.is_none());
    }

    #[test]
    fn load_saved_request_notices_unsupported_body_type() {
        let mut app = CurlyApp::default_state();
        let mut saved = SavedRequest::new("upload", Method::POST, "https://example.com");
        saved.body = Some(SavedBody::Form {
            fields: vec![("a".to_string(), "b".to_string())],
        });

        app.load_saved_request(&saved);

        assert!(app.body.is_empty());
        assert!(app.load_notice.unwrap().contains("body isn't plain text"));
    }

    #[test]
    fn load_saved_request_folds_bearer_auth_into_header() {
        let mut app = CurlyApp::default_state();
        let mut saved = SavedRequest::new("get-me", Method::GET, "https://example.com");
        saved.auth = Some(SavedAuth::Bearer {
            token: "abc123".to_string(),
        });

        app.load_saved_request(&saved);

        assert_eq!(app.headers.len(), 1);
        assert_eq!(app.headers[0].name, "Authorization");
        assert_eq!(app.headers[0].value, "Bearer abc123");
        assert!(app.load_notice.is_none());
    }

    #[test]
    fn load_saved_request_notices_basic_auth_not_loaded() {
        let mut app = CurlyApp::default_state();
        let mut saved = SavedRequest::new("get-me", Method::GET, "https://example.com");
        saved.auth = Some(SavedAuth::Basic {
            username: "alice".to_string(),
            password: "s3cret".to_string(),
        });

        app.load_saved_request(&saved);

        assert!(app.headers.iter().all(|h| h.name != "Authorization"));
        assert!(app.load_notice.unwrap().contains("Basic auth"));
    }

    #[test]
    fn load_saved_request_notices_variables_undefined_in_current_environment() {
        let mut app = CurlyApp::default_state();
        let saved = SavedRequest::new("get-user", Method::GET, "{{BASE_URL}}/users/{{ID}}");

        app.load_saved_request(&saved);

        let notice = app.load_notice.unwrap();
        assert!(notice.contains("BASE_URL"));
        assert!(notice.contains("ID"));
        assert!(notice.contains("no environment selected"));
    }

    #[test]
    fn load_saved_request_clears_previous_response() {
        let mut app = CurlyApp::default_state();
        app.response = Some(Ok(ResponseSummary {
            status: 200,
            headers: vec![],
            body: "stale".to_string(),
            elapsed: Duration::from_millis(1),
        }));
        let saved = SavedRequest::new("get-me", Method::GET, "https://example.com");

        app.load_saved_request(&saved);

        assert!(app.response.is_none());
    }

    // --- FR-21: environment switcher + {{variable}} substitution ---

    fn project_with(dir: &tempfile::TempDir) -> CurlyApp {
        let mut app = CurlyApp::default_state();
        app.open_project_at(dir.path());
        app
    }

    #[test]
    fn merged_variables_is_empty_with_no_project_open() {
        let app = CurlyApp::default_state();
        assert!(app.merged_variables().is_empty());
    }

    #[test]
    fn merged_variables_includes_the_global_environment_even_when_unselected() {
        let dir = tempfile::tempdir().unwrap();
        let app = project_with(&dir);

        let mut global = Environment::new("global");
        global.variables.push(Variable {
            key: "HOST".to_string(),
            value: "global.example.com".to_string(),
            secret: false,
        });
        app.active_project().unwrap().storage.save_environment(&global).unwrap();

        let variables = app.merged_variables();
        assert_eq!(variables.get("HOST"), Some(&"global.example.com".to_string()));
    }

    #[test]
    fn merged_variables_selected_environment_overrides_global() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = project_with(&dir);
        let storage = &app.active_project().unwrap().storage;

        let mut global = Environment::new("global");
        global.variables.push(Variable {
            key: "HOST".to_string(),
            value: "global.example.com".to_string(),
            secret: false,
        });
        storage.save_environment(&global).unwrap();

        let mut dev = Environment::new("dev");
        dev.variables.push(Variable {
            key: "HOST".to_string(),
            value: "dev.example.com".to_string(),
            secret: false,
        });
        storage.save_environment(&dev).unwrap();

        app.active_environment = Some("dev".to_string());

        assert_eq!(
            app.merged_variables().get("HOST"),
            Some(&"dev.example.com".to_string())
        );
    }

    #[test]
    fn merged_variables_reads_the_selected_environments_session() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = project_with(&dir);
        let storage = &app.active_project().unwrap().storage;

        let mut session = Environment::new("dev");
        session.set("TOKEN", "abc123", true);
        storage.save_session(&session).unwrap();

        app.active_environment = Some("dev".to_string());

        assert_eq!(
            app.merged_variables().get("TOKEN"),
            Some(&"abc123".to_string())
        );
    }

    #[test]
    fn merged_variables_falls_back_to_the_global_session_when_no_environment_selected() {
        let dir = tempfile::tempdir().unwrap();
        let app = project_with(&dir);
        let storage = &app.active_project().unwrap().storage;

        let mut session = Environment::new("global");
        session.set("TOKEN", "xyz", true);
        storage.save_session(&session).unwrap();

        assert_eq!(app.merged_variables().get("TOKEN"), Some(&"xyz".to_string()));
    }

    #[test]
    fn build_request_substitutes_variables_from_the_selected_environment() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = project_with(&dir);
        let mut dev = Environment::new("dev");
        dev.variables.push(Variable {
            key: "HOST".to_string(),
            value: "api.example.com".to_string(),
            secret: false,
        });
        app.active_project().unwrap().storage.save_environment(&dev).unwrap();
        app.active_environment = Some("dev".to_string());

        app.url = "https://{{HOST}}/get".to_string();
        app.headers = vec![HeaderRow {
            name: "X-Env".to_string(),
            value: "{{HOST}}".to_string(),
            enabled: true,
        }];
        app.body = r#"{"host":"{{HOST}}"}"#.to_string();

        let request = app.build_request().unwrap();

        assert_eq!(request.url, "https://api.example.com/get");
        assert_eq!(request.headers[0].1, "api.example.com");
        assert!(matches!(request.body, Some(Body::Raw(ref s)) if s.contains("api.example.com")));
    }

    #[test]
    fn build_request_errors_on_a_variable_undefined_in_the_current_environment() {
        let mut app = CurlyApp::default_state();
        app.url = "https://{{HOST}}/get".to_string();

        let err = app.build_request().unwrap_err();
        assert!(err.to_string().contains("HOST"));
    }

    #[test]
    fn switching_the_active_environment_changes_what_a_send_resolves_to() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = project_with(&dir);
        let storage = &app.active_project().unwrap().storage;

        let mut dev = Environment::new("dev");
        dev.variables.push(Variable {
            key: "HOST".to_string(),
            value: "dev.example.com".to_string(),
            secret: false,
        });
        storage.save_environment(&dev).unwrap();

        let mut prod = Environment::new("prod");
        prod.variables.push(Variable {
            key: "HOST".to_string(),
            value: "prod.example.com".to_string(),
            secret: false,
        });
        storage.save_environment(&prod).unwrap();

        app.url = "https://{{HOST}}/get".to_string();

        app.active_environment = Some("dev".to_string());
        assert_eq!(app.build_request().unwrap().url, "https://dev.example.com/get");

        app.active_environment = Some("prod".to_string());
        assert_eq!(app.build_request().unwrap().url, "https://prod.example.com/get");
    }

    #[test]
    fn opening_a_new_project_resets_the_active_environment() {
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        let mut app = project_with(&dir_a);
        app.active_environment = Some("dev".to_string());

        app.open_project_at(dir_b.path());

        assert!(app.active_environment.is_none());
    }

    #[test]
    fn load_saved_request_notice_clears_once_the_selected_environment_defines_the_variable() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = project_with(&dir);
        let mut dev = Environment::new("dev");
        dev.variables.push(Variable {
            key: "BASE_URL".to_string(),
            value: "https://api.example.com".to_string(),
            secret: false,
        });
        app.active_project().unwrap().storage.save_environment(&dev).unwrap();
        app.active_environment = Some("dev".to_string());

        let saved = SavedRequest::new("get-user", Method::GET, "{{BASE_URL}}/users/1");
        app.load_saved_request(&saved);

        assert!(app.load_notice.is_none());
    }

    // --- FR-21: creating/editing/deleting environments from the GUI ---

    #[test]
    fn create_environment_makes_it_selectable_and_selects_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = project_with(&dir);

        app.create_environment("staging".to_string());

        assert_eq!(app.environments, vec!["staging".to_string()]);
        assert_eq!(app.active_environment.as_deref(), Some("staging"));
        assert!(app.env_editor_variables.is_empty());
        assert!(app.env_notice.unwrap().contains("staging"));
    }

    #[test]
    fn create_environment_clears_the_name_field() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = project_with(&dir);
        app.new_environment_name = "staging".to_string();

        app.create_environment("staging".to_string());

        assert!(app.new_environment_name.is_empty());
    }

    #[test]
    fn selecting_an_environment_loads_its_variables_into_the_editor() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = project_with(&dir);
        let mut dev = Environment::new("dev");
        dev.variables.push(Variable {
            key: "HOST".to_string(),
            value: "dev.example.com".to_string(),
            secret: false,
        });
        app.active_project().unwrap().storage.save_environment(&dev).unwrap();

        app.select_environment(Some("dev".to_string()));

        assert_eq!(app.env_editor_variables.len(), 1);
        assert_eq!(app.env_editor_variables[0].key, "HOST");
        assert_eq!(app.env_editor_variables[0].value, "dev.example.com");
    }

    #[test]
    fn deselecting_an_environment_clears_the_editor() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = project_with(&dir);
        app.create_environment("dev".to_string());
        app.env_editor_variables.push(VariableRow::empty());

        app.select_environment(None);

        assert!(app.active_environment.is_none());
        assert!(app.env_editor_variables.is_empty());
    }

    #[test]
    fn save_active_environment_persists_the_editors_variables() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = project_with(&dir);
        app.create_environment("dev".to_string());
        app.env_editor_variables.push(VariableRow {
            key: "TOKEN".to_string(),
            value: "abc123".to_string(),
            secret: true,
        });

        app.save_active_environment();

        let saved = app.active_project().unwrap().storage.load_environment("dev").unwrap();
        assert_eq!(saved.variables.len(), 1);
        assert_eq!(saved.variables[0].key, "TOKEN");
        assert_eq!(saved.variables[0].value, "abc123");
        assert!(saved.variables[0].secret);
        assert!(app.env_notice.unwrap().contains('1'));
    }

    #[test]
    fn save_active_environment_skips_rows_with_a_blank_key() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = project_with(&dir);
        app.create_environment("dev".to_string());
        app.env_editor_variables.push(VariableRow {
            key: "   ".to_string(),
            value: "ignored".to_string(),
            secret: false,
        });

        app.save_active_environment();

        let saved = app.active_project().unwrap().storage.load_environment("dev").unwrap();
        assert!(saved.variables.is_empty());
    }

    #[test]
    fn saved_variables_are_immediately_usable_by_merged_variables() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = project_with(&dir);
        app.create_environment("dev".to_string());
        app.env_editor_variables.push(VariableRow {
            key: "HOST".to_string(),
            value: "dev.example.com".to_string(),
            secret: false,
        });

        app.save_active_environment();

        assert_eq!(
            app.merged_variables().get("HOST"),
            Some(&"dev.example.com".to_string())
        );
    }

    #[test]
    fn delete_active_environment_removes_it_and_deselects() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = project_with(&dir);
        app.create_environment("dev".to_string());

        app.delete_active_environment();

        assert!(app.active_environment.is_none());
        assert!(app.env_editor_variables.is_empty());
        assert!(app.environments.is_empty());
        assert!(app
            .active_project()
            .unwrap()
            .storage
            .load_environment("dev")
            .is_err());
        assert!(app.env_notice.unwrap().contains("dev"));
    }

    #[test]
    fn save_active_environment_is_a_no_op_with_no_environment_selected() {
        let mut app = CurlyApp::default_state();
        app.save_active_environment();
        assert!(app.env_notice.is_none());
    }

    #[test]
    fn delete_active_environment_is_a_no_op_with_no_environment_selected() {
        let mut app = CurlyApp::default_state();
        app.delete_active_environment();
        assert!(app.env_notice.is_none());
    }

    // --- FR-21: --var-style ad-hoc overrides ---

    #[test]
    fn var_override_applies_with_no_project_or_environment_at_all() {
        let mut app = CurlyApp::default_state();
        app.var_overrides.push(OverrideRow {
            key: "HOST".to_string(),
            value: "example.com".to_string(),
            enabled: true,
        });

        assert_eq!(
            app.merged_variables().get("HOST"),
            Some(&"example.com".to_string())
        );
    }

    #[test]
    fn var_override_takes_precedence_over_the_selected_environment() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = project_with(&dir);
        let mut dev = Environment::new("dev");
        dev.variables.push(Variable {
            key: "HOST".to_string(),
            value: "dev.example.com".to_string(),
            secret: false,
        });
        app.active_project().unwrap().storage.save_environment(&dev).unwrap();
        app.active_environment = Some("dev".to_string());
        app.var_overrides.push(OverrideRow {
            key: "HOST".to_string(),
            value: "overridden.example.com".to_string(),
            enabled: true,
        });

        assert_eq!(
            app.merged_variables().get("HOST"),
            Some(&"overridden.example.com".to_string())
        );
    }

    #[test]
    fn var_override_with_a_blank_key_is_ignored() {
        let mut app = CurlyApp::default_state();
        app.var_overrides.push(OverrideRow {
            key: "   ".to_string(),
            value: "ignored".to_string(),
            enabled: true,
        });

        assert!(app.merged_variables().is_empty());
    }

    #[test]
    fn disabled_var_override_is_ignored() {
        let mut app = CurlyApp::default_state();
        app.var_overrides.push(OverrideRow {
            key: "HOST".to_string(),
            value: "example.com".to_string(),
            enabled: false,
        });

        assert!(app.merged_variables().is_empty());
    }

    #[test]
    fn re_enabling_a_var_override_makes_it_apply_again() {
        let mut app = CurlyApp::default_state();
        app.var_overrides.push(OverrideRow {
            key: "HOST".to_string(),
            value: "example.com".to_string(),
            enabled: false,
        });
        assert!(app.merged_variables().is_empty());

        app.var_overrides[0].enabled = true;

        assert_eq!(
            app.merged_variables().get("HOST"),
            Some(&"example.com".to_string())
        );
    }

    #[test]
    fn build_request_uses_a_var_override() {
        let mut app = CurlyApp::default_state();
        app.url = "https://{{HOST}}/get".to_string();
        app.var_overrides.push(OverrideRow {
            key: "HOST".to_string(),
            value: "example.com".to_string(),
            enabled: true,
        });

        let request = app.build_request().unwrap();
        assert_eq!(request.url, "https://example.com/get");
    }

    #[test]
    fn load_saved_request_notice_clears_when_a_var_override_defines_the_missing_variable() {
        let mut app = CurlyApp::default_state();
        app.var_overrides.push(OverrideRow {
            key: "BASE_URL".to_string(),
            value: "https://api.example.com".to_string(),
            enabled: true,
        });

        let saved = SavedRequest::new("get-user", Method::GET, "{{BASE_URL}}/users/1");
        app.load_saved_request(&saved);

        assert!(app.load_notice.is_none());
    }
}
