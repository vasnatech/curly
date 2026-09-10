//! The GUI's slices so far: FR-17's core loop, browsing/loading/saving/
//! deleting requests (FR-18/20), all of FR-21 (environments, overrides,
//! extraction-into-session), most of FR-22 (JSON syntax highlighting,
//! search, save-to-file — not image preview, see docs/DESIGN.md), most of
//! FR-23 (send/save/new-tab shortcuts), a toolbar with real zoom, and now
//! FR-19 — multiple request tabs. Still not built: multiple
//! simultaneously-open projects (see project.rs's doc comment for why the
//! data model already supports it) and image preview.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use curly_core::exec::{self, ResponseSummary};
use curly_core::extraction::{self, Extraction};
use curly_core::model::{Auth, Body, Request};
use curly_core::storage::{
    Collection, Environment, Folder, HistoryAuthKind, HistoryEntry, KvPair, SavedAuth, SavedBody,
    SavedRequest, Storage, Variable, REDACTED,
};
use curly_core::substitution;
use reqwest::Method;
use serde::{Deserialize, Serialize};

use crate::persistence;
use crate::project::Project;

const METHODS: [&str; 7] = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];

/// Multiplicative step for one zoom in/out click or Ctrl+/Ctrl- press.
/// `egui::Context::zoom_factor()` defaults to `1.0` ("100%") — the
/// baseline this app is laid out for is a 1024×768 window at that
/// default, so "100%" here means exactly what it says, not some other
/// implicit scale.
const ZOOM_STEP: f32 = 1.1;
const MIN_ZOOM: f32 = 0.5;
const MAX_ZOOM: f32 = 3.0;

/// How many recent history entries the sidebar shows — matches `curly
/// history`'s own CLI default (`--limit 20`).
const HISTORY_DISPLAY_LIMIT: usize = 20;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct HeaderRow {
    pub(crate) name: String,
    pub(crate) value: String,
    pub(crate) enabled: bool,
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

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) enum AuthKind {
    #[default]
    None,
    Basic,
    Bearer,
}

/// Editor state for the Authorization section — all three text fields stay
/// populated regardless of `kind` so switching Basic/Bearer/None in the UI
/// never loses what was typed.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct AuthEditor {
    pub(crate) kind: AuthKind,
    pub(crate) username: String,
    pub(crate) password: String,
    pub(crate) token: String,
}

/// A snapshot of everything that makes a request "unsaved" (M8) — captured
/// whenever a tab's content is known to match disk (right after loading or
/// saving) and compared against the live editor fields each frame to decide
/// whether the tab needs a "*" (see `is_tab_dirty`). Deliberately narrower
/// than `RequestTab` itself: no response/search/notice fields, none of
/// which affect whether there's unsaved *request* content.
#[derive(Clone, Default, PartialEq)]
struct EditableSnapshot {
    method: String,
    url: String,
    headers: Vec<HeaderRow>,
    body: String,
    auth: AuthEditor,
    extract_rules: Vec<Extraction>,
}

impl EditableSnapshot {
    fn capture(
        method: &str,
        url: &str,
        headers: &[HeaderRow],
        body: &str,
        auth: &AuthEditor,
        extract_rules: &[Extraction],
    ) -> Self {
        Self {
            method: method.to_string(),
            url: url.to_string(),
            headers: headers.to_vec(),
            body: body.to_string(),
            auth: auth.clone(),
            extract_rules: extract_rules.to_vec(),
        }
    }

    /// What a freshly opened blank tab looks like — matches
    /// `RequestTab::blank()`'s own fields exactly, so a genuinely untouched
    /// new tab isn't flagged dirty the moment it's opened.
    fn blank() -> Self {
        Self::capture("GET", "", &[HeaderRow::empty()], "", &AuthEditor::default(), &[])
    }
}

/// Whether the given editor fields differ from `snapshot` (the last known
/// on-disk state, `None` meaning "never saved") — a pure function so it's
/// directly unit-testable without an `egui::Context`. Used by `tab_label`
/// for the "*" indicator and by `save_all_in_collection` to decide which
/// open tabs actually need saving.
fn is_tab_dirty(
    snapshot: &Option<EditableSnapshot>,
    method: &str,
    url: &str,
    headers: &[HeaderRow],
    body: &str,
    auth: &AuthEditor,
    extract_rules: &[Extraction],
) -> bool {
    let current = EditableSnapshot::capture(method, url, headers, body, auth, extract_rules);
    match snapshot {
        Some(saved) => *saved != current,
        // No snapshot at all means this tab was never routed through
        // `blank()`/a load/a save — treat as dirty rather than assume clean.
        None => true,
    }
}

/// Which `Extraction` variant a rule-editor row currently is — used only to
/// drive the Body/Header/Template selector; `Extraction` itself (from
/// `curly-core`) stays the single source of truth for the rule's data.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ExtractionKindTag {
    Body,
    Header,
    Template,
}

fn extraction_kind(rule: &Extraction) -> ExtractionKindTag {
    match rule {
        Extraction::Body { .. } => ExtractionKindTag::Body,
        Extraction::Header { .. } => ExtractionKindTag::Header,
        Extraction::Template { .. } => ExtractionKindTag::Template,
    }
}

/// Reconstruct `rule` as a different variant, carrying `name`/`secret` over
/// and starting the kind-specific field blank — used when the editor's
/// Body/Header/Template selector changes for an existing row.
fn rebuild_extraction(rule: &Extraction, kind: ExtractionKindTag) -> Extraction {
    let (name, secret) = match rule {
        Extraction::Body { name, secret, .. } => (name.clone(), *secret),
        Extraction::Header { name, secret, .. } => (name.clone(), *secret),
        Extraction::Template { name, secret, .. } => (name.clone(), *secret),
    };
    match kind {
        ExtractionKindTag::Body => Extraction::Body {
            name,
            path: String::new(),
            secret,
        },
        ExtractionKindTag::Header => Extraction::Header {
            name,
            header: String::new(),
            secret,
        },
        ExtractionKindTag::Template => Extraction::Template {
            name,
            template: String::new(),
            secret,
        },
    }
}

impl From<HistoryAuthKind> for AuthKind {
    fn from(kind: HistoryAuthKind) -> Self {
        match kind {
            HistoryAuthKind::Basic => AuthKind::Basic,
            HistoryAuthKind::Bearer => AuthKind::Bearer,
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

/// Which sub-tab the Response pane shows (M8) — Body/Headers used to be
/// stacked (always both visible); History is new, reusing the project-wide
/// send log filtered to the active tab's own request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum ResponseSubtab {
    #[default]
    Body,
    Headers,
    History,
}

/// Which top-level view the left sidebar shows (M8) — a small switcher
/// above the Collections/Environments/Session stack, replacing what used to
/// be one long always-visible scroll with History mixed in the middle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum SidebarTab {
    #[default]
    Project,
    History,
}

/// Which collection/folder tab an `ExtraTabKind` entry in the tab strip
/// resolves to when clicked — kept as a small conversion rather than
/// duplicating the match everywhere `central_view` is set from one.
impl ExtraTabKind {
    fn central_view(&self) -> CentralView {
        match self {
            ExtraTabKind::Collection(name) => CentralView::Collection(name.clone()),
            ExtraTabKind::Folder(collection, path) => CentralView::Folder(collection.clone(), path.clone()),
        }
    }
}

/// A collection or folder opened as its own main tab (M8) — on demand, by
/// clicking its name in the sidebar tree (`open_or_focus_extra_tab`).
/// Unlike request tabs, these don't exist until opened, and their content
/// (`show_collection_tab`/`show_folder_tab`) is derived straight from the
/// path here rather than from any per-tab editor state.
#[derive(Clone, Debug, PartialEq, Eq)]
enum ExtraTabKind {
    Collection(String),
    Folder(String, String),
}

/// What the central panel is currently showing (M8) — `Request` defers to
/// `active_tab_idx`/the take-restore-backed top-level fields exactly as
/// before M8; the other three are new, non-request views that leave
/// whichever request tab is active completely untouched underneath.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
enum CentralView {
    #[default]
    Request,
    Collection(String),
    Folder(String, String),
    Environment,
}

/// Identifies where a loaded request lives, so a plain "Save" knows what to
/// overwrite without re-asking — the collection's name plus its
/// slash-separated path within that collection (e.g. `"Auth/OAuth/login"`,
/// matching `Collection::find_request`'s own path convention exactly; not
/// including the collection name itself).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LoadedRequestRef {
    pub(crate) collection: String,
    pub(crate) path: String,
}

/// A click in the sidebar's Collections tree, applied after the rendering
/// loop finishes (same deferred-action reason as `EnvAction`) — either load
/// a request into the editor, or delete it from its collection.
enum TreeAction {
    Load(String, SavedRequest),
    Delete(String),
    Rename(String),
    Duplicate(String),
    /// Create a new folder under the given parent path (`""` = the
    /// collection's top level) — from a folder's or the collection root's
    /// "⋯" menu (M8).
    NewFolder(String),
    /// Start a new blank request saved under the given parent path — from a
    /// folder's or the collection root's "⋯" menu (M8).
    NewRequest(String),
    /// Remove the folder at the given path and everything inside it (M8).
    DeleteFolder(String),
    /// Open (or focus) the given folder's own main tab (M8) — clicking the
    /// folder's name, as opposed to its "⋯" menu or its expand/collapse
    /// arrow.
    OpenFolder(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PathDialogMode {
    Rename,
    Duplicate,
}

/// State for the "Rename/Move" and "Duplicate" floating windows (FR-20) —
/// deliberately a separate type from `SaveDialog` rather than merged into
/// it: same shape (collection/folder/name), but a different purpose (moving
/// an *existing* tree entry vs. saving the editor's current content), and
/// merging them would need a mode flag threaded through every method that
/// already exists for `SaveDialog` alone.
struct PathDialog {
    mode: PathDialogMode,
    collection: String,
    old_path: String,
    folder_path: String,
    name: String,
    error: Option<String>,
}

/// State for the "New Folder" floating window (M8) — opened from a
/// collection's or folder's "⋯" menu; `parent_path` is `""` for a
/// top-level folder.
struct FolderDialog {
    collection: String,
    parent_path: String,
    name: String,
    error: Option<String>,
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

/// One request tab's worth of editor + response state (FR-19) — every
/// field that used to live directly on `CurlyApp` before tabs existed.
/// Environment/project selection stays on `CurlyApp` itself, shared across
/// all tabs (matching how Postman-style tools treat "environment" as a
/// workspace-level concept, not a per-request one), as does the Save
/// dialog (only one can be open at a time; switching tabs cancels it
/// rather than risk it silently saving the wrong tab's content — see
/// `switch_to_tab`'s doc comment).
///
/// `CurlyApp`'s own top-level fields of the same names *are* the active
/// tab's live state — `tabs[active_tab_idx]` is a stale placeholder while
/// that tab is active, only ever read for its label (`tab_label` special-
/// cases the active index to read the real top-level fields instead) and
/// overwritten wholesale the moment a switch actually happens. This
/// swap-based design — rather than converting every method to route
/// through `self.active_tab_mut().field` — was deliberate: it means
/// `build_request`/`send`/`poll_response`/`load_saved_request`/
/// `merged_variables`/every existing test/etc. needed zero changes to
/// support tabs; only tab *management* (open/close/switch) is new code.
///
/// One consequence worth knowing: `rx`/`response_save_rx`/
/// `pending_extraction` move with their tab when you switch away — an
/// in-flight send on tab A keeps running in the background (the async
/// task itself doesn't care which tab is visible), but its result just
/// waits unpolled in `tabs[A]` until you switch back to tab A, rather
/// than completing while some other tab is in view. No response is ever
/// lost, just not delivered until that tab is visible again.
#[derive(Default)]
struct RequestTab {
    method: String,
    url: String,
    headers: Vec<HeaderRow>,
    body: String,
    auth: AuthEditor,
    response: Option<Result<ResponseSummary, String>>,
    /// A decoded texture for an image response (see Part F / `is_image_content_type`),
    /// `None` for a non-image response or before it's been decoded yet.
    /// Not persisted (a `TextureHandle` is a runtime GPU resource) and not
    /// part of `PersistedTab`.
    response_image: Option<egui::TextureHandle>,
    in_flight: bool,
    rx: Option<mpsc::Receiver<SendOutcome>>,
    var_overrides: Vec<OverrideRow>,
    extract_rules: Vec<Extraction>,
    pending_extraction: Option<PendingExtraction>,
    session_notice: Option<String>,
    load_notice: Option<String>,
    loaded_request: Option<LoadedRequestRef>,
    response_search: String,
    /// Which match `response_search` is currently jumped to (0-indexed,
    /// wraps in both directions) — reset to 0 whenever the search text
    /// changes or a new response lands, so it never points past the end of
    /// a shorter match list.
    response_search_current: usize,
    response_save_rx: Option<mpsc::Receiver<Option<String>>>,
    response_save_notice: Option<String>,
    /// Which Response sub-tab is showing (M8).
    response_subtab: ResponseSubtab,
    /// The editor content as of the last load or save (M8) — compared
    /// against the live fields to drive the tab strip's "*" indicator. See
    /// `is_tab_dirty`.
    saved_snapshot: Option<EditableSnapshot>,
    /// The breadcrumb's editable trailing segment (M8) — the loaded
    /// request's name, editable independent of the rest of the request;
    /// saving it renames the request (see `save_breadcrumb_name`). Kept in
    /// sync with `loaded_request` reactively (see `sync_breadcrumb`) rather
    /// than threaded through every place that sets `loaded_request`.
    breadcrumb_name: String,
    /// Which `loaded_request.path` `breadcrumb_name` was last synced from —
    /// `sync_breadcrumb` resyncs whenever this no longer matches
    /// `loaded_request`, so an external change (a different request loaded,
    /// or this one renamed elsewhere) is picked up without every call site
    /// needing to remember to call a sync method itself.
    breadcrumb_synced_path: Option<String>,
    /// Set when a response arrives while this tab isn't the active one
    /// (see `poll_background_tab_responses`) — drives a "response ready"
    /// marker on the tab strip. Naturally resets to `false` on its own once
    /// the tab becomes active: `switch_to_tab`'s `std::mem::take` leaves a
    /// fresh `Default` (i.e. `false`) sitting in the now-inactive slot,
    /// since this field is deliberately not part of `restore_active_tab`'s
    /// field list (the active tab itself never needs to show its own
    /// badge).
    has_unseen_response: bool,
}

impl RequestTab {
    /// A blank tab — method defaults to GET and headers start with one
    /// empty editable row, matching `CurlyApp::default_state`'s own
    /// pre-tabs defaults exactly (deriving `Default` alone would give
    /// `method: ""` and `headers: vec![]`, neither of which match what a
    /// freshly opened tab looked like before this struct existed).
    fn blank() -> Self {
        Self {
            method: "GET".to_string(),
            headers: vec![HeaderRow::empty()],
            saved_snapshot: Some(EditableSnapshot::blank()),
            ..Default::default()
        }
    }
}

pub struct CurlyApp {
    method: String,
    url: String,
    headers: Vec<HeaderRow>,
    body: String,
    auth: AuthEditor,

    response: Option<Result<ResponseSummary, String>>,
    response_image: Option<egui::TextureHandle>,
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
    /// The active project's recent send history (FR-6/FR-18), most recent
    /// first, capped at `HISTORY_DISPLAY_LIMIT` — refreshed whenever the
    /// project changes and right after every send, so a just-sent request
    /// shows up without switching projects. Read-only in the GUI beyond
    /// clicking an entry to load it back into the editor (`load_history_entry`).
    history_entries: Vec<HistoryEntry>,
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
    /// Open while the "Rename/Move" or "Duplicate" window is showing (FR-20);
    /// `None` otherwise. Opening this closes `save_dialog` and vice versa —
    /// only one floating dialog is ever open at a time.
    path_dialog: Option<PathDialog>,
    /// Open while the "New Folder" window is showing (M8); `None` otherwise.
    /// Same one-at-a-time rule as `save_dialog`/`path_dialog`.
    folder_dialog: Option<FolderDialog>,
    /// Feedback from saving/deleting a request in a collection — separate
    /// from `load_notice` (about the editor's own content) and
    /// `env_notice`/`session_notice` (environments/sessions).
    collection_notice: Option<String>,
    /// The Response pane's search box contents — highlights every
    /// case-insensitive match in the body regardless of whether it's JSON
    /// (FR-22). Persists across sends deliberately, so re-sending the same
    /// request keeps the same search highlighted.
    response_search: String,
    response_search_current: usize,
    /// The "Save Response…" async file dialog's result channel — same
    /// fire-and-poll shape as `project_dialog_rx`, but the spawned task
    /// also writes the file itself before reporting back, so there's only
    /// one round trip instead of picking a path here and writing
    /// separately once poll sees it.
    response_save_rx: Option<mpsc::Receiver<Option<String>>>,
    /// Feedback from the last "Save Response…" (success or failure);
    /// `None` after a cancelled dialog, deliberately — cancelling isn't
    /// worth a notice.
    response_save_notice: Option<String>,
    /// See `RequestTab::response_subtab` — the active tab's copy, swapped
    /// like every other top-level field of the same name.
    response_subtab: ResponseSubtab,
    /// See `RequestTab::saved_snapshot` — this is the active tab's copy,
    /// swapped like every other top-level field of the same name.
    saved_snapshot: Option<EditableSnapshot>,
    /// See `RequestTab::breadcrumb_name`/`breadcrumb_synced_path` — the
    /// active tab's copies, swapped like every other top-level field of the
    /// same name.
    breadcrumb_name: String,
    breadcrumb_synced_path: Option<String>,
    /// Every tab's state (FR-19) — see `RequestTab`'s own doc comment for
    /// the swap-based design: `tabs[active_tab_idx]` is a stale placeholder
    /// while `CurlyApp`'s own top-level fields above hold that tab's real,
    /// live state. Never empty — closing the last tab resets it in place
    /// instead of leaving zero tabs open.
    tabs: Vec<RequestTab>,
    active_tab_idx: usize,

    /// What the central panel is currently showing (M8) — workspace-level,
    /// not per-tab: switching to a Collection/Folder/Environment view
    /// leaves whichever request tab is active untouched underneath.
    central_view: CentralView,
    /// Collection/folder tabs opened on demand (M8) — see `ExtraTabKind`.
    extra_tabs: Vec<ExtraTabKind>,

    /// Which top-level view the left sidebar shows (M8) — Project
    /// (Collections/Environments/Session) or History.
    sidebar_tab: SidebarTab,

    /// Cached copies of `ui.ctx().zoom_factor()`/`.theme()`, refreshed once
    /// per frame — `eframe::App::on_exit` (where these get persisted) isn't
    /// handed an `egui::Context`, so there's nothing else to read them from
    /// at that point.
    last_zoom: f32,
    last_theme: egui::Theme,
    /// Whether the theme should keep following the OS preference live
    /// (checked periodically — see `ui()`) or has been pinned by a manual
    /// toggle click. Persisted across restarts (`persistence::PersistedState`)
    /// so a manual choice sticks until toggled again, the same way a manual
    /// light/dark override behaves in most apps that also support "follow
    /// the OS".
    theme_follows_system: bool,
    last_theme_check: std::time::Instant,
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
    /// Also restores persisted zoom/theme/tabs (see `persistence`) where a
    /// preferences file exists; otherwise seeds the theme from the OS's
    /// light/dark preference where it can be detected (FR-24) — see
    /// `detect_system_theme`'s doc comment for why egui's own built-in
    /// "follow the system theme" doesn't work on Linux.
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut app = Self::default_state();

        match persistence::load() {
            Some(prefs) => {
                cc.egui_ctx.set_zoom_factor(clamp_zoom(prefs.zoom));
                app.theme_follows_system = prefs.theme_follows_system;
                if prefs.theme_follows_system {
                    if let Some(theme) = detect_system_theme() {
                        cc.egui_ctx.set_theme(theme);
                    }
                } else {
                    cc.egui_ctx.set_theme(egui::Theme::from(prefs.theme));
                }
                if !prefs.tabs.is_empty() {
                    app.tabs = prefs
                        .tabs
                        .into_iter()
                        .map(|t| RequestTab {
                            method: t.method,
                            url: t.url,
                            headers: t.headers,
                            body: t.body,
                            auth: t.auth,
                            loaded_request: t.loaded_request,
                            ..Default::default()
                        })
                        .collect();
                    let idx = prefs.active_tab_idx.min(app.tabs.len() - 1);
                    app.active_tab_idx = idx;
                    let active = std::mem::take(&mut app.tabs[idx]);
                    app.restore_active_tab(active);
                }
            }
            None => {
                if let Some(theme) = detect_system_theme() {
                    cc.egui_ctx.set_theme(theme);
                }
            }
        }

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
            auth: AuthEditor::default(),
            response: None,
            response_image: None,
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
            history_entries: Vec::new(),
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
            path_dialog: None,
            folder_dialog: None,
            collection_notice: None,
            response_search: String::new(),
            response_search_current: 0,
            response_save_rx: None,
            response_save_notice: None,
            response_subtab: ResponseSubtab::default(),
            saved_snapshot: Some(EditableSnapshot::blank()),
            breadcrumb_name: String::new(),
            breadcrumb_synced_path: None,
            tabs: vec![RequestTab::blank()],
            active_tab_idx: 0,
            central_view: CentralView::default(),
            extra_tabs: Vec::new(),
            sidebar_tab: SidebarTab::default(),
            last_zoom: 1.0,
            last_theme: egui::Theme::Dark,
            theme_follows_system: true,
            last_theme_check: std::time::Instant::now(),
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

    /// Show a native "Save As" dialog (async, same reason `open_project_dialog`
    /// is — see its doc comment) and write `body` to wherever the user
    /// picks. `default_name` seeds the dialog's filename (e.g.
    /// `response.json` vs `response.txt`) — purely a starting suggestion,
    /// the user can change it. A cancelled dialog reports nothing; a write
    /// failure or success both set `response_save_notice`.
    ///
    /// Takes the raw `body_bytes`, not `body` (the lossy-UTF8-decoded
    /// `String`) — writing `body` would silently corrupt any response
    /// that isn't valid UTF-8 text (an image response, most notably).
    fn save_response_to_file(&mut self, ctx: &egui::Context, body: Vec<u8>, default_name: &str) {
        let (tx, rx) = mpsc::channel();
        self.response_save_rx = Some(rx);
        let ctx = ctx.clone();
        let default_name = default_name.to_string();
        self.runtime.spawn(async move {
            let picked = rfd::AsyncFileDialog::new().set_file_name(&default_name).save_file().await;
            let message = match picked {
                Some(handle) => match tokio::fs::write(handle.path(), &body).await {
                    Ok(()) => Some(format!("saved response to {}", handle.path().display())),
                    Err(e) => Some(format!("failed to save response: {e}")),
                },
                None => None,
            };
            let _ = tx.send(message);
            ctx.request_repaint();
        });
    }

    fn poll_response_save(&mut self) {
        let Some(rx) = &self.response_save_rx else {
            return;
        };
        if let Ok(message) = rx.try_recv() {
            self.response_save_rx = None;
            if let Some(message) = message {
                self.response_save_notice = Some(message);
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
        self.refresh_history();
    }

    /// Reload the sidebar's history list from the active project's storage
    /// (FR-6/FR-18) — called on project open/switch (via
    /// `refresh_project_lists`) and again right after every send, so a
    /// just-sent request shows up without needing to reopen the project.
    fn refresh_history(&mut self) {
        self.history_entries = self
            .active_project()
            .and_then(|p| p.storage.list_history(HISTORY_DISPLAY_LIMIT).ok())
            .unwrap_or_default();
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

        self.auth = match &saved.auth {
            Some(SavedAuth::Basic { username, password }) => AuthEditor {
                kind: AuthKind::Basic,
                username: username.clone(),
                password: password.clone(),
                ..Default::default()
            },
            Some(SavedAuth::Bearer { token }) => AuthEditor {
                kind: AuthKind::Bearer,
                token: token.clone(),
                ..Default::default()
            },
            None => AuthEditor::default(),
        };

        if self.headers.is_empty() {
            self.headers.push(HeaderRow::empty());
        }

        // One combined substitution attempt (url + all header values + body +
        // auth fields, newline-joined) rather than several separate ones, so
        // a request with multiple undefined variables gets a single notice
        // naming all of them (substitute() already reports every undefined
        // name, not just the first) instead of overlapping ones.
        let combined = format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            self.url,
            self.headers.iter().map(|h| h.value.as_str()).collect::<Vec<_>>().join("\n"),
            self.body,
            self.auth.username,
            self.auth.password,
            self.auth.token,
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
        self.saved_snapshot = Some(self.current_snapshot());
    }

    /// Load a history entry back into the editor for a re-run (FR-6). Unlike
    /// `load_saved_request`, a `HistoryEntry` is already fully resolved (no
    /// `{{variable}}` templates — it was captured post-substitution at send
    /// time), so there's no undefined-variable check here. It also isn't a
    /// saved collection entry, so `loaded_request`/`extract_rules` are
    /// cleared rather than carried over — a later Save must go through
    /// Save As, not silently overwrite some unrelated saved request.
    fn load_history_entry(&mut self, entry: &HistoryEntry) {
        self.method = entry.method.clone();
        self.url = entry.url.clone();

        let mut notices: Vec<String> = Vec::new();

        let mut redacted_count = 0;
        self.headers = entry
            .request_headers
            .iter()
            .filter(|(_, value)| {
                let redacted = value == REDACTED;
                if redacted {
                    redacted_count += 1;
                }
                !redacted
            })
            .map(|(name, value)| HeaderRow {
                name: name.clone(),
                value: value.clone(),
                enabled: true,
            })
            .collect();
        if redacted_count > 0 {
            notices.push(format!(
                "{redacted_count} header(s) were redacted in history and weren't restored — re-add them if needed"
            ));
        }
        if self.headers.is_empty() {
            self.headers.push(HeaderRow::empty());
        }

        self.body = entry.request_body.clone().unwrap_or_default();

        self.auth = match entry.request_auth_kind {
            Some(kind) => {
                notices.push(
                    "this request used Basic/Bearer auth — credentials aren't stored in \
                     history, re-enter them before sending"
                        .to_string(),
                );
                AuthEditor {
                    kind: kind.into(),
                    ..Default::default()
                }
            }
            None => AuthEditor::default(),
        };

        self.loaded_request = None;
        self.extract_rules.clear();
        self.session_notice = None;
        self.response = None;
        self.load_notice = if notices.is_empty() {
            None
        } else {
            Some(format!(
                "Loaded from history (\"{} {}\"), but {}.",
                entry.method,
                entry.url,
                notices.join("; and ")
            ))
        };
        // Unlike a saved request, there's nothing on disk this matches — it
        // shows as needing a save (Save As) rather than falsely "clean".
        self.saved_snapshot = None;
    }

    /// Capture the active tab's current editor fields as an `EditableSnapshot`.
    fn current_snapshot(&self) -> EditableSnapshot {
        EditableSnapshot::capture(&self.method, &self.url, &self.headers, &self.body, &self.auth, &self.extract_rules)
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
        match self.auth.kind {
            AuthKind::None => {}
            AuthKind::Basic => {
                let username = substitution::substitute(&self.auth.username, &variables)?;
                let password = substitution::substitute(&self.auth.password, &variables)?;
                request = request.with_auth(Auth::Basic { username, password });
            }
            AuthKind::Bearer => {
                let token = substitution::substitute(&self.auth.token, &variables)?;
                request = request.with_auth(Auth::Bearer { token });
            }
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

        // Captured before the request goes out, same reasoning as
        // `pending_extraction` above — the active project shouldn't be able
        // to change out from under an in-flight send.
        let history_storage = self.active_project().map(|p| p.storage.clone());

        let client = self.client.clone();
        let ctx = ctx.clone();
        self.runtime.spawn(async move {
            let outcome = match exec::send(&client, &request).await {
                Ok(resp) => {
                    if let Some(storage) = &history_storage {
                        // A history-write failure is swallowed, same as the
                        // CLI's own `execute.rs` — never worth failing a
                        // successful send over.
                        let _ = storage.append_history(&HistoryEntry::new(&request, &resp));
                    }
                    SendOutcome::Success(resp)
                }
                Err(e) => SendOutcome::Error(e.to_string()),
            };
            let _ = tx.send(outcome);
            ctx.request_repaint();
        });
    }

    fn poll_response(&mut self) {
        if let Some(rx) = &self.rx {
            if let Ok(outcome) = rx.try_recv() {
                self.in_flight = false;
                match outcome {
                    SendOutcome::Success(r) => {
                        self.apply_pending_extraction(&r);
                        self.response = Some(Ok(r));
                        self.response_image = None;
                        self.response_search_current = 0;
                        self.refresh_history();
                    }
                    SendOutcome::Error(e) => {
                        self.pending_extraction = None;
                        self.response = Some(Err(e));
                        self.response_image = None;
                        self.response_search_current = 0;
                    }
                }
                self.rx = None;
            }
        }
        self.poll_background_tab_responses();
    }

    /// The same handling `poll_response` gives the *active* tab's `rx`,
    /// applied to every other tab's — a background send keeps running
    /// while you're looking at a different tab (see `RequestTab`'s doc
    /// comment), and its result/extraction/history entry shouldn't wait
    /// for you to switch back to see it. Marks `has_unseen_response` so the
    /// tab strip can show a "response ready" indicator.
    fn poll_background_tab_responses(&mut self) {
        for i in 0..self.tabs.len() {
            if i == self.active_tab_idx {
                continue;
            }
            let Some(rx) = &self.tabs[i].rx else { continue };
            let Ok(outcome) = rx.try_recv() else { continue };
            self.tabs[i].in_flight = false;
            self.tabs[i].rx = None;
            self.tabs[i].has_unseen_response = true;
            match outcome {
                SendOutcome::Success(r) => {
                    let pending = self.tabs[i].pending_extraction.take();
                    if let Some(pending) = pending {
                        if let Some(notice) = self.apply_extraction_for(pending, &r) {
                            self.tabs[i].session_notice = Some(notice);
                        }
                    }
                    self.tabs[i].response = Some(Ok(r));
                    self.tabs[i].response_image = None;
                    self.tabs[i].response_search_current = 0;
                    self.refresh_history();
                }
                SendOutcome::Error(e) => {
                    self.tabs[i].pending_extraction = None;
                    self.tabs[i].response = Some(Err(e));
                    self.tabs[i].response_image = None;
                    self.tabs[i].response_search_current = 0;
                }
            }
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
        if let Some(notice) = self.apply_extraction_for(pending, response) {
            self.session_notice = Some(notice);
        }
    }

    /// The extraction-on-success logic itself, decoupled from *which* tab's
    /// `pending_extraction`/`session_notice` it's operating on — reused by
    /// `apply_pending_extraction` (active tab) and
    /// `poll_background_tab_responses` (any other tab). `None` means
    /// "nothing to report" (a non-2xx response — an expected absence, not
    /// a problem); `Some(message)` covers both an extraction failure and a
    /// success.
    fn apply_extraction_for(
        &mut self,
        pending: PendingExtraction,
        response: &ResponseSummary,
    ) -> Option<String> {
        if !(200..300).contains(&response.status) {
            return None;
        }

        let extracted = match extraction::apply(&pending.rules, response, &pending.variables) {
            Ok(e) => e,
            Err(e) => return Some(format!("extraction failed: {e}")),
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
        Some(message)
    }

    /// Build a `SavedRequest` from the editor's current fields (FR-20) — a
    /// disabled or blank-name header is dropped, same rule `build_request`
    /// already applies when actually sending; extraction rules carry over
    /// as-is from `extract_rules` (so editing a loaded request and saving
    /// it back keeps whatever rules it already had). `auth` carries over
    /// raw (unsubstituted), same treatment as headers/body.
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
        saved.auth = match self.auth.kind {
            AuthKind::None => None,
            AuthKind::Basic => Some(SavedAuth::Basic {
                username: self.auth.username.clone(),
                password: self.auth.password.clone(),
            }),
            AuthKind::Bearer => Some(SavedAuth::Bearer {
                token: self.auth.token.clone(),
            }),
        };
        saved.extract = self
            .extract_rules
            .iter()
            .filter(|e| !e.name().trim().is_empty())
            .cloned()
            .collect();
        Ok(saved)
    }

    /// Reset the editor to a blank state, without touching the active
    /// project/environment/overrides — the counterpart to loading a saved
    /// request, so starting a new one doesn't risk a later "Save"
    /// overwriting whatever was loaded before.
    fn new_request(&mut self) {
        self.restore_active_tab(RequestTab::blank());
    }

    /// Move every per-request field out of `self` into an owned
    /// `RequestTab` — see `RequestTab`'s doc comment for why this
    /// move-based swap, rather than a `self.active_tab_mut().field` style,
    /// is how tabs work here. Leaves `self`'s own fields at their `Default`
    /// (mostly empty/`None`) until `restore_active_tab` is called with
    /// something to put back — a caller that forgets to immediately follow
    /// up would leave the editor looking blank, same visible failure mode
    /// as any other bug, not a silent one.
    fn take_active_tab(&mut self) -> RequestTab {
        RequestTab {
            method: std::mem::take(&mut self.method),
            url: std::mem::take(&mut self.url),
            headers: std::mem::take(&mut self.headers),
            body: std::mem::take(&mut self.body),
            auth: std::mem::take(&mut self.auth),
            response: self.response.take(),
            response_image: self.response_image.take(),
            in_flight: std::mem::take(&mut self.in_flight),
            rx: self.rx.take(),
            var_overrides: std::mem::take(&mut self.var_overrides),
            extract_rules: std::mem::take(&mut self.extract_rules),
            pending_extraction: self.pending_extraction.take(),
            session_notice: self.session_notice.take(),
            load_notice: self.load_notice.take(),
            loaded_request: self.loaded_request.take(),
            response_search: std::mem::take(&mut self.response_search),
            response_search_current: std::mem::take(&mut self.response_search_current),
            response_save_rx: self.response_save_rx.take(),
            response_save_notice: self.response_save_notice.take(),
            response_subtab: std::mem::take(&mut self.response_subtab),
            saved_snapshot: self.saved_snapshot.take(),
            breadcrumb_name: std::mem::take(&mut self.breadcrumb_name),
            breadcrumb_synced_path: self.breadcrumb_synced_path.take(),
            // The active tab never shows its own "response ready" badge —
            // see the field's doc comment.
            has_unseen_response: false,
        }
    }

    /// The inverse of `take_active_tab` — moves `tab`'s fields back onto
    /// `self`, making it the live, active state. `tab.has_unseen_response`
    /// is deliberately dropped here — see that field's doc comment.
    fn restore_active_tab(&mut self, tab: RequestTab) {
        self.method = tab.method;
        self.url = tab.url;
        self.headers = tab.headers;
        self.body = tab.body;
        self.auth = tab.auth;
        self.response = tab.response;
        self.response_image = tab.response_image;
        self.in_flight = tab.in_flight;
        self.rx = tab.rx;
        self.var_overrides = tab.var_overrides;
        self.extract_rules = tab.extract_rules;
        self.pending_extraction = tab.pending_extraction;
        self.session_notice = tab.session_notice;
        self.load_notice = tab.load_notice;
        self.loaded_request = tab.loaded_request;
        self.response_search = tab.response_search;
        self.response_search_current = tab.response_search_current;
        self.response_save_rx = tab.response_save_rx;
        self.response_save_notice = tab.response_save_notice;
        self.response_subtab = tab.response_subtab;
        self.saved_snapshot = tab.saved_snapshot;
        self.breadcrumb_name = tab.breadcrumb_name;
        self.breadcrumb_synced_path = tab.breadcrumb_synced_path;
    }

    /// Open a new blank tab and make it active — the editor equivalent of
    /// `new_request`, except the tab you were on is kept (parked in
    /// `tabs`) rather than overwritten. Bound to the toolbar's `+` and
    /// Ctrl/Cmd+T (FR-23).
    fn open_new_tab(&mut self) {
        let outgoing = self.take_active_tab();
        self.tabs[self.active_tab_idx] = outgoing;
        self.restore_active_tab(RequestTab::blank());
        self.tabs.push(RequestTab::blank());
        self.active_tab_idx = self.tabs.len() - 1;
        self.save_dialog = None;
    }

    /// Switch the active tab — parks the current tab's live state into
    /// `tabs[active_tab_idx]` and loads `tabs[idx]`'s state back onto
    /// `self`. A no-op for an out-of-range or already-active `idx`.
    /// Cancels any open Save dialog: it was pre-filled from whichever tab
    /// was active when it opened, and confirming it after switching tabs
    /// would save the *new* active tab's content under that stale
    /// pre-fill — safer to just close it than risk that mismatch.
    fn switch_to_tab(&mut self, idx: usize) {
        if idx >= self.tabs.len() || idx == self.active_tab_idx {
            return;
        }
        let outgoing = self.take_active_tab();
        self.tabs[self.active_tab_idx] = outgoing;
        let incoming = std::mem::take(&mut self.tabs[idx]);
        self.restore_active_tab(incoming);
        self.active_tab_idx = idx;
        self.save_dialog = None;
    }

    /// Close the tab at `idx`. Closing the last remaining tab resets it to
    /// blank in place instead — there's always at least one tab open, same
    /// as there's always a Response pane even with nothing sent yet.
    /// Closing the active tab picks a neighbor (the same index if
    /// possible, otherwise the new last tab) to become active; closing an
    /// *inactive* tab just drops its slot and shifts `active_tab_idx` down
    /// by one if it came after the closed tab.
    fn close_tab(&mut self, idx: usize) {
        if idx >= self.tabs.len() {
            return;
        }
        if self.tabs.len() == 1 {
            self.new_request();
            return;
        }
        if idx == self.active_tab_idx {
            self.tabs.remove(idx);
            let new_active = idx.min(self.tabs.len() - 1);
            let incoming = std::mem::take(&mut self.tabs[new_active]);
            self.restore_active_tab(incoming);
            self.active_tab_idx = new_active;
        } else {
            self.tabs.remove(idx);
            if idx < self.active_tab_idx {
                self.active_tab_idx -= 1;
            }
        }
        self.save_dialog = None;
    }

    /// A short display label for the tab at `idx` — the loaded request's
    /// path if it has one, else "METHOD url", else "New Request". Reads
    /// the *live* top-level fields for the active tab rather than
    /// `tabs[active_tab_idx]` (a stale placeholder while active — see
    /// `RequestTab`'s doc comment), so the tab bar reflects what you're
    /// typing right now, not what was there before you last switched away.
    fn tab_label(&self, idx: usize) -> String {
        let (loaded, method, url, headers, body, auth, extract_rules, snapshot) = if idx == self.active_tab_idx {
            (
                &self.loaded_request,
                self.method.as_str(),
                self.url.as_str(),
                self.headers.as_slice(),
                self.body.as_str(),
                &self.auth,
                self.extract_rules.as_slice(),
                &self.saved_snapshot,
            )
        } else {
            let tab = &self.tabs[idx];
            (
                &tab.loaded_request,
                tab.method.as_str(),
                tab.url.as_str(),
                tab.headers.as_slice(),
                tab.body.as_str(),
                &tab.auth,
                tab.extract_rules.as_slice(),
                &tab.saved_snapshot,
            )
        };

        let label = if let Some(loaded) = loaded {
            loaded.path.clone()
        } else if !url.trim().is_empty() {
            format!("{method} {url}")
        } else {
            "New Request".to_string()
        };

        const MAX_LEN: usize = 24;
        let label = if label.chars().count() > MAX_LEN {
            format!("{}…", label.chars().take(MAX_LEN).collect::<String>())
        } else {
            label
        };

        // "*" (M8) — needs saving, mirroring how most editors mark an
        // unsaved buffer.
        if is_tab_dirty(snapshot, method, url, headers, body, auth, extract_rules) {
            format!("* {label}")
        } else {
            label
        }
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
        self.path_dialog = None;
        self.folder_dialog = None;
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
                self.saved_snapshot = Some(self.current_snapshot());
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
            self.saved_snapshot = Some(self.current_snapshot());
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

    /// Open the "Rename/Move" or "Duplicate" dialog for the request at
    /// `path` in `collection` (the sidebar tree's "✎"/"⧉" buttons),
    /// pre-filled from its current location.
    fn open_path_dialog(&mut self, mode: PathDialogMode, collection: String, path: String) {
        let (folder_path, name) = split_folder_and_name(&path);
        self.path_dialog = Some(PathDialog {
            mode,
            collection,
            old_path: path,
            folder_path,
            name,
            error: None,
        });
        self.save_dialog = None;
        self.folder_dialog = None;
    }

    fn cancel_path_dialog(&mut self) {
        self.path_dialog = None;
    }

    /// Render the "Rename/Move"/"Duplicate" floating window if `path_dialog`
    /// is `Some` — same local-flags structure as `show_save_dialog`, for
    /// the same borrow-checking reason (acting on `self` while `dialog`
    /// still borrows `self.path_dialog` wouldn't compile).
    fn show_path_dialog(&mut self, ctx: &egui::Context) {
        let mut confirmed = false;
        let mut cancelled = false;

        if let Some(dialog) = self.path_dialog.as_mut() {
            let title = match dialog.mode {
                PathDialogMode::Rename => "Rename / Move Request",
                PathDialogMode::Duplicate => "Duplicate Request",
            };
            let mut open = true;
            egui::Window::new(title)
                .collapsible(false)
                .resizable(false)
                .open(&mut open)
                .show(ctx, |ui| {
                    ui.label(format!("From: \"{}/{}\"", dialog.collection, dialog.old_path));
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
                        let confirm_label = match dialog.mode {
                            PathDialogMode::Rename => "Rename",
                            PathDialogMode::Duplicate => "Duplicate",
                        };
                        if ui.button(confirm_label).clicked() {
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
            self.confirm_path_dialog();
        } else if cancelled {
            self.cancel_path_dialog();
        }
    }

    /// Rename/move the request at `old_path` in `collection_name` to
    /// `new_path` on disk — shared by `confirm_path_dialog`'s Rename mode
    /// and the breadcrumb's inline rename-Save (M8), so there's one
    /// implementation of "rename and update whatever currently points at
    /// the old path," not two. If the currently *loaded* request is the
    /// one being moved, `loaded_request` is updated to its new location
    /// (mirrors `delete_saved_request`'s "is this the thing that's
    /// loaded?" check). Returns an error message on failure — the caller
    /// decides where to show it (a dialog's own `error` field, or
    /// `collection_notice`).
    fn rename_saved_request(&mut self, collection_name: &str, old_path: &str, new_path: &str) -> Result<(), String> {
        let result: anyhow::Result<()> = match self.active_project() {
            Some(project) => (|| {
                let mut collection = project.storage.load_collection(collection_name)?;
                collection.rename_or_move_request(old_path, new_path)?;
                project.storage.save_collection(&collection)
            })(),
            None => Err(anyhow::anyhow!("no project is open")),
        };
        match result {
            Ok(()) => {
                if self
                    .loaded_request
                    .as_ref()
                    .is_some_and(|r| r.collection == collection_name && r.path == old_path)
                {
                    self.loaded_request = Some(LoadedRequestRef {
                        collection: collection_name.to_string(),
                        path: new_path.to_string(),
                    });
                }
                self.refresh_project_lists();
                Ok(())
            }
            Err(e) => Err(e.to_string()),
        }
    }

    /// Copy the request at `path` in `collection_name` to `new_path`,
    /// leaving the original (and `loaded_request`) untouched — the
    /// original stays loaded, the copy isn't auto-loaded. Shared by
    /// `confirm_path_dialog`'s Duplicate mode; kept separate from
    /// `rename_saved_request` since a duplicate never touches
    /// `loaded_request`.
    fn duplicate_saved_request(&mut self, collection_name: &str, path: &str, new_path: &str) -> Result<(), String> {
        let result: anyhow::Result<()> = match self.active_project() {
            Some(project) => (|| {
                let mut collection = project.storage.load_collection(collection_name)?;
                collection.duplicate_request(path, new_path)?;
                project.storage.save_collection(&collection)
            })(),
            None => Err(anyhow::anyhow!("no project is open")),
        };
        match result {
            Ok(()) => {
                self.refresh_project_lists();
                Ok(())
            }
            Err(e) => Err(e.to_string()),
        }
    }

    /// Apply the rename/move or duplicate the dialog describes.
    fn confirm_path_dialog(&mut self) {
        let Some(dialog) = self.path_dialog.as_ref() else {
            return;
        };
        let name = dialog.name.trim().to_string();
        if name.is_empty() {
            self.path_dialog.as_mut().unwrap().error = Some("name is required".to_string());
            return;
        }
        let folder = dialog.folder_path.trim().trim_matches('/').to_string();
        let new_path = if folder.is_empty() { name } else { format!("{folder}/{name}") };
        let collection_name = dialog.collection.clone();
        let old_path = dialog.old_path.clone();
        let mode = dialog.mode;

        let result = match mode {
            PathDialogMode::Rename => self.rename_saved_request(&collection_name, &old_path, &new_path),
            PathDialogMode::Duplicate => self.duplicate_saved_request(&collection_name, &old_path, &new_path),
        };

        match result {
            Ok(()) => {
                self.collection_notice = Some(match mode {
                    PathDialogMode::Rename => format!("renamed \"{old_path}\" to \"{new_path}\""),
                    PathDialogMode::Duplicate => format!("duplicated \"{old_path}\" to \"{new_path}\""),
                });
                self.path_dialog = None;
            }
            Err(e) => self.path_dialog.as_mut().unwrap().error = Some(e),
        }
    }

    /// Open the "New Folder" window (M8) — from a collection's or folder's
    /// "⋯" menu. `parent_path` is `""` for a new top-level folder.
    fn open_folder_dialog(&mut self, collection: String, parent_path: String) {
        self.folder_dialog = Some(FolderDialog {
            collection,
            parent_path,
            name: String::new(),
            error: None,
        });
        self.save_dialog = None;
        self.path_dialog = None;
    }

    fn cancel_folder_dialog(&mut self) {
        self.folder_dialog = None;
    }

    /// Render the "New Folder" floating window if `folder_dialog` is
    /// `Some` — same local-flags structure as `show_save_dialog`/
    /// `show_path_dialog`.
    fn show_folder_dialog(&mut self, ctx: &egui::Context) {
        let mut confirmed = false;
        let mut cancelled = false;

        if let Some(dialog) = self.folder_dialog.as_mut() {
            let mut open = true;
            egui::Window::new("New Folder")
                .collapsible(false)
                .resizable(false)
                .open(&mut open)
                .show(ctx, |ui| {
                    let parent = if dialog.parent_path.is_empty() {
                        dialog.collection.clone()
                    } else {
                        format!("{}/{}", dialog.collection, dialog.parent_path)
                    };
                    ui.label(format!("In: \"{parent}\""));
                    ui.horizontal(|ui| {
                        ui.label("Name:");
                        ui.text_edit_singleline(&mut dialog.name);
                    });
                    if let Some(err) = &dialog.error {
                        ui.colored_label(egui::Color32::from_rgb(220, 80, 80), err);
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Create").clicked() {
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
            self.confirm_folder_dialog();
        } else if cancelled {
            self.cancel_folder_dialog();
        }
    }

    fn confirm_folder_dialog(&mut self) {
        let Some(dialog) = self.folder_dialog.as_ref() else {
            return;
        };
        let name = dialog.name.trim().trim_matches('/').to_string();
        if name.is_empty() {
            self.folder_dialog.as_mut().unwrap().error = Some("name is required".to_string());
            return;
        }
        let path = if dialog.parent_path.is_empty() {
            name.clone()
        } else {
            format!("{}/{name}", dialog.parent_path)
        };
        let collection_name = dialog.collection.clone();

        let result: anyhow::Result<()> = match self.active_project() {
            Some(project) => (|| {
                let mut collection = project
                    .storage
                    .load_collection_opt(&collection_name)?
                    .unwrap_or_else(|| Collection::new(&collection_name));
                collection.add_folder(&path);
                project.storage.save_collection(&collection)
            })(),
            None => Err(anyhow::anyhow!("no project is open")),
        };

        match result {
            Ok(()) => {
                self.refresh_project_lists();
                self.collection_notice = Some(format!("created folder \"{path}\" in \"{collection_name}\""));
                self.folder_dialog = None;
            }
            Err(e) => self.folder_dialog.as_mut().unwrap().error = Some(e.to_string()),
        }
    }

    /// Same as `open_save_dialog`, but pre-filled with an explicit
    /// collection/folder instead of defaulting from `loaded_request` —
    /// used by a folder's/collection's "⋯" → New Request (M8), where the
    /// blank tab just opened has no `loaded_request` of its own to infer
    /// a location from.
    fn open_save_dialog_in(&mut self, collection: String, folder_path: String) {
        self.save_dialog = Some(SaveDialog {
            collection,
            folder_path,
            name: String::new(),
            error: None,
        });
        self.path_dialog = None;
        self.folder_dialog = None;
    }

    /// Remove the folder at `path` (M8) — refuses (surfacing the error via
    /// `collection_notice`, same as every other collection-mutating method
    /// here) rather than cascading through a non-empty folder; delete its
    /// contents individually first. Mirrors `delete_saved_request`'s
    /// "acts immediately, no confirmation dialog" convention for the empty
    /// case.
    fn delete_folder(&mut self, collection_name: &str, path: &str) {
        let result: anyhow::Result<bool> = match self.active_project() {
            Some(project) => (|| {
                let mut collection = project.storage.load_collection(collection_name)?;
                let removed = collection.remove_folder(path, false)?;
                project.storage.save_collection(&collection)?;
                Ok(removed)
            })(),
            None => Err(anyhow::anyhow!("no project is open")),
        };

        match result {
            Ok(true) => {
                self.refresh_project_lists();
                self.collection_notice = Some(format!("deleted folder \"{path}\" from \"{collection_name}\""));
            }
            Ok(false) => {
                self.collection_notice = Some(format!("no folder found at \"{path}\""));
            }
            Err(e) => self.collection_notice = Some(format!("failed to delete \"{path}\": {e}")),
        }
    }

    /// Save every open tab whose loaded request lives in `collection_name`
    /// and has unsaved changes (M8's collection "⋯" → Save All). Reuses
    /// `switch_to_tab`/`save_over_loaded` as-is for each dirty tab in turn
    /// — the same take/restore swap that already backs normal tab
    /// switching — rather than a parallel "save a background tab" code
    /// path, then restores whichever tab was active beforehand.
    fn save_all_in_collection(&mut self, collection_name: &str) {
        let original_active = self.active_tab_idx;
        let mut saved_count = 0;
        for i in 0..self.tabs.len() {
            let (loaded, dirty) = if i == self.active_tab_idx {
                (self.loaded_request.clone(), self.is_active_tab_dirty())
            } else {
                let tab = &self.tabs[i];
                (
                    tab.loaded_request.clone(),
                    is_tab_dirty(
                        &tab.saved_snapshot,
                        &tab.method,
                        &tab.url,
                        &tab.headers,
                        &tab.body,
                        &tab.auth,
                        &tab.extract_rules,
                    ),
                )
            };
            let matches = loaded.is_some_and(|l| l.collection == collection_name);
            if matches && dirty {
                self.switch_to_tab(i);
                self.save_over_loaded();
                saved_count += 1;
            }
        }
        self.switch_to_tab(original_active);
        self.collection_notice = Some(if saved_count == 0 {
            format!("nothing to save in \"{collection_name}\"")
        } else {
            format!("saved {saved_count} request(s) in \"{collection_name}\"")
        });
    }

    /// Resync `breadcrumb_name` from `loaded_request` if it's fallen out of
    /// date (a different request just loaded, this one was just renamed
    /// elsewhere, or nothing's loaded at all) — called once per frame
    /// before the breadcrumb renders, rather than threaded through every
    /// place that sets `loaded_request`. A no-op once synced, so it's safe
    /// to call unconditionally every frame even while the user is mid-edit.
    fn sync_breadcrumb(&mut self) {
        let current_path = self.loaded_request.as_ref().map(|l| l.path.clone());
        if self.breadcrumb_synced_path != current_path {
            self.breadcrumb_name = current_path
                .as_deref()
                .map(|p| split_folder_and_name(p).1)
                .unwrap_or_default();
            self.breadcrumb_synced_path = current_path;
        }
    }

    /// Commit the breadcrumb's editable trailing segment as the loaded
    /// request's new name (M8) — the folder stays fixed; only the request's
    /// own name changes. A no-op if nothing's loaded or the name didn't
    /// actually change.
    fn save_breadcrumb_name(&mut self) {
        let Some(loaded) = self.loaded_request.clone() else {
            return;
        };
        let new_name = self.breadcrumb_name.trim().to_string();
        if new_name.is_empty() {
            self.collection_notice = Some("name is required".to_string());
            return;
        }
        let (folder, _old_name) = split_folder_and_name(&loaded.path);
        let new_path = if folder.is_empty() { new_name } else { format!("{folder}/{new_name}") };
        if new_path == loaded.path {
            return;
        }
        match self.rename_saved_request(&loaded.collection, &loaded.path, &new_path) {
            Ok(()) => {
                self.collection_notice = Some(format!("renamed \"{}\" to \"{new_path}\"", loaded.path));
                self.sync_breadcrumb();
            }
            Err(e) => self.collection_notice = Some(format!("failed to rename: {e}")),
        }
    }

    /// Open (or, if already open, just focus) the one collection/folder tab
    /// for `entry` — shared by the sidebar's collection-name and
    /// folder-name clicks (M8).
    fn open_or_focus_extra_tab(&mut self, entry: ExtraTabKind) {
        if !self.extra_tabs.contains(&entry) {
            self.extra_tabs.push(entry.clone());
        }
        self.central_view = entry.central_view();
    }

    /// Render a Collection main tab's content (M8) — the collection's
    /// top-level folders/requests, plus the same New Folder/New
    /// Request/Save All actions as its sidebar "⋯" menu.
    fn show_collection_tab(&mut self, ui: &mut egui::Ui, collection_name: &str) {
        ui.heading(collection_name);
        let Some(collection) = self.collections.iter().find(|c| c.name == collection_name).cloned() else {
            ui.weak("This collection no longer exists — it may have been renamed or deleted.");
            return;
        };
        ui.horizontal(|ui| {
            if ui.button("Save All").clicked() {
                self.save_all_in_collection(collection_name);
            }
            if ui.button("New Folder").clicked() {
                self.open_folder_dialog(collection_name.to_string(), String::new());
            }
            if ui.button("New Request").clicked() {
                self.open_new_tab();
                self.open_save_dialog_in(collection_name.to_string(), String::new());
            }
        });
        if let Some(notice) = &self.collection_notice {
            ui.colored_label(egui::Color32::from_rgb(120, 190, 130), notice);
        }
        ui.separator();
        for folder in &collection.folders {
            if ui.selectable_label(false, format!("{}/", folder.name)).clicked() {
                self.open_or_focus_extra_tab(ExtraTabKind::Folder(collection_name.to_string(), folder.name.clone()));
            }
        }
        for request in &collection.requests {
            let label = format!("{:<7} {}", request.method, request.name);
            if ui.selectable_label(false, label).clicked() {
                self.loaded_request = Some(LoadedRequestRef {
                    collection: collection_name.to_string(),
                    path: request.name.clone(),
                });
                self.load_saved_request(request);
                self.central_view = CentralView::Request;
            }
        }
    }

    /// Render a Folder main tab's content (M8) — the folder's own
    /// sub-folders/requests, plus New Folder/New Request scoped to it.
    fn show_folder_tab(&mut self, ui: &mut egui::Ui, collection_name: &str, folder_path: &str) {
        let leaf = split_folder_and_name(folder_path).1;
        ui.heading(format!("{leaf}/"));
        ui.weak(format!("{collection_name} › {folder_path}"));
        let Some(collection) = self.collections.iter().find(|c| c.name == collection_name).cloned() else {
            ui.weak("This collection no longer exists — it may have been renamed or deleted.");
            return;
        };
        let Some(folder) = find_folder(&collection.folders, folder_path).cloned() else {
            ui.weak("This folder no longer exists — it may have been renamed or deleted.");
            return;
        };
        ui.horizontal(|ui| {
            if ui.button("New Folder").clicked() {
                self.open_folder_dialog(collection_name.to_string(), folder_path.to_string());
            }
            if ui.button("New Request").clicked() {
                self.open_new_tab();
                self.open_save_dialog_in(collection_name.to_string(), folder_path.to_string());
            }
        });
        if let Some(notice) = &self.collection_notice {
            ui.colored_label(egui::Color32::from_rgb(120, 190, 130), notice);
        }
        ui.separator();
        for sub in &folder.folders {
            let sub_path = format!("{folder_path}/{}", sub.name);
            if ui.selectable_label(false, format!("{}/", sub.name)).clicked() {
                self.open_or_focus_extra_tab(ExtraTabKind::Folder(collection_name.to_string(), sub_path));
            }
        }
        for request in &folder.requests {
            let label = format!("{:<7} {}", request.method, request.name);
            if ui.selectable_label(false, label).clicked() {
                let path = format!("{folder_path}/{}", request.name);
                self.loaded_request = Some(LoadedRequestRef {
                    collection: collection_name.to_string(),
                    path,
                });
                self.load_saved_request(request);
                self.central_view = CentralView::Request;
            }
        }
    }

    /// Render the active environment's variable-table editor (M8) — the
    /// Environment main tab's content, extracted from what used to be an
    /// inline sidebar "Edit "{name}"" section, since a full tab gives it
    /// more room (more consistent with request tabs' own real estate). The
    /// sidebar's Environments section keeps only the name list + "+ New".
    fn show_environment_editor(&mut self, ui: &mut egui::Ui) {
        let Some(name) = self.active_environment.clone() else {
            ui.weak("No environment selected — pick or create one in the sidebar.");
            return;
        };
        ui.heading(format!("Environment: {name}"));
        let mut remove_idx = None;
        for (i, v) in self.env_editor_variables.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut v.key).hint_text("KEY").desired_width(140.0));
                ui.add(egui::TextEdit::singleline(&mut v.value).hint_text("value").desired_width(240.0));
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
        if let Some(notice) = &self.env_notice {
            ui.colored_label(egui::Color32::from_rgb(120, 190, 130), notice);
        }
    }

    fn is_active_tab_dirty(&self) -> bool {
        is_tab_dirty(
            &self.saved_snapshot,
            &self.method,
            &self.url,
            &self.headers,
            &self.body,
            &self.auth,
            &self.extract_rules,
        )
    }

    /// Snapshot the fields `persistence` cares about — every tab, with the
    /// active index's live top-level fields substituted in for
    /// `tabs[active_tab_idx]`'s own stale placeholder (same "idx ==
    /// active_tab_idx reads self, else reads tabs[idx]" pattern
    /// `tab_label` uses).
    fn persisted_state(&self) -> persistence::PersistedState {
        let tabs = (0..self.tabs.len())
            .map(|i| {
                if i == self.active_tab_idx {
                    persistence::PersistedTab {
                        method: self.method.clone(),
                        url: self.url.clone(),
                        headers: self.headers.clone(),
                        body: self.body.clone(),
                        auth: self.auth.clone(),
                        loaded_request: self.loaded_request.clone(),
                    }
                } else {
                    let tab = &self.tabs[i];
                    persistence::PersistedTab {
                        method: tab.method.clone(),
                        url: tab.url.clone(),
                        headers: tab.headers.clone(),
                        body: tab.body.clone(),
                        auth: tab.auth.clone(),
                        loaded_request: tab.loaded_request.clone(),
                    }
                }
            })
            .collect();

        persistence::PersistedState {
            zoom: self.last_zoom,
            theme: self.last_theme.into(),
            theme_follows_system: self.theme_follows_system,
            tabs,
            active_tab_idx: self.active_tab_idx,
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

/// Detect the OS's light/dark preference. Needed because winit 0.30
/// (egui/eframe's windowing backend) unconditionally returns `None` from
/// `EventLoop::system_theme()` on both X11 and Wayland, so egui's own
/// `ThemePreference::System` (the default) silently falls back to a
/// hardcoded dark theme on Linux rather than actually following the OS.
/// Dispatches on `$XDG_CURRENT_DESKTOP` to the matching desktop-specific
/// heuristic (GNOME via `gsettings`, KDE via reading `kdeglobals` directly);
/// `None` (leave whatever theme is already set alone) on any other desktop,
/// or if the check fails — the manual toggle always works regardless.
/// Called both once at startup and periodically while `theme_follows_system`
/// is true (see `ui()`), so it needs to stay cheap.
fn detect_system_theme() -> Option<egui::Theme> {
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().to_lowercase();
    if desktop.contains("gnome") {
        detect_gnome_theme()
    } else if desktop.contains("kde") {
        detect_kde_theme()
    } else {
        None
    }
}

/// GNOME's light/dark preference via `gsettings` — the same source GTK/GNOME
/// apps themselves read.
fn detect_gnome_theme() -> Option<egui::Theme> {
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

/// KDE's light/dark preference by reading `~/.config/kdeglobals` directly
/// (every KDE user has this file) rather than shelling out to
/// `kreadconfig5`/`kreadconfig6` — the binary name differs between KDE
/// major versions and may not even be installed, whereas the config file
/// itself is guaranteed to exist. Looks for the `[General]` section's
/// `ColorScheme=` key and treats a scheme name containing "dark"
/// (case-insensitively — e.g. `BreezeDark`) as dark, anything else as
/// light; `None` if the file, section, or key is missing.
fn detect_kde_theme() -> Option<egui::Theme> {
    let path = dirs::config_dir()?.join("kdeglobals");
    let contents = std::fs::read_to_string(path).ok()?;
    parse_kde_globals(&contents)
}

fn parse_kde_globals(contents: &str) -> Option<egui::Theme> {
    let mut in_general = false;
    for line in contents.lines() {
        let line = line.trim();
        if let Some(section) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            in_general = section == "General";
            continue;
        }
        if in_general {
            if let Some(value) = line.strip_prefix("ColorScheme=") {
                return Some(if value.to_lowercase().contains("dark") {
                    egui::Theme::Dark
                } else {
                    egui::Theme::Light
                });
            }
        }
    }
    None
}

// --- FR-22: JSON syntax highlighting + in-body search (Response pane) ---

/// A JSON token's category, for coloring — keys and string values share
/// `String` rather than getting their own colors; distinguishing them
/// would mean peeking ahead for a following `:`, which isn't worth the
/// complexity for what's meant to be a readability aid, not a full editor.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum JsonTokenKind {
    String,
    Number,
    Keyword,
    Punct,
    Other,
}

/// Split `text` into `(byte_range, kind)` spans covering the whole string
/// with no gaps — including whitespace, folded into `Other` along with
/// anything unrecognized. Byte-indexed rather than char-indexed, but safe
/// to slice on those boundaries regardless: every match pattern here is an
/// ASCII byte, which can never appear as a continuation byte of a
/// multi-byte UTF-8 sequence, so `i` only ever advances to real char
/// boundaries.
fn tokenize_json(text: &str) -> Vec<(std::ops::Range<usize>, JsonTokenKind)> {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut spans = Vec::new();
    let mut i = 0;

    while i < len {
        let start = i;
        match bytes[i] {
            b'"' => {
                i += 1;
                while i < len {
                    if bytes[i] == b'\\' && i + 1 < len {
                        i += 2;
                        continue;
                    }
                    let is_close = bytes[i] == b'"';
                    i += 1;
                    if is_close {
                        break;
                    }
                }
                spans.push((start..i, JsonTokenKind::String));
            }
            b'{' | b'}' | b'[' | b']' | b':' | b',' => {
                i += 1;
                spans.push((start..i, JsonTokenKind::Punct));
            }
            b'-' | b'0'..=b'9' => {
                i += 1;
                while i < len && matches!(bytes[i], b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-') {
                    i += 1;
                }
                spans.push((start..i, JsonTokenKind::Number));
            }
            b't' | b'f' | b'n' if text[i..].starts_with("true")
                || text[i..].starts_with("false")
                || text[i..].starts_with("null") =>
            {
                let word_len = if text[i..].starts_with("false") { 5 } else { 4 };
                i += word_len;
                spans.push((start..i, JsonTokenKind::Keyword));
            }
            _ => {
                i += 1;
                while i < len
                    && !matches!(
                        bytes[i],
                        b'"' | b'{' | b'}' | b'[' | b']' | b':' | b',' | b'-' | b'0'..=b'9' | b't' | b'f' | b'n'
                    )
                {
                    i += 1;
                }
                spans.push((start..i, JsonTokenKind::Other));
            }
        }
    }

    spans
}

struct JsonColors {
    string: egui::Color32,
    number: egui::Color32,
    keyword: egui::Color32,
    punct: egui::Color32,
    default: egui::Color32,
    highlight_bg: egui::Color32,
    highlight_fg: egui::Color32,
    /// The *current* find-next/previous match gets this instead of
    /// `highlight_bg`/`highlight_fg` — a visibly different color from every
    /// other match's plain highlight, since otherwise "next"/"previous"
    /// has nothing distinguishable to show (every match already looks the
    /// same under highlight-all).
    current_highlight_bg: egui::Color32,
    current_highlight_fg: egui::Color32,
}

fn json_colors(dark_mode: bool) -> JsonColors {
    if dark_mode {
        JsonColors {
            string: egui::Color32::from_rgb(152, 195, 121),
            number: egui::Color32::from_rgb(209, 154, 102),
            keyword: egui::Color32::from_rgb(198, 120, 221),
            punct: egui::Color32::from_rgb(150, 150, 150),
            default: egui::Color32::from_rgb(220, 220, 220),
            highlight_bg: egui::Color32::from_rgb(255, 220, 90),
            highlight_fg: egui::Color32::BLACK,
            current_highlight_bg: egui::Color32::from_rgb(255, 140, 0),
            current_highlight_fg: egui::Color32::BLACK,
        }
    } else {
        JsonColors {
            string: egui::Color32::from_rgb(60, 120, 50),
            number: egui::Color32::from_rgb(170, 90, 20),
            keyword: egui::Color32::from_rgb(130, 60, 150),
            punct: egui::Color32::from_rgb(100, 100, 100),
            default: egui::Color32::from_rgb(30, 30, 30),
            highlight_bg: egui::Color32::from_rgb(255, 230, 120),
            highlight_fg: egui::Color32::BLACK,
            current_highlight_bg: egui::Color32::from_rgb(255, 140, 0),
            current_highlight_fg: egui::Color32::BLACK,
        }
    }
}

/// Append `text` to `job` in `color`, splitting further at every
/// case-insensitive occurrence of `search_lower` (already lowercased by
/// the caller, once, rather than per-call) to give matches a highlighted
/// background — a no-op split when `search_lower` is empty. `token_start`
/// is `text`'s own byte offset within the *whole* response body (0 for a
/// single-call-covers-everything caller like `plain_layout_job`, a
/// per-token offset for `json_layout_job`, which calls this once per
/// syntax token) — added to a local match's offset to get its global byte
/// position, compared against `current_match_byte` (the byte offset of
/// whichever match find-next/previous currently points at) so that one
/// match, if any falls within this call, renders with
/// `current_highlight_bg`/`current_highlight_fg` instead of the plain
/// `highlight_bg`/`highlight_fg` every other match gets — otherwise
/// next/previous would have no visible effect, since every match already
/// looks identical under highlight-all. Byte-slicing `text`/
/// `text.to_lowercase()` in lockstep is exact for ASCII search terms (the
/// overwhelming common case — field names, tokens); a non-ASCII search
/// term whose lowercasing changes byte length could mis-slice, an accepted
/// approximation rather than pulling in a full Unicode-aware search
/// library for a highlight-only feature.
fn append_with_search_highlight(
    job: &mut egui::text::LayoutJob,
    text: &str,
    color: egui::Color32,
    font_id: egui::FontId,
    search_lower: &str,
    token_start: usize,
    current_match_byte: Option<usize>,
    highlight_bg: egui::Color32,
    highlight_fg: egui::Color32,
    current_highlight_bg: egui::Color32,
    current_highlight_fg: egui::Color32,
) {
    if search_lower.is_empty() {
        job.append(text, 0.0, egui::text::TextFormat {
            font_id,
            color,
            ..Default::default()
        });
        return;
    }

    let text_lower = text.to_lowercase();
    let mut pos = 0;
    while let Some(rel) = text_lower.get(pos..).and_then(|s| s.find(search_lower)) {
        let match_start = pos + rel;
        let match_end = match_start + search_lower.len();
        if match_start > pos {
            job.append(&text[pos..match_start], 0.0, egui::text::TextFormat {
                font_id: font_id.clone(),
                color,
                ..Default::default()
            });
        }
        if let Some(matched) = text.get(match_start..match_end) {
            let is_current = current_match_byte == Some(token_start + match_start);
            let (bg, fg) = if is_current {
                (current_highlight_bg, current_highlight_fg)
            } else {
                (highlight_bg, highlight_fg)
            };
            job.append(matched, 0.0, egui::text::TextFormat {
                font_id: font_id.clone(),
                color: fg,
                background: bg,
                ..Default::default()
            });
        }
        pos = match_end;
    }
    if pos < text.len() {
        job.append(&text[pos..], 0.0, egui::text::TextFormat {
            font_id,
            color,
            ..Default::default()
        });
    }
}

/// Build a syntax-highlighted (+ search-highlighted) `LayoutJob` for a JSON
/// response body. `current_match_byte` — see `append_with_search_highlight`'s
/// doc comment — is the byte offset (into `text`) of whichever match
/// find-next/previous currently points at, or `None`.
fn json_layout_job(
    text: &str,
    dark_mode: bool,
    search_lower: &str,
    current_match_byte: Option<usize>,
) -> egui::text::LayoutJob {
    let colors = json_colors(dark_mode);
    let font_id = egui::FontId::monospace(13.0);
    let mut job = egui::text::LayoutJob::default();

    for (range, kind) in tokenize_json(text) {
        let color = match kind {
            JsonTokenKind::String => colors.string,
            JsonTokenKind::Number => colors.number,
            JsonTokenKind::Keyword => colors.keyword,
            JsonTokenKind::Punct => colors.punct,
            JsonTokenKind::Other => colors.default,
        };
        let token_start = range.start;
        append_with_search_highlight(
            &mut job,
            &text[range],
            color,
            font_id.clone(),
            search_lower,
            token_start,
            current_match_byte,
            colors.highlight_bg,
            colors.highlight_fg,
            colors.current_highlight_bg,
            colors.current_highlight_fg,
        );
    }

    job
}

/// Same search-highlighting as `json_layout_job`, but no syntax coloring —
/// for a non-JSON response body (plain text, XML, HTML, ...).
fn plain_layout_job(
    text: &str,
    dark_mode: bool,
    search_lower: &str,
    current_match_byte: Option<usize>,
) -> egui::text::LayoutJob {
    let colors = json_colors(dark_mode);
    let font_id = egui::FontId::monospace(13.0);
    let mut job = egui::text::LayoutJob::default();
    append_with_search_highlight(
        &mut job,
        text,
        colors.default,
        font_id,
        search_lower,
        0,
        current_match_byte,
        colors.highlight_bg,
        colors.highlight_fg,
        colors.current_highlight_bg,
        colors.current_highlight_fg,
    );
    job
}

/// Count of case-insensitive, non-overlapping occurrences of `search` in
/// `text` — shown next to the search box so "0 matches" vs "N matches" is
/// explicit rather than the user having to eyeball the highlighting.
fn count_matches(text: &str, search: &str) -> usize {
    if search.is_empty() {
        return 0;
    }
    text.to_lowercase().matches(&search.to_lowercase()).count()
}

// --- FR-22: image preview for a response ---

/// Whether `headers` name a `Content-Type` starting with `image/` — the
/// signal for switching the Response pane from the text/JSON viewer to an
/// image preview. Header-name matching is case-insensitive (HTTP headers
/// are); the value check only looks at the prefix, ignoring anything after
/// a `;` (e.g. `image/png; charset=binary`, unusual but not unheard of).
fn is_image_content_type(headers: &[(String, String)]) -> bool {
    headers.iter().any(|(name, value)| {
        name.eq_ignore_ascii_case("content-type")
            && value.split(';').next().unwrap_or("").trim().to_lowercase().starts_with("image/")
    })
}

/// A filename extension guessed from the `Content-Type` header (`png` for
/// `image/png`, `jpg` for `image/jpeg`, etc.) — purely a starting suggestion
/// for "Save Response…"'s default filename, same spirit as the existing
/// `response.json`/`response.txt` guess for text bodies. Falls back to
/// `bin` if there's no recognizable `image/*` content type.
fn image_file_extension(headers: &[(String, String)]) -> String {
    headers
        .iter()
        .find_map(|(name, value)| {
            if !name.eq_ignore_ascii_case("content-type") {
                return None;
            }
            let subtype = value.split(';').next().unwrap_or("").trim().to_lowercase();
            subtype.strip_prefix("image/").map(|s| match s {
                "jpeg" => "jpg".to_string(),
                other => other.to_string(),
            })
        })
        .unwrap_or_else(|| "bin".to_string())
}

// --- FR-22: find-next/scroll-to-match for the response search ---

/// Byte offsets (into `text`) of every case-insensitive occurrence of
/// `search_lower`, in order — the same lowercasing approach
/// `count_matches`/`append_with_search_highlight` already use (and the
/// same accepted assumption that lowercasing doesn't shift byte length for
/// realistic search terms/content). `search_lower` must already be
/// lowercased by the caller (avoids repeating that work — `find_next_match`
/// below shares one lowercased copy across a whole frame).
fn find_match_starts(text: &str, search_lower: &str) -> Vec<usize> {
    if search_lower.is_empty() {
        return Vec::new();
    }
    text.to_lowercase().match_indices(search_lower).map(|(i, _)| i).collect()
}

/// Advance (or retreat) `current` by one match, wrapping in both directions —
/// `total == 0` always yields `0` (nothing to wrap around).
fn advance_match(current: usize, total: usize, forward: bool) -> usize {
    if total == 0 {
        return 0;
    }
    if forward {
        (current + 1) % total
    } else {
        (current + total - 1) % total
    }
}

// --- Toolbar: zoom ---

/// Clamp a zoom factor to `[MIN_ZOOM, MAX_ZOOM]` so a stray double-press
/// (or a saved value from a future settings feature) can't shrink/blow up
/// the UI to the point of being unusable. Kept as a pure function, unlike
/// `set_zoom_factor`/`apply_zoom_delta` below: `egui::Context::zoom_factor()`
/// doesn't reflect a `set_zoom_factor()` call until the *next* pass begins
/// (documented on `Context::set_zoom_factor` itself), so those two aren't
/// meaningfully unit-testable by reading the value back outside a running
/// app — this is, by staying decoupled from `Context` entirely.
fn clamp_zoom(factor: f32) -> f32 {
    factor.clamp(MIN_ZOOM, MAX_ZOOM)
}

fn set_zoom_factor(ctx: &egui::Context, factor: f32) {
    ctx.set_zoom_factor(clamp_zoom(factor));
}

/// Multiply the current zoom factor by `delta` (e.g. `ZOOM_STEP` to zoom
/// in, `1.0 / ZOOM_STEP` to zoom out) — used by both the toolbar's +/−
/// buttons and the Ctrl+/Ctrl- keyboard shortcuts, so the two stay in sync.
fn apply_zoom_delta(ctx: &egui::Context, delta: f32) {
    set_zoom_factor(ctx, ctx.zoom_factor() * delta);
}

/// Apply egui's own Ctrl+scroll-wheel zoom gesture (and pinch-to-zoom on a
/// touchpad/touchscreen) on top of the toolbar/keyboard-shortcut zoom —
/// egui tracks the gesture as an input delta (`InputState::zoom_delta`)
/// but doesn't apply it to `zoom_factor` on its own; an app has to do that
/// itself each frame.
fn apply_ctrl_scroll_zoom(ctx: &egui::Context) {
    let delta = ctx.input(|i| i.zoom_delta());
    if delta != 1.0 {
        apply_zoom_delta(ctx, delta);
    }
}

impl eframe::App for CurlyApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.last_zoom = ui.ctx().zoom_factor();
        self.last_theme = ui.ctx().theme();

        self.poll_response();
        self.poll_project_dialog();
        self.poll_response_save();
        self.show_save_dialog(ui.ctx());
        self.show_path_dialog(ui.ctx());
        self.show_folder_dialog(ui.ctx());
        apply_ctrl_scroll_zoom(ui.ctx());

        // Live-follow the OS theme while the user hasn't manually pinned it
        // (see `theme_follows_system`'s doc comment) — polled on a timer
        // rather than reacting to some "OS theme changed" event, since none
        // of winit/egui/gsettings/kdeglobals expose one on Linux.
        if self.theme_follows_system {
            if self.last_theme_check.elapsed() >= Duration::from_secs(2) {
                self.last_theme_check = std::time::Instant::now();
                if let Some(theme) = detect_system_theme() {
                    if theme != ui.ctx().theme() {
                        ui.ctx().set_theme(theme);
                    }
                }
            }
            ui.ctx().request_repaint_after(Duration::from_secs(2));
        }

        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.horizontal(|ui| {
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

                let current_theme = ui.ctx().theme();
                let (icon, next, hover) = match current_theme {
                    egui::Theme::Dark => ("☀", egui::Theme::Light, "Switch to light mode"),
                    egui::Theme::Light => ("🌙", egui::Theme::Dark, "Switch to dark mode"),
                };
                if ui.button(icon).on_hover_text(hover).clicked() {
                    ui.ctx().set_theme(next);
                    self.theme_follows_system = false;
                }

                ui.separator();

                ui.label("Zoom:");
                if ui.button("−").on_hover_text("Zoom out (Ctrl+-)").clicked() {
                    apply_zoom_delta(ui.ctx(), 1.0 / ZOOM_STEP);
                }
                ui.label(format!("{:.0}%", ui.ctx().zoom_factor() * 100.0));
                if ui.button("+").on_hover_text("Zoom in (Ctrl++)").clicked() {
                    apply_zoom_delta(ui.ctx(), ZOOM_STEP);
                }
                if ui.button("Reset").on_hover_text("Reset zoom to 100% (Ctrl+0)").clicked() {
                    set_zoom_factor(ui.ctx(), 1.0);
                }
            });
        });

        let mut zoom_delta: Option<f32> = None;
        let mut zoom_reset = false;
        let mut new_tab_shortcut = false;
        ui.input(|i| {
            if i.modifiers.command {
                if i.key_pressed(egui::Key::Plus) || i.key_pressed(egui::Key::Equals) {
                    zoom_delta = Some(ZOOM_STEP);
                }
                if i.key_pressed(egui::Key::Minus) {
                    zoom_delta = Some(1.0 / ZOOM_STEP);
                }
                if i.key_pressed(egui::Key::Num0) {
                    zoom_reset = true;
                }
                if i.key_pressed(egui::Key::T) {
                    new_tab_shortcut = true;
                }
            }
        });
        if let Some(delta) = zoom_delta {
            apply_zoom_delta(ui.ctx(), delta);
        }
        if zoom_reset {
            set_zoom_factor(ui.ctx(), 1.0);
        }
        if new_tab_shortcut {
            self.open_new_tab();
        }

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

                ui.separator();

                ui.horizontal(|ui| {
                    if ui
                        .selectable_label(self.sidebar_tab == SidebarTab::Project, "Project")
                        .clicked()
                    {
                        self.sidebar_tab = SidebarTab::Project;
                    }
                    if ui
                        .selectable_label(self.sidebar_tab == SidebarTab::History, "History")
                        .clicked()
                    {
                        self.sidebar_tab = SidebarTab::History;
                    }
                });
                ui.separator();

                if self.sidebar_tab == SidebarTab::History {
                    let mut history_click: Option<usize> = None;
                    if self.history_entries.is_empty() {
                        ui.weak("  (no history yet)");
                    }
                    for (i, entry) in self.history_entries.iter().enumerate() {
                        let mut label = format!(
                            "{} {}  {}  ({} ms)",
                            entry.method, entry.status, entry.url, entry.elapsed_ms
                        );
                        if label.len() > 60 {
                            label.truncate(57);
                            label.push_str("...");
                        }
                        if ui.selectable_label(false, label).clicked() {
                            history_click = Some(i);
                        }
                    }
                    if let Some(i) = history_click {
                        let entry = self.history_entries[i].clone();
                        self.load_history_entry(&entry);
                    }
                    return;
                }

                ui.label(format!("Collections ({})", self.collections.len()));
                if self.collections.is_empty() {
                    ui.weak("  (none yet)");
                }
                let mut tree_action: Option<(String, TreeAction)> = None;
                let mut save_all_collection: Option<String> = None;
                let mut open_collection: Option<String> = None;
                for collection in &self.collections {
                    let state_id = ui.make_persistent_id(("collection_state", collection.id));
                    let header = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), state_id, false)
                        .show_header(ui, |ui| {
                            if let Some(i) = hover_ellipsis_row(
                                ui,
                                ("collection_ellipsis", collection.id),
                                &["New Folder", "New Request", "Save All"],
                                |ui| {
                                    if ui.selectable_label(false, &collection.name).clicked() {
                                        open_collection = Some(collection.name.clone());
                                    }
                                },
                            ) {
                                match i {
                                    0 => {
                                        tree_action =
                                            Some((collection.name.clone(), TreeAction::NewFolder(String::new())))
                                    }
                                    1 => {
                                        tree_action =
                                            Some((collection.name.clone(), TreeAction::NewRequest(String::new())))
                                    }
                                    _ => save_all_collection = Some(collection.name.clone()),
                                }
                            }
                        });
                    header.body(|ui| {
                        if let Some(a) = show_collection_tree(ui, "", &collection.folders, &collection.requests) {
                            tree_action = Some((collection.name.clone(), a));
                        }
                    });
                }
                if let Some(name) = open_collection {
                    self.open_or_focus_extra_tab(ExtraTabKind::Collection(name));
                }
                if let Some(collection_name) = save_all_collection {
                    self.save_all_in_collection(&collection_name);
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
                        TreeAction::Rename(path) => {
                            self.open_path_dialog(PathDialogMode::Rename, collection_name, path);
                        }
                        TreeAction::Duplicate(path) => {
                            self.open_path_dialog(PathDialogMode::Duplicate, collection_name, path);
                        }
                        TreeAction::NewFolder(parent) => {
                            self.open_folder_dialog(collection_name, parent);
                        }
                        TreeAction::NewRequest(parent) => {
                            self.open_new_tab();
                            self.open_save_dialog_in(collection_name, parent);
                        }
                        TreeAction::DeleteFolder(path) => {
                            self.delete_folder(&collection_name, &path);
                        }
                        TreeAction::OpenFolder(path) => {
                            self.open_or_focus_extra_tab(ExtraTabKind::Folder(collection_name, path));
                        }
                    }
                }

                ui.separator();

                // Compact list only (M8) — the actual variable-editing
                // table moved to the Environment main tab
                // (`show_environment_editor`), which has more room.
                // Clicking a name here both selects it (for substitution)
                // and navigates to that tab to edit it; the tab strip's own
                // "Active environment" selector, by contrast, only selects
                // without navigating away from whatever you're editing.
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
                        EnvAction::Select(name) => {
                            let navigate = name.is_some();
                            self.select_environment(name);
                            if navigate {
                                self.central_view = CentralView::Environment;
                            }
                        }
                        EnvAction::Create(name) => {
                            self.create_environment(name);
                            self.central_view = CentralView::Environment;
                        }
                    }
                }

                if let Some(notice) = &self.env_notice {
                    ui.colored_label(egui::Color32::from_rgb(120, 190, 130), notice);
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
            // Deferred like every other `self`-mutating action triggered
            // from inside a `match &self.response { ... }` arm below: `resp`
            // there borrows `self.response` specifically, but
            // `save_response_to_file` takes `&mut self` (opaque to the
            // borrow checker — it can't see that the method never touches
            // `self.response`), so the actual call has to wait until that
            // match's borrow of `self.response` has ended. Declared up here
            // (not inside the `CentralView::Request` match arm below) so
            // they're still in scope where they're read, after that arm's
            // block closes.
            let mut save_response_request: Option<(Vec<u8>, String)> = None;
            // Same deferred-write reason as `save_response_request` above —
            // `ui.ctx().load_texture(...)` itself doesn't need `&mut self`,
            // but caching the result into `self.response_image` does, and
            // `resp` below still borrows `self.response` at that point.
            let mut decoded_image_texture: Option<egui::TextureHandle> = None;

            egui::ScrollArea::horizontal().id_salt("tab_bar").show(ui, |ui| {
                ui.horizontal(|ui| {
                    let mut switch_to: Option<usize> = None;
                    let mut close: Option<usize> = None;
                    for i in 0..self.tabs.len() {
                        let selected = i == self.active_tab_idx;
                        let label = if !selected && self.tabs[i].has_unseen_response {
                            format!("● {}", self.tab_label(i))
                        } else {
                            self.tab_label(i)
                        };
                        ui.horizontal(|ui| {
                            if ui.selectable_label(selected, label).clicked() {
                                switch_to = Some(i);
                            }
                            if ui.small_button("✕").clicked() {
                                close = Some(i);
                            }
                        });
                    }
                    if ui.button("+").on_hover_text("New tab (Ctrl+T)").clicked() {
                        self.open_new_tab();
                    }
                    if let Some(i) = switch_to {
                        self.switch_to_tab(i);
                        self.central_view = CentralView::Request;
                    }
                    if let Some(i) = close {
                        self.close_tab(i);
                    }

                    // Collection/Folder tabs (M8) — opened on demand by
                    // clicking a collection's or folder's name in the
                    // sidebar tree (`open_or_focus_extra_tab`); they persist
                    // in the strip until explicitly closed, same as request
                    // tabs, but don't participate in the take/restore swap
                    // (their content is derived straight from `central_view`,
                    // not from any `RequestTab`).
                    let mut focus_extra: Option<usize> = None;
                    let mut close_extra: Option<usize> = None;
                    for i in 0..self.extra_tabs.len() {
                        let entry = self.extra_tabs[i].clone();
                        let label = match &entry {
                            ExtraTabKind::Collection(name) => name.clone(),
                            ExtraTabKind::Folder(_, path) => format!("{path}/"),
                        };
                        let selected = self.central_view == entry.central_view();
                        ui.horizontal(|ui| {
                            if ui.selectable_label(selected, label).clicked() {
                                focus_extra = Some(i);
                            }
                            if ui.small_button("✕").clicked() {
                                close_extra = Some(i);
                            }
                        });
                    }
                    if let Some(i) = focus_extra {
                        self.central_view = self.extra_tabs[i].central_view();
                    }
                    if let Some(i) = close_extra {
                        let closed = self.extra_tabs.remove(i);
                        if self.central_view == closed.central_view() {
                            self.central_view = CentralView::Request;
                        }
                    }

                    // Environment (M8) — always present, symmetric with
                    // request tabs (more room to edit variables than the
                    // sidebar's compact list); see `show_environment_editor`.
                    if ui
                        .selectable_label(self.central_view == CentralView::Environment, "Environment")
                        .clicked()
                    {
                        self.central_view = CentralView::Environment;
                    }
                });
            });

            // Active-environment selector (M8) — unlike clicking "Environment"
            // above or a name in the sidebar, this only changes which
            // environment `{{variable}}` tokens resolve against; it doesn't
            // navigate away from whatever tab you're editing.
            ui.horizontal(|ui| {
                ui.label("Active environment:");
                egui::ComboBox::from_id_salt("active_env_selector")
                    .selected_text(self.active_environment.clone().unwrap_or_else(|| "(none)".to_string()))
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(self.active_environment.is_none(), "(none)").clicked() {
                            self.select_environment(None);
                        }
                        for name in self.environments.clone() {
                            let selected = self.active_environment.as_deref() == Some(name.as_str());
                            if ui.selectable_label(selected, &name).clicked() {
                                self.select_environment(Some(name));
                            }
                        }
                    });
            });
            ui.separator();

            match self.central_view.clone() {
            CentralView::Collection(name) => {
                self.show_collection_tab(ui, &name);
            }
            CentralView::Folder(collection, path) => {
                self.show_folder_tab(ui, &collection, &path);
            }
            CentralView::Environment => {
                self.show_environment_editor(ui);
            }
            CentralView::Request => {

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

            egui::CollapsingHeader::new(format!("Extraction rules ({})", self.extract_rules.len()))
                .default_open(false)
                .show(ui, |ui| {
                    ui.weak(
                        "Pulls a value out of a successful response into the active \
                         environment's session — the GUI equivalent of `curly collections \
                         add-request --extract-body/--extract-header/--extract-template`.",
                    );
                    let mut remove_idx = None;
                    for (i, rule) in self.extract_rules.iter_mut().enumerate() {
                        ui.horizontal(|ui| {
                            let current_kind = extraction_kind(rule);
                            let mut kind = current_kind;
                            ui.selectable_value(&mut kind, ExtractionKindTag::Body, "Body");
                            ui.selectable_value(&mut kind, ExtractionKindTag::Header, "Header");
                            ui.selectable_value(&mut kind, ExtractionKindTag::Template, "Template");
                            if kind != current_kind {
                                *rule = rebuild_extraction(rule, kind);
                            }
                            match rule {
                                Extraction::Body { name, path, secret } => {
                                    ui.add(
                                        egui::TextEdit::singleline(name)
                                            .hint_text("NAME")
                                            .desired_width(80.0),
                                    );
                                    ui.add(
                                        egui::TextEdit::singleline(path)
                                            .hint_text("JSON path")
                                            .desired_width(140.0),
                                    );
                                    ui.checkbox(secret, "secret");
                                }
                                Extraction::Header { name, header, secret } => {
                                    ui.add(
                                        egui::TextEdit::singleline(name)
                                            .hint_text("NAME")
                                            .desired_width(80.0),
                                    );
                                    ui.add(
                                        egui::TextEdit::singleline(header)
                                            .hint_text("Header name")
                                            .desired_width(140.0),
                                    );
                                    ui.checkbox(secret, "secret");
                                }
                                Extraction::Template { name, template, secret } => {
                                    ui.add(
                                        egui::TextEdit::singleline(name)
                                            .hint_text("NAME")
                                            .desired_width(80.0),
                                    );
                                    ui.add(
                                        egui::TextEdit::singleline(template)
                                            .hint_text("template, e.g. Bearer {{TOKEN}}")
                                            .desired_width(180.0),
                                    );
                                    ui.checkbox(secret, "secret");
                                }
                            }
                            if ui.small_button("✕").clicked() {
                                remove_idx = Some(i);
                            }
                        });
                    }
                    if let Some(i) = remove_idx {
                        self.extract_rules.remove(i);
                    }
                    if ui.button("+ Add rule").clicked() {
                        self.extract_rules.push(Extraction::Body {
                            name: String::new(),
                            path: String::new(),
                            secret: false,
                        });
                    }
                });

            if let Some(notice) = &self.session_notice {
                ui.colored_label(egui::Color32::from_rgb(120, 190, 130), notice);
            }

            // Breadcrumb (M8) — collection/folder trail with an editable
            // trailing segment, positioned above the URL row per the .jfd
            // reference. Saving it renames the request; it does not touch
            // the request's content (that's plain Save/Save As, below).
            self.sync_breadcrumb();
            ui.horizontal(|ui| {
                match &self.loaded_request {
                    Some(loaded) => {
                        let (folder, _name) = split_folder_and_name(&loaded.path);
                        let prefix = if folder.is_empty() {
                            format!("{} › ", loaded.collection)
                        } else {
                            format!("{} › {folder} › ", loaded.collection)
                        };
                        ui.weak(prefix);
                        let name_response = ui.add(
                            egui::TextEdit::singleline(&mut self.breadcrumb_name)
                                .id_salt("breadcrumb_name")
                                .desired_width(160.0),
                        );
                        let enter_pressed =
                            name_response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if ui.small_button("✓").on_hover_text("Save name").clicked() || enter_pressed {
                            self.save_breadcrumb_name();
                        }
                    }
                    None => {
                        ui.weak("New request — not yet saved in a collection");
                    }
                }
            });

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
                // Save/Save As styled as prominently as Send (M8) — same
                // green fill so they read as the primary actions they are,
                // distinguished from Send by color rather than shape.
                let save_fill = egui::Color32::from_rgb(46, 125, 50);
                if ui.add(egui::Button::new("Save").fill(save_fill)).clicked() {
                    self.save_over_loaded();
                }
                if ui.add(egui::Button::new("Save As…").fill(save_fill)).clicked() {
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

            egui::CollapsingHeader::new("Authorization")
                .default_open(false)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut self.auth.kind, AuthKind::None, "None");
                        ui.selectable_value(&mut self.auth.kind, AuthKind::Basic, "Basic");
                        ui.selectable_value(&mut self.auth.kind, AuthKind::Bearer, "Bearer");
                    });
                    match self.auth.kind {
                        AuthKind::None => {}
                        AuthKind::Basic => {
                            ui.horizontal(|ui| {
                                ui.label("Username:");
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.auth.username)
                                        .desired_width(200.0),
                                );
                            });
                            ui.horizontal(|ui| {
                                ui.label("Password:");
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.auth.password)
                                        .password(true)
                                        .desired_width(200.0),
                                );
                            });
                        }
                        AuthKind::Bearer => {
                            ui.horizontal(|ui| {
                                ui.label("Token:");
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.auth.token)
                                        .desired_width(300.0),
                                );
                            });
                        }
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

            // Sub-tabs (M8) — Body/Headers were stacked (always both
            // visible) before this; History is new, reusing the
            // project-wide send log (`history_entries`) filtered to the
            // active tab's own method+URL, rather than a separate
            // per-request history feature curly doesn't otherwise have.
            ui.horizontal(|ui| {
                for (subtab, label) in [
                    (ResponseSubtab::Body, "Body"),
                    (ResponseSubtab::Headers, "Headers"),
                    (ResponseSubtab::History, "History"),
                ] {
                    if ui.selectable_label(self.response_subtab == subtab, label).clicked() {
                        self.response_subtab = subtab;
                    }
                }
            });

            if self.response_subtab == ResponseSubtab::History {
                let matching: Vec<usize> = self
                    .history_entries
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| e.method == self.method && e.url == self.url)
                    .map(|(i, _)| i)
                    .collect();
                if matching.is_empty() {
                    ui.weak("No history for this request yet — send it to start building history.");
                }
                let mut history_click: Option<usize> = None;
                for i in matching {
                    let entry = &self.history_entries[i];
                    let label = format!(
                        "{}  {}  ({} ms)",
                        entry.status,
                        entry.sent_at.format("%Y-%m-%d %H:%M:%S"),
                        entry.elapsed_ms
                    );
                    if ui.selectable_label(false, label).clicked() {
                        history_click = Some(i);
                    }
                }
                if let Some(i) = history_click {
                    let entry = self.history_entries[i].clone();
                    self.load_history_entry(&entry);
                }
            } else {
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

                    if self.response_subtab == ResponseSubtab::Headers {
                        for (k, v) in &resp.headers {
                            ui.label(format!("{k}: {v}"));
                        }
                    } else if is_image_content_type(&resp.headers) {
                        if self.response_image.is_none() {
                            if let Ok(decoded) = image::load_from_memory(&resp.body_bytes) {
                                let rgba = decoded.into_rgba8();
                                let (width, height) = rgba.dimensions();
                                let color_image = egui::ColorImage::from_rgba_unmultiplied(
                                    [width as usize, height as usize],
                                    rgba.as_raw(),
                                );
                                decoded_image_texture = Some(ui.ctx().load_texture(
                                    "response-image",
                                    color_image,
                                    egui::TextureOptions::default(),
                                ));
                            }
                        }

                        if ui.button("Save Response…").clicked() {
                            let default_name = format!("response.{}", image_file_extension(&resp.headers));
                            save_response_request = Some((resp.body_bytes.clone(), default_name));
                        }
                        if let Some(notice) = &self.response_save_notice {
                            ui.colored_label(egui::Color32::from_rgb(120, 190, 130), notice);
                        }

                        match self.response_image.as_ref().or(decoded_image_texture.as_ref()) {
                            Some(texture) => {
                                egui::ScrollArea::both().show(ui, |ui| {
                                    ui.add(
                                        egui::Image::from_texture(texture)
                                            .max_width(ui.available_width())
                                            .shrink_to_fit(),
                                    );
                                });
                            }
                            None => {
                                ui.colored_label(
                                    egui::Color32::from_rgb(220, 80, 80),
                                    "Content-Type looks like an image, but the body couldn't be decoded.",
                                );
                            }
                        }
                    } else {
                        let is_json = serde_json::from_str::<serde_json::Value>(&resp.body).is_ok();
                        let mut pretty = if is_json {
                            serde_json::from_str::<serde_json::Value>(&resp.body)
                                .ok()
                                .and_then(|v| serde_json::to_string_pretty(&v).ok())
                                .unwrap_or_else(|| resp.body.clone())
                        } else {
                            resp.body.clone()
                        };

                        let mut jump_to_current_match = false;
                        ui.horizontal(|ui| {
                            ui.label("Search:");
                            let search_response = ui.add(
                                egui::TextEdit::singleline(&mut self.response_search)
                                    .desired_width(180.0)
                                    .hint_text("filter body…"),
                            );
                            if search_response.changed() {
                                self.response_search_current = 0;
                            }
                            if !self.response_search.is_empty() {
                                let match_count = count_matches(&pretty, &self.response_search);
                                if match_count > 0 {
                                    if ui.small_button("◀").on_hover_text("Previous match").clicked() {
                                        self.response_search_current = advance_match(
                                            self.response_search_current,
                                            match_count,
                                            false,
                                        );
                                        jump_to_current_match = true;
                                    }
                                    ui.weak(format!(
                                        "match {} of {match_count}",
                                        self.response_search_current + 1
                                    ));
                                    if ui.small_button("▶").on_hover_text("Next match").clicked() {
                                        self.response_search_current = advance_match(
                                            self.response_search_current,
                                            match_count,
                                            true,
                                        );
                                        jump_to_current_match = true;
                                    }
                                } else {
                                    ui.weak("0 match(es)");
                                }
                            }
                            if ui.button("Save Response…").clicked() {
                                let default_name =
                                    if is_json { "response.json" } else { "response.txt" }.to_string();
                                save_response_request = Some((resp.body_bytes.clone(), default_name));
                            }
                        });

                        if let Some(notice) = &self.response_save_notice {
                            ui.colored_label(egui::Color32::from_rgb(120, 190, 130), notice);
                        }

                        let dark_mode = ui.visuals().dark_mode;
                        let search_lower = self.response_search.to_lowercase();
                        // The byte offset of whichever match find-next/previous
                        // currently points at (if any) — computed once per
                        // frame and threaded into the layouter so *that*
                        // match, specifically, renders in a different color
                        // from every other match's plain highlight (see
                        // `append_with_search_highlight`'s doc comment for why
                        // that distinction matters: without it, clicking
                        // next/previous has no visible effect at all, since
                        // highlight-all already marks every match the same).
                        let current_match_byte =
                            find_match_starts(&pretty, &search_lower).get(self.response_search_current).copied();
                        let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
                            let text = buf.as_str();
                            let mut job = if is_json {
                                json_layout_job(text, dark_mode, &search_lower, current_match_byte)
                            } else {
                                plain_layout_job(text, dark_mode, &search_lower, current_match_byte)
                            };
                            job.wrap.max_width = wrap_width;
                            ui.fonts_mut(|f| f.layout_job(job))
                        };

                        egui::ScrollArea::vertical().show(ui, |ui| {
                            let output = egui::TextEdit::multiline(&mut pretty)
                                .id_salt("response_body")
                                .font(egui::TextStyle::Monospace)
                                .desired_width(f32::INFINITY)
                                .layouter(&mut layouter)
                                .show(ui);
                            // Scroll to the *exact* on-screen position of the
                            // current match, via the just-laid-out galley —
                            // `Response::scroll_to_me` on the whole multiline
                            // box wouldn't target a specific position inside
                            // it, which is the point of "next"/"previous".
                            if jump_to_current_match {
                                if let Some(byte_start) = current_match_byte {
                                    let char_start = pretty[..byte_start].chars().count();
                                    let cursor_rect = output
                                        .galley
                                        .pos_from_cursor(egui::text::CCursor::new(char_start));
                                    let screen_rect = cursor_rect.translate(output.galley_pos.to_vec2());
                                    ui.scroll_to_rect(screen_rect, Some(egui::Align::Center));
                                }
                            }
                        });
                    }
                }
            }
            }
            } // end CentralView::Request
            }

            if let Some(texture) = decoded_image_texture {
                self.response_image = Some(texture);
            }

            if let Some((body, default_name)) = save_response_request {
                self.save_response_to_file(&ctx, body, &default_name);
            }

            if should_send {
                self.send(&ctx);
            }
        });

        if self.in_flight {
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
    }

    /// Persist zoom/theme/open tabs (see `persistence`) — the only hook
    /// `eframe::App` gives on window close; there's no `egui::Context` here,
    /// which is why `last_zoom`/`last_theme` are cached once per frame in
    /// `ui()` instead of read directly.
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        persistence::save(&self.persisted_state());
    }
}

/// Renders a row's trailing "⋯" menu button, visible only while the row is
/// hovered (M8) — checked from last frame's hover state, since egui has no
/// live CSS `:hover` (a one-frame reveal lag, imperceptible in practice) —
/// or while its own popup is already open, so opening the menu doesn't
/// immediately hide the button that opened it. `row` renders everything to
/// the button's left, inside the same `ui.horizontal`, so hovering anywhere
/// across the row (not just over the button itself) keeps it visible.
/// Returns the index into `actions` that was clicked this frame, if any.
fn hover_ellipsis_row(
    ui: &mut egui::Ui,
    id_salt: impl std::hash::Hash + std::fmt::Debug,
    actions: &[&str],
    row: impl FnOnce(&mut egui::Ui),
) -> Option<usize> {
    let row_id = ui.make_persistent_id(id_salt);
    let popup_id = row_id.with("ellipsis_popup");
    let was_hovered = ui.ctx().data(|d| d.get_temp::<bool>(row_id)).unwrap_or(false);
    let show = was_hovered || egui::Popup::is_id_open(ui.ctx(), popup_id);

    let mut clicked = None;
    let row_response = ui
        .horizontal(|ui| {
            row(ui);
            let btn = ui.add_visible(show, egui::Button::new("⋯").small());
            if show {
                egui::Popup::new(popup_id, ui.ctx().clone(), &btn, btn.layer_id)
                    .open_memory(btn.clicked().then_some(egui::SetOpenCommand::Toggle))
                    .kind(egui::PopupKind::Menu)
                    .layout(egui::Layout::top_down_justified(egui::Align::Min))
                    .gap(0.0)
                    .show(|ui| {
                        for (i, label) in actions.iter().enumerate() {
                            if ui.button(*label).clicked() {
                                clicked = Some(i);
                                ui.close();
                            }
                        }
                    });
            }
        })
        .response;

    let now_hovered = row_response.hovered();
    if now_hovered != was_hovered {
        ui.ctx().data_mut(|d| d.insert_temp(row_id, now_hovered));
        ui.ctx().request_repaint();
    }

    clicked
}

/// Render a collection's (or folder's) sub-folders and requests, recursing
/// into each sub-folder as its own collapsing section. Returns the tree
/// action clicked this frame, if any — a free function rather than a
/// method since it only needs the tree data, not the rest of `CurlyApp`.
/// Navigate `folders` by a slash-separated path (M8, `show_folder_tab`) —
/// `None` if any segment along the way doesn't exist.
fn find_folder<'a>(folders: &'a [Folder], path: &str) -> Option<&'a Folder> {
    let mut current = folders;
    let mut found = None;
    for segment in path.split('/').filter(|s| !s.is_empty()) {
        found = current.iter().find(|f| f.name == segment);
        current = found.map(|f| f.folders.as_slice()).unwrap_or(&[]);
    }
    found
}

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
        let state_id = ui.make_persistent_id(("folder_state", folder.id));
        let header = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), state_id, false)
            .show_header(ui, |ui| {
                if let Some(i) = hover_ellipsis_row(
                    ui,
                    ("folder_ellipsis", folder.id),
                    &["New Folder", "New Request", "Delete Folder"],
                    |ui| {
                        if ui.selectable_label(false, format!("{}/", folder.name)).clicked() {
                            action = Some(TreeAction::OpenFolder(folder_prefix.clone()));
                        }
                    },
                ) {
                    action = Some(match i {
                        0 => TreeAction::NewFolder(folder_prefix.clone()),
                        1 => TreeAction::NewRequest(folder_prefix.clone()),
                        _ => TreeAction::DeleteFolder(folder_prefix.clone()),
                    });
                }
            });
        header.body(|ui| {
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
        let label = format!("{:<7} {}", request.method, request.name);
        if let Some(i) = hover_ellipsis_row(ui, ("request_ellipsis", request.id), &["Rename", "Duplicate", "Delete"], |ui| {
            if ui.selectable_label(false, &label).clicked() {
                action = Some(TreeAction::Load(path.clone(), request.clone()));
            }
        }) {
            action = Some(match i {
                0 => TreeAction::Rename(path.clone()),
                1 => TreeAction::Duplicate(path.clone()),
                _ => TreeAction::Delete(path.clone()),
            });
        }
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
    fn refresh_history_loads_from_storage() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = CurlyApp::default_state();
        app.open_project_at(dir.path());

        let request = Request::new(Method::GET, "https://example.com/a");
        app.active_project()
            .unwrap()
            .storage
            .append_history(&HistoryEntry::new(&request, &response(200, "ok")))
            .unwrap();

        app.refresh_history();

        assert_eq!(app.history_entries.len(), 1);
        assert_eq!(app.history_entries[0].url, "https://example.com/a");
    }

    #[test]
    fn refresh_history_is_empty_with_no_project_open() {
        let mut app = CurlyApp::default_state();
        app.refresh_history();
        assert!(app.history_entries.is_empty());
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
    fn load_saved_request_loads_bearer_auth() {
        let mut app = CurlyApp::default_state();
        let mut saved = SavedRequest::new("get-me", Method::GET, "https://example.com");
        saved.auth = Some(SavedAuth::Bearer {
            token: "abc123".to_string(),
        });

        app.load_saved_request(&saved);

        assert_eq!(app.auth.kind, AuthKind::Bearer);
        assert_eq!(app.auth.token, "abc123");
        assert!(app.headers.iter().all(|h| h.name != "Authorization"));
        assert!(app.load_notice.is_none());
    }

    #[test]
    fn load_saved_request_loads_basic_auth() {
        let mut app = CurlyApp::default_state();
        let mut saved = SavedRequest::new("get-me", Method::GET, "https://example.com");
        saved.auth = Some(SavedAuth::Basic {
            username: "alice".to_string(),
            password: "s3cret".to_string(),
        });

        app.load_saved_request(&saved);

        assert_eq!(app.auth.kind, AuthKind::Basic);
        assert_eq!(app.auth.username, "alice");
        assert_eq!(app.auth.password, "s3cret");
        assert!(app.headers.iter().all(|h| h.name != "Authorization"));
        assert!(app.load_notice.is_none());
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
    fn load_history_entry_carries_method_url_body_and_headers() {
        let mut app = CurlyApp::default_state();
        let request = Request::new(Method::POST, "https://example.com/users")
            .with_header("Accept", "application/json")
            .with_body(Body::Raw(r#"{"a":1}"#.to_string()));
        let entry = HistoryEntry::new(&request, &response(201, "ok"));

        app.load_history_entry(&entry);

        assert_eq!(app.method, "POST");
        assert_eq!(app.url, "https://example.com/users");
        assert_eq!(app.body, r#"{"a":1}"#);
        assert_eq!(app.headers.len(), 1);
        assert_eq!(app.headers[0].name, "Accept");
        assert_eq!(app.headers[0].value, "application/json");
        assert!(app.loaded_request.is_none());
        assert!(app.load_notice.is_none());
    }

    #[test]
    fn load_history_entry_drops_redacted_headers_with_a_notice() {
        let mut app = CurlyApp::default_state();
        let request = Request::new(Method::GET, "https://example.com")
            .with_header("Authorization", "Bearer super-secret")
            .with_header("Accept", "application/json");
        let entry = HistoryEntry::new(&request, &response(200, "ok"));

        app.load_history_entry(&entry);

        assert!(app.headers.iter().all(|h| h.name != "Authorization"));
        assert_eq!(app.headers.len(), 1);
        assert_eq!(app.headers[0].name, "Accept");
        assert!(app.load_notice.unwrap().contains("redacted"));
    }

    #[test]
    fn load_history_entry_notices_when_credentials_would_be_needed() {
        let mut app = CurlyApp::default_state();
        let request = Request::new(Method::GET, "https://example.com").with_auth(Auth::Bearer {
            token: "abc123".to_string(),
        });
        let entry = HistoryEntry::new(&request, &response(200, "ok"));

        app.load_history_entry(&entry);

        assert_eq!(app.auth.kind, AuthKind::Bearer);
        assert!(app.auth.token.is_empty());
        assert!(app.load_notice.unwrap().contains("credentials aren't stored"));
    }

    #[test]
    fn load_history_entry_no_auth_has_no_notice() {
        let mut app = CurlyApp::default_state();
        let request = Request::new(Method::GET, "https://example.com");
        let entry = HistoryEntry::new(&request, &response(200, "ok"));

        app.load_history_entry(&entry);

        assert_eq!(app.auth.kind, AuthKind::None);
        assert!(app.load_notice.is_none());
    }

    #[test]
    fn load_saved_request_clears_previous_response() {
        let mut app = CurlyApp::default_state();
        app.response = Some(Ok(ResponseSummary {
            status: 200,
            headers: vec![],
            body: "stale".to_string(),
            body_bytes: b"stale".to_vec(),
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
    fn build_request_applies_basic_auth() {
        let mut app = CurlyApp::default_state();
        app.url = "https://example.com".to_string();
        app.auth = AuthEditor {
            kind: AuthKind::Basic,
            username: "alice".to_string(),
            password: "s3cret".to_string(),
            ..Default::default()
        };

        let request = app.build_request().unwrap();

        assert!(matches!(
            request.auth,
            Some(Auth::Basic { ref username, ref password })
                if username == "alice" && password == "s3cret"
        ));
    }

    #[test]
    fn build_request_applies_bearer_auth() {
        let mut app = CurlyApp::default_state();
        app.url = "https://example.com".to_string();
        app.auth = AuthEditor {
            kind: AuthKind::Bearer,
            token: "abc123".to_string(),
            ..Default::default()
        };

        let request = app.build_request().unwrap();

        assert!(matches!(request.auth, Some(Auth::Bearer { ref token }) if token == "abc123"));
    }

    #[test]
    fn build_request_no_auth_by_default() {
        let mut app = CurlyApp::default_state();
        app.url = "https://example.com".to_string();

        let request = app.build_request().unwrap();

        assert!(request.auth.is_none());
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
            body_bytes: body.as_bytes().to_vec(),
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

    // --- M8: collection/folder tabs, environment tab ---

    #[test]
    fn find_folder_navigates_a_nested_path() {
        let mut oauth = Folder::new("OAuth");
        oauth.requests.push(SavedRequest::new("refresh", Method::POST, "https://example.com"));
        let mut auth = Folder::new("Auth");
        auth.folders.push(oauth);
        let folders = vec![auth];

        let found = find_folder(&folders, "Auth/OAuth").unwrap();
        assert_eq!(found.name, "OAuth");
        assert_eq!(found.requests[0].name, "refresh");
    }

    #[test]
    fn find_folder_none_for_a_missing_segment() {
        let folders = vec![Folder::new("Auth")];
        assert!(find_folder(&folders, "Auth/Missing").is_none());
    }

    #[test]
    fn open_or_focus_extra_tab_reuses_an_existing_entry_instead_of_duplicating_it() {
        let mut app = CurlyApp::default_state();
        app.open_or_focus_extra_tab(ExtraTabKind::Collection("my-api".to_string()));
        app.central_view = CentralView::Request; // navigate away
        app.open_or_focus_extra_tab(ExtraTabKind::Collection("my-api".to_string()));

        assert_eq!(app.extra_tabs.len(), 1);
        assert_eq!(app.central_view, CentralView::Collection("my-api".to_string()));
    }

    #[test]
    fn open_or_focus_extra_tab_opens_distinct_folders_separately() {
        let mut app = CurlyApp::default_state();
        app.open_or_focus_extra_tab(ExtraTabKind::Folder("my-api".to_string(), "Auth".to_string()));
        app.open_or_focus_extra_tab(ExtraTabKind::Folder("my-api".to_string(), "Health".to_string()));

        assert_eq!(app.extra_tabs.len(), 2);
        assert_eq!(
            app.central_view,
            CentralView::Folder("my-api".to_string(), "Health".to_string())
        );
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
    fn build_saved_request_drops_blank_named_extraction_rules() {
        let mut app = CurlyApp::default_state();
        app.extract_rules = vec![
            Extraction::Body {
                name: "ID".to_string(),
                path: "id".to_string(),
                secret: false,
            },
            Extraction::Header {
                name: "   ".to_string(),
                header: "X-Trace".to_string(),
                secret: false,
            },
        ];

        let saved = app.build_saved_request("x").unwrap();

        assert_eq!(saved.extract.len(), 1);
        assert_eq!(saved.extract[0].name(), "ID");
    }

    #[test]
    fn extraction_kind_identifies_each_variant() {
        assert!(matches!(
            extraction_kind(&Extraction::Body {
                name: "a".to_string(),
                path: "p".to_string(),
                secret: false
            }),
            ExtractionKindTag::Body
        ));
        assert!(matches!(
            extraction_kind(&Extraction::Header {
                name: "a".to_string(),
                header: "h".to_string(),
                secret: false
            }),
            ExtractionKindTag::Header
        ));
        assert!(matches!(
            extraction_kind(&Extraction::Template {
                name: "a".to_string(),
                template: "t".to_string(),
                secret: false
            }),
            ExtractionKindTag::Template
        ));
    }

    #[test]
    fn rebuild_extraction_carries_over_name_and_secret() {
        let rule = Extraction::Body {
            name: "TOKEN".to_string(),
            path: "token".to_string(),
            secret: true,
        };

        let rebuilt = rebuild_extraction(&rule, ExtractionKindTag::Header);

        match rebuilt {
            Extraction::Header { name, header, secret } => {
                assert_eq!(name, "TOKEN");
                assert!(header.is_empty());
                assert!(secret);
            }
            other => panic!("expected Header, got {other:?}"),
        }
    }

    #[test]
    fn build_saved_request_empty_body_is_none() {
        let app = CurlyApp::default_state();
        let saved = app.build_saved_request("x").unwrap();
        assert!(saved.body.is_none());
    }

    #[test]
    fn build_saved_request_carries_over_auth() {
        let mut app = CurlyApp::default_state();
        app.auth = AuthEditor {
            kind: AuthKind::Basic,
            username: "alice".to_string(),
            password: "s3cret".to_string(),
            ..Default::default()
        };

        let saved = app.build_saved_request("x").unwrap();

        match saved.auth {
            Some(SavedAuth::Basic { username, password }) => {
                assert_eq!(username, "alice");
                assert_eq!(password, "s3cret");
            }
            other => panic!("expected Basic auth, got {other:?}"),
        }
    }

    #[test]
    fn build_saved_request_no_auth_is_none() {
        let app = CurlyApp::default_state();
        let saved = app.build_saved_request("x").unwrap();
        assert!(saved.auth.is_none());
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
    fn open_path_dialog_prefills_folder_and_name_from_the_path() {
        let mut app = CurlyApp::default_state();
        app.open_path_dialog(PathDialogMode::Rename, "my-api".to_string(), "Auth/login".to_string());

        let dialog = app.path_dialog.unwrap();
        assert_eq!(dialog.mode, PathDialogMode::Rename);
        assert_eq!(dialog.collection, "my-api");
        assert_eq!(dialog.old_path, "Auth/login");
        assert_eq!(dialog.folder_path, "Auth");
        assert_eq!(dialog.name, "login");
    }

    #[test]
    fn opening_the_path_dialog_closes_an_open_save_dialog_and_vice_versa() {
        let mut app = CurlyApp::default_state();
        app.open_save_dialog();
        assert!(app.save_dialog.is_some());

        app.open_path_dialog(PathDialogMode::Rename, "my-api".to_string(), "login".to_string());
        assert!(app.save_dialog.is_none());
        assert!(app.path_dialog.is_some());

        app.open_save_dialog();
        assert!(app.path_dialog.is_none());
    }

    #[test]
    fn cancel_path_dialog_clears_it() {
        let mut app = CurlyApp::default_state();
        app.open_path_dialog(PathDialogMode::Rename, "my-api".to_string(), "login".to_string());
        app.cancel_path_dialog();
        assert!(app.path_dialog.is_none());
    }

    #[test]
    fn sync_breadcrumb_reflects_the_loaded_requests_name() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = project_with(&dir);
        let mut collection = Collection::new("my-api");
        collection.add_request("Auth/login", SavedRequest::new("login", Method::GET, "https://example.com")).unwrap();
        app.active_project().unwrap().storage.save_collection(&collection).unwrap();
        app.loaded_request = Some(LoadedRequestRef {
            collection: "my-api".to_string(),
            path: "Auth/login".to_string(),
        });

        app.sync_breadcrumb();

        assert_eq!(app.breadcrumb_name, "login");
    }

    #[test]
    fn sync_breadcrumb_is_empty_with_nothing_loaded() {
        let mut app = CurlyApp::default_state();
        app.breadcrumb_name = "stale".to_string();
        app.breadcrumb_synced_path = Some("stale/path".to_string());

        app.sync_breadcrumb();

        assert_eq!(app.breadcrumb_name, "");
    }

    #[test]
    fn save_breadcrumb_name_renames_the_request_and_resyncs() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = project_with(&dir);
        let mut collection = Collection::new("my-api");
        collection.add_request("Auth/login", SavedRequest::new("login", Method::GET, "https://example.com")).unwrap();
        app.active_project().unwrap().storage.save_collection(&collection).unwrap();
        app.loaded_request = Some(LoadedRequestRef {
            collection: "my-api".to_string(),
            path: "Auth/login".to_string(),
        });
        app.sync_breadcrumb();
        app.breadcrumb_name = "sign-in".to_string();

        app.save_breadcrumb_name();

        assert_eq!(
            app.loaded_request,
            Some(LoadedRequestRef {
                collection: "my-api".to_string(),
                path: "Auth/sign-in".to_string(),
            })
        );
        assert_eq!(app.breadcrumb_name, "sign-in");
        let reloaded = app.active_project().unwrap().storage.load_collection("my-api").unwrap();
        assert!(reloaded.find_request("Auth/login").is_none());
        assert!(reloaded.find_request("Auth/sign-in").is_some());
    }

    #[test]
    fn save_breadcrumb_name_rejects_a_blank_name_without_renaming() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = project_with(&dir);
        let mut collection = Collection::new("my-api");
        collection.add_request("login", SavedRequest::new("login", Method::GET, "https://example.com")).unwrap();
        app.active_project().unwrap().storage.save_collection(&collection).unwrap();
        app.loaded_request = Some(LoadedRequestRef {
            collection: "my-api".to_string(),
            path: "login".to_string(),
        });
        app.sync_breadcrumb();
        app.breadcrumb_name = "   ".to_string();

        app.save_breadcrumb_name();

        assert_eq!(app.loaded_request.unwrap().path, "login");
        assert!(app.collection_notice.unwrap().contains("name"));
    }

    #[test]
    fn confirm_path_dialog_renames_and_updates_the_loaded_request() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = project_with(&dir);
        let mut collection = Collection::new("my-api");
        collection.add_request("login", SavedRequest::new("login", Method::GET, "https://example.com")).unwrap();
        app.active_project().unwrap().storage.save_collection(&collection).unwrap();
        app.loaded_request = Some(LoadedRequestRef {
            collection: "my-api".to_string(),
            path: "login".to_string(),
        });
        app.open_path_dialog(PathDialogMode::Rename, "my-api".to_string(), "login".to_string());
        app.path_dialog.as_mut().unwrap().name = "sign-in".to_string();

        app.confirm_path_dialog();

        assert!(app.path_dialog.is_none());
        let reloaded = app.active_project().unwrap().storage.load_collection("my-api").unwrap();
        assert!(reloaded.find_request("login").is_none());
        assert!(reloaded.find_request("sign-in").is_some());
        assert_eq!(
            app.loaded_request.unwrap(),
            LoadedRequestRef {
                collection: "my-api".to_string(),
                path: "sign-in".to_string(),
            }
        );
    }

    #[test]
    fn confirm_path_dialog_duplicate_leaves_the_loaded_request_alone() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = project_with(&dir);
        let mut collection = Collection::new("my-api");
        collection.add_request("login", SavedRequest::new("login", Method::GET, "https://example.com")).unwrap();
        app.active_project().unwrap().storage.save_collection(&collection).unwrap();
        app.loaded_request = Some(LoadedRequestRef {
            collection: "my-api".to_string(),
            path: "login".to_string(),
        });
        app.open_path_dialog(PathDialogMode::Duplicate, "my-api".to_string(), "login".to_string());
        app.path_dialog.as_mut().unwrap().name = "login-copy".to_string();

        app.confirm_path_dialog();

        assert!(app.path_dialog.is_none());
        let reloaded = app.active_project().unwrap().storage.load_collection("my-api").unwrap();
        assert!(reloaded.find_request("login").is_some());
        assert!(reloaded.find_request("login-copy").is_some());
        assert_eq!(
            app.loaded_request.unwrap(),
            LoadedRequestRef {
                collection: "my-api".to_string(),
                path: "login".to_string(),
            }
        );
    }

    #[test]
    fn confirm_path_dialog_rejects_a_blank_name() {
        let mut app = CurlyApp::default_state();
        app.open_path_dialog(PathDialogMode::Rename, "my-api".to_string(), "login".to_string());
        app.path_dialog.as_mut().unwrap().name = "  ".to_string();

        app.confirm_path_dialog();

        assert!(app.path_dialog.is_some());
        assert!(app.path_dialog.unwrap().error.unwrap().contains("name"));
    }

    #[test]
    fn confirm_path_dialog_reports_a_collision_without_closing() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = project_with(&dir);
        let mut collection = Collection::new("my-api");
        collection.add_request("login", SavedRequest::new("login", Method::GET, "https://example.com")).unwrap();
        collection.add_request("logout", SavedRequest::new("logout", Method::GET, "https://example.com")).unwrap();
        app.active_project().unwrap().storage.save_collection(&collection).unwrap();
        app.open_path_dialog(PathDialogMode::Rename, "my-api".to_string(), "login".to_string());
        app.path_dialog.as_mut().unwrap().name = "logout".to_string();

        app.confirm_path_dialog();

        assert!(app.path_dialog.is_some());
        assert!(app.path_dialog.unwrap().error.is_some());
    }

    #[test]
    fn confirm_folder_dialog_creates_a_folder_in_a_new_collection() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = project_with(&dir);
        app.open_folder_dialog("my-api".to_string(), String::new());
        app.folder_dialog.as_mut().unwrap().name = "Auth".to_string();

        app.confirm_folder_dialog();

        assert!(app.folder_dialog.is_none());
        let reloaded = app.active_project().unwrap().storage.load_collection("my-api").unwrap();
        assert!(reloaded.folders.iter().any(|f| f.name == "Auth"));
    }

    #[test]
    fn confirm_folder_dialog_nests_under_the_given_parent() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = project_with(&dir);
        let mut collection = Collection::new("my-api");
        collection.add_folder("Auth");
        app.active_project().unwrap().storage.save_collection(&collection).unwrap();

        app.open_folder_dialog("my-api".to_string(), "Auth".to_string());
        app.folder_dialog.as_mut().unwrap().name = "OAuth".to_string();
        app.confirm_folder_dialog();

        let reloaded = app.active_project().unwrap().storage.load_collection("my-api").unwrap();
        let auth = reloaded.folders.iter().find(|f| f.name == "Auth").unwrap();
        assert!(auth.folders.iter().any(|f| f.name == "OAuth"));
    }

    #[test]
    fn confirm_folder_dialog_rejects_a_blank_name() {
        let mut app = CurlyApp::default_state();
        app.open_folder_dialog("my-api".to_string(), String::new());
        app.folder_dialog.as_mut().unwrap().name = "  ".to_string();

        app.confirm_folder_dialog();

        assert!(app.folder_dialog.is_some());
        assert!(app.folder_dialog.unwrap().error.unwrap().contains("name"));
    }

    #[test]
    fn delete_folder_removes_an_empty_folder() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = project_with(&dir);
        let mut collection = Collection::new("my-api");
        collection.add_folder("Auth");
        app.active_project().unwrap().storage.save_collection(&collection).unwrap();

        app.delete_folder("my-api", "Auth");

        let reloaded = app.active_project().unwrap().storage.load_collection("my-api").unwrap();
        assert!(reloaded.folders.is_empty());
        assert!(app.collection_notice.unwrap().contains("deleted"));
    }

    #[test]
    fn delete_folder_refuses_a_non_empty_folder_without_deleting_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = project_with(&dir);
        let mut collection = Collection::new("my-api");
        collection.add_request("Auth/login", SavedRequest::new("login", Method::GET, "https://example.com")).unwrap();
        app.active_project().unwrap().storage.save_collection(&collection).unwrap();

        app.delete_folder("my-api", "Auth");

        let reloaded = app.active_project().unwrap().storage.load_collection("my-api").unwrap();
        assert!(reloaded.folders.iter().any(|f| f.name == "Auth"));
        assert!(app.collection_notice.unwrap().contains("failed"));
    }

    #[test]
    fn save_all_in_collection_saves_every_dirty_tab_in_that_collection_and_restores_the_active_one() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = project_with(&dir);
        let mut collection = Collection::new("my-api");
        collection.add_request("login", SavedRequest::new("login", Method::GET, "https://old.example.com")).unwrap();
        collection.add_request("logout", SavedRequest::new("logout", Method::GET, "https://old.example.com")).unwrap();
        app.active_project().unwrap().storage.save_collection(&collection).unwrap();

        // Tab 0: load + dirty "login".
        let login = collection.find_request("login").unwrap().clone();
        app.loaded_request = Some(LoadedRequestRef {
            collection: "my-api".to_string(),
            path: "login".to_string(),
        });
        app.load_saved_request(&login);
        app.url = "https://new.example.com/login".to_string();

        // Tab 1 (active): load + dirty "logout" in a second, unrelated tab.
        app.open_new_tab();
        let logout = collection.find_request("logout").unwrap().clone();
        app.loaded_request = Some(LoadedRequestRef {
            collection: "my-api".to_string(),
            path: "logout".to_string(),
        });
        app.load_saved_request(&logout);
        app.url = "https://new.example.com/logout".to_string();

        app.save_all_in_collection("my-api");

        assert_eq!(app.active_tab_idx, 1, "the originally active tab should still be active afterward");
        assert_eq!(app.url, "https://new.example.com/logout", "the active tab's live state must survive the swap");
        let reloaded = app.active_project().unwrap().storage.load_collection("my-api").unwrap();
        assert_eq!(reloaded.find_request("login").unwrap().url, "https://new.example.com/login");
        assert_eq!(reloaded.find_request("logout").unwrap().url, "https://new.example.com/logout");
        assert!(app.collection_notice.unwrap().contains("2 request"));
    }

    #[test]
    fn save_all_in_collection_skips_clean_tabs_and_other_collections() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = project_with(&dir);
        let mut collection = Collection::new("my-api");
        collection.add_request("login", SavedRequest::new("login", Method::GET, "https://example.com")).unwrap();
        app.active_project().unwrap().storage.save_collection(&collection).unwrap();

        // Loaded, but never edited — not dirty.
        let login = collection.find_request("login").unwrap().clone();
        app.loaded_request = Some(LoadedRequestRef {
            collection: "my-api".to_string(),
            path: "login".to_string(),
        });
        app.load_saved_request(&login);

        app.save_all_in_collection("my-api");

        assert!(app.collection_notice.unwrap().contains("nothing to save"));
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

    #[test]
    fn parse_kde_globals_dark_scheme() {
        let contents = "[General]\nColorScheme=BreezeDark\nName=Breeze Dark\n";
        assert_eq!(parse_kde_globals(contents), Some(egui::Theme::Dark));
    }

    #[test]
    fn parse_kde_globals_light_scheme() {
        let contents = "[General]\nColorScheme=Breeze\n";
        assert_eq!(parse_kde_globals(contents), Some(egui::Theme::Light));
    }

    #[test]
    fn parse_kde_globals_ignores_color_scheme_outside_general_section() {
        let contents = "[Other]\nColorScheme=BreezeDark\n[General]\nSomethingElse=1\n";
        assert_eq!(parse_kde_globals(contents), None);
    }

    #[test]
    fn parse_kde_globals_missing_key_is_none() {
        assert_eq!(parse_kde_globals("[General]\nSomethingElse=1\n"), None);
    }

    #[test]
    fn parse_kde_globals_empty_is_none() {
        assert_eq!(parse_kde_globals(""), None);
    }

    // --- FR-22: JSON syntax highlighting + in-body search ---

    #[test]
    fn tokenize_json_identifies_a_string() {
        let text = r#""hello""#;
        let spans = tokenize_json(text);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].1, JsonTokenKind::String);
        assert_eq!(spans[0].0, 0..text.len());
    }

    #[test]
    fn tokenize_json_identifies_a_number() {
        let spans = tokenize_json("42");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].1, JsonTokenKind::Number);
    }

    #[test]
    fn tokenize_json_identifies_a_negative_float() {
        let spans = tokenize_json("-3.14");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].1, JsonTokenKind::Number);
        assert_eq!(spans[0].0, 0..5);
    }

    #[test]
    fn tokenize_json_identifies_keywords() {
        for word in ["true", "false", "null"] {
            let spans = tokenize_json(word);
            assert_eq!(spans.len(), 1, "word: {word}");
            assert_eq!(spans[0].1, JsonTokenKind::Keyword, "word: {word}");
            assert_eq!(spans[0].0, 0..word.len(), "word: {word}");
        }
    }

    #[test]
    fn tokenize_json_identifies_punctuation() {
        let spans = tokenize_json("{}[]:,");
        assert_eq!(spans.len(), 6);
        for (_, kind) in &spans {
            assert_eq!(*kind, JsonTokenKind::Punct);
        }
    }

    #[test]
    fn tokenize_json_handles_an_escaped_quote_inside_a_string() {
        let text = r#""a\"b""#;
        let spans = tokenize_json(text);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].1, JsonTokenKind::String);
        assert_eq!(spans[0].0, 0..text.len());
    }

    #[test]
    fn tokenize_json_covers_the_whole_input_with_no_gaps() {
        let text = r#"{"name": "curly", "count": 3, "ok": true, "note": null}"#;
        let spans = tokenize_json(text);
        let mut expected_start = 0;
        for (range, _) in &spans {
            assert_eq!(range.start, expected_start);
            expected_start = range.end;
        }
        assert_eq!(expected_start, text.len());
    }

    #[test]
    fn tokenize_json_handles_multibyte_utf8_string_content_without_panicking() {
        let text = "\"héllo wörld 🎉\"";
        let spans = tokenize_json(text);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].1, JsonTokenKind::String);
        assert_eq!(spans[0].0, 0..text.len());
    }

    #[test]
    fn count_matches_is_zero_for_an_empty_search() {
        assert_eq!(count_matches("hello world", ""), 0);
    }

    #[test]
    fn count_matches_counts_case_insensitively() {
        assert_eq!(count_matches("Hello HELLO hello", "hello"), 3);
    }

    #[test]
    fn count_matches_is_zero_when_not_found() {
        assert_eq!(count_matches("hello world", "xyz"), 0);
    }

    #[test]
    fn is_image_content_type_detects_image_png() {
        let headers = vec![("Content-Type".to_string(), "image/png".to_string())];
        assert!(is_image_content_type(&headers));
    }

    #[test]
    fn is_image_content_type_is_case_insensitive_on_header_name() {
        let headers = vec![("content-type".to_string(), "image/jpeg".to_string())];
        assert!(is_image_content_type(&headers));
    }

    #[test]
    fn is_image_content_type_ignores_charset_suffix() {
        let headers = vec![("Content-Type".to_string(), "image/png; charset=binary".to_string())];
        assert!(is_image_content_type(&headers));
    }

    #[test]
    fn is_image_content_type_false_for_json() {
        let headers = vec![("Content-Type".to_string(), "application/json".to_string())];
        assert!(!is_image_content_type(&headers));
    }

    #[test]
    fn is_image_content_type_false_with_no_header() {
        assert!(!is_image_content_type(&[]));
    }

    #[test]
    fn image_file_extension_maps_jpeg_to_jpg() {
        let headers = vec![("Content-Type".to_string(), "image/jpeg".to_string())];
        assert_eq!(image_file_extension(&headers), "jpg");
    }

    #[test]
    fn image_file_extension_uses_subtype_directly_for_png() {
        let headers = vec![("Content-Type".to_string(), "image/png".to_string())];
        assert_eq!(image_file_extension(&headers), "png");
    }

    #[test]
    fn image_file_extension_falls_back_to_bin_with_no_image_header() {
        assert_eq!(image_file_extension(&[]), "bin");
    }

    #[test]
    fn find_match_starts_empty_search_is_empty() {
        assert_eq!(find_match_starts("hello world", ""), Vec::<usize>::new());
    }

    #[test]
    fn find_match_starts_no_match_is_empty() {
        assert_eq!(find_match_starts("hello world", "xyz"), Vec::<usize>::new());
    }

    #[test]
    fn find_match_starts_finds_every_occurrence_in_order() {
        assert_eq!(find_match_starts("Hello HELLO hello", "hello"), vec![0, 6, 12]);
    }

    #[test]
    fn find_match_starts_single_match() {
        assert_eq!(find_match_starts("hello world", "world"), vec![6]);
    }

    #[test]
    fn advance_match_wraps_forward() {
        assert_eq!(advance_match(0, 3, true), 1);
        assert_eq!(advance_match(2, 3, true), 0);
    }

    #[test]
    fn advance_match_wraps_backward() {
        assert_eq!(advance_match(1, 3, false), 0);
        assert_eq!(advance_match(0, 3, false), 2);
    }

    #[test]
    fn advance_match_with_no_matches_stays_zero() {
        assert_eq!(advance_match(0, 0, true), 0);
        assert_eq!(advance_match(0, 0, false), 0);
    }

    fn highlight_job(text: &str, search_lower: &str) -> egui::text::LayoutJob {
        highlight_job_with_current(text, search_lower, None)
    }

    fn highlight_job_with_current(
        text: &str,
        search_lower: &str,
        current_match_byte: Option<usize>,
    ) -> egui::text::LayoutJob {
        let mut job = egui::text::LayoutJob::default();
        append_with_search_highlight(
            &mut job,
            text,
            egui::Color32::WHITE,
            egui::FontId::monospace(13.0),
            search_lower,
            0,
            current_match_byte,
            egui::Color32::YELLOW,
            egui::Color32::BLACK,
            egui::Color32::ORANGE,
            egui::Color32::BLACK,
        );
        job
    }

    fn section_texts(job: &egui::text::LayoutJob) -> Vec<String> {
        job.sections
            .iter()
            .map(|s| job.text[s.byte_range.start.0..s.byte_range.end.0].to_string())
            .collect()
    }

    #[test]
    fn append_with_search_highlight_empty_search_is_one_plain_section() {
        let job = highlight_job("hello world", "");
        assert_eq!(job.text, "hello world");
        assert_eq!(section_texts(&job), vec!["hello world".to_string()]);
        assert_eq!(job.sections[0].format.background, egui::Color32::TRANSPARENT);
    }

    #[test]
    fn append_with_search_highlight_no_match_is_one_plain_section() {
        let job = highlight_job("hello world", "xyz");
        assert_eq!(job.text, "hello world");
        assert_eq!(section_texts(&job), vec!["hello world".to_string()]);
        assert_eq!(job.sections[0].format.background, egui::Color32::TRANSPARENT);
    }

    #[test]
    fn append_with_search_highlight_splits_around_a_middle_match() {
        let job = highlight_job("aXb", "x");
        assert_eq!(job.text, "aXb");
        assert_eq!(
            section_texts(&job),
            vec!["a".to_string(), "X".to_string(), "b".to_string()]
        );
        assert_eq!(job.sections[1].format.background, egui::Color32::YELLOW);
        assert_eq!(job.sections[0].format.background, egui::Color32::TRANSPARENT);
        assert_eq!(job.sections[2].format.background, egui::Color32::TRANSPARENT);
    }

    #[test]
    fn append_with_search_highlight_match_at_start() {
        let job = highlight_job("Xab", "x");
        assert_eq!(section_texts(&job), vec!["X".to_string(), "ab".to_string()]);
        assert_eq!(job.sections[0].format.background, egui::Color32::YELLOW);
    }

    #[test]
    fn append_with_search_highlight_match_at_end() {
        let job = highlight_job("abX", "x");
        assert_eq!(section_texts(&job), vec!["ab".to_string(), "X".to_string()]);
        assert_eq!(job.sections[1].format.background, egui::Color32::YELLOW);
    }

    #[test]
    fn append_with_search_highlight_two_matches() {
        let job = highlight_job("aXbXc", "x");
        assert_eq!(job.text, "aXbXc");
        assert_eq!(
            section_texts(&job),
            vec![
                "a".to_string(),
                "X".to_string(),
                "b".to_string(),
                "X".to_string(),
                "c".to_string(),
            ]
        );
    }

    #[test]
    fn append_with_search_highlight_is_case_insensitive() {
        let job = highlight_job("ABC", "b");
        assert_eq!(
            section_texts(&job),
            vec!["A".to_string(), "B".to_string(), "C".to_string()]
        );
        assert_eq!(job.sections[1].format.background, egui::Color32::YELLOW);
    }

    #[test]
    fn append_with_search_highlight_marks_the_current_match_distinctly() {
        // Two matches ("X" at byte 1 and byte 3) — mark the second as current.
        let job = highlight_job_with_current("aXbXc", "x", Some(3));
        assert_eq!(job.sections[1].format.background, egui::Color32::YELLOW); // first X: not current
        assert_eq!(job.sections[3].format.background, egui::Color32::ORANGE); // second X: current
    }

    #[test]
    fn append_with_search_highlight_no_current_match_uses_plain_highlight_for_all() {
        let job = highlight_job_with_current("aXbXc", "x", None);
        assert_eq!(job.sections[1].format.background, egui::Color32::YELLOW);
        assert_eq!(job.sections[3].format.background, egui::Color32::YELLOW);
    }

    #[test]
    fn append_with_search_highlight_current_match_byte_is_relative_to_token_start() {
        // token_start = 10, so a match at local byte 1 is global byte 11.
        let mut job = egui::text::LayoutJob::default();
        append_with_search_highlight(
            &mut job,
            "aXb",
            egui::Color32::WHITE,
            egui::FontId::monospace(13.0),
            "x",
            10,
            Some(11),
            egui::Color32::YELLOW,
            egui::Color32::BLACK,
            egui::Color32::ORANGE,
            egui::Color32::BLACK,
        );
        assert_eq!(job.sections[1].format.background, egui::Color32::ORANGE);
    }

    // --- Toolbar: zoom ---
    //
    // set_zoom_factor/apply_zoom_delta themselves aren't unit-tested here:
    // egui::Context::set_zoom_factor's own doc comment says the change
    // doesn't reflect in zoom_factor() until the *next* pass begins, which
    // a bare Context::default() in a test never runs — so clamp_zoom (the
    // actual decision logic) is tested standalone, decoupled from Context
    // entirely, and the thin ctx-touching wrappers are left as untested
    // integration glue, matching how detect_system_theme's gsettings call
    // and open_project_dialog's rfd call are also left untested.

    #[test]
    fn clamp_zoom_leaves_an_in_range_value_alone() {
        assert_eq!(clamp_zoom(1.5), 1.5);
    }

    #[test]
    fn clamp_zoom_clamps_below_the_minimum() {
        assert_eq!(clamp_zoom(0.1), MIN_ZOOM);
    }

    #[test]
    fn clamp_zoom_clamps_above_the_maximum() {
        assert_eq!(clamp_zoom(10.0), MAX_ZOOM);
    }

    #[test]
    fn clamp_zoom_leaves_the_default_100_percent_alone() {
        assert_eq!(clamp_zoom(1.0), 1.0);
    }

    #[test]
    fn zoom_step_in_then_out_returns_to_the_starting_value() {
        let zoomed_in = clamp_zoom(1.0 * ZOOM_STEP);
        let back_out = clamp_zoom(zoomed_in / ZOOM_STEP);
        assert!((back_out - 1.0).abs() < 1e-5);
    }

    // --- FR-19: multiple request tabs ---

    #[test]
    fn default_state_has_exactly_one_tab() {
        let app = CurlyApp::default_state();
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.active_tab_idx, 0);
    }

    #[test]
    fn open_new_tab_adds_a_second_tab_and_makes_it_active() {
        let mut app = CurlyApp::default_state();
        app.url = "https://example.com/original".to_string();

        app.open_new_tab();

        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.active_tab_idx, 1);
        assert!(app.url.is_empty());
        assert_eq!(app.method, "GET");
    }

    #[test]
    fn switching_tabs_round_trips_editor_state() {
        let mut app = CurlyApp::default_state();
        app.url = "https://example.com/first".to_string();
        app.method = "POST".to_string();
        app.body = "first-body".to_string();

        app.open_new_tab();
        app.url = "https://example.com/second".to_string();
        app.method = "DELETE".to_string();

        app.switch_to_tab(0);
        assert_eq!(app.url, "https://example.com/first");
        assert_eq!(app.method, "POST");
        assert_eq!(app.body, "first-body");

        app.switch_to_tab(1);
        assert_eq!(app.url, "https://example.com/second");
        assert_eq!(app.method, "DELETE");
    }

    #[test]
    fn poll_response_marks_a_background_tab_with_an_unseen_response() {
        let mut app = CurlyApp::default_state();
        app.open_new_tab();
        assert_eq!(app.active_tab_idx, 1);

        let (tx, rx) = mpsc::channel();
        app.tabs[0].rx = Some(rx);
        app.tabs[0].in_flight = true;
        tx.send(SendOutcome::Success(response(200, "ok"))).unwrap();

        app.poll_response();

        assert!(app.tabs[0].has_unseen_response);
        assert!(!app.tabs[0].in_flight);
        assert!(app.tabs[0].rx.is_none());
        assert!(matches!(app.tabs[0].response, Some(Ok(_))));
    }

    #[test]
    fn poll_response_does_not_mark_the_active_tab() {
        let mut app = CurlyApp::default_state();
        let (tx, rx) = mpsc::channel();
        app.rx = Some(rx);
        tx.send(SendOutcome::Success(response(200, "ok"))).unwrap();

        app.poll_response();

        // The active tab's "unseen" state has nowhere to live (no such
        // field on CurlyApp) — its parked placeholder should also read
        // false once it's actually parked.
        let parked = app.take_active_tab();
        assert!(!parked.has_unseen_response);
    }

    #[test]
    fn switching_to_a_tab_leaves_a_fresh_unseen_marker_behind() {
        let mut app = CurlyApp::default_state();
        app.open_new_tab();
        let (tx, rx) = mpsc::channel();
        app.tabs[0].rx = Some(rx);
        tx.send(SendOutcome::Success(response(200, "ok"))).unwrap();
        app.poll_response();
        assert!(app.tabs[0].has_unseen_response);

        app.switch_to_tab(0);

        // tabs[0]'s content (including the marker) moved onto `self`; the
        // slot left behind (now holding the *other* tab, parked) is a
        // fresh default.
        assert!(!app.tabs[1].has_unseen_response);
    }

    #[test]
    fn persisted_state_reflects_every_tabs_content_including_the_active_one() {
        let mut app = CurlyApp::default_state();
        app.url = "https://example.com/first".to_string();
        app.method = "POST".to_string();

        app.open_new_tab();
        app.url = "https://example.com/second".to_string();
        app.method = "DELETE".to_string();

        let state = app.persisted_state();
        assert_eq!(state.tabs.len(), 2);
        assert_eq!(state.tabs[0].url, "https://example.com/first");
        assert_eq!(state.tabs[0].method, "POST");
        assert_eq!(state.tabs[1].url, "https://example.com/second");
        assert_eq!(state.tabs[1].method, "DELETE");
        assert_eq!(state.active_tab_idx, 1);
    }

    #[test]
    fn switch_to_tab_is_a_no_op_for_the_already_active_tab() {
        let mut app = CurlyApp::default_state();
        app.url = "https://example.com".to_string();

        app.switch_to_tab(0);

        assert_eq!(app.url, "https://example.com");
    }

    #[test]
    fn switch_to_tab_is_a_no_op_for_an_out_of_range_index() {
        let mut app = CurlyApp::default_state();
        app.url = "https://example.com".to_string();

        app.switch_to_tab(5);

        assert_eq!(app.active_tab_idx, 0);
        assert_eq!(app.url, "https://example.com");
    }

    #[test]
    fn closing_the_only_tab_resets_it_instead_of_removing_it() {
        let mut app = CurlyApp::default_state();
        app.url = "https://example.com".to_string();

        app.close_tab(0);

        assert_eq!(app.tabs.len(), 1);
        assert!(app.url.is_empty());
    }

    #[test]
    fn closing_an_inactive_tab_leaves_the_active_one_untouched() {
        let mut app = CurlyApp::default_state();
        app.url = "https://example.com/first".to_string();
        app.open_new_tab();
        app.url = "https://example.com/second".to_string();

        app.close_tab(0);

        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.active_tab_idx, 0);
        assert_eq!(app.url, "https://example.com/second");
    }

    #[test]
    fn closing_an_inactive_tab_before_the_active_one_shifts_the_active_index_down() {
        let mut app = CurlyApp::default_state();
        app.open_new_tab();
        app.open_new_tab();
        // tabs: [0]=blank, [1]=blank, [2]=active
        app.url = "https://example.com/active".to_string();

        app.close_tab(0);

        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.active_tab_idx, 1);
        assert_eq!(app.url, "https://example.com/active");
    }

    #[test]
    fn closing_the_active_tab_switches_to_a_neighbor() {
        let mut app = CurlyApp::default_state();
        app.url = "https://example.com/first".to_string();
        app.open_new_tab();
        app.url = "https://example.com/second".to_string();

        app.close_tab(1);

        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.active_tab_idx, 0);
        assert_eq!(app.url, "https://example.com/first");
    }

    #[test]
    fn close_tab_is_a_no_op_for_an_out_of_range_index() {
        let mut app = CurlyApp::default_state();
        app.url = "https://example.com".to_string();

        app.close_tab(9);

        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.url, "https://example.com");
    }

    #[test]
    fn is_tab_dirty_a_blank_tab_matches_its_own_snapshot_and_is_clean() {
        let snapshot = Some(EditableSnapshot::blank());
        assert!(!is_tab_dirty(
            &snapshot,
            "GET",
            "",
            &[HeaderRow::empty()],
            "",
            &AuthEditor::default(),
            &[]
        ));
    }

    #[test]
    fn is_tab_dirty_true_once_a_field_diverges_from_the_snapshot() {
        let snapshot = Some(EditableSnapshot::blank());
        assert!(is_tab_dirty(
            &snapshot,
            "GET",
            "http://x.io",
            &[HeaderRow::empty()],
            "",
            &AuthEditor::default(),
            &[]
        ));
    }

    #[test]
    fn is_tab_dirty_false_once_the_field_reverts_to_match_the_snapshot() {
        // Round-tripping a value back to what was last saved clears dirty —
        // it's derived from a comparison, not a sticky flag.
        let snapshot = Some(EditableSnapshot::capture(
            "GET",
            "http://x.io",
            &[],
            "",
            &AuthEditor::default(),
            &[],
        ));
        assert!(!is_tab_dirty(
            &snapshot,
            "GET",
            "http://x.io",
            &[],
            "",
            &AuthEditor::default(),
            &[]
        ));
    }

    #[test]
    fn is_tab_dirty_with_no_snapshot_at_all_is_always_dirty() {
        assert!(is_tab_dirty(&None, "GET", "", &[], "", &AuthEditor::default(), &[]));
    }

    #[test]
    fn tab_label_is_new_request_for_a_blank_tab() {
        let app = CurlyApp::default_state();
        assert_eq!(app.tab_label(0), "New Request");
    }

    #[test]
    fn tab_label_shows_method_and_url_when_not_loaded_from_a_saved_request() {
        let mut app = CurlyApp::default_state();
        app.method = "POST".to_string();
        app.url = "http://x.io/a".to_string();

        // Dirty (M8) — content was typed but nothing's been saved yet.
        assert_eq!(app.tab_label(0), "* POST http://x.io/a");
    }

    #[test]
    fn tab_label_shows_the_loaded_requests_path() {
        let mut app = CurlyApp::default_state();
        app.loaded_request = Some(LoadedRequestRef {
            collection: "my-api".to_string(),
            path: "Auth/login".to_string(),
        });

        assert_eq!(app.tab_label(0), "Auth/login");
    }

    #[test]
    fn tab_label_truncates_a_long_url() {
        let mut app = CurlyApp::default_state();
        app.url = "https://example.com/a/very/long/path/that/goes/on".to_string();

        let label = app.tab_label(0);
        assert!(label.ends_with('…'));
        assert!(label.chars().count() <= 30);
    }

    #[test]
    fn tab_label_reflects_the_active_tabs_live_state_not_a_stale_snapshot() {
        let mut app = CurlyApp::default_state();
        app.open_new_tab();
        // tabs[0] is a stale placeholder now that tab 1 is active; typing
        // in the still-active tab 1 should show up in its own label
        // immediately, without needing to switch away and back first.
        app.url = "http://x.io/typing".to_string();

        assert_eq!(app.tab_label(1), "* GET http://x.io/typing");
    }

    #[test]
    fn opening_a_new_tab_cancels_an_open_save_dialog() {
        let mut app = CurlyApp::default_state();
        app.open_save_dialog();
        assert!(app.save_dialog.is_some());

        app.open_new_tab();

        assert!(app.save_dialog.is_none());
    }

    #[test]
    fn switching_tabs_cancels_an_open_save_dialog() {
        let mut app = CurlyApp::default_state();
        app.open_new_tab();
        app.open_save_dialog();
        assert!(app.save_dialog.is_some());

        app.switch_to_tab(0);

        assert!(app.save_dialog.is_none());
    }
}
