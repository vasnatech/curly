//! Flag groups shared between one-shot mode and `curly run`: everything
//! about *how* a request is sent (redirects/timeout/proxy/TLS) and *how* the
//! response is shown (output/include/json/pretty/verbose/fail), as opposed
//! to *what* the request contains, which is one-shot- and
//! collections-add-request-specific (see one_shot.rs / commands/collections.rs).

use std::fs;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::Args;
use curly_core::exec;

#[derive(Args, Debug, Clone)]
pub struct ConnectionArgs {
    /// Skip TLS certificate verification
    #[arg(short = 'k', long = "insecure")]
    pub insecure: bool,

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

#[derive(Args, Debug, Clone)]
pub struct OutputArgs {
    /// Write the response body to a file instead of stdout
    #[arg(short = 'o', long = "output")]
    pub output_path: Option<PathBuf>,

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

    /// Exit with a non-zero status if the response is a 4xx/5xx
    #[arg(long = "fail")]
    pub fail: bool,
}

/// Build the reqwest client from the connection-level flags (redirects,
/// timeout, proxy, TLS verification) — everything that's per-client rather
/// than per-request in reqwest's API.
pub fn build_client(args: &ConnectionArgs) -> Result<reqwest::Client> {
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
pub fn render_body(args: &OutputArgs, response: &exec::ResponseSummary) -> Result<String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn connection_defaults() -> ConnectionArgs {
        ConnectionArgs {
            insecure: false,
            location: false,
            max_redirects: None,
            max_time: None,
            proxy: None,
            cacert: None,
            cert: None,
            key: None,
        }
    }

    fn output_defaults() -> OutputArgs {
        OutputArgs {
            output_path: None,
            include: false,
            json_output: false,
            pretty: false,
            verbose: false,
            fail: false,
        }
    }

    #[test]
    fn build_client_defaults_are_fine() {
        assert!(build_client(&connection_defaults()).is_ok());
    }

    #[test]
    fn invalid_proxy_url_errors() {
        let mut args = connection_defaults();
        args.proxy = Some("not a url".into());
        assert!(build_client(&args).is_err());
    }

    #[test]
    fn missing_cacert_file_errors() {
        let mut args = connection_defaults();
        args.cacert = Some(PathBuf::from("/nonexistent/ca.pem"));
        assert!(build_client(&args).is_err());
    }

    #[test]
    fn cert_without_key_errors() {
        let mut args = connection_defaults();
        args.cert = Some(PathBuf::from("/nonexistent/cert.pem"));
        assert!(build_client(&args).is_err());
    }

    #[test]
    fn key_without_cert_errors() {
        let mut args = connection_defaults();
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
        let args = output_defaults();
        let rendered = render_body(&args, &sample_response(r#"{"a":1}"#)).unwrap();
        assert_eq!(rendered, r#"{"a":1}"#);
    }

    #[test]
    fn render_body_pretty_formats_json() {
        let mut args = output_defaults();
        args.pretty = true;
        let rendered = render_body(&args, &sample_response(r#"{"a":1}"#)).unwrap();
        assert_eq!(rendered, "{\n  \"a\": 1\n}");
    }

    #[test]
    fn render_body_json_output_wraps_status_headers_body() {
        let mut args = output_defaults();
        args.json_output = true;
        let rendered = render_body(&args, &sample_response("hello")).unwrap();
        let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(value["status"], 200);
        assert_eq!(value["body"], "hello");
        assert_eq!(value["headers"]["Content-Type"], "application/json");
        assert_eq!(value["elapsed_ms"], 42);
    }
}
