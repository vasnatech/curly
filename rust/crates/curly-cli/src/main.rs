mod args;
mod commands;
mod execute;
mod one_shot;

use std::path::PathBuf;

use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};
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
    /// Where the graphical interface went (it is a separate application now)
    Gui,
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
    /// Inspect or clear session variables extracted by `run` (see collections
    /// add-request's --extract-* flags)
    Session {
        #[command(subcommand)]
        action: commands::session::SessionCommand,
    },
    /// Run a multi-request scenario (M6) — creating/editing one is JSON-only,
    /// see docs/ROADMAP.md; .curly/scenarios/<name>.json
    Scenario {
        #[command(subcommand)]
        action: commands::scenario::ScenarioCommand,
    },
}

/// **The GUI is no longer part of this binary.** `crates/curly-gui` was an
/// egui app linked in behind a default-on `gui` feature, and it was deleted
/// once the Tauri + React app superseded it — `docs/GUI.md` had been calling
/// it "the parked `curly-gui`" for a while, while `default = ["gui"]` kept
/// building and shipping it, so the binary and the docs disagreed about what
/// `curly` with no arguments did. It prints help now, and this says where the
/// GUI went rather than failing with "unknown subcommand".
fn gui_moved() -> Result<()> {
    anyhow::bail!(
        "the graphical interface is a separate application and is not part of this binary — \
         see docs/GUI.md for how to run it"
    )
}

fn main() -> Result<()> {
    // A bare `curly` used to launch the bundled GUI; with that gone it prints
    // help, which is what a CLI with no arguments should do. Checked before
    // clap runs because one-shot mode's URL positional is optional, so clap
    // itself would otherwise accept an empty invocation and try to send a
    // request to no URL.
    if std::env::args().len() <= 1 {
        return Cli::command().print_help().map_err(Into::into);
    }

    let cli = Cli::parse();

    // `init` creates the very directory storage resolution would otherwise
    // look for, so it can't itself depend on a resolved Storage. `gui` needs
    // no storage either, since it only reports where the GUI went.
    match cli.command {
        Some(Command::Init) => return commands::init::run(),
        Some(Command::Gui) => return gui_moved(),
        _ => {}
    }

    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(run_cli(cli))
}

async fn run_cli(cli: Cli) -> Result<()> {
    let storage = Storage::resolve_default(cli.data_dir.as_deref())?;

    match cli.command {
        Some(Command::Init) | Some(Command::Gui) => unreachable!("handled in main"),
        Some(Command::Run(args)) => commands::run::run(args, &storage).await,
        Some(Command::Env { action }) => commands::env::run(action, &storage),
        Some(Command::Collections { action }) => commands::collections::run(action, &storage),
        Some(Command::History(args)) => commands::history::run(args, &storage),
        Some(Command::Session { action }) => commands::session::run(action, &storage),
        Some(Command::Scenario { action }) => commands::scenario::run(action, &storage).await,
        None => one_shot::run(&cli.one_shot, &storage).await,
    }
}
