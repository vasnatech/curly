//! Post-response variable extraction (FR-10's declarative shape, not a
//! scripting language): pull a value out of a response body (JSON path) or
//! header, or compute one from a template referencing already-known
//! variables, and hand it back for the caller to persist (as session state —
//! see `curly-cli`'s `commands::session`, not this crate, which stays
//! network/CLI-agnostic).
//!
//! Deliberately declarative rather than embedding a scripting language
//! (Rhai etc., which DESIGN.md's tech-choices table left TBD for FR-10):
//! "pull this path out of the body" / "this header" / "this template" covers
//! the actual use case (chaining an auth token into the next request)
//! without the complexity, security surface, or version-compat burden of an
//! embedded interpreter.

use std::collections::BTreeMap;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::exec::ResponseSummary;
use crate::substitution;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum Extraction {
    /// Pull a value out of the JSON response body at `path` (dot/bracket
    /// syntax, e.g. `"data.user.id"` or `"items[0].token"`).
    Body {
        name: String,
        path: String,
        #[serde(default)]
        secret: bool,
    },
    /// Pull a response header's value by name (case-insensitive).
    Header {
        name: String,
        header: String,
        #[serde(default)]
        secret: bool,
    },
    /// Compute a value by substituting `{{variable}}` tokens in `template`
    /// against already-known variables (the merged scope this request ran
    /// with, plus anything extracted earlier in this same rule list).
    Template {
        name: String,
        template: String,
        #[serde(default)]
        secret: bool,
    },
}

impl Extraction {
    pub fn name(&self) -> &str {
        match self {
            Extraction::Body { name, .. }
            | Extraction::Header { name, .. }
            | Extraction::Template { name, .. } => name,
        }
    }

    pub fn set_secret(&mut self, secret: bool) {
        match self {
            Extraction::Body { secret: s, .. }
            | Extraction::Header { secret: s, .. }
            | Extraction::Template { secret: s, .. } => *s = secret,
        }
    }

    fn is_secret(&self) -> bool {
        match self {
            Extraction::Body { secret, .. }
            | Extraction::Header { secret, .. }
            | Extraction::Template { secret, .. } => *secret,
        }
    }
}

/// One successfully extracted variable: name, value, and whether it should
/// be masked when displayed (`curly session show`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extracted {
    pub name: String,
    pub value: String,
    pub secret: bool,
}

/// Run `rules` in order against `response`, returning every extracted
/// variable. Each rule can reference variables extracted by an earlier rule
/// in the same list (in addition to `known_vars`, the scope the request
/// itself ran with). Errors on the first rule that can't be satisfied
/// (missing path/header, non-JSON body for a `Body` rule) — callers are
/// expected to only call this for a 2xx response, where a failing rule
/// indicates a real problem rather than an expected absence.
pub fn apply(
    rules: &[Extraction],
    response: &ResponseSummary,
    known_vars: &BTreeMap<String, String>,
) -> Result<Vec<Extracted>> {
    if rules.is_empty() {
        return Ok(Vec::new());
    }

    let body_json: Option<Value> = serde_json::from_str(&response.body).ok();
    let mut scope = known_vars.clone();
    let mut results = Vec::with_capacity(rules.len());

    for rule in rules {
        let value = match rule {
            Extraction::Body { name, path, .. } => {
                let json = body_json.as_ref().ok_or_else(|| {
                    anyhow!("response body is not JSON, cannot extract \"{name}\" via path \"{path}\"")
                })?;
                let found = get_json_path(json, path)
                    .ok_or_else(|| anyhow!("path \"{path}\" not found in response body (extracting \"{name}\")"))?;
                json_value_to_string(found)
            }
            Extraction::Header { name, header, .. } => response
                .headers
                .iter()
                .find(|(h, _)| h.eq_ignore_ascii_case(header))
                .map(|(_, v)| v.clone())
                .ok_or_else(|| anyhow!("header \"{header}\" not found in response (extracting \"{name}\")"))?,
            Extraction::Template { template, .. } => substitution::substitute(template, &scope)?,
        };

        scope.insert(rule.name().to_string(), value.clone());
        results.push(Extracted {
            name: rule.name().to_string(),
            value,
            secret: rule.is_secret(),
        });
    }

    Ok(results)
}

fn json_value_to_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

enum PathSegment<'a> {
    Key(&'a str),
    Index(usize),
}

/// Parse `"data.items[0].id"` into `[Key("data"), Key("items"), Index(0), Key("id")]`.
/// A leading `[N]` with no preceding key (`"[0].id"`) is also valid.
fn parse_json_path(path: &str) -> Vec<PathSegment<'_>> {
    let mut segments = Vec::new();
    for part in path.split('.') {
        let mut rest = part;
        match rest.find('[') {
            None => {
                if !rest.is_empty() {
                    segments.push(PathSegment::Key(rest));
                }
            }
            Some(bracket_pos) => {
                let key = &rest[..bracket_pos];
                if !key.is_empty() {
                    segments.push(PathSegment::Key(key));
                }
                rest = &rest[bracket_pos..];
                while let Some(after_open) = rest.strip_prefix('[') {
                    let Some(close) = after_open.find(']') else {
                        break;
                    };
                    if let Ok(idx) = after_open[..close].parse::<usize>() {
                        segments.push(PathSegment::Index(idx));
                    }
                    rest = &after_open[close + 1..];
                }
            }
        }
    }
    segments
}

/// Evaluate a dot/bracket JSON path (see [`parse_json_path`]) against `root`.
/// `None` if any segment doesn't resolve (missing key, out-of-range index,
/// or indexing into a non-object/non-array).
pub fn get_json_path<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = root;
    for segment in parse_json_path(path) {
        current = match segment {
            PathSegment::Key(key) => current.get(key)?,
            PathSegment::Index(idx) => current.get(idx)?,
        };
    }
    Some(current)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn response(body: &str, headers: &[(&str, &str)]) -> ResponseSummary {
        ResponseSummary {
            status: 200,
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            body: body.to_string(),
            elapsed: Duration::from_millis(1),
        }
    }

    #[test]
    fn json_path_top_level_string() {
        let v: Value = serde_json::from_str(r#"{"token":"abc"}"#).unwrap();
        assert_eq!(get_json_path(&v, "token").unwrap(), "abc");
    }

    #[test]
    fn json_path_nested_object() {
        let v: Value = serde_json::from_str(r#"{"data":{"user":{"id":42}}}"#).unwrap();
        assert_eq!(get_json_path(&v, "data.user.id").unwrap(), 42);
    }

    #[test]
    fn json_path_array_index() {
        let v: Value = serde_json::from_str(r#"{"items":[{"id":"a"},{"id":"b"}]}"#).unwrap();
        assert_eq!(get_json_path(&v, "items[1].id").unwrap(), "b");
    }

    #[test]
    fn json_path_leading_index() {
        let v: Value = serde_json::from_str(r#"[{"id":"a"}]"#).unwrap();
        assert_eq!(get_json_path(&v, "[0].id").unwrap(), "a");
    }

    #[test]
    fn json_path_missing_key_is_none() {
        let v: Value = serde_json::from_str(r#"{"token":"abc"}"#).unwrap();
        assert!(get_json_path(&v, "nope").is_none());
    }

    #[test]
    fn json_path_out_of_range_index_is_none() {
        let v: Value = serde_json::from_str(r#"{"items":[]}"#).unwrap();
        assert!(get_json_path(&v, "items[0]").is_none());
    }

    #[test]
    fn apply_extracts_from_body() {
        let rules = vec![Extraction::Body {
            name: "TOKEN".to_string(),
            path: "token".to_string(),
            secret: true,
        }];
        let resp = response(r#"{"token":"tok123"}"#, &[]);
        let extracted = apply(&rules, &resp, &BTreeMap::new()).unwrap();
        assert_eq!(
            extracted,
            vec![Extracted {
                name: "TOKEN".to_string(),
                value: "tok123".to_string(),
                secret: true,
            }]
        );
    }

    #[test]
    fn apply_extracts_from_header() {
        let rules = vec![Extraction::Header {
            name: "REQ_ID".to_string(),
            header: "x-request-id".to_string(),
            secret: false,
        }];
        let resp = response("{}", &[("X-Request-Id", "req-42")]);
        let extracted = apply(&rules, &resp, &BTreeMap::new()).unwrap();
        assert_eq!(extracted[0].value, "req-42");
    }

    #[test]
    fn apply_template_references_earlier_extraction_and_known_vars() {
        let rules = vec![
            Extraction::Body {
                name: "TOKEN".to_string(),
                path: "token".to_string(),
                secret: false,
            },
            Extraction::Template {
                name: "AUTH_HEADER".to_string(),
                template: "Bearer {{TOKEN}} for {{HOST}}".to_string(),
                secret: true,
            },
        ];
        let resp = response(r#"{"token":"abc"}"#, &[]);
        let mut known = BTreeMap::new();
        known.insert("HOST".to_string(), "example.com".to_string());

        let extracted = apply(&rules, &resp, &known).unwrap();
        assert_eq!(extracted[1].name, "AUTH_HEADER");
        assert_eq!(extracted[1].value, "Bearer abc for example.com");
        assert!(extracted[1].secret);
    }

    #[test]
    fn apply_missing_body_path_errors() {
        let rules = vec![Extraction::Body {
            name: "TOKEN".to_string(),
            path: "nope".to_string(),
            secret: false,
        }];
        let resp = response(r#"{"token":"abc"}"#, &[]);
        assert!(apply(&rules, &resp, &BTreeMap::new()).is_err());
    }

    #[test]
    fn apply_non_json_body_errors_for_body_rule() {
        let rules = vec![Extraction::Body {
            name: "TOKEN".to_string(),
            path: "token".to_string(),
            secret: false,
        }];
        let resp = response("not json", &[]);
        assert!(apply(&rules, &resp, &BTreeMap::new()).is_err());
    }

    #[test]
    fn apply_missing_header_errors() {
        let rules = vec![Extraction::Header {
            name: "X".to_string(),
            header: "Nope".to_string(),
            secret: false,
        }];
        let resp = response("{}", &[]);
        assert!(apply(&rules, &resp, &BTreeMap::new()).is_err());
    }

    #[test]
    fn apply_empty_rules_is_empty_ok() {
        let resp = response("not even json", &[]);
        assert_eq!(apply(&[], &resp, &BTreeMap::new()).unwrap(), Vec::new());
    }

    #[test]
    fn apply_non_string_json_value_uses_json_text() {
        let rules = vec![Extraction::Body {
            name: "COUNT".to_string(),
            path: "count".to_string(),
            secret: false,
        }];
        let resp = response(r#"{"count":42}"#, &[]);
        let extracted = apply(&rules, &resp, &BTreeMap::new()).unwrap();
        assert_eq!(extracted[0].value, "42");
    }
}
