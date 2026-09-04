//! The shared "send a built `Request` and show the result" pipeline used by
//! both one-shot mode (one_shot.rs) and `curly run` (commands/run.rs): build
//! the client, send, record history, and render output — identical for both,
//! since only how the `Request` gets built differs between them.

use std::fs;

use anyhow::{bail, Context, Result};
use curly_core::model::Request;
use curly_core::storage::{HistoryEntry, Storage};

use crate::args::{build_client, render_body, ConnectionArgs, OutputArgs};

pub async fn run(request: &Request, connection: &ConnectionArgs, output: &OutputArgs) -> Result<()> {
    if output.verbose {
        eprintln!("> {} {}", request.method, request.url);
        for (name, value) in &request.headers {
            eprintln!("> {name}: {value}");
        }
    }

    let client = build_client(connection)?;
    let response = curly_core::exec::send(&client, request).await?;

    if let Ok(storage) = Storage::default_location() {
        let _ = storage.append_history(&HistoryEntry::new(request, &response));
    }

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

    if output.fail && response.status >= 400 {
        bail!("server returned HTTP {}", response.status);
    }

    Ok(())
}
