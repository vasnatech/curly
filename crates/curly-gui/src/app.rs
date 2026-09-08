//! The GUI's slices so far: FR-17's core loop (method/URL bar, Send,
//! Headers/Body panels, a response pane), the start of FR-18 (a project
//! picker plus browsing a collection's tree and clicking a request to load
//! it into the editor), and all of FR-21 — an environment switcher,
//! creating/editing/deleting environments, ad-hoc `--var`-style overrides,
//! and now a loaded request's extraction rules running on a successful send
//! and writing into a session, same as `curly run`. Still not built: saving
//! requests, or multiple simultaneously-open projects (see project.rs's doc
//! comment for why the data model already supports the last one).

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use curly_core::exec::{self, ResponseSummary};
use curly_core::extraction::{self, Extraction};
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

/// Captured at `send()` time (before the request even goes out) rather than
/// recomputed once the response lands, so extraction runs against exactly
/// the same variable scope the request itself was substituted against —
/// mirroring `curly-cli`'s own `run.rs`, which captures `variables` once and
/// reuses it in its extraction closure rather than re-deriving it.
struct PendingExtraction {
    rules: Vec<Extraction>,
    variables: BTreeMap<String, String>,
    /// Which session to write into — the active environment's name, or
    /// "global" if none is selected, matching `curly run`'s own
    /// `session_scope` default.
    session_scope: String,
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

/// Identifies where a loaded request lives, so a plain "Save" knows what to
/// overwrite without re-asking — the collection's name plus its
/// slash-separated path within that collection (e.g. `"Auth/OAuth/login"`,
/// matching `Collection::find_request`'s own path convention exactly; not
/// including the collection name itself).
#[derive(Clone, Debug, PartialEq, Eq)]
struct LoadedRequestRef {
    collection: String,
    path: String,
}

/// A click in the sidebar's Collections tree, applied after the rendering
/// loop finishes (same deferred-action reason as `EnvAction`) — either load
/// a request into the editor, or delete it from its collection.
enum TreeAction {
    Load(String, SavedRequest),
    Delete(String),
}

/// State for the "Save Request" floating window (FR-20) — collection/folder
/// path/name are free text rather than, say, a dropdown limited to existing
/// collections, since typing a new collection or folder name is exactly how
/// you create one (mirrors `curly collections add-request`'s own
/// create-if-missing behavior).
struct SaveDialog {
    collection: String,
    folder_path: String,
    name: String,
    error: Option<String>,
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
    /// The "Open Project…" folder picker's result channel, `Some` while a
    /// pick is in flight — polled each frame like `rx`/`poll_response`. The
    /// dialog itself runs on `runtime` via `rfd::AsyncFileDialog` rather
    /// than the blocking `rfd::FileDialog`, which would freeze the whole
    /// event loop (and every window's redraw with it) for as long as the OS
    /// dialog stays open — long enough that GNOME's compositor reports the
    /// window as "Not Responding", even though the app isn't actually
    /// hung, just not pumping events while stuck in a synchronous call.
    project_dialog_rx: Option<mpsc::Receiver<Option<std::path::PathBuf>>>,
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
    /// Read-only display of `session_scope()`'s session variables — the
    /// ones extraction writes into, distinct from `env_editor_variables`
    /// (the environment *file*, editable). Refreshed whenever the
    /// environment selection changes or extraction just wrote into it, so
    /// what an extracted variable actually landed as is visible somewhere
    /// in the GUI rather than only discoverable via `curly session show`.
    session_variables: Vec<Variable>,
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
    /// The currently loaded saved request's extraction rules (FR-10), if
    /// any — carried over by `load_saved_request`, empty for a request
    /// built from scratch. Applied on a successful send via
    /// `pending_extraction`, same as `curly run`.
    extract_rules: Vec<Extraction>,
    /// Set by `send()` right before the request goes out (if
    /// `extract_rules` is non-empty), consumed by `poll_response` once the
    /// response lands. `None` in between sends, and `None` for a send with
    /// nothing to extract — extraction is skip-if-nothing-to-do, not a
    /// mandatory step.
    pending_extraction: Option<PendingExtraction>,
    /// Feedback from applying extraction rules after a send (success or
    /// error) — separate from `env_notice`/`load_notice`.
    session_notice: Option<String>,
    /// Set after loading a saved request whose body/auth couldn't be fully
    /// represented in the editor, or that references a variable undefined
    /// in the currently selected environment — shown once as a notice near
    /// the editor, replaced (or cleared) on the next load.
    load_notice: Option<String>,
    /// Which saved request (if any) the editor currently mirrors — set by
    /// clicking a request in the Collections tree, and by a successful Save
    /// (new or overwrite). `None` for a request built from scratch or after
    /// deleting the loaded request out from under itself. Drives whether
    /// plain "Save" overwrites in place or falls back to the "Save As…"
    /// dialog (FR-20).
    loaded_request: Option<LoadedRequestRef>,
    /// Open while the "Save Request" window is showing; `None` otherwise.
    save_dialog: Option<SaveDialog>,
    /// Feedback from saving/deleting a request in a collection — separate
    /// from `load_notice` (about the editor's own content) and
    /// `env_notice`/`session_notice` (environments/sessions).
    collection_notice: Option<String>,
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
    ///
    /// Also seeds the theme from the OS's light/dark preference where it can
    /// be detected (FR-24) — see `detect_system_theme`'s doc comment for why
    /// egui's own built-in "follow the system theme" doesn't work on Linux.
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        if let Some(theme) = detect_system_theme() {
            cc.egui_ctx.set_theme(theme);
        }

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
            project_dialog_rx: None,
            collections: Vec::new(),
            environments: Vec::new(),
            active_environment: None,
            session_variables: Vec::new(),
            env_editor_variables: Vec::new(),
            new_environment_name: String::new(),
            env_notice: None,
            var_overrides: Vec::new(),
            extract_rules: Vec::new(),
            pending_extraction: None,
            session_notice: None,
            load_notice: None,
            loaded_request: None,
            save_dialog: None,
            collection_notice: None,
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

    /// Show the native folder picker on `runtime` without blocking the
    /// event loop — see `project_dialog_rx`'s doc comment for why the
    /// blocking `rfd::FileDialog` API isn't used here.
    fn open_project_dialog(&mut self, ctx: &egui::Context) {
        let (tx, rx) = mpsc::channel();
        self.project_dialog_rx = Some(rx);
        let ctx = ctx.clone();
        self.runtime.spawn(async move {
            let picked = rfd::AsyncFileDialog::new()
                .pick_folder()
                .await
                .map(|handle| handle.path().to_path_buf());
            let _ = tx.send(picked);
            ctx.request_repaint();
        });
    }

    /// Polled each frame, like `poll_response` — opens the picked directory
    /// as a project once the async dialog resolves. A cancelled dialog
    /// (`None`) is a no-op: the previously active project, if any, stays
    /// active.
    fn poll_project_dialog(&mut self) {
        let Some(rx) = &self.project_dialog_rx else {
            return;
        };
        if let Ok(picked) = rx.try_recv() {
            self.project_dialog_rx = None;
            if let Some(dir) = picked {
                self.open_project_at(&dir);
            }
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
        self.refresh_session_variables();
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
        self.refresh_session_variables();
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
            if let Ok(session) = project.storage.load_session(self.session_scope()) {
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
    /// can represent so far), its auth if it's a Bearer token (folded into
    /// an `Authorization` header — the editor has no separate auth concept
    /// yet), and its extraction rules (carried over as-is into
    /// `extract_rules` — there's no GUI editor for these yet, only `curly
    /// collections add-request --extract-*` defines them, but a loaded
    /// request's rules run on a successful send the same as `curly run`'s
    /// do). Anything that can't be represented, or any `{{variable}}` token
    /// that the currently selected environment doesn't define, surfaces as
    /// `load_notice` rather than being silently dropped or silently sent
    /// literally.
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
        self.extract_rules = saved.extract.clone();
        self.session_notice = None;

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

    /// Which session `merged_variables`/extraction read from and write
    /// into — the active environment's name, or "global" if none is
    /// selected. Matches `curly run`'s own `session_scope` default exactly.
    fn session_scope(&self) -> &str {
        self.active_environment.as_deref().unwrap_or("global")
    }

    /// Re-read `session_variables` (the read-only sidebar display) from
    /// disk for the current `session_scope()` — called whenever the
    /// environment selection changes or a send just wrote new variables
    /// into the session, so the display never goes stale without an
    /// explicit reload step.
    fn refresh_session_variables(&mut self) {
        self.session_variables = match self.active_project() {
            Some(project) => project
                .storage
                .load_session(self.session_scope())
                .map(|s| s.variables)
                .unwrap_or_default(),
            None => Vec::new(),
        };
    }

    /// Delete the current `session_scope()`'s session entirely — mirrors
    /// `curly session clear`. A no-op (not an error) if there was nothing
    /// to clear, same as the storage layer it calls.
    fn clear_session(&mut self) {
        let scope = self.session_scope().to_string();
        let result = match self.active_project() {
            Some(project) => project.storage.clear_session(&scope),
            None => return,
        };
        match result {
            Ok(()) => {
                self.session_notice = Some(format!("cleared session \"{scope}\""));
                self.refresh_session_variables();
            }
            Err(e) => self.session_notice = Some(format!("failed to clear session \"{scope}\": {e}")),
        }
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

        // Captured now, before the request goes out, not recomputed once
        // the response lands — see PendingExtraction's doc comment.
        self.pending_extraction = if self.extract_rules.is_empty() {
            None
        } else {
            Some(PendingExtraction {
                rules: self.extract_rules.clone(),
                variables: self.merged_variables(),
                session_scope: self.session_scope().to_string(),
            })
        };
        self.session_notice = None;

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
            match outcome {
                SendOutcome::Success(r) => {
                    self.apply_pending_extraction(&r);
                    self.response = Some(Ok(r));
                }
                SendOutcome::Error(e) => {
                    self.pending_extraction = None;
                    self.response = Some(Err(e));
                }
            }
            self.rx = None;
        }
    }

    /// Run this send's captured extraction rules (if any) against the
    /// response that just landed, and write the results into the right
    /// session — same "only on a 2xx, a failing rule on a real success is a
    /// real problem, an unmet rule on any other status is an expected
    /// absence" behavior as `curly run`'s own extraction closure. Sets
    /// `session_notice` either way (success or failure) so extraction
    /// happening is never silent, matching the rest of this app's "no
    /// silent failure" pattern.
    fn apply_pending_extraction(&mut self, response: &ResponseSummary) {
        let Some(pending) = self.pending_extraction.take() else {
            return;
        };
        if !(200..300).contains(&response.status) {
            return;
        }

        let extracted = match extraction::apply(&pending.rules, response, &pending.variables) {
            Ok(e) => e,
            Err(e) => {
                self.session_notice = Some(format!("extraction failed: {e}"));
                return;
            }
        };

        let result: anyhow::Result<()> = match self.active_project() {
            Some(project) => (|| {
                let mut session = project.storage.load_session(&pending.session_scope)?;
                for var in &extracted {
                    session.set(var.name.as_str(), var.value.as_str(), var.secret);
                }
                project.storage.save_session(&session)
            })(),
            None => Err(anyhow::anyhow!("no project is open")),
        };

        let message = match &result {
            Ok(()) => {
                let names: Vec<&str> = extracted.iter().map(|v| v.name.as_str()).collect();
                format!(
                    "extracted {} into session \"{}\"",
                    names.join(", "),
                    pending.session_scope
                )
            }
            Err(e) => format!("extracted variable(s) but failed to save the session: {e}"),
        };
        if result.is_ok() {
            self.refresh_session_variables();
        }
        self.session_notice = Some(message);
    }

    /// Build a `SavedRequest` from the editor's current fields (FR-20) — a
    /// disabled or blank-name header is dropped, same rule `build_request`
    /// already applies when actually sending; extraction rules carry over
    /// as-is from `extract_rules` (so editing a loaded request and saving
    /// it back keeps whatever rules it already had). No auth is derived
    /// from the headers — the editor has no separate auth concept, so an
    /// `Authorization` header saves as a plain header, not `SavedAuth`.
    fn build_saved_request(&self, name: &str) -> anyhow::Result<SavedRequest> {
        let method = Method::from_bytes(self.method.to_uppercase().as_bytes())?;
        let mut saved = SavedRequest::new(name, method, self.url.clone());
        saved.headers = self
            .headers
            .iter()
            .filter(|h| !h.name.trim().is_empty())
            .map(|h| KvPair {
                name: h.name.clone(),
                value: h.value.clone(),
                enabled: h.enabled,
            })
            .collect();
        saved.body = if self.body.is_empty() {
            None
        } else {
            Some(SavedBody::Raw {
                content: self.body.clone(),
            })
        };
        saved.extract = self.extract_rules.clone();
        Ok(saved)
    }

    /// Reset the editor to a blank state, without touching the active
    /// project/environment/overrides — the counterpart to loading a saved
    /// request, so starting a new one doesn't risk a later "Save"
    /// overwriting whatever was loaded before.
    fn new_request(&mut self) {
        self.method = "GET".to_string();
        self.url.clear();
        self.headers = vec![HeaderRow::empty()];
        self.body.clear();
        self.extract_rules.clear();
        self.loaded_request = None;
        self.load_notice = None;
        self.response = None;
    }

    /// Pre-fill the Save dialog from `loaded_request` if there is one (so
    /// "Save As…" on a loaded request defaults to *its* location — change
    /// the name or folder to save a copy instead of overwriting), or from
    /// the first available collection otherwise.
    fn open_save_dialog(&mut self) {
        let (collection, folder_path, name) = match &self.loaded_request {
            Some(loaded) => {
                let (folder, name) = split_folder_and_name(&loaded.path);
                (loaded.collection.clone(), folder, name)
            }
            None => (
                self.collections.first().map(|c| c.name.clone()).unwrap_or_default(),
                String::new(),
                String::new(),
            ),
        };
        self.save_dialog = Some(SaveDialog {
            collection,
            folder_path,
            name,
            error: None,
        });
    }

    fn cancel_save_dialog(&mut self) {
        self.save_dialog = None;
    }

    /// Render the "Save Request" floating window if `save_dialog` is
    /// `Some`. Kept to local `confirmed`/`cancelled` flags rather than
    /// calling `confirm_save_dialog`/`cancel_save_dialog` from inside the
    /// closure below — that closure only needs `dialog` (already a
    /// `&mut SaveDialog`), and acting on `self` while `self.save_dialog`
    /// is still mutably borrowed for `dialog` wouldn't borrow-check.
    fn show_save_dialog(&mut self, ctx: &egui::Context) {
        let mut confirmed = false;
        let mut cancelled = false;

        if let Some(dialog) = self.save_dialog.as_mut() {
            let mut open = true;
            egui::Window::new("Save Request")
                .collapsible(false)
                .resizable(false)
                .open(&mut open)
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Collection:");
                        ui.text_edit_singleline(&mut dialog.collection);
                    });
                    ui.horizontal(|ui| {
                        ui.label("Folder (optional):");
                        ui.add(
                            egui::TextEdit::singleline(&mut dialog.folder_path).hint_text("Auth/OAuth"),
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.label("Name:");
                        ui.text_edit_singleline(&mut dialog.name);
                    });
                    if let Some(err) = &dialog.error {
                        ui.colored_label(egui::Color32::from_rgb(220, 80, 80), err);
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Save").clicked() {
                            confirmed = true;
                        }
                        if ui.button("Cancel").clicked() {
                            cancelled = true;
                        }
                    });
                });
            if !open {
                cancelled = true;
            }
        }

        if confirmed {
            self.confirm_save_dialog();
        } else if cancelled {
            self.cancel_save_dialog();
        }
    }

    /// Save the dialog's collection/folder/name as a *new* request — always
    /// creates, mirroring `Collection::add_request`'s own "errors if a
    /// request already exists at that exact path" behavior rather than
    /// silently overwriting (overwriting the currently loaded request is
    /// what plain "Save" — `save_over_loaded` — is for).
    fn confirm_save_dialog(&mut self) {
        let Some(dialog) = self.save_dialog.as_ref() else {
            return;
        };
        let collection_name = dialog.collection.trim().to_string();
        let name = dialog.name.trim().to_string();
        let folder = dialog.folder_path.trim().trim_matches('/').to_string();

        if collection_name.is_empty() {
            self.save_dialog.as_mut().unwrap().error = Some("collection name is required".to_string());
            return;
        }
        if name.is_empty() {
            self.save_dialog.as_mut().unwrap().error = Some("request name is required".to_string());
            return;
        }

        let path = if folder.is_empty() {
            name.clone()
        } else {
            format!("{folder}/{name}")
        };

        let saved = match self.build_saved_request(&name) {
            Ok(s) => s,
            Err(e) => {
                self.save_dialog.as_mut().unwrap().error = Some(format!("invalid request: {e}"));
                return;
            }
        };

        let result: anyhow::Result<()> = match self.active_project() {
            Some(project) => (|| {
                let mut collection = project
                    .storage
                    .load_collection_opt(&collection_name)?
                    .unwrap_or_else(|| Collection::new(&collection_name));
                collection.add_request(&path, saved)?;
                project.storage.save_collection(&collection)
            })(),
            None => Err(anyhow::anyhow!("no project is open")),
        };

        match result {
            Ok(()) => {
                self.loaded_request = Some(LoadedRequestRef {
                    collection: collection_name.clone(),
                    path: path.clone(),
                });
                self.refresh_project_lists();
                self.collection_notice = Some(format!("saved \"{path}\" in \"{collection_name}\""));
                self.save_dialog = None;
            }
            Err(e) => self.save_dialog.as_mut().unwrap().error = Some(e.to_string()),
        }
    }

    /// Overwrite the currently loaded request in place — its own identity
    /// (`SavedRequest::id`) is preserved rather than regenerated, and its
    /// position in the tree doesn't move. Falls back to opening the Save
    /// dialog when nothing is loaded, so a single "Save" button always does
    /// something reasonable regardless of editor state.
    fn save_over_loaded(&mut self) {
        let Some(loaded) = self.loaded_request.clone() else {
            self.open_save_dialog();
            return;
        };
        let (_, name) = split_folder_and_name(&loaded.path);
        let saved = match self.build_saved_request(&name) {
            Ok(s) => s,
            Err(e) => {
                self.collection_notice = Some(format!("failed to save: {e}"));
                return;
            }
        };

        let result: anyhow::Result<()> = match self.active_project() {
            Some(project) => (|| {
                let mut collection = project.storage.load_collection(&loaded.collection)?;
                match collection.find_request_mut(&loaded.path) {
                    Some(existing) => {
                        let id = existing.id;
                        *existing = saved;
                        existing.id = id;
                    }
                    None => collection.add_request(&loaded.path, saved)?,
                }
                project.storage.save_collection(&collection)
            })(),
            None => Err(anyhow::anyhow!("no project is open")),
        };

        if result.is_ok() {
            self.refresh_project_lists();
        }
        self.collection_notice = Some(match result {
            Ok(()) => format!("saved \"{}\" in \"{}\"", loaded.path, loaded.collection),
            Err(e) => format!("failed to save: {e}"),
        });
    }

    /// Remove a request from a collection (the sidebar tree's `✕` button) —
    /// no confirmation step, matching this app's established "delete acts
    /// immediately" pattern (environment delete, override row removal).
    /// Clears `loaded_request` if the deleted request was the one loaded
    /// into the editor, so a later "Save" doesn't try to overwrite
    /// something that no longer exists.
    fn delete_saved_request(&mut self, collection_name: &str, path: &str) {
        let result: anyhow::Result<()> = match self.active_project() {
            Some(project) => (|| {
                let mut collection = project.storage.load_collection(collection_name)?;
                collection.remove_request(path);
                project.storage.save_collection(&collection)
            })(),
            None => Err(anyhow::anyhow!("no project is open")),
        };

        match result {
            Ok(()) => {
                if self
                    .loaded_request
                    .as_ref()
                    .is_some_and(|r| r.collection == collection_name && r.path == path)
                {
                    self.loaded_request = None;
                }
                self.refresh_project_lists();
                self.collection_notice = Some(format!("deleted \"{path}\" from \"{collection_name}\""));
            }
            Err(e) => self.collection_notice = Some(format!("failed to delete \"{path}\": {e}")),
        }
    }
}

/// Split `"Auth/OAuth/login"` into (`"Auth/OAuth"`, `"login"`) — a bare
/// `"login"` (no `/`) splits into (`""`, `"login"`). The inverse of how the
/// Save dialog's folder-path + name fields join back into one path.
fn split_folder_and_name(path: &str) -> (String, String) {
    match path.rsplit_once('/') {
        Some((folder, name)) => (folder.to_string(), name.to_string()),
        None => (String::new(), path.to_string()),
    }
}

/// Detect the OS's light/dark preference on GNOME via `gsettings` — the
/// same source GTK/GNOME apps themselves read. Needed because winit 0.30
/// (egui/eframe's windowing backend) unconditionally returns `None` from
/// `EventLoop::system_theme()` on both X11 and Wayland, so egui's own
/// `ThemePreference::System` (the default) silently falls back to a
/// hardcoded dark theme on Linux rather than actually following the OS —
/// there's no live "OS theme changed" signal wired up either way; this is
/// a one-shot check at startup, not a running watch (see the sidebar's
/// light/dark toggle for changing it after launch). Returns `None` (leave
/// egui's own default alone) if `gsettings` isn't installed, the call
/// fails, or the value doesn't parse — most likely a non-GNOME desktop,
/// where this specific heuristic doesn't apply.
fn detect_system_theme() -> Option<egui::Theme> {
    let output = std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "color-scheme"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_gnome_color_scheme(&String::from_utf8_lossy(&output.stdout))
}

/// Parse `gsettings get org.gnome.desktop.interface color-scheme`'s output
/// (`'default'`, `'prefer-dark'`, or `'prefer-light'`, single-quoted) into
/// a `Theme`. `'default'` means light — GNOME's traditional base theme
/// before dark mode existed as an explicit opt-in.
fn parse_gnome_color_scheme(raw: &str) -> Option<egui::Theme> {
    match raw.trim().trim_matches('\'') {
        "prefer-dark" => Some(egui::Theme::Dark),
        "prefer-light" | "default" => Some(egui::Theme::Light),
        _ => None,
    }
}

impl eframe::App for CurlyApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll_response();
        self.poll_project_dialog();
        self.show_save_dialog(ui.ctx());

        egui::Panel::left("project_sidebar")
            .resizable(true)
            .default_size(220.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("Project");
                    let current = ui.ctx().theme();
                    let (icon, next, hover) = match current {
                        egui::Theme::Dark => ("☀", egui::Theme::Light, "Switch to light mode"),
                        egui::Theme::Light => ("🌙", egui::Theme::Dark, "Switch to dark mode"),
                    };
                    if ui.small_button(icon).on_hover_text(hover).clicked() {
                        ui.ctx().set_theme(next);
                    }
                });

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

                let dialog_pending = self.project_dialog_rx.is_some();
                if ui
                    .add_enabled(
                        !dialog_pending,
                        egui::Button::new(if dialog_pending { "Choosing…" } else { "Open Project…" }),
                    )
                    .clicked()
                {
                    let ctx = ui.ctx().clone();
                    self.open_project_dialog(&ctx);
                }

                ui.separator();

                ui.label(format!("Collections ({})", self.collections.len()));
                if self.collections.is_empty() {
                    ui.weak("  (none yet)");
                }
                let mut tree_action: Option<(String, TreeAction)> = None;
                for collection in &self.collections {
                    egui::CollapsingHeader::new(&collection.name)
                        .id_salt(collection.id)
                        .show(ui, |ui| {
                            if let Some(a) = show_collection_tree(ui, "", &collection.folders, &collection.requests)
                            {
                                tree_action = Some((collection.name.clone(), a));
                            }
                        });
                }
                if let Some((collection_name, action)) = tree_action {
                    match action {
                        TreeAction::Load(path, saved) => {
                            self.loaded_request = Some(LoadedRequestRef {
                                collection: collection_name,
                                path,
                            });
                            self.load_saved_request(&saved);
                        }
                        TreeAction::Delete(path) => {
                            self.delete_saved_request(&collection_name, &path);
                        }
                    }
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

                ui.separator();

                let mut clear_session_clicked = false;
                egui::CollapsingHeader::new(format!(
                    "Session \"{}\" ({})",
                    self.session_scope(),
                    self.session_variables.len()
                ))
                .default_open(false)
                .show(ui, |ui| {
                    ui.weak(
                        "Read-only — written by extraction rules on a successful send, \
                         not the environment file. Edit the environment above to change a value by hand.",
                    );
                    if self.session_variables.is_empty() {
                        ui.weak("  (empty)");
                    }
                    for var in &self.session_variables {
                        let value = if var.secret { "***" } else { var.value.as_str() };
                        ui.label(format!("  {} = {value}", var.key));
                    }
                    if !self.session_variables.is_empty() && ui.button("Clear session").clicked() {
                        clear_session_clicked = true;
                    }
                });
                if clear_session_clicked {
                    self.clear_session();
                }
            });

        egui::CentralPanel::default().show(ui, |ui| {
            let ctx = ui.ctx().clone();
            let mut should_send = false;

            if let Some(notice) = &self.load_notice {
                ui.colored_label(egui::Color32::from_rgb(230, 190, 60), notice);
            }

            if !self.extract_rules.is_empty() {
                let names: Vec<&str> = self.extract_rules.iter().map(|e| e.name()).collect();
                let scope = self.active_environment.as_deref().unwrap_or("global");
                ui.weak(format!(
                    "On a successful send, extracts {} into session \"{scope}\".",
                    names.join(", ")
                ));
            }

            if let Some(notice) = &self.session_notice {
                ui.colored_label(egui::Color32::from_rgb(120, 190, 130), notice);
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

            ui.horizontal(|ui| {
                match &self.loaded_request {
                    Some(loaded) => ui.weak(format!("Editing \"{}/{}\"", loaded.collection, loaded.path)),
                    None => ui.weak("New request (not saved)"),
                };
                if ui.button("New").clicked() {
                    self.new_request();
                }
                if ui.button("Save").clicked() {
                    self.save_over_loaded();
                }
                if ui.button("Save As…").clicked() {
                    self.open_save_dialog();
                }
            });
            if ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::S)) {
                self.save_over_loaded();
            }

            if let Some(notice) = &self.collection_notice {
                ui.colored_label(egui::Color32::from_rgb(120, 190, 130), notice);
            }

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
    prefix: &str,
    folders: &[Folder],
    requests: &[SavedRequest],
) -> Option<TreeAction> {
    let mut action = None;

    for folder in folders {
        let folder_prefix = if prefix.is_empty() {
            folder.name.clone()
        } else {
            format!("{prefix}/{}", folder.name)
        };
        egui::CollapsingHeader::new(format!("{}/", folder.name))
            .id_salt(folder.id)
            .show(ui, |ui| {
                if let Some(a) = show_collection_tree(ui, &folder_prefix, &folder.folders, &folder.requests) {
                    action = Some(a);
                }
            });
    }

    for request in requests {
        let path = if prefix.is_empty() {
            request.name.clone()
        } else {
            format!("{prefix}/{}", request.name)
        };
        ui.horizontal(|ui| {
            let label = format!("{:<7} {}", request.method, request.name);
            if ui.selectable_label(false, label).clicked() {
                action = Some(TreeAction::Load(path.clone(), request.clone()));
            }
            if ui.small_button("✕").clicked() {
                action = Some(TreeAction::Delete(path.clone()));
            }
        });
    }

    action
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

    // --- FR-21: extraction rules writing into a session on a successful send ---

    fn response(status: u16, body: &str) -> ResponseSummary {
        ResponseSummary {
            status,
            headers: vec![],
            body: body.to_string(),
            elapsed: Duration::from_millis(1),
        }
    }

    #[test]
    fn load_saved_request_carries_over_extraction_rules() {
        let mut app = CurlyApp::default_state();
        let mut saved = SavedRequest::new("login", Method::POST, "https://example.com");
        saved.extract.push(Extraction::Body {
            name: "TOKEN".to_string(),
            path: "token".to_string(),
            secret: true,
        });

        app.load_saved_request(&saved);

        assert_eq!(app.extract_rules.len(), 1);
        assert_eq!(app.extract_rules[0].name(), "TOKEN");
    }

    #[test]
    fn load_saved_request_clears_stale_session_notice() {
        let mut app = CurlyApp::default_state();
        app.session_notice = Some("stale".to_string());

        app.load_saved_request(&SavedRequest::new("get-me", Method::GET, "https://example.com"));

        assert!(app.session_notice.is_none());
    }

    #[test]
    fn send_captures_pending_extraction_before_the_request_goes_out() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = project_with(&dir);
        app.url = "http://127.0.0.1:1/get".to_string();
        app.extract_rules = vec![Extraction::Body {
            name: "TOKEN".to_string(),
            path: "token".to_string(),
            secret: false,
        }];
        app.active_environment = Some("dev".to_string());

        let ctx = egui::Context::default();
        app.send(&ctx);

        let pending = app.pending_extraction.as_ref().unwrap();
        assert_eq!(pending.rules.len(), 1);
        assert_eq!(pending.session_scope, "dev");
    }

    #[test]
    fn send_captures_no_pending_extraction_when_the_request_has_no_rules() {
        let mut app = CurlyApp::default_state();
        app.url = "http://127.0.0.1:1/get".to_string();

        let ctx = egui::Context::default();
        app.send(&ctx);

        assert!(app.pending_extraction.is_none());
    }

    #[test]
    fn send_defaults_the_session_scope_to_global_with_no_environment_selected() {
        let mut app = CurlyApp::default_state();
        app.url = "http://127.0.0.1:1/get".to_string();
        app.extract_rules = vec![Extraction::Body {
            name: "TOKEN".to_string(),
            path: "token".to_string(),
            secret: false,
        }];

        let ctx = egui::Context::default();
        app.send(&ctx);

        assert_eq!(app.pending_extraction.unwrap().session_scope, "global");
    }

    #[test]
    fn apply_pending_extraction_writes_into_the_session_on_a_successful_response() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = project_with(&dir);
        app.pending_extraction = Some(PendingExtraction {
            rules: vec![Extraction::Body {
                name: "TOKEN".to_string(),
                path: "token".to_string(),
                secret: true,
            }],
            variables: BTreeMap::new(),
            session_scope: "dev".to_string(),
        });

        app.apply_pending_extraction(&response(200, r#"{"token":"abc123"}"#));

        let session = app.active_project().unwrap().storage.load_session("dev").unwrap();
        assert_eq!(session.variables.len(), 1);
        assert_eq!(session.variables[0].key, "TOKEN");
        assert_eq!(session.variables[0].value, "abc123");
        assert!(session.variables[0].secret);
        assert!(app.session_notice.unwrap().contains("TOKEN"));
        assert!(app.pending_extraction.is_none());
    }

    #[test]
    fn apply_pending_extraction_is_skipped_for_a_non_2xx_response() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = project_with(&dir);
        app.pending_extraction = Some(PendingExtraction {
            rules: vec![Extraction::Body {
                name: "TOKEN".to_string(),
                path: "token".to_string(),
                secret: false,
            }],
            variables: BTreeMap::new(),
            session_scope: "dev".to_string(),
        });

        app.apply_pending_extraction(&response(400, r#"{"error":"nope"}"#));

        let session = app.active_project().unwrap().storage.load_session("dev").unwrap();
        assert!(session.variables.is_empty());
        assert!(app.session_notice.is_none());
        assert!(app.pending_extraction.is_none());
    }

    #[test]
    fn apply_pending_extraction_is_a_no_op_with_nothing_pending() {
        let mut app = CurlyApp::default_state();
        app.apply_pending_extraction(&response(200, "{}"));
        assert!(app.session_notice.is_none());
    }

    #[test]
    fn apply_pending_extraction_reports_an_extraction_failure() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = project_with(&dir);
        app.pending_extraction = Some(PendingExtraction {
            rules: vec![Extraction::Body {
                name: "TOKEN".to_string(),
                path: "missing".to_string(),
                secret: false,
            }],
            variables: BTreeMap::new(),
            session_scope: "dev".to_string(),
        });

        app.apply_pending_extraction(&response(200, r#"{"token":"abc"}"#));

        assert!(app.session_notice.unwrap().contains("extraction failed"));
    }

    #[test]
    fn apply_pending_extraction_reports_no_project_open() {
        let mut app = CurlyApp::default_state();
        app.pending_extraction = Some(PendingExtraction {
            rules: vec![Extraction::Body {
                name: "TOKEN".to_string(),
                path: "token".to_string(),
                secret: false,
            }],
            variables: BTreeMap::new(),
            session_scope: "dev".to_string(),
        });

        app.apply_pending_extraction(&response(200, r#"{"token":"abc"}"#));

        assert!(app.session_notice.unwrap().contains("no project is open"));
    }

    // --- Session variables display (the read-only "Session" sidebar panel) ---

    #[test]
    fn apply_pending_extraction_refreshes_the_session_display() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = project_with(&dir);
        app.pending_extraction = Some(PendingExtraction {
            rules: vec![Extraction::Body {
                name: "TOKEN".to_string(),
                path: "token".to_string(),
                secret: true,
            }],
            variables: BTreeMap::new(),
            session_scope: "global".to_string(),
        });

        app.apply_pending_extraction(&response(200, r#"{"token":"abc123"}"#));

        assert_eq!(app.session_variables.len(), 1);
        assert_eq!(app.session_variables[0].key, "TOKEN");
        assert_eq!(app.session_variables[0].value, "abc123");
        assert!(app.session_variables[0].secret);
    }

    #[test]
    fn apply_pending_extraction_does_not_refresh_the_display_on_failure() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = project_with(&dir);
        app.pending_extraction = Some(PendingExtraction {
            rules: vec![Extraction::Body {
                name: "TOKEN".to_string(),
                path: "missing".to_string(),
                secret: false,
            }],
            variables: BTreeMap::new(),
            session_scope: "global".to_string(),
        });

        app.apply_pending_extraction(&response(200, r#"{"token":"abc"}"#));

        assert!(app.session_variables.is_empty());
    }

    #[test]
    fn selecting_an_environment_loads_its_sessions_variables() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = project_with(&dir);
        let mut session = Environment::new("dev");
        session.set("TOKEN", "abc", false);
        app.active_project().unwrap().storage.save_session(&session).unwrap();

        app.select_environment(Some("dev".to_string()));

        assert_eq!(app.session_variables.len(), 1);
        assert_eq!(app.session_variables[0].key, "TOKEN");
    }

    #[test]
    fn deselecting_an_environment_falls_back_to_the_global_session() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = project_with(&dir);
        let mut global_session = Environment::new("global");
        global_session.set("REQ_ID", "xyz", false);
        app.active_project().unwrap().storage.save_session(&global_session).unwrap();
        app.select_environment(Some("dev".to_string()));

        app.select_environment(None);

        assert_eq!(app.session_variables.len(), 1);
        assert_eq!(app.session_variables[0].key, "REQ_ID");
    }

    #[test]
    fn opening_a_new_project_refreshes_the_session_display() {
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        let mut app = project_with(&dir_a);
        let mut session = Environment::new("global");
        session.set("LEFTOVER", "stale", false);
        app.active_project().unwrap().storage.save_session(&session).unwrap();
        app.refresh_session_variables();
        assert_eq!(app.session_variables.len(), 1);

        app.open_project_at(dir_b.path());

        assert!(app.session_variables.is_empty());
    }

    #[test]
    fn clear_session_removes_it_and_refreshes_the_display() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = project_with(&dir);
        let mut session = Environment::new("global");
        session.set("TOKEN", "abc", false);
        app.active_project().unwrap().storage.save_session(&session).unwrap();
        app.refresh_session_variables();
        assert_eq!(app.session_variables.len(), 1);

        app.clear_session();

        assert!(app.session_variables.is_empty());
        let reloaded = app.active_project().unwrap().storage.load_session("global").unwrap();
        assert!(reloaded.variables.is_empty());
        assert!(app.session_notice.unwrap().contains("cleared session"));
    }

    #[test]
    fn clear_session_is_a_no_op_with_no_project_open() {
        let mut app = CurlyApp::default_state();
        app.clear_session();
        assert!(app.session_notice.is_none());
    }

    #[test]
    fn session_scope_defaults_to_global_and_follows_the_active_environment() {
        let mut app = CurlyApp::default_state();
        assert_eq!(app.session_scope(), "global");

        app.active_environment = Some("dev".to_string());
        assert_eq!(app.session_scope(), "dev");
    }

    // --- FR-20: save/new/delete a request from the GUI ---

    #[test]
    fn split_folder_and_name_splits_a_nested_path() {
        assert_eq!(
            split_folder_and_name("Auth/OAuth/login"),
            ("Auth/OAuth".to_string(), "login".to_string())
        );
    }

    #[test]
    fn split_folder_and_name_top_level_has_no_folder() {
        assert_eq!(split_folder_and_name("login"), (String::new(), "login".to_string()));
    }

    #[test]
    fn build_saved_request_carries_over_editor_fields() {
        let mut app = CurlyApp::default_state();
        app.method = "post".to_string();
        app.url = "https://example.com/users".to_string();
        app.headers = vec![
            HeaderRow {
                name: "X-Test".to_string(),
                value: "1".to_string(),
                enabled: true,
            },
            HeaderRow {
                name: "X-Off".to_string(),
                value: "2".to_string(),
                enabled: false,
            },
            HeaderRow {
                name: "   ".to_string(),
                value: "ignored".to_string(),
                enabled: true,
            },
        ];
        app.body = r#"{"a":1}"#.to_string();
        app.extract_rules = vec![Extraction::Body {
            name: "ID".to_string(),
            path: "id".to_string(),
            secret: false,
        }];

        let saved = app.build_saved_request("create-user").unwrap();

        assert_eq!(saved.name, "create-user");
        assert_eq!(saved.method, "POST");
        assert_eq!(saved.url, "https://example.com/users");
        assert_eq!(saved.headers.len(), 2);
        assert_eq!(saved.headers[0].name, "X-Test");
        assert!(!saved.headers[1].enabled);
        match saved.body {
            Some(SavedBody::Raw { content }) => assert_eq!(content, r#"{"a":1}"#),
            other => panic!("expected Raw body, got {other:?}"),
        }
        assert_eq!(saved.extract.len(), 1);
    }

    #[test]
    fn build_saved_request_empty_body_is_none() {
        let app = CurlyApp::default_state();
        let saved = app.build_saved_request("x").unwrap();
        assert!(saved.body.is_none());
    }

    #[test]
    fn build_saved_request_errors_on_an_invalid_method() {
        let mut app = CurlyApp::default_state();
        app.method = "NOT A METHOD".to_string();
        assert!(app.build_saved_request("x").is_err());
    }

    #[test]
    fn new_request_resets_the_editor_and_clears_loaded_request() {
        let mut app = CurlyApp::default_state();
        app.method = "POST".to_string();
        app.url = "https://example.com".to_string();
        app.body = "stuff".to_string();
        app.extract_rules = vec![Extraction::Header {
            name: "X".to_string(),
            header: "X".to_string(),
            secret: false,
        }];
        app.loaded_request = Some(LoadedRequestRef {
            collection: "api".to_string(),
            path: "login".to_string(),
        });

        app.new_request();

        assert_eq!(app.method, "GET");
        assert!(app.url.is_empty());
        assert!(app.body.is_empty());
        assert!(app.extract_rules.is_empty());
        assert!(app.loaded_request.is_none());
    }

    #[test]
    fn open_save_dialog_prefills_from_the_loaded_request() {
        let mut app = CurlyApp::default_state();
        app.loaded_request = Some(LoadedRequestRef {
            collection: "api".to_string(),
            path: "Auth/login".to_string(),
        });

        app.open_save_dialog();

        let dialog = app.save_dialog.unwrap();
        assert_eq!(dialog.collection, "api");
        assert_eq!(dialog.folder_path, "Auth");
        assert_eq!(dialog.name, "login");
    }

    #[test]
    fn open_save_dialog_defaults_to_the_first_collection_with_nothing_loaded() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = project_with(&dir);
        app.active_project()
            .unwrap()
            .storage
            .save_collection(&Collection::new("my-api"))
            .unwrap();
        app.refresh_project_lists();

        app.open_save_dialog();

        let dialog = app.save_dialog.unwrap();
        assert_eq!(dialog.collection, "my-api");
        assert!(dialog.folder_path.is_empty());
        assert!(dialog.name.is_empty());
    }

    #[test]
    fn cancel_save_dialog_clears_it() {
        let mut app = CurlyApp::default_state();
        app.open_save_dialog();
        assert!(app.save_dialog.is_some());

        app.cancel_save_dialog();

        assert!(app.save_dialog.is_none());
    }

    #[test]
    fn confirm_save_dialog_creates_a_new_request_in_a_new_collection() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = project_with(&dir);
        app.url = "https://example.com/get".to_string();
        app.save_dialog = Some(SaveDialog {
            collection: "my-api".to_string(),
            folder_path: "Auth".to_string(),
            name: "login".to_string(),
            error: None,
        });

        app.confirm_save_dialog();

        assert!(app.save_dialog.is_none());
        let collection = app.active_project().unwrap().storage.load_collection("my-api").unwrap();
        let saved = collection.find_request("Auth/login").unwrap();
        assert_eq!(saved.url, "https://example.com/get");
        assert_eq!(
            app.loaded_request.unwrap(),
            LoadedRequestRef {
                collection: "my-api".to_string(),
                path: "Auth/login".to_string(),
            }
        );
        assert_eq!(app.collections.len(), 1);
    }

    #[test]
    fn confirm_save_dialog_at_the_top_level_has_no_folder() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = project_with(&dir);
        app.save_dialog = Some(SaveDialog {
            collection: "my-api".to_string(),
            folder_path: String::new(),
            name: "ping".to_string(),
            error: None,
        });

        app.confirm_save_dialog();

        let collection = app.active_project().unwrap().storage.load_collection("my-api").unwrap();
        assert!(collection.find_request("ping").is_some());
    }

    #[test]
    fn confirm_save_dialog_rejects_a_blank_collection_name() {
        let mut app = CurlyApp::default_state();
        app.save_dialog = Some(SaveDialog {
            collection: "   ".to_string(),
            folder_path: String::new(),
            name: "ping".to_string(),
            error: None,
        });

        app.confirm_save_dialog();

        assert!(app.save_dialog.is_some());
        assert!(app.save_dialog.unwrap().error.unwrap().contains("collection"));
    }

    #[test]
    fn confirm_save_dialog_rejects_a_blank_name() {
        let mut app = CurlyApp::default_state();
        app.save_dialog = Some(SaveDialog {
            collection: "my-api".to_string(),
            folder_path: String::new(),
            name: "  ".to_string(),
            error: None,
        });

        app.confirm_save_dialog();

        assert!(app.save_dialog.is_some());
        assert!(app.save_dialog.unwrap().error.unwrap().contains("name"));
    }

    #[test]
    fn confirm_save_dialog_reports_a_collision_without_closing() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = project_with(&dir);
        let mut collection = Collection::new("my-api");
        collection
            .add_request("ping", SavedRequest::new("ping", Method::GET, "https://example.com"))
            .unwrap();
        app.active_project().unwrap().storage.save_collection(&collection).unwrap();
        app.save_dialog = Some(SaveDialog {
            collection: "my-api".to_string(),
            folder_path: String::new(),
            name: "ping".to_string(),
            error: None,
        });

        app.confirm_save_dialog();

        assert!(app.save_dialog.is_some());
        assert!(app.save_dialog.unwrap().error.is_some());
    }

    #[test]
    fn save_over_loaded_opens_the_dialog_when_nothing_is_loaded() {
        let mut app = CurlyApp::default_state();
        app.save_over_loaded();
        assert!(app.save_dialog.is_some());
    }

    #[test]
    fn save_over_loaded_overwrites_in_place_and_preserves_the_id() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = project_with(&dir);
        let mut collection = Collection::new("my-api");
        let original = SavedRequest::new("login", Method::GET, "https://old.example.com");
        let original_id = original.id;
        collection.add_request("Auth/login", original).unwrap();
        app.active_project().unwrap().storage.save_collection(&collection).unwrap();

        app.loaded_request = Some(LoadedRequestRef {
            collection: "my-api".to_string(),
            path: "Auth/login".to_string(),
        });
        app.method = "POST".to_string();
        app.url = "https://new.example.com".to_string();

        app.save_over_loaded();

        let reloaded = app.active_project().unwrap().storage.load_collection("my-api").unwrap();
        let saved = reloaded.find_request("Auth/login").unwrap();
        assert_eq!(saved.url, "https://new.example.com");
        assert_eq!(saved.method, "POST");
        assert_eq!(saved.id, original_id);
        assert!(app.collection_notice.unwrap().contains("Auth/login"));
    }

    #[test]
    fn delete_saved_request_removes_it_and_refreshes() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = project_with(&dir);
        let mut collection = Collection::new("my-api");
        collection
            .add_request("ping", SavedRequest::new("ping", Method::GET, "https://example.com"))
            .unwrap();
        app.active_project().unwrap().storage.save_collection(&collection).unwrap();
        app.refresh_project_lists();

        app.delete_saved_request("my-api", "ping");

        let reloaded = app.active_project().unwrap().storage.load_collection("my-api").unwrap();
        assert!(reloaded.find_request("ping").is_none());
        assert!(app.collection_notice.unwrap().contains("deleted"));
    }

    #[test]
    fn delete_saved_request_clears_loaded_request_if_it_was_the_one_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = project_with(&dir);
        let mut collection = Collection::new("my-api");
        collection
            .add_request("ping", SavedRequest::new("ping", Method::GET, "https://example.com"))
            .unwrap();
        app.active_project().unwrap().storage.save_collection(&collection).unwrap();
        app.loaded_request = Some(LoadedRequestRef {
            collection: "my-api".to_string(),
            path: "ping".to_string(),
        });

        app.delete_saved_request("my-api", "ping");

        assert!(app.loaded_request.is_none());
    }

    #[test]
    fn delete_saved_request_leaves_a_different_loaded_request_alone() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = project_with(&dir);
        let mut collection = Collection::new("my-api");
        collection
            .add_request("ping", SavedRequest::new("ping", Method::GET, "https://example.com"))
            .unwrap();
        collection
            .add_request("pong", SavedRequest::new("pong", Method::GET, "https://example.com"))
            .unwrap();
        app.active_project().unwrap().storage.save_collection(&collection).unwrap();
        app.loaded_request = Some(LoadedRequestRef {
            collection: "my-api".to_string(),
            path: "pong".to_string(),
        });

        app.delete_saved_request("my-api", "ping");

        assert!(app.loaded_request.is_some());
    }

    // --- FR-24: theming ---

    #[test]
    fn parse_gnome_color_scheme_prefer_dark() {
        assert_eq!(parse_gnome_color_scheme("'prefer-dark'\n"), Some(egui::Theme::Dark));
    }

    #[test]
    fn parse_gnome_color_scheme_prefer_light() {
        assert_eq!(parse_gnome_color_scheme("'prefer-light'\n"), Some(egui::Theme::Light));
    }

    #[test]
    fn parse_gnome_color_scheme_default_means_light() {
        assert_eq!(parse_gnome_color_scheme("'default'\n"), Some(egui::Theme::Light));
    }

    #[test]
    fn parse_gnome_color_scheme_unrecognized_value_is_none() {
        assert_eq!(parse_gnome_color_scheme("'something-new'\n"), None);
    }

    #[test]
    fn parse_gnome_color_scheme_empty_is_none() {
        assert_eq!(parse_gnome_color_scheme(""), None);
    }
}
