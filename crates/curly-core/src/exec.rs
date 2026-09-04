use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use reqwest::multipart;

use crate::model::{Auth, Body, MultipartField, Request};

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

    if !request.query_params.is_empty() {
        builder = builder.query(&request.query_params);
    }

    for (name, value) in &request.headers {
        builder = builder.header(name, value);
    }

    builder = match &request.auth {
        Some(Auth::Basic { username, password }) => builder.basic_auth(username, Some(password)),
        Some(Auth::Bearer { token }) => builder.bearer_auth(token),
        None => builder,
    };

    builder = match &request.body {
        Some(Body::Raw(raw)) => builder.body(raw.clone()),
        Some(Body::Binary(bytes)) => builder.body(bytes.clone()),
        Some(Body::Form(pairs)) => builder.form(pairs),
        Some(Body::Multipart(fields)) => builder.multipart(build_multipart_form(fields).await?),
        None => builder,
    };

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

async fn build_multipart_form(fields: &[MultipartField]) -> Result<multipart::Form> {
    let mut form = multipart::Form::new();
    for field in fields {
        form = match field {
            MultipartField::Text { name, value } => form.text(name.clone(), value.clone()),
            MultipartField::File { name, path } => {
                let bytes = tokio::fs::read(path)
                    .await
                    .with_context(|| format!("failed to read file for --form: {}", path.display()))?;
                let filename = path
                    .file_name()
                    .map(|f| f.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let part = multipart::Part::bytes(bytes).file_name(filename);
                form.part(name.clone(), part)
            }
        };
    }
    Ok(form)
}
