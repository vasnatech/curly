//! Import a curl command string into a [`SavedRequest`] (FR-7 / M3). A
//! hand-rolled tokenizer + flag parser for the common subset of curl flags
//! (`-X`, `-H`, `-d`/`--data*`, `-u`, `-F`, `--url`) — matching DESIGN.md's
//! tech-choices table: full curl flag coverage is explicitly out of scope,
//! this covers what a command copy-pasted from docs, a script, or a
//! browser's "Copy as cURL" actually uses.

use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use reqwest::Method;

use crate::storage::{KvPair, SavedAuth, SavedBody, SavedMultipartField, SavedRequest};

/// Split a curl command into shell-ish words: single-quoted strings are
/// literal, double-quoted strings support `\"`/`\\`/`\$`/`` \` `` escapes,
/// unquoted text is split on whitespace with backslash-escaping a single
/// character. A practical subset of POSIX shell word-splitting — enough for
/// real curl commands, not a full shell grammar. Line continuations
/// (`\` at end of line, common when a multi-line curl command is pasted in)
/// are joined first.
fn tokenize(input: &str) -> Vec<String> {
    let joined = input.replace("\\\r\n", " ").replace("\\\n", " ");
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_token = false;
    let mut chars = joined.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' | '\n' | '\r' => {
                if in_token {
                    tokens.push(std::mem::take(&mut current));
                    in_token = false;
                }
            }
            '\'' => {
                in_token = true;
                for c2 in chars.by_ref() {
                    if c2 == '\'' {
                        break;
                    }
                    current.push(c2);
                }
            }
            '"' => {
                in_token = true;
                while let Some(c2) = chars.next() {
                    if c2 == '"' {
                        break;
                    }
                    if c2 == '\\' {
                        match chars.peek() {
                            Some('"') | Some('\\') | Some('$') | Some('`') => {
                                current.push(chars.next().unwrap());
                            }
                            _ => current.push(c2),
                        }
                    } else {
                        current.push(c2);
                    }
                }
            }
            '\\' => {
                in_token = true;
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            _ => {
                in_token = true;
                current.push(c);
            }
        }
    }
    if in_token {
        tokens.push(current);
    }
    tokens
}

/// `-d`/`--data`/`--data-raw`/`--data-binary` all accept an optional leading
/// `@` meaning "read from this file" (same convention as curly's own
/// `--data-binary` one-shot flag).
fn data_value_to_body(raw: &str) -> SavedBody {
    match raw.strip_prefix('@') {
        Some(path) => SavedBody::Binary {
            path: PathBuf::from(path),
        },
        None => SavedBody::Raw {
            content: raw.to_string(),
        },
    }
}

fn parse_form_field(raw: &str) -> Result<SavedMultipartField> {
    let (name, value) = raw
        .split_once('=')
        .ok_or_else(|| anyhow!("invalid -F/--form value (expected \"name=value\"): {raw}"))?;
    Ok(match value.strip_prefix('@') {
        Some(path) => SavedMultipartField::File {
            name: name.to_string(),
            path: PathBuf::from(path),
        },
        None => SavedMultipartField::Text {
            name: name.to_string(),
            value: value.to_string(),
        },
    })
}

/// Flags recognized with no argument (so a following bare token isn't
/// mistaken for their value or for the URL).
fn is_known_flag_argument(flag: &str) -> bool {
    matches!(
        flag,
        "-s" | "--silent"
            | "-v"
            | "--verbose"
            | "-i"
            | "--include"
            | "-k"
            | "--insecure"
            | "-L"
            | "--location"
            | "-G"
            | "--get"
            | "--compressed"
    )
}

/// Import a curl command string as a [`SavedRequest`] named `name`.
pub fn import_curl(name: &str, command: &str) -> Result<SavedRequest> {
    let mut tokens = tokenize(command);
    if tokens.first().map(String::as_str) == Some("curl") {
        tokens.remove(0);
    }

    let mut method: Option<String> = None;
    let mut headers: Vec<(String, String)> = Vec::new();
    let mut data: Option<String> = None;
    let mut form: Vec<String> = Vec::new();
    let mut user: Option<String> = None;
    let mut explicit_url: Option<String> = None;
    let mut url_candidates: Vec<String> = Vec::new();

    let mut iter = tokens.into_iter().peekable();
    while let Some(tok) = iter.next() {
        match tok.as_str() {
            "-X" | "--request" => method = iter.next(),
            "-H" | "--header" => {
                if let Some(raw) = iter.next() {
                    let (k, v) = raw
                        .split_once(':')
                        .ok_or_else(|| anyhow!("invalid -H/--header value (expected \"Name: value\"): {raw}"))?;
                    headers.push((k.trim().to_string(), v.trim().to_string()));
                }
            }
            "-d" | "--data" | "--data-raw" | "--data-binary" | "--data-ascii" => {
                data = iter.next();
            }
            "-F" | "--form" => {
                if let Some(v) = iter.next() {
                    form.push(v);
                }
            }
            "-u" | "--user" => user = iter.next(),
            "--url" => explicit_url = iter.next(),
            flag if is_known_flag_argument(flag) => {}
            flag if flag.starts_with('-') => {
                // Unrecognized flag: best-effort skip. If the next token
                // looks like another flag (or is absent), assume this flag
                // took no argument; otherwise skip its value too, so it
                // doesn't get mistaken for the URL.
                if let Some(next) = iter.peek() {
                    if !next.starts_with('-') {
                        iter.next();
                    }
                }
            }
            bare => url_candidates.push(bare.to_string()),
        }
    }

    let url = explicit_url
        .or_else(|| {
            url_candidates
                .iter()
                .find(|c| c.starts_with("http://") || c.starts_with("https://") || c.starts_with("{{"))
                .cloned()
        })
        .or_else(|| url_candidates.last().cloned())
        .ok_or_else(|| anyhow!("no URL found in curl command"))?;

    let sources_given = [data.is_some(), !form.is_empty()]
        .iter()
        .filter(|g| **g)
        .count();
    if sources_given > 1 {
        bail_multiple_bodies()?;
    }

    let body = if let Some(raw) = &data {
        Some(data_value_to_body(raw))
    } else if !form.is_empty() {
        let fields = form
            .iter()
            .map(|raw| parse_form_field(raw))
            .collect::<Result<Vec<_>>>()?;
        Some(SavedBody::Multipart { fields })
    } else {
        None
    };

    let method = match method {
        Some(m) => Method::from_bytes(m.to_uppercase().as_bytes())
            .with_context(|| format!("invalid HTTP method in curl command: {m}"))?,
        None if body.is_some() => Method::POST,
        None => Method::GET,
    };

    let mut saved = SavedRequest::new(name, method, url);
    for (k, v) in headers {
        saved.headers.push(KvPair::new(k, v));
    }
    saved.body = body;

    if let Some(user_pass) = user {
        let (username, password) = user_pass
            .split_once(':')
            .ok_or_else(|| anyhow!("invalid -u/--user value (expected \"user:password\"): {user_pass}"))?;
        saved.auth = Some(SavedAuth::Basic {
            username: username.to_string(),
            password: password.to_string(),
        });
    }

    Ok(saved)
}

fn bail_multiple_bodies() -> Result<()> {
    Err(anyhow!("only one of -d/--data* or -F/--form may be used per curl command"))
}
