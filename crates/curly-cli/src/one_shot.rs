//! `curly <url> [flags]` with no subcommand — curl-like one-shot mode (FR-11).

use std::fs;
use std::path::PathBuf;

use anyhow::{anyhow, bail, Context, Result};
use clap::Args;
use curly_core::exec;
use curly_core::model::{Auth, Body, MultipartField, Request};
use curly_core::storage::{HistoryEntry, Storage};
use reqwest::Method;

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

    /// Write the response body to a file instead of stdout
    #[arg(short = 'o', long = "output")]
    pub output: Option<PathBuf>,

    /// Include response status and headers in the output
    #[arg(short = 'i', long = "include")]
    pub include: bool,

    /// Print status/headers/body as a single machine-readable JSON object (takes precedence over -i)
    #[arg(long = "json")]
    pub json_output: bool,

    /// Pretty-print the response body if it's JSON (leaves non-JSON bodies unchanged)
    #[arg(short = 'p', long = "pretty")]
    pub pretty: bool,

    /// Print request/response trace to stderr
    #[arg(short = 'v', long = "verbose")]
    pub verbose: bool,

    /// Skip TLS certificate verification
    #[arg(short = 'k', long = "insecure")]
    pub insecure: bool,

    /// Exit with a non-zero status if the response is a 4xx/5xx
    #[arg(long = "fail")]
    pub fail: bool,

    /// Follow HTTP redirects (off by default, like curl)
    #[arg(short = 'L', long = "location")]
    pub location: bool,

    /// Maximum number of redirects to follow when -L/--location is set (default 10)
    #[arg(long = "max-redirects")]
    pub max_redirects: Option<usize>,

    /// Abort the request after this many seconds
    #[arg(long = "max-time")]
    pub max_time: Option<f64>,

    /// Send the request through an HTTP/HTTPS proxy, e.g. http://localhost:8080
    #[arg(short = 'x', long = "proxy")]
    pub proxy: Option<String>,

    /// Trust an additional CA certificate (PEM) when verifying the server, e.g. for a self-signed dev server
    #[arg(long = "cacert")]
    pub cacert: Option<PathBuf>,

    /// Client certificate (PEM) to present for mutual TLS — requires --key
    #[arg(long = "cert")]
    pub cert: Option<PathBuf>,

    /// Private key (PEM) matching --cert — requires --cert
    #[arg(long = "key")]
    pub key: Option<PathBuf>,
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

/// Build the reqwest client from the connection-level CLI flags (redirects,
/// timeout, proxy, TLS verification) — everything that's per-client rather
/// than per-request in reqwest's API.
pub fn build_client(args: &OneShotArgs) -> Result<reqwest::Client> {
    let redirect_policy = if args.location {
        reqwest::redirect::Policy::limited(args.max_redirects.unwrap_or(10))
    } else {
        reqwest::redirect::Policy::none()
    };

    let mut builder = reqwest::Client::builder()
        .danger_accept_invalid_certs(args.insecure)
        .redirect(redirect_policy);

    if let Some(secs) = args.max_time {
        builder = builder.timeout(std::time::Duration::from_secs_f64(secs));
    }

    if let Some(proxy_url) = &args.proxy {
        let proxy = reqwest::Proxy::all(proxy_url)
            .with_context(|| format!("invalid --proxy URL: {proxy_url}"))?;
        builder = builder.proxy(proxy);
    }

    if let Some(path) = &args.cacert {
        let pem = fs::read(path)
            .with_context(|| format!("failed to read --cacert file: {}", path.display()))?;
        let cert = reqwest::Certificate::from_pem(&pem)
            .with_context(|| format!("invalid PEM in --cacert file: {}", path.display()))?;
        builder = builder.add_root_certificate(cert);
    }

    match (&args.cert, &args.key) {
        (Some(cert_path), Some(key_path)) => {
            let mut pem = fs::read(cert_path)
                .with_context(|| format!("failed to read --cert file: {}", cert_path.display()))?;
            let mut key_pem = fs::read(key_path)
                .with_context(|| format!("failed to read --key file: {}", key_path.display()))?;
            pem.push(b'\n');
            pem.append(&mut key_pem);
            let identity = reqwest::Identity::from_pem(&pem).with_context(|| {
                format!(
                    "invalid PEM in --cert/--key files: {} / {}",
                    cert_path.display(),
                    key_path.display()
                )
            })?;
            builder = builder.identity(identity);
        }
        (None, None) => {}
        _ => bail!("--cert and --key must be used together"),
    }

    builder.build().context("failed to build HTTP client")
}

/// Pretty-print `body` if it parses as JSON; otherwise return it unchanged.
pub fn pretty_json(body: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(body) {
        Ok(value) => serde_json::to_string_pretty(&value).unwrap_or_else(|_| body.to_string()),
        Err(_) => body.to_string(),
    }
}

/// Render the response as the string that ultimately gets printed (or written
/// to -o's file): the raw body by default, pretty-printed JSON with -p, or a
/// single machine-readable JSON object (status/headers/body/timing) with --json.
pub fn render_body(args: &OneShotArgs, response: &exec::ResponseSummary) -> Result<String> {
    if args.json_output {
        let headers: serde_json::Map<String, serde_json::Value> = response
            .headers
            .iter()
            .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
            .collect();
        let payload = serde_json::json!({
            "status": response.status,
            "elapsed_ms": response.elapsed.as_millis(),
            "headers": headers,
            "body": response.body,
        });
        serde_json::to_string_pretty(&payload).context("failed to serialize --json output")
    } else if args.pretty {
        Ok(pretty_json(&response.body))
    } else {
        Ok(response.body.clone())
    }
}

pub async fn run(args: &OneShotArgs) -> Result<()> {
    let request = build_request(args)?;

    if args.verbose {
        eprintln!("> {} {}", request.method, request.url);
        for (name, value) in &request.headers {
            eprintln!("> {name}: {value}");
        }
    }

    let client = build_client(args)?;
    let response = exec::send(&client, &request).await?;

    if let Ok(storage) = Storage::default_location() {
        let _ = storage.append_history(&HistoryEntry::new(&request, &response));
    }

    if args.verbose {
        eprintln!("< status: {} ({:?})", response.status, response.elapsed);
        for (name, value) in &response.headers {
            eprintln!("< {name}: {value}");
        }
    }

    if args.include && !args.json_output {
        println!("HTTP {}", response.status);
        for (name, value) in &response.headers {
            println!("{name}: {value}");
        }
        println!();
    }

    let rendered = render_body(args, &response)?;
    match &args.output {
        Some(path) => {
            fs::write(path, &rendered)
                .with_context(|| format!("failed to write response body to {}", path.display()))?;
        }
        None => println!("{}", rendered),
    }

    if args.fail && response.status >= 400 {
        bail!("server returned HTTP {}", response.status);
    }

    Ok(())
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
            output: None,
            include: false,
            json_output: false,
            pretty: false,
            verbose: false,
            insecure: false,
            fail: false,
            location: false,
            max_redirects: None,
            max_time: None,
            proxy: None,
            cacert: None,
            cert: None,
            key: None,
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

    #[test]
    fn build_client_defaults_are_fine() {
        let args = base_args("https://example.com");
        assert!(build_client(&args).is_ok());
    }

    #[test]
    fn invalid_proxy_url_errors() {
        let mut args = base_args("https://example.com");
        args.proxy = Some("not a url".into());
        assert!(build_client(&args).is_err());
    }

    #[test]
    fn missing_cacert_file_errors() {
        let mut args = base_args("https://example.com");
        args.cacert = Some(PathBuf::from("/nonexistent/ca.pem"));
        assert!(build_client(&args).is_err());
    }

    #[test]
    fn cert_without_key_errors() {
        let mut args = base_args("https://example.com");
        args.cert = Some(PathBuf::from("/nonexistent/cert.pem"));
        assert!(build_client(&args).is_err());
    }

    #[test]
    fn key_without_cert_errors() {
        let mut args = base_args("https://example.com");
        args.key = Some(PathBuf::from("/nonexistent/key.pem"));
        assert!(build_client(&args).is_err());
    }

    fn sample_response(body: &str) -> exec::ResponseSummary {
        exec::ResponseSummary {
            status: 200,
            headers: vec![("Content-Type".to_string(), "application/json".to_string())],
            body: body.to_string(),
            elapsed: std::time::Duration::from_millis(42),
        }
    }

    #[test]
    fn pretty_json_formats_valid_json() {
        assert_eq!(pretty_json(r#"{"a":1}"#), "{\n  \"a\": 1\n}");
    }

    #[test]
    fn pretty_json_leaves_non_json_unchanged() {
        assert_eq!(pretty_json("not json"), "not json");
    }

    #[test]
    fn render_body_defaults_to_raw() {
        let args = base_args("https://example.com");
        let rendered = render_body(&args, &sample_response(r#"{"a":1}"#)).unwrap();
        assert_eq!(rendered, r#"{"a":1}"#);
    }

    #[test]
    fn render_body_pretty_formats_json() {
        let mut args = base_args("https://example.com");
        args.pretty = true;
        let rendered = render_body(&args, &sample_response(r#"{"a":1}"#)).unwrap();
        assert_eq!(rendered, "{\n  \"a\": 1\n}");
    }

    #[test]
    fn render_body_json_output_wraps_status_headers_body() {
        let mut args = base_args("https://example.com");
        args.json_output = true;
        let rendered = render_body(&args, &sample_response("hello")).unwrap();
        let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(value["status"], 200);
        assert_eq!(value["body"], "hello");
        assert_eq!(value["headers"]["Content-Type"], "application/json");
        assert_eq!(value["elapsed_ms"], 42);
    }
}
