mod commands;
mod one_shot;

use anyhow::Result;
use clap::{Parser, Subcommand};

use one_shot::OneShotArgs;

/// curly - a curl-like HTTP client (CLI face of the Curly project).
#[derive(Parser, Debug)]
#[command(name = "curly", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    #[command(flatten)]
    one_shot: OneShotArgs,
}

#[derive(Subcommand, Debug)]
enum Command {
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
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(Command::Env { action }) => commands::env::run(action),
        Some(Command::Collections { action }) => commands::collections::run(action),
        None => one_shot::run(&cli.one_shot).await,
    }
}
