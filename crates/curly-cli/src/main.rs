use std::fs;
use std::path::PathBuf;

use anyhow::{anyhow, bail, Context, Result};
use clap::Parser;
use curly_core::exec;
use curly_core::model::{Auth, Body, MultipartField, Request};
use reqwest::Method;

/// curly - a curl-like HTTP client (CLI face of the Curly project).
#[derive(Parser, Debug)]
#[command(name = "curly", version, about)]
struct Cli {
    /// URL to request
    url: String,

    /// HTTP method (defaults to GET, or POST if a body flag is given)
    #[arg(short = 'X', long = "request")]
    method: Option<String>,

    /// Add a request header, e.g. -H "Content-Type: application/json" (repeatable)
    #[arg(short = 'H', long = "header")]
    headers: Vec<String>,

    /// Add a query parameter, e.g. -Q "id=42" (repeatable)
    #[arg(short = 'Q', long = "query")]
    query: Vec<String>,

    /// Send this as the raw request body
    #[arg(short = 'd', long = "data")]
    data: Option<String>,

    /// Add an application/x-www-form-urlencoded field, e.g. --data-urlencode "name=a" (repeatable)
    #[arg(long = "data-urlencode")]
    data_urlencode: Vec<String>,

    /// Send the contents of a file as the raw request body (accepts an optional leading '@')
    #[arg(long = "data-binary")]
    data_binary: Option<PathBuf>,

    /// Add a multipart/form-data field: "name=value" for text, "name=@path" to upload a file (repeatable)
    #[arg(short = 'F', long = "form")]
    form: Vec<String>,

    /// Basic auth credentials as "user:password"
    #[arg(short = 'u', long = "user")]
    user: Option<String>,

    /// Send a Bearer token in the Authorization header
    #[arg(long = "bearer")]
    bearer: Option<String>,

    /// Write the response body to a file instead of stdout
    #[arg(short = 'o', long = "output")]
    output: Option<PathBuf>,

    /// Include response status and headers in the output
    #[arg(short = 'i', long = "include")]
    include: bool,

    /// Print status/headers/body as a single machine-readable JSON object (takes precedence over -i)
    #[arg(long = "json")]
    json_output: bool,

    /// Pretty-print the response body if it's JSON (leaves non-JSON bodies unchanged)
    #[arg(short = 'p', long = "pretty")]
    pretty: bool,

    /// Print request/response trace to stderr
    #[arg(short = 'v', long = "verbose")]
    verbose: bool,

    /// Skip TLS certificate verification
    #[arg(short = 'k', long = "insecure")]
    insecure: bool,

    /// Exit with a non-zero status if the response is a 4xx/5xx
    #[arg(long = "fail")]
    fail: bool,

    /// Follow HTTP redirects (off by default, like curl)
    #[arg(short = 'L', long = "location")]
    location: bool,

    /// Maximum number of redirects to follow when -L/--location is set (default 10)
    #[arg(long = "max-redirects")]
    max_redirects: Option<usize>,

    /// Abort the request after this many seconds
    #[arg(long = "max-time")]
    max_time: Option<f64>,

    /// Send the request through an HTTP/HTTPS proxy, e.g. http://localhost:8080
    #[arg(short = 'x', long = "proxy")]
    proxy: Option<String>,

    /// Trust an additional CA certificate (PEM) when verifying the server, e.g. for a self-signed dev server
    #[arg(long = "cacert")]
    cacert: Option<PathBuf>,

    /// Client certificate (PEM) to present for mutual TLS — requires --key
    #[arg(long = "cert")]
    cert: Option<PathBuf>,

    /// Private key (PEM) matching --cert — requires --cert
    #[arg(long = "key")]
    key: Option<PathBuf>,
}

/// Split "key=value" into its two halves. Used for -Q, --data-urlencode, and text -F fields.
fn parse_kv(raw: &str, flag: &str) -> Result<(String, String)> {
    let (k, v) = raw
        .split_once('=')
        .ok_or_else(|| anyhow!("invalid {flag} value (expected \"key=value\"): {raw}"))?;
    Ok((k.to_string(), v.to_string()))
}

/// Split "Name: value" into its two halves. Used for -H.
fn parse_header(raw: &str) -> Result<(String, String)> {
    let (name, value) = raw
        .split_once(':')
        .ok_or_else(|| anyhow!("invalid header (expected \"Name: value\"): {raw}"))?;
    Ok((name.trim().to_string(), value.trim().to_string()))
}

/// Parse a -F field into a MultipartField: "name=@path" uploads a file, "name=value" is text.
fn parse_form_field(raw: &str) -> Result<MultipartField> {
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
fn build_body(cli: &Cli) -> Result<Option<Body>> {
    let sources_given = [
        cli.data.is_some(),
        !cli.data_urlencode.is_empty(),
        cli.data_binary.is_some(),
        !cli.form.is_empty(),
    ]
    .iter()
    .filter(|given| **given)
    .count();

    if sources_given > 1 {
        bail!("only one of -d/--data-urlencode/--data-binary/-F may be used per request");
    }

    if let Some(data) = &cli.data {
        return Ok(Some(Body::Raw(data.clone())));
    }

    if !cli.data_urlencode.is_empty() {
        let pairs = cli
            .data_urlencode
            .iter()
            .map(|raw| parse_kv(raw, "--data-urlencode"))
            .collect::<Result<Vec<_>>>()?;
        return Ok(Some(Body::Form(pairs)));
    }

    if let Some(path) = &cli.data_binary {
        let path = path
            .to_str()
            .and_then(|s| s.strip_prefix('@'))
            .map(PathBuf::from)
            .unwrap_or_else(|| path.clone());
        let bytes = fs::read(&path)
            .with_context(|| format!("failed to read --data-binary file: {}", path.display()))?;
        return Ok(Some(Body::Binary(bytes)));
    }

    if !cli.form.is_empty() {
        let fields = cli
            .form
            .iter()
            .map(|raw| parse_form_field(raw))
            .collect::<Result<Vec<_>>>()?;
        return Ok(Some(Body::Multipart(fields)));
    }

    Ok(None)
}

/// Build the `Auth` (if any) from -u/--user and --bearer, rejecting both at once.
fn build_auth(cli: &Cli) -> Result<Option<Auth>> {
    match (&cli.user, &cli.bearer) {
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

fn build_request(cli: &Cli) -> Result<Request> {
    let body = build_body(cli)?;
    let auth = build_auth(cli)?;

    let method = match &cli.method {
        Some(m) => Method::from_bytes(m.to_uppercase().as_bytes())
            .with_context(|| format!("invalid HTTP method: {m}"))?,
        None if body.is_some() => Method::POST,
        None => Method::GET,
    };

    let mut request = Request::new(method, &cli.url);

    for raw_header in &cli.headers {
        let (name, value) = parse_header(raw_header)?;
        request = request.with_header(name, value);
    }

    for raw_query in &cli.query {
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
fn build_client(cli: &Cli) -> Result<reqwest::Client> {
    let redirect_policy = if cli.location {
        reqwest::redirect::Policy::limited(cli.max_redirects.unwrap_or(10))
    } else {
        reqwest::redirect::Policy::none()
    };

    let mut builder = reqwest::Client::builder()
        .danger_accept_invalid_certs(cli.insecure)
        .redirect(redirect_policy);

    if let Some(secs) = cli.max_time {
        builder = builder.timeout(std::time::Duration::from_secs_f64(secs));
    }

    if let Some(proxy_url) = &cli.proxy {
        let proxy = reqwest::Proxy::all(proxy_url)
            .with_context(|| format!("invalid --proxy URL: {proxy_url}"))?;
        builder = builder.proxy(proxy);
    }

    if let Some(path) = &cli.cacert {
        let pem = fs::read(path)
            .with_context(|| format!("failed to read --cacert file: {}", path.display()))?;
        let cert = reqwest::Certificate::from_pem(&pem)
            .with_context(|| format!("invalid PEM in --cacert file: {}", path.display()))?;
        builder = builder.add_root_certificate(cert);
    }

    match (&cli.cert, &cli.key) {
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
fn pretty_json(body: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(body) {
        Ok(value) => serde_json::to_string_pretty(&value).unwrap_or_else(|_| body.to_string()),
        Err(_) => body.to_string(),
    }
}

/// Render the response as the string that ultimately gets printed (or written
/// to -o's file): the raw body by default, pretty-printed JSON with -p, or a
/// single machine-readable JSON object (status/headers/body/timing) with --json.
fn render_body(cli: &Cli, response: &exec::ResponseSummary) -> Result<String> {
    if cli.json_output {
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
    } else if cli.pretty {
        Ok(pretty_json(&response.body))
    } else {
        Ok(response.body.clone())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let request = build_request(&cli)?;

    if cli.verbose {
        eprintln!("> {} {}", request.method, request.url);
        for (name, value) in &request.headers {
            eprintln!("> {name}: {value}");
        }
    }

    let client = build_client(&cli)?;

    let response = exec::send(&client, &request).await?;

    if cli.verbose {
        eprintln!("< status: {} ({:?})", response.status, response.elapsed);
        for (name, value) in &response.headers {
            eprintln!("< {name}: {value}");
        }
    }

    if cli.include && !cli.json_output {
        println!("HTTP {}", response.status);
        for (name, value) in &response.headers {
            println!("{name}: {value}");
        }
        println!();
    }

    let rendered = render_body(&cli, &response)?;
    match &cli.output {
        Some(path) => {
            fs::write(path, &rendered)
                .with_context(|| format!("failed to write response body to {}", path.display()))?;
        }
        None => println!("{}", rendered),
    }

    if cli.fail && response.status >= 400 {
        bail!("server returned HTTP {}", response.status);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_cli(url: &str) -> Cli {
        Cli {
            url: url.to_string(),
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
        let request = build_request(&base_cli("https://example.com")).unwrap();
        assert_eq!(request.method, Method::GET);
    }

    #[test]
    fn defaults_to_post_when_data_given() {
        let mut cli = base_cli("https://example.com");
        cli.data = Some("hi".into());
        let request = build_request(&cli).unwrap();
        assert_eq!(request.method, Method::POST);
        assert!(matches!(request.body, Some(Body::Raw(ref s)) if s == "hi"));
    }

    #[test]
    fn explicit_method_overrides_default() {
        let mut cli = base_cli("https://example.com");
        cli.method = Some("delete".into());
        let request = build_request(&cli).unwrap();
        assert_eq!(request.method, Method::DELETE);
    }

    #[test]
    fn invalid_method_errors() {
        let mut cli = base_cli("https://example.com");
        cli.method = Some("NOT A METHOD".into());
        assert!(build_request(&cli).is_err());
    }

    #[test]
    fn parses_headers_and_query() {
        let mut cli = base_cli("https://example.com");
        cli.headers = vec!["Content-Type: application/json".into()];
        cli.query = vec!["id=42".into()];
        let request = build_request(&cli).unwrap();
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
        let mut cli = base_cli("https://example.com");
        cli.headers = vec!["no-colon-here".into()];
        assert!(build_request(&cli).is_err());
    }

    #[test]
    fn data_urlencode_builds_form_body() {
        let mut cli = base_cli("https://example.com");
        cli.data_urlencode = vec!["name=a b".into(), "id=1".into()];
        let request = build_request(&cli).unwrap();
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
        let mut cli = base_cli("https://example.com");
        cli.data = Some("raw".into());
        cli.form = vec!["a=b".into()];
        assert!(build_request(&cli).is_err());
    }

    #[test]
    fn user_flag_builds_basic_auth() {
        let mut cli = base_cli("https://example.com");
        cli.user = Some("alice:s3cret".into());
        let request = build_request(&cli).unwrap();
        assert!(matches!(
            request.auth,
            Some(Auth::Basic { ref username, ref password })
                if username == "alice" && password == "s3cret"
        ));
    }

    #[test]
    fn user_flag_without_colon_errors() {
        let mut cli = base_cli("https://example.com");
        cli.user = Some("alice".into());
        assert!(build_request(&cli).is_err());
    }

    #[test]
    fn bearer_flag_builds_bearer_auth() {
        let mut cli = base_cli("https://example.com");
        cli.bearer = Some("tok123".into());
        let request = build_request(&cli).unwrap();
        assert!(matches!(
            request.auth,
            Some(Auth::Bearer { ref token }) if token == "tok123"
        ));
    }

    #[test]
    fn user_and_bearer_together_error() {
        let mut cli = base_cli("https://example.com");
        cli.user = Some("alice:s3cret".into());
        cli.bearer = Some("tok123".into());
        assert!(build_request(&cli).is_err());
    }

    #[test]
    fn build_client_defaults_are_fine() {
        let cli = base_cli("https://example.com");
        assert!(build_client(&cli).is_ok());
    }

    #[test]
    fn invalid_proxy_url_errors() {
        let mut cli = base_cli("https://example.com");
        cli.proxy = Some("not a url".into());
        assert!(build_client(&cli).is_err());
    }

    #[test]
    fn missing_cacert_file_errors() {
        let mut cli = base_cli("https://example.com");
        cli.cacert = Some(PathBuf::from("/nonexistent/ca.pem"));
        assert!(build_client(&cli).is_err());
    }

    #[test]
    fn cert_without_key_errors() {
        let mut cli = base_cli("https://example.com");
        cli.cert = Some(PathBuf::from("/nonexistent/cert.pem"));
        assert!(build_client(&cli).is_err());
    }

    #[test]
    fn key_without_cert_errors() {
        let mut cli = base_cli("https://example.com");
        cli.key = Some(PathBuf::from("/nonexistent/key.pem"));
        assert!(build_client(&cli).is_err());
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
        let cli = base_cli("https://example.com");
        let rendered = render_body(&cli, &sample_response(r#"{"a":1}"#)).unwrap();
        assert_eq!(rendered, r#"{"a":1}"#);
    }

    #[test]
    fn render_body_pretty_formats_json() {
        let mut cli = base_cli("https://example.com");
        cli.pretty = true;
        let rendered = render_body(&cli, &sample_response(r#"{"a":1}"#)).unwrap();
        assert_eq!(rendered, "{\n  \"a\": 1\n}");
    }

    #[test]
    fn render_body_json_output_wraps_status_headers_body() {
        let mut cli = base_cli("https://example.com");
        cli.json_output = true;
        let rendered = render_body(&cli, &sample_response("hello")).unwrap();
        let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(value["status"], 200);
        assert_eq!(value["body"], "hello");
        assert_eq!(value["headers"]["Content-Type"], "application/json");
        assert_eq!(value["elapsed_ms"], 42);
    }
}
