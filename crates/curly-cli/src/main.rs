mod args;
mod commands;
mod execute;
mod one_shot;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use curly_core::storage::Storage;

use one_shot::OneShotArgs;

/// curly - a curl-like HTTP client (CLI face of the Curly project).
#[derive(Parser, Debug)]
#[command(name = "curly", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Where to store collections/environments/history (default: an auto-detected
    /// ./.curly, CURLY_DATA_DIR, or the OS-wide default — see `curly init`)
    #[arg(long = "data-dir", global = true)]
    data_dir: Option<PathBuf>,

    #[command(flatten)]
    one_shot: OneShotArgs,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Create a project-local ./.curly storage directory (like `git init`)
    Init,
    /// Execute a saved request: curly run <collection>/<request-name>
    Run(commands::run::RunArgs),
    /// Manage environments (named {{variable}} sets)
    Env {
        #[command(subcommand)]
        action: commands::env::EnvCommand,
    },
    /// Manage collections and their saved requests
    Collections {
        #[command(subcommand)]
        action: commands::collections::CollectionsCommand,
    },
    /// Browse request history
    History(commands::history::HistoryArgs),
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // `init` creates the very directory storage resolution would otherwise
    // look for, so it can't itself depend on a resolved Storage.
    if matches!(cli.command, Some(Command::Init)) {
        return commands::init::run();
    }

    let storage = Storage::resolve_default(cli.data_dir.as_deref())?;

    match cli.command {
        Some(Command::Init) => unreachable!("handled above"),
        Some(Command::Run(args)) => commands::run::run(args, &storage).await,
        Some(Command::Env { action }) => commands::env::run(action, &storage),
        Some(Command::Collections { action }) => commands::collections::run(action, &storage),
        Some(Command::History(args)) => commands::history::run(args, &storage),
        None => one_shot::run(&cli.one_shot, &storage).await,
    }
}
