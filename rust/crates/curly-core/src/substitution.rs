//! `{{variable}}` substitution against a merged variable scope (FR-4).
//!
//! Unlike Postman/GUI-style tools, which silently leave an unresolved
//! `{{var}}` in place, `resolve` errors out on any undefined variable. For a
//! CLI/CI tool, silently sending a literal `{{var}}` to a server is almost
//! always a mistake worth failing loudly on rather than a valid request.

use std::collections::BTreeMap;

use anyhow::{bail, Result};

use crate::storage::{SavedAuth, SavedBody, SavedRequest};

/// Replace every `{{name}}` token in `input` with `variables[name]`.
/// Returns an error naming every undefined variable found (not just the first).
pub fn substitute(input: &str, variables: &BTreeMap<String, String>) -> Result<String> {
    let mut output = String::with_capacity(input.len());
    let mut undefined: Vec<String> = Vec::new();
    let mut rest = input;

    while let Some(start) = rest.find("{{") {
        output.push_str(&rest[..start]);
        let after_open = &rest[start + 2..];
        match after_open.find("}}") {
            Some(end) => {
                let name = after_open[..end].trim();
                match variables.get(name) {
                    Some(value) => output.push_str(value),
                    None => {
                        if !undefined.contains(&name.to_string()) {
                            undefined.push(name.to_string());
                        }
                    }
                }
                rest = &after_open[end + 2..];
            }
            None => {
                // Unterminated "{{" - treat the rest of the string as literal.
                output.push_str("{{");
                rest = after_open;
            }
        }
    }
    output.push_str(rest);

    if undefined.is_empty() {
        Ok(output)
    } else {
        bail!("undefined variable(s): {}", undefined.join(", "))
    }
}

/// Substitute `{{variable}}` tokens throughout a [`SavedRequest`] — URL,
/// enabled header/query values, and `Raw`/`Form` body content/auth fields —
/// producing a fully-resolved copy ready for [`crate::storage::into_request`].
/// `Multipart`/`Binary` bodies aren't substituted (file paths and uploaded
/// file contents aren't meaningful places for environment variables).
pub fn resolve(saved: &SavedRequest, variables: &BTreeMap<String, String>) -> Result<SavedRequest> {
    let mut resolved = saved.clone();

    resolved.url = substitute(&saved.url, variables)?;

    for header in &mut resolved.headers {
        header.value = substitute(&header.value, variables)?;
    }
    for param in &mut resolved.query_params {
        param.value = substitute(&param.value, variables)?;
    }

    resolved.body = match &saved.body {
        Some(SavedBody::Raw { content }) => Some(SavedBody::Raw {
            content: substitute(content, variables)?,
        }),
        Some(SavedBody::Form { fields }) => {
            let mut resolved_fields = Vec::with_capacity(fields.len());
            for (name, value) in fields {
                resolved_fields.push((name.clone(), substitute(value, variables)?));
            }
            Some(SavedBody::Form {
                fields: resolved_fields,
            })
        }
        other => other.clone(),
    };

    resolved.auth = match &saved.auth {
        Some(SavedAuth::Basic { username, password }) => Some(SavedAuth::Basic {
            username: substitute(username, variables)?,
            password: substitute(password, variables)?,
        }),
        Some(SavedAuth::Bearer { token }) => Some(SavedAuth::Bearer {
            token: substitute(token, variables)?,
        }),
        None => None,
    };

    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn replaces_a_single_variable() {
        let result = substitute("https://{{host}}/get", &vars(&[("host", "example.com")])).unwrap();
        assert_eq!(result, "https://example.com/get");
    }

    #[test]
    fn replaces_multiple_variables() {
        let result = substitute(
            "{{scheme}}://{{host}}/{{path}}",
            &vars(&[
                ("scheme", "https"),
                ("host", "example.com"),
                ("path", "get"),
            ]),
        )
        .unwrap();
        assert_eq!(result, "https://example.com/get");
    }

    #[test]
    fn trims_whitespace_inside_braces() {
        let result = substitute("{{ host }}", &vars(&[("host", "example.com")])).unwrap();
        assert_eq!(result, "example.com");
    }

    #[test]
    fn no_variables_is_a_no_op() {
        let result = substitute("https://example.com/get", &vars(&[])).unwrap();
        assert_eq!(result, "https://example.com/get");
    }

    #[test]
    fn undefined_variable_errors() {
        let err = substitute("https://{{host}}/get", &vars(&[])).unwrap_err();
        assert!(err.to_string().contains("host"));
    }

    #[test]
    fn reports_all_undefined_variables_not_just_first() {
        let err = substitute("{{a}}-{{b}}-{{a}}", &vars(&[])).unwrap_err();
        let message = err.to_string();
        assert!(message.contains('a'));
        assert!(message.contains('b'));
    }

    #[test]
    fn unterminated_braces_are_left_literal() {
        let result = substitute("no closing {{here", &vars(&[])).unwrap();
        assert_eq!(result, "no closing {{here");
    }

    #[test]
    fn resolve_substitutes_url_headers_query_and_body() {
        use crate::storage::KvPair;

        let mut saved = SavedRequest::new(
            "get-user",
            reqwest::Method::POST,
            "https://{{host}}/users/{{id}}",
        );
        saved.headers.push(KvPair::new("X-Env", "{{env_name}}"));
        saved.query_params.push(KvPair::new("id", "{{id}}"));
        saved.body = Some(SavedBody::Raw {
            content: r#"{"id":"{{id}}"}"#.to_string(),
        });

        let variables = vars(&[
            ("host", "api.example.com"),
            ("id", "42"),
            ("env_name", "dev"),
        ]);
        let resolved = resolve(&saved, &variables).unwrap();

        assert_eq!(resolved.url, "https://api.example.com/users/42");
        assert_eq!(resolved.headers[0].value, "dev");
        assert_eq!(resolved.query_params[0].value, "42");
        match resolved.body {
            Some(SavedBody::Raw { content }) => assert_eq!(content, r#"{"id":"42"}"#),
            other => panic!("expected Raw body, got {other:?}"),
        }
    }

    #[test]
    fn resolve_substitutes_auth_fields() {
        let mut saved = SavedRequest::new("whoami", reqwest::Method::GET, "https://example.com");
        saved.auth = Some(SavedAuth::Bearer {
            token: "{{token}}".to_string(),
        });

        let resolved = resolve(&saved, &vars(&[("token", "abc123")])).unwrap();
        match resolved.auth {
            Some(SavedAuth::Bearer { token }) => assert_eq!(token, "abc123"),
            other => panic!("expected Bearer auth, got {other:?}"),
        }
    }

    #[test]
    fn resolve_propagates_undefined_variable_error() {
        let saved = SavedRequest::new("get-user", reqwest::Method::GET, "https://{{host}}/get");
        assert!(resolve(&saved, &vars(&[])).is_err());
    }
}
