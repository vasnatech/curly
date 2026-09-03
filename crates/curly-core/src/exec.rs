use std::time::{Duration, Instant};

use anyhow::Result;

use crate::model::{Body, Request};

#[derive(Debug, Clone)]
pub struct ResponseSummary {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
    pub elapsed: Duration,
}

/// Build and send a `Request`, producing a `ResponseSummary`. This is the
/// single execution path shared by the CLI and (later) the GUI.
pub async fn send(client: &reqwest::Client, request: &Request) -> Result<ResponseSummary> {
    let mut builder = client.request(request.method.clone(), &request.url);

    for (name, value) in &request.headers {
        builder = builder.header(name, value);
    }

    if let Some(Body::Raw(raw)) = &request.body {
        builder = builder.body(raw.clone());
    }

    let started = Instant::now();
    let response = builder.send().await?;
    let status = response.status().as_u16();
    let headers = response
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.to_string(),
                value.to_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    let body = response.text().await?;
    let elapsed = started.elapsed();

    Ok(ResponseSummary {
        status,
        headers,
        body,
        elapsed,
    })
}
