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
    /// Launch the graphical interface (also launched by running curly with no arguments)
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
}

/// Launch the GUI (DESIGN.md §3). Deliberately kept off the async runtime
/// below — `eframe::run_native` blocks the calling thread until the window
/// closes and spins up its own background tokio runtime for sends
/// (curly-gui's `CurlyApp`), so it runs cleanest on a plain thread rather
/// than nested inside `#[tokio::main]`'s runtime.
fn launch_gui() -> Result<()> {
    #[cfg(feature = "gui")]
    {
        curly_gui::run()
    }
    #[cfg(not(feature = "gui"))]
    {
        anyhow::bail!(
            "this build of curly was compiled without the gui feature \
             (--no-default-features); pass a URL or a subcommand instead — see --help"
        )
    }
}

fn main() -> Result<()> {
    // A bare `curly` with nothing else at all launches the GUI, per
    // DESIGN.md §3 — checked before clap even runs, since `curly gui` and
    // one-shot mode's URL positional both need to stay optional at the
    // clap level for that coexistence to parse (see one_shot.rs).
    if std::env::args().len() <= 1 {
        return launch_gui();
    }

    let cli = Cli::parse();

    // `init` creates the very directory storage resolution would otherwise
    // look for, so it can't itself depend on a resolved Storage. `gui`
    // doesn't touch storage in this first slice either — see launch_gui.
    match cli.command {
        Some(Command::Init) => return commands::init::run(),
        Some(Command::Gui) => return launch_gui(),
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
        None => one_shot::run(&cli.one_shot, &storage).await,
    }
}
