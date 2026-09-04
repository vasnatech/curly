//! `curly <url> [flags]` with no subcommand — curl-like one-shot mode (FR-11).

use std::fs;
use std::path::PathBuf;

use anyhow::{anyhow, bail, Context, Result};
use clap::Args;
use curly_core::model::{Auth, Body, MultipartField, Request};
use reqwest::Method;

use crate::args::{ConnectionArgs, OutputArgs};
use crate::execute;

#[derive(Args, Debug, Clone)]
pub struct OneShotArgs {
    /// URL to request (required unless a subcommand is given)
    pub url: Option<String>,

    /// HTTP method (defaults to GET, or POST if a body flag is given)
    #[arg(short = 'X', long = "request")]
    pub method: Option<String>,

    /// Add a request header, e.g. -H "Content-Type: application/json" (repeatable)
    #[arg(short = 'H', long = "header")]
    pub headers: Vec<String>,

    /// Add a query parameter, e.g. -Q "id=42" (repeatable)
    #[arg(short = 'Q', long = "query")]
    pub query: Vec<String>,

    /// Send this as the raw request body
    #[arg(short = 'd', long = "data")]
    pub data: Option<String>,

    /// Add an application/x-www-form-urlencoded field, e.g. --data-urlencode "name=a" (repeatable)
    #[arg(long = "data-urlencode")]
    pub data_urlencode: Vec<String>,

    /// Send the contents of a file as the raw request body (accepts an optional leading '@')
    #[arg(long = "data-binary")]
    pub data_binary: Option<PathBuf>,

    /// Add a multipart/form-data field: "name=value" for text, "name=@path" to upload a file (repeatable)
    #[arg(short = 'F', long = "form")]
    pub form: Vec<String>,

    /// Basic auth credentials as "user:password"
    #[arg(short = 'u', long = "user")]
    pub user: Option<String>,

    /// Send a Bearer token in the Authorization header
    #[arg(long = "bearer")]
    pub bearer: Option<String>,

    #[command(flatten)]
    pub connection: ConnectionArgs,

    #[command(flatten)]
    pub output: OutputArgs,
}

/// Split "key=value" into its two halves. Used for -Q, --data-urlencode, and text -F fields.
pub fn parse_kv(raw: &str, flag: &str) -> Result<(String, String)> {
    let (k, v) = raw
        .split_once('=')
        .ok_or_else(|| anyhow!("invalid {flag} value (expected \"key=value\"): {raw}"))?;
    Ok((k.to_string(), v.to_string()))
}

/// Split "Name: value" into its two halves. Used for -H.
pub fn parse_header(raw: &str) -> Result<(String, String)> {
    let (name, value) = raw
        .split_once(':')
        .ok_or_else(|| anyhow!("invalid header (expected \"Name: value\"): {raw}"))?;
    Ok((name.trim().to_string(), value.trim().to_string()))
}

/// Parse a -F field into a MultipartField: "name=@path" uploads a file, "name=value" is text.
pub fn parse_form_field(raw: &str) -> Result<MultipartField> {
    let (name, value) = parse_kv(raw, "-F/--form")?;
    if let Some(path) = value.strip_prefix('@') {
        Ok(MultipartField::File {
            name,
            path: PathBuf::from(path),
        })
    } else {
        Ok(MultipartField::Text { name, value })
    }
}

/// Build the `Body` (if any) from the CLI's body-related flags, rejecting
/// combinations that specify more than one body source at once.
fn build_body(args: &OneShotArgs) -> Result<Option<Body>> {
    let sources_given = [
        args.data.is_some(),
        !args.data_urlencode.is_empty(),
        args.data_binary.is_some(),
        !args.form.is_empty(),
    ]
    .iter()
    .filter(|given| **given)
    .count();

    if sources_given > 1 {
        bail!("only one of -d/--data-urlencode/--data-binary/-F may be used per request");
    }

    if let Some(data) = &args.data {
        return Ok(Some(Body::Raw(data.clone())));
    }

    if !args.data_urlencode.is_empty() {
        let pairs = args
            .data_urlencode
            .iter()
            .map(|raw| parse_kv(raw, "--data-urlencode"))
            .collect::<Result<Vec<_>>>()?;
        return Ok(Some(Body::Form(pairs)));
    }

    if let Some(path) = &args.data_binary {
        let path = path
            .to_str()
            .and_then(|s| s.strip_prefix('@'))
            .map(PathBuf::from)
            .unwrap_or_else(|| path.clone());
        let bytes = fs::read(&path)
            .with_context(|| format!("failed to read --data-binary file: {}", path.display()))?;
        return Ok(Some(Body::Binary(bytes)));
    }

    if !args.form.is_empty() {
        let fields = args
            .form
            .iter()
            .map(|raw| parse_form_field(raw))
            .collect::<Result<Vec<_>>>()?;
        return Ok(Some(Body::Multipart(fields)));
    }

    Ok(None)
}

/// Build the `Auth` (if any) from -u/--user and --bearer, rejecting both at once.
fn build_auth(args: &OneShotArgs) -> Result<Option<Auth>> {
    match (&args.user, &args.bearer) {
        (Some(_), Some(_)) => bail!("only one of -u/--user or --bearer may be used per request"),
        (Some(user_pass), None) => {
            let (username, password) = user_pass.split_once(':').ok_or_else(|| {
                anyhow!("invalid -u/--user value (expected \"user:password\"): {user_pass}")
            })?;
            Ok(Some(Auth::Basic {
                username: username.to_string(),
                password: password.to_string(),
            }))
        }
        (None, Some(token)) => Ok(Some(Auth::Bearer {
            token: token.clone(),
        })),
        (None, None) => Ok(None),
    }
}

fn build_request(args: &OneShotArgs) -> Result<Request> {
    let url = args
        .url
        .as_deref()
        .ok_or_else(|| anyhow!("a URL is required (or use a subcommand: env, collections, run, history)"))?;

    let body = build_body(args)?;
    let auth = build_auth(args)?;

    let method = match &args.method {
        Some(m) => Method::from_bytes(m.to_uppercase().as_bytes())
            .with_context(|| format!("invalid HTTP method: {m}"))?,
        None if body.is_some() => Method::POST,
        None => Method::GET,
    };

    let mut request = Request::new(method, url);

    for raw_header in &args.headers {
        let (name, value) = parse_header(raw_header)?;
        request = request.with_header(name, value);
    }

    for raw_query in &args.query {
        let (name, value) = parse_kv(raw_query, "-Q/--query")?;
        request = request.with_query(name, value);
    }

    if let Some(body) = body {
        request = request.with_body(body);
    }

    if let Some(auth) = auth {
        request = request.with_auth(auth);
    }

    Ok(request)
}

pub async fn run(args: &OneShotArgs) -> Result<()> {
    let request = build_request(args)?;
    execute::run(&request, &args.connection, &args.output).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_args(url: &str) -> OneShotArgs {
        OneShotArgs {
            url: Some(url.to_string()),
            method: None,
            headers: Vec::new(),
            query: Vec::new(),
            data: None,
            data_urlencode: Vec::new(),
            data_binary: None,
            form: Vec::new(),
            user: None,
            bearer: None,
            connection: ConnectionArgs {
                insecure: false,
                location: false,
                max_redirects: None,
                max_time: None,
                proxy: None,
                cacert: None,
                cert: None,
                key: None,
            },
            output: OutputArgs {
                output_path: None,
                include: false,
                json_output: false,
                pretty: false,
                verbose: false,
                fail: false,
            },
        }
    }

    #[test]
    fn defaults_to_get() {
        let request = build_request(&base_args("https://example.com")).unwrap();
        assert_eq!(request.method, Method::GET);
    }

    #[test]
    fn missing_url_errors() {
        let mut args = base_args("https://example.com");
        args.url = None;
        assert!(build_request(&args).is_err());
    }

    #[test]
    fn defaults_to_post_when_data_given() {
        let mut args = base_args("https://example.com");
        args.data = Some("hi".into());
        let request = build_request(&args).unwrap();
        assert_eq!(request.method, Method::POST);
        assert!(matches!(request.body, Some(Body::Raw(ref s)) if s == "hi"));
    }

    #[test]
    fn explicit_method_overrides_default() {
        let mut args = base_args("https://example.com");
        args.method = Some("delete".into());
        let request = build_request(&args).unwrap();
        assert_eq!(request.method, Method::DELETE);
    }

    #[test]
    fn invalid_method_errors() {
        let mut args = base_args("https://example.com");
        args.method = Some("NOT A METHOD".into());
        assert!(build_request(&args).is_err());
    }

    #[test]
    fn parses_headers_and_query() {
        let mut args = base_args("https://example.com");
        args.headers = vec!["Content-Type: application/json".into()];
        args.query = vec!["id=42".into()];
        let request = build_request(&args).unwrap();
        assert_eq!(
            request.headers,
            vec![("Content-Type".to_string(), "application/json".to_string())]
        );
        assert_eq!(
            request.query_params,
            vec![("id".to_string(), "42".to_string())]
        );
    }

    #[test]
    fn malformed_header_errors() {
        let mut args = base_args("https://example.com");
        args.headers = vec!["no-colon-here".into()];
        assert!(build_request(&args).is_err());
    }

    #[test]
    fn data_urlencode_builds_form_body() {
        let mut args = base_args("https://example.com");
        args.data_urlencode = vec!["name=a b".into(), "id=1".into()];
        let request = build_request(&args).unwrap();
        match request.body {
            Some(Body::Form(pairs)) => assert_eq!(
                pairs,
                vec![
                    ("name".to_string(), "a b".to_string()),
                    ("id".to_string(), "1".to_string())
                ]
            ),
            other => panic!("expected Form body, got {other:?}"),
        }
    }

    #[test]
    fn form_field_with_at_prefix_is_a_file() {
        let field = parse_form_field("avatar=@photo.png").unwrap();
        assert!(matches!(
            field,
            MultipartField::File { name, path }
                if name == "avatar" && path == PathBuf::from("photo.png")
        ));
    }

    #[test]
    fn form_field_without_at_prefix_is_text() {
        let field = parse_form_field("name=alice").unwrap();
        assert!(matches!(
            field,
            MultipartField::Text { name, value }
                if name == "name" && value == "alice"
        ));
    }

    #[test]
    fn conflicting_body_sources_error() {
        let mut args = base_args("https://example.com");
        args.data = Some("raw".into());
        args.form = vec!["a=b".into()];
        assert!(build_request(&args).is_err());
    }

    #[test]
    fn user_flag_builds_basic_auth() {
        let mut args = base_args("https://example.com");
        args.user = Some("alice:s3cret".into());
        let request = build_request(&args).unwrap();
        assert!(matches!(
            request.auth,
            Some(Auth::Basic { ref username, ref password })
                if username == "alice" && password == "s3cret"
        ));
    }

    #[test]
    fn user_flag_without_colon_errors() {
        let mut args = base_args("https://example.com");
        args.user = Some("alice".into());
        assert!(build_request(&args).is_err());
    }

    #[test]
    fn bearer_flag_builds_bearer_auth() {
        let mut args = base_args("https://example.com");
        args.bearer = Some("tok123".into());
        let request = build_request(&args).unwrap();
        assert!(matches!(
            request.auth,
            Some(Auth::Bearer { ref token }) if token == "tok123"
        ));
    }

    #[test]
    fn user_and_bearer_together_error() {
        let mut args = base_args("https://example.com");
        args.user = Some("alice:s3cret".into());
        args.bearer = Some("tok123".into());
        assert!(build_request(&args).is_err());
    }
}
