use std::fs;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::Parser;
use curly_core::exec;
use curly_core::model::{Body, Request};
use reqwest::Method;

/// curly - a curl-like HTTP client (CLI face of the Curly project).
#[derive(Parser, Debug)]
#[command(name = "curly", version, about)]
struct Cli {
    /// URL to request
    url: String,

    /// HTTP method (defaults to GET, or POST if --data is given)
    #[arg(short = 'X', long = "request")]
    method: Option<String>,

    /// Add a request header, e.g. -H "Content-Type: application/json" (repeatable)
    #[arg(short = 'H', long = "header")]
    headers: Vec<String>,

    /// Send this as the request body
    #[arg(short = 'd', long = "data")]
    data: Option<String>,

    /// Write the response body to a file instead of stdout
    #[arg(short = 'o', long = "output")]
    output: Option<PathBuf>,

    /// Include response status and headers in the output
    #[arg(short = 'i', long = "include")]
    include: bool,

    /// Print request/response trace to stderr
    #[arg(short = 'v', long = "verbose")]
    verbose: bool,

    /// Skip TLS certificate verification
    #[arg(short = 'k', long = "insecure")]
    insecure: bool,

    /// Exit with a non-zero status if the response is a 4xx/5xx
    #[arg(long = "fail")]
    fail: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let method = match &cli.method {
        Some(m) => Method::from_bytes(m.to_uppercase().as_bytes())
            .with_context(|| format!("invalid HTTP method: {m}"))?,
        None if cli.data.is_some() => Method::POST,
        None => Method::GET,
    };

    let mut request = Request::new(method, &cli.url);
    for raw_header in &cli.headers {
        let (name, value) = raw_header
            .split_once(':')
            .with_context(|| format!("invalid header (expected \"Name: value\"): {raw_header}"))?;
        request = request.with_header(name.trim(), value.trim());
    }
    if let Some(data) = &cli.data {
        request = request.with_body(Body::Raw(data.clone()));
    }

    if cli.verbose {
        eprintln!("> {} {}", request.method, request.url);
        for (name, value) in &request.headers {
            eprintln!("> {name}: {value}");
        }
    }

    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(cli.insecure)
        .build()
        .context("failed to build HTTP client")?;

    let response = exec::send(&client, &request).await?;

    if cli.verbose {
        eprintln!("< status: {} ({:?})", response.status, response.elapsed);
        for (name, value) in &response.headers {
            eprintln!("< {name}: {value}");
        }
    }

    if cli.include {
        println!("HTTP {}", response.status);
        for (name, value) in &response.headers {
            println!("{name}: {value}");
        }
        println!();
    }

    match &cli.output {
        Some(path) => {
            fs::write(path, &response.body)
                .with_context(|| format!("failed to write response body to {}", path.display()))?;
        }
        None => println!("{}", response.body),
    }

    if cli.fail && response.status >= 400 {
        bail!("server returned HTTP {}", response.status);
    }

    Ok(())
}
