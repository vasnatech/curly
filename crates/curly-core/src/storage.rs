//! Local, file-based persistence for collections, environments, and history.
//!
//! Layout under the storage root (see [`Storage::default_location`]):
//!
//! ```text
//! curly/
//! ├── collections/<slug>.json   # one file per collection, requests embedded inline
//! ├── environments/<slug>.json  # one file per environment
//! └── history/<yyyy-mm-dd>.jsonl
//! ```
//!
//! This deviates from DESIGN.md's original sketch (`collections/<slug>/collection.json`
//! + a `requests/<id>.json` per saved request) by keeping each collection as a
//! single JSON file with its requests embedded. That's simpler to implement
//! correctly, still git-friendly (a whole collection diffs as one file, the
//! way a Postman collection export already does), and still satisfies NFR-5
//! (CLI and GUI read/write the same files) since nothing here is CLI-specific.
//! `collection.json`'s `requests/` subdirectory and `Folder` nesting are not
//! implemented in M2 — collections are a flat, ordered list of requests for now.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::exec::ResponseSummary;
use crate::model::{Auth, Body, MultipartField, Request};

/// A name/value pair that can be individually disabled without deleting it —
/// mirrors how Postman lets you toggle a header/param off. `enabled` defaults
/// to `true` so hand-written JSON doesn't need to specify it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KvPair {
    pub name: String,
    pub value: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

impl KvPair {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
            enabled: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SavedBody {
    Raw { content: String },
    Binary { path: PathBuf },
    Form { fields: Vec<(String, String)> },
    Multipart { fields: Vec<SavedMultipartField> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SavedMultipartField {
    Text { name: String, value: String },
    File { name: String, path: PathBuf },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SavedAuth {
    Basic { username: String, password: String },
    Bearer { token: String },
}

/// A saved, reusable request template. May contain `{{variable}}` tokens in
/// `url`, header/query values, or a `Raw`/`Form` body — resolved against an
/// [`Environment`] at run time by `substitution::resolve`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedRequest {
    pub id: Uuid,
    pub name: String,
    /// Stored as a string (not `reqwest::Method`, which isn't (de)serializable)
    /// and validated on load.
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub query_params: Vec<KvPair>,
    #[serde(default)]
    pub headers: Vec<KvPair>,
    #[serde(default)]
    pub body: Option<SavedBody>,
    #[serde(default)]
    pub auth: Option<SavedAuth>,
}

impl SavedRequest {
    pub fn new(name: impl Into<String>, method: Method, url: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            method: method.to_string(),
            url: url.into(),
            query_params: Vec::new(),
            headers: Vec::new(),
            body: None,
            auth: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Collection {
    #[serde(default = "Uuid::new_v4")]
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub requests: Vec<SavedRequest>,
}

impl Collection {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            requests: Vec::new(),
        }
    }

    pub fn find_request(&self, name: &str) -> Option<&SavedRequest> {
        self.requests.iter().find(|r| r.name == name)
    }

    pub fn find_request_mut(&mut self, name: &str) -> Option<&mut SavedRequest> {
        self.requests.iter_mut().find(|r| r.name == name)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Variable {
    pub key: String,
    pub value: String,
    #[serde(default)]
    pub secret: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Environment {
    pub name: String,
    #[serde(default)]
    pub variables: Vec<Variable>,
}

impl Environment {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            variables: Vec::new(),
        }
    }

    pub fn set(&mut self, key: impl Into<String>, value: impl Into<String>, secret: bool) {
        let key = key.into();
        if let Some(existing) = self.variables.iter_mut().find(|v| v.key == key) {
            existing.value = value.into();
            existing.secret = secret;
        } else {
            self.variables.push(Variable {
                key,
                value: value.into(),
                secret,
            });
        }
    }

    pub fn unset(&mut self, key: &str) -> bool {
        let before = self.variables.len();
        self.variables.retain(|v| v.key != key);
        self.variables.len() != before
    }
}

/// One completed request/response, recorded for `curly history`. Header
/// values that look like credentials are redacted (NFR-4) and the body is
/// capped at [`MAX_HISTORY_BODY_BYTES`] to keep history files small.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub sent_at: DateTime<Utc>,
    pub method: String,
    pub url: String,
    pub request_headers: Vec<(String, String)>,
    pub status: u16,
    pub elapsed_ms: u128,
    pub response_headers: Vec<(String, String)>,
    pub response_body: String,
    pub response_body_truncated: bool,
}

pub const MAX_HISTORY_BODY_BYTES: usize = 8192;
const REDACTED: &str = "***redacted***";

impl HistoryEntry {
    pub fn new(request: &Request, response: &ResponseSummary) -> Self {
        let request_headers = request
            .headers
            .iter()
            .map(|(name, value)| {
                let redacted = name.eq_ignore_ascii_case("authorization")
                    || name.eq_ignore_ascii_case("x-api-key")
                    || name.eq_ignore_ascii_case("cookie");
                (
                    name.clone(),
                    if redacted {
                        REDACTED.to_string()
                    } else {
                        value.clone()
                    },
                )
            })
            .collect();

        let truncated = response.body.len() > MAX_HISTORY_BODY_BYTES;
        let response_body = if truncated {
            response
                .body
                .char_indices()
                .take_while(|(byte_idx, _)| *byte_idx < MAX_HISTORY_BODY_BYTES)
                .map(|(_, c)| c)
                .collect()
        } else {
            response.body.clone()
        };

        Self {
            sent_at: Utc::now(),
            method: request.method.to_string(),
            url: request.url.clone(),
            request_headers,
            status: response.status,
            elapsed_ms: response.elapsed.as_millis(),
            response_headers: response.headers.clone(),
            response_body,
            response_body_truncated: truncated,
        }
    }
}

/// Turn a display name into a filesystem-safe slug: lowercase, `[a-z0-9-_]`
/// only, runs of anything else collapsed to a single `-`.
pub fn slugify(name: &str) -> String {
    let mut slug = String::with_capacity(name.len());
    let mut last_was_dash = false;
    for c in name.trim().chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
            last_was_dash = false;
        } else if c == '-' || c == '_' {
            slug.push(c);
            last_was_dash = false;
        } else if !last_was_dash {
            slug.push('-');
            last_was_dash = true;
        }
    }
    let trimmed = slug.trim_matches('-');
    if trimmed.is_empty() {
        "untitled".to_string()
    } else {
        trimmed.to_string()
    }
}

pub struct Storage {
    root: PathBuf,
}

impl Storage {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The OS-appropriate default data directory: `~/.local/share/curly` on
    /// Linux, `~/Library/Application Support/curly` on macOS, `%APPDATA%\curly`
    /// on Windows.
    pub fn default_location() -> Result<Self> {
        let base = dirs::data_dir()
            .ok_or_else(|| anyhow!("could not determine the user data directory for this OS"))?;
        Ok(Self::new(base.join("curly")))
    }

    fn collections_dir(&self) -> PathBuf {
        self.root.join("collections")
    }

    fn environments_dir(&self) -> PathBuf {
        self.root.join("environments")
    }

    fn history_dir(&self) -> PathBuf {
        self.root.join("history")
    }

    fn collection_path(&self, slug: &str) -> PathBuf {
        self.collections_dir().join(format!("{slug}.json"))
    }

    fn environment_path(&self, slug: &str) -> PathBuf {
        self.environments_dir().join(format!("{slug}.json"))
    }

    pub fn list_collections(&self) -> Result<Vec<String>> {
        list_json_names(&self.collections_dir())
    }

    pub fn load_collection(&self, name: &str) -> Result<Collection> {
        let path = self.collection_path(&slugify(name));
        let bytes = std::fs::read(&path)
            .with_context(|| format!("no collection named \"{name}\" (looked in {})", path.display()))?;
        serde_json::from_slice(&bytes)
            .with_context(|| format!("failed to parse collection file: {}", path.display()))
    }

    pub fn save_collection(&self, collection: &Collection) -> Result<()> {
        let dir = self.collections_dir();
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("failed to create {}", dir.display()))?;
        let path = self.collection_path(&slugify(&collection.name));
        let json = serde_json::to_string_pretty(collection)?;
        std::fs::write(&path, json)
            .with_context(|| format!("failed to write {}", path.display()))
    }

    /// Load a collection if it exists, or `None` if it doesn't — used by
    /// `collections add-request` to create the collection on first use.
    pub fn load_collection_opt(&self, name: &str) -> Result<Option<Collection>> {
        let path = self.collection_path(&slugify(name));
        if !path.exists() {
            return Ok(None);
        }
        self.load_collection(name).map(Some)
    }

    pub fn delete_collection(&self, name: &str) -> Result<()> {
        let path = self.collection_path(&slugify(name));
        std::fs::remove_file(&path)
            .with_context(|| format!("no collection named \"{name}\" (looked in {})", path.display()))
    }

    pub fn list_environments(&self) -> Result<Vec<String>> {
        list_json_names(&self.environments_dir())
    }

    pub fn load_environment(&self, name: &str) -> Result<Environment> {
        let path = self.environment_path(&slugify(name));
        let bytes = std::fs::read(&path).with_context(|| {
            format!("no environment named \"{name}\" (looked in {})", path.display())
        })?;
        serde_json::from_slice(&bytes)
            .with_context(|| format!("failed to parse environment file: {}", path.display()))
    }

    /// Load an environment if it exists, or `None` if it doesn't — used for
    /// the always-merged-in "global" environment, where absence isn't an error.
    pub fn load_environment_opt(&self, name: &str) -> Result<Option<Environment>> {
        let path = self.environment_path(&slugify(name));
        if !path.exists() {
            return Ok(None);
        }
        self.load_environment(name).map(Some)
    }

    pub fn save_environment(&self, env: &Environment) -> Result<()> {
        let dir = self.environments_dir();
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("failed to create {}", dir.display()))?;
        let path = self.environment_path(&slugify(&env.name));
        let json = serde_json::to_string_pretty(env)?;
        std::fs::write(&path, json)
            .with_context(|| format!("failed to write {}", path.display()))
    }

    pub fn delete_environment(&self, name: &str) -> Result<()> {
        let path = self.environment_path(&slugify(name));
        std::fs::remove_file(&path).with_context(|| {
            format!("no environment named \"{name}\" (looked in {})", path.display())
        })
    }

    pub fn append_history(&self, entry: &HistoryEntry) -> Result<()> {
        use std::io::Write;

        let dir = self.history_dir();
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("failed to create {}", dir.display()))?;
        let path = dir.join(format!("{}.jsonl", entry.sent_at.format("%Y-%m-%d")));
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("failed to open {}", path.display()))?;
        let line = serde_json::to_string(entry)?;
        writeln!(file, "{line}").with_context(|| format!("failed to write {}", path.display()))
    }

    /// Most recent entries first, across all history files, capped at `limit`.
    pub fn list_history(&self, limit: usize) -> Result<Vec<HistoryEntry>> {
        let dir = self.history_dir();
        if !dir.exists() {
            return Ok(Vec::new());
        }

        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
            .with_context(|| format!("failed to read {}", dir.display()))?
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("jsonl"))
            .collect();
        files.sort();
        files.reverse(); // most recent date file first

        let mut entries = Vec::new();
        for path in files {
            let content = std::fs::read_to_string(&path)
                .with_context(|| format!("failed to read {}", path.display()))?;
            let mut day_entries: Vec<HistoryEntry> = content
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(serde_json::from_str)
                .collect::<Result<_, _>>()
                .with_context(|| format!("failed to parse {}", path.display()))?;
            day_entries.reverse(); // most recent within the day first
            entries.extend(day_entries);
            if entries.len() >= limit {
                break;
            }
        }
        entries.truncate(limit);
        Ok(entries)
    }
}

fn list_json_names(dir: &Path) -> Result<Vec<String>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .with_context(|| format!("failed to read {}", dir.display()))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("json"))
        .filter_map(|path| path.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .collect();
    names.sort();
    Ok(names)
}

/// Build an executable [`Request`] from a [`SavedRequest`] whose variables
/// have already been substituted (see `substitution::resolve`). Only
/// validates/converts (method string -> `Method`, saved body/auth -> the
/// execution-time enums) — no more `{{var}}` scanning happens here.
pub fn into_request(saved: &SavedRequest) -> Result<Request> {
    let method = Method::from_bytes(saved.method.to_uppercase().as_bytes())
        .with_context(|| format!("invalid HTTP method in saved request: {}", saved.method))?;

    let mut request = Request::new(method, &saved.url);

    for header in saved.headers.iter().filter(|h| h.enabled) {
        request = request.with_header(&header.name, &header.value);
    }
    for param in saved.query_params.iter().filter(|p| p.enabled) {
        request = request.with_query(&param.name, &param.value);
    }

    if let Some(body) = &saved.body {
        let body = match body {
            SavedBody::Raw { content } => Body::Raw(content.clone()),
            SavedBody::Binary { path } => {
                let bytes = std::fs::read(path)
                    .with_context(|| format!("failed to read body file: {}", path.display()))?;
                Body::Binary(bytes)
            }
            SavedBody::Form { fields } => Body::Form(fields.clone()),
            SavedBody::Multipart { fields } => Body::Multipart(
                fields
                    .iter()
                    .map(|f| match f {
                        SavedMultipartField::Text { name, value } => MultipartField::Text {
                            name: name.clone(),
                            value: value.clone(),
                        },
                        SavedMultipartField::File { name, path } => MultipartField::File {
                            name: name.clone(),
                            path: path.clone(),
                        },
                    })
                    .collect(),
            ),
        };
        request = request.with_body(body);
    }

    if let Some(auth) = &saved.auth {
        let auth = match auth {
            SavedAuth::Basic { username, password } => Auth::Basic {
                username: username.clone(),
                password: password.clone(),
            },
            SavedAuth::Bearer { token } => Auth::Bearer {
                token: token.clone(),
            },
        };
        request = request.with_auth(auth);
    }

    Ok(request)
}

/// Parse `"collection/request"` into its two halves, as used by `curly run`.
pub fn parse_run_target(target: &str) -> Result<(&str, &str)> {
    target
        .split_once('/')
        .filter(|(c, r)| !c.is_empty() && !r.is_empty())
        .ok_or_else(|| {
            anyhow!("invalid run target (expected \"collection/request-name\"): {target}")
        })
}
