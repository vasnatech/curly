//! `curly history` — browse recorded requests (FR-6). Recording itself
//! happens in execute.rs, shared by one-shot mode and `curly run`; this
//! module only reads it back.

use anyhow::Result;
use clap::Args;
use curly_core::storage::Storage;

#[derive(Args, Debug)]
pub struct HistoryArgs {
    /// Maximum number of entries to show (most recent first)
    #[arg(long = "limit", default_value_t = 20)]
    pub limit: usize,
}

pub fn run(args: HistoryArgs) -> Result<()> {
    let storage = Storage::default_location()?;
    let entries = storage.list_history(args.limit)?;

    if entries.is_empty() {
        println!("no history yet");
        return Ok(());
    }

    for entry in entries {
        println!(
            "{}  {:<6} {:<3} {}  ({} ms)",
            entry.sent_at.format("%Y-%m-%d %H:%M:%S UTC"),
            entry.method,
            entry.status,
            entry.url,
            entry.elapsed_ms
        );
    }

    Ok(())
}
