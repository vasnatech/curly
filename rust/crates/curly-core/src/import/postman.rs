//! Import from Postman's export formats (FR-7 / M3). Postman has three
//! distinct file shapes relevant here (a fourth, the full workspace/data-dump
//! export, isn't supported — it's rarely what someone hands you when they
//! say "here's my Postman collection"):
//!
//! 1. **Collection Format v2.1** — `import_collection`.
//! 2. **Environment** export — `import_environment`.
//! 3. **Globals** export — same shape as an environment, distinguished by
//!    `_postman_variable_scope: "globals"`; also handled by `import_environment`,
//!    which maps it onto curly's own `global` environment (a natural fit —
//!    curly already treats "global" as the always-merged-in base scope).
//!
//! Postman's `{{variable}}` syntax is identical to curly's, so URL/header/
//! body strings import verbatim — no token translation needed.
//!
//! Known, deliberate gaps (documented rather than silently mishandled):
//! - Pre-request/test scripts (`event`, JS-based) are dropped. Curly has no
//!   scripting (see substitution.rs's module doc / extraction.rs for why);
//!   there's no reliable way to translate arbitrary JS into curly's
//!   declarative `extract` rules.
//! - Auth is read from each request directly; collection/folder-level
//!   inherited auth is not resolved.
//! - Only `bearer` and `basic` auth types map to curly's `SavedAuth`
//!   (matching curly's own auth support); other types (apikey, oauth2,
//!   digest, ...) are dropped.
//! - `graphql`-mode bodies are dropped (curly has no GraphQL concept).
//! - `formdata`/`file` entries carry a `src` path from the machine that
//!   exported the collection — it's almost certainly wrong on a different
//!   machine, a limitation of Postman's own export format, not something an
//!   importer can fix.
//! - Disabled environment variables are dropped rather than imported disabled
//!   (curly's `Variable` has no enabled/disabled concept).

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::Deserialize;
use uuid::Uuid;

use crate::storage::{
    Collection, Environment, Folder, KvPair, SavedAuth, SavedBody, SavedMultipartField,
    SavedRequest,
};
use reqwest::Method;

#[derive(Debug, Deserialize)]
struct PmCollection {
    info: PmInfo,
    #[serde(default)]
    item: Vec<PmItem>,
}

#[derive(Debug, Deserialize)]
struct PmInfo {
    name: String,
}

#[derive(Debug, Deserialize)]
struct PmItem {
    name: String,
    /// Present (even if empty) on a folder item; absent on a request item.
    #[serde(default)]
    item: Option<Vec<PmItem>>,
    #[serde(default)]
    request: Option<PmRequest>,
}

#[derive(Debug, Deserialize)]
struct PmRequest {
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    header: Vec<PmHeader>,
    #[serde(default)]
    body: Option<PmBody>,
    #[serde(default)]
    auth: Option<PmAuth>,
    url: PmUrl,
}

#[derive(Debug, Deserialize)]
struct PmHeader {
    key: String,
    #[serde(default)]
    value: String,
    #[serde(default)]
    disabled: bool,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum PmUrl {
    Raw(String),
    Detailed { raw: String },
}

#[derive(Debug, Deserialize)]
struct PmBody {
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    raw: Option<String>,
    #[serde(default)]
    urlencoded: Vec<PmFormParam>,
    #[serde(default)]
    formdata: Vec<PmFormDataParam>,
    #[serde(default)]
    file: Option<PmFileBody>,
}

#[derive(Debug, Deserialize)]
struct PmFileBody {
    #[serde(default)]
    src: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PmFormParam {
    key: String,
    #[serde(default)]
    value: String,
    #[serde(default)]
    disabled: bool,
}

#[derive(Debug, Deserialize)]
struct PmFormDataParam {
    key: String,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    src: Option<String>,
    #[serde(rename = "type", default)]
    kind: Option<String>,
    #[serde(default)]
    disabled: bool,
}

#[derive(Debug, Deserialize)]
struct PmAuth {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    bearer: Vec<PmAuthParam>,
    #[serde(default)]
    basic: Vec<PmAuthParam>,
}

#[derive(Debug, Deserialize)]
struct PmAuthParam {
    key: String,
    value: String,
}

/// Import a Postman Collection Format v2.1 export into a curly [`Collection`].
pub fn import_collection(json: &str) -> Result<Collection> {
    let parsed: PmCollection =
        serde_json::from_str(json).context("failed to parse Postman collection JSON")?;
    let (folders, requests) = map_items(parsed.item)?;
    Ok(Collection {
        id: Uuid::new_v4(),
        name: parsed.info.name,
        folders,
        requests,
    })
}

fn map_items(items: Vec<PmItem>) -> Result<(Vec<Folder>, Vec<SavedRequest>)> {
    let mut folders = Vec::new();
    let mut requests = Vec::new();
    for item in items {
        match (item.item, item.request) {
            (Some(sub_items), _) => {
                let (sub_folders, sub_requests) = map_items(sub_items)?;
                folders.push(Folder {
                    id: Uuid::new_v4(),
                    name: item.name,
                    folders: sub_folders,
                    requests: sub_requests,
                });
            }
            (None, Some(request)) => {
                requests.push(map_request(&item.name, request)?);
            }
            (None, None) => {} // neither folder nor request — skip
        }
    }
    Ok((folders, requests))
}

fn map_request(name: &str, req: PmRequest) -> Result<SavedRequest> {
    let method_str = req.method.unwrap_or_else(|| "GET".to_string());
    let method = Method::from_bytes(method_str.to_uppercase().as_bytes())
        .with_context(|| format!("invalid HTTP method in Postman request \"{name}\": {method_str}"))?;
    let url = match req.url {
        PmUrl::Raw(s) => s,
        PmUrl::Detailed { raw } => raw,
    };

    let mut saved = SavedRequest::new(name, method, url);

    for h in req.header {
        if !h.disabled {
            saved.headers.push(KvPair::new(h.key, h.value));
        }
    }
    if let Some(body) = req.body {
        saved.body = map_body(body);
    }
    if let Some(auth) = req.auth {
        saved.auth = map_auth(auth);
    }

    Ok(saved)
}

fn map_body(body: PmBody) -> Option<SavedBody> {
    match body.mode.as_deref() {
        Some("raw") => body.raw.map(|content| SavedBody::Raw { content }),
        Some("urlencoded") => {
            let fields = body
                .urlencoded
                .into_iter()
                .filter(|p| !p.disabled)
                .map(|p| (p.key, p.value))
                .collect();
            Some(SavedBody::Form { fields })
        }
        Some("formdata") => {
            let fields = body
                .formdata
                .into_iter()
                .filter(|p| !p.disabled)
                .map(|p| {
                    if p.kind.as_deref() == Some("file") {
                        SavedMultipartField::File {
                            name: p.key,
                            path: PathBuf::from(p.src.unwrap_or_default()),
                        }
                    } else {
                        SavedMultipartField::Text {
                            name: p.key,
                            value: p.value.unwrap_or_default(),
                        }
                    }
                })
                .collect();
            Some(SavedBody::Multipart { fields })
        }
        Some("file") => body
            .file
            .and_then(|f| f.src)
            .map(|src| SavedBody::Binary { path: PathBuf::from(src) }),
        _ => None, // graphql or unrecognized mode — dropped, see module doc
    }
}

fn map_auth(auth: PmAuth) -> Option<SavedAuth> {
    match auth.kind.as_str() {
        "bearer" => auth
            .bearer
            .iter()
            .find(|p| p.key == "token")
            .map(|p| SavedAuth::Bearer {
                token: p.value.clone(),
            }),
        "basic" => {
            let username = auth.basic.iter().find(|p| p.key == "username");
            let password = auth.basic.iter().find(|p| p.key == "password");
            match (username, password) {
                (Some(u), Some(p)) => Some(SavedAuth::Basic {
                    username: u.value.clone(),
                    password: p.value.clone(),
                }),
                _ => None,
            }
        }
        _ => None, // apikey/oauth2/digest/etc. — dropped, see module doc
    }
}

#[derive(Debug, Deserialize)]
struct PmEnvironment {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    values: Vec<PmEnvValue>,
    #[serde(default, rename = "_postman_variable_scope")]
    scope: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PmEnvValue {
    key: String,
    #[serde(default)]
    value: String,
    #[serde(default, rename = "type")]
    kind: Option<String>,
    #[serde(default = "default_true")]
    enabled: bool,
}

fn default_true() -> bool {
    true
}

/// Import a Postman Environment *or* Globals export (same JSON shape,
/// distinguished by `_postman_variable_scope`) into a curly [`Environment`].
/// `name_override` takes precedence over the file's own name; absent both,
/// a globals export becomes curly's "global" environment (curly's own
/// always-merged-in base scope — a natural fit), and an environment export
/// keeps its own name.
pub fn import_environment(json: &str, name_override: Option<&str>) -> Result<Environment> {
    let parsed: PmEnvironment =
        serde_json::from_str(json).context("failed to parse Postman environment JSON")?;

    let name = name_override.map(str::to_string).unwrap_or_else(|| {
        if parsed.scope.as_deref() == Some("globals") {
            "global".to_string()
        } else {
            parsed.name.unwrap_or_else(|| "imported".to_string())
        }
    });

    let mut env = Environment::new(name);
    for v in parsed.values {
        if v.enabled {
            let secret = v.kind.as_deref() == Some("secret");
            env.set(v.key, v.value, secret);
        }
    }
    Ok(env)
}
