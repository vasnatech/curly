mod args;
mod commands;
mod execute;
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

    match cli.command {
        Some(Command::Run(args)) => commands::run::run(args).await,
        Some(Command::Env { action }) => commands::env::run(action),
        Some(Command::Collections { action }) => commands::collections::run(action),
        Some(Command::History(args)) => commands::history::run(args),
        None => one_shot::run(&cli.one_shot).await,
    }
}
