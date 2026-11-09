//! The shared "send a built `Request` and show the result" pipeline used by
//! both one-shot mode (one_shot.rs) and `curly run` (commands/run.rs): build
//! the client, send, record history, and render output — identical for both,
//! since only how the `Request` gets built differs between them.

use std::fs;

use anyhow::{bail, Context, Result};
use curly_core::exec::ResponseSummary;
use curly_core::model::Request;
use curly_core::storage::{HistoryEntry, Storage};

use crate::args::{build_client, render_body, ConnectionArgs, OutputArgs};

/// `after_send` runs once the response has been fully printed/written —
/// `curly run` uses it to apply the saved request's extraction rules (see
/// `commands::run`); one-shot mode has no rules, so it passes a no-op. Kept
/// as a generic hook rather than baking extraction into this function so
/// execute.rs stays agnostic of what "extraction" even is. Deliberately
/// runs *after* output is shown, not before: if an extraction rule fails
/// (bad path, missing header), you still want to see the response you're
/// debugging that failure against, not have it swallowed by the error.
pub async fn run<F>(
    request: &Request,
    connection: &ConnectionArgs,
    output: &OutputArgs,
    storage: &Storage,
    after_send: F,
) -> Result<()>
where
    F: FnOnce(&ResponseSummary) -> Result<()>,
{
    if output.verbose {
        eprintln!("> {} {}", request.method, request.url);
        for (name, value) in &request.headers {
            eprintln!("> {name}: {value}");
        }
    }

    let client = build_client(connection)?;
    let response = curly_core::exec::send(&client, request).await?;

    let _ = storage.append_history(&HistoryEntry::new(request, &response));

    if output.verbose {
        eprintln!("< status: {} ({:?})", response.status, response.elapsed);
        for (name, value) in &response.headers {
            eprintln!("< {name}: {value}");
        }
    }

    if output.include && !output.json_output {
        println!("HTTP {}", response.status);
        for (name, value) in &response.headers {
            println!("{name}: {value}");
        }
        println!();
    }

    let rendered = render_body(output, &response)?;
    match &output.output_path {
        Some(path) => {
            fs::write(path, &rendered)
                .with_context(|| format!("failed to write response body to {}", path.display()))?;
        }
        None => println!("{}", rendered),
    }

    after_send(&response)?;

    if output.fail && response.status >= 400 {
        bail!("server returned HTTP {}", response.status);
    }

    Ok(())
}
