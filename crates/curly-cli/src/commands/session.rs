//! `curly session` — inspect/clear the machine-written variable store
//! `curly run`'s extraction rules write into (see `curly_core::extraction`
//! and `commands::run`). Not for editing by hand — use `curly env` for
//! variables you set yourself.

use anyhow::Result;
use clap::Subcommand;
use curly_core::storage::Storage;

#[derive(Subcommand, Debug)]
pub enum SessionCommand {
    /// Show one environment's session variables (secret values masked)
    Show {
        /// Defaults to "global" (the session used when `run` is called without --env)
        #[arg(long = "env")]
        env: Option<String>,
    },
    /// Clear a session, e.g. after a token expires
    Clear {
        /// Defaults to "global"; ignored if --all is given
        #[arg(long = "env", conflicts_with = "all")]
        env: Option<String>,
        /// Clear every environment's session, not just one
        #[arg(long = "all")]
        all: bool,
    },
}

pub fn run(command: SessionCommand, storage: &Storage) -> Result<()> {
    match command {
        SessionCommand::Show { env } => {
            let env_name = env.as_deref().unwrap_or("global");
            let session = storage.load_session(env_name)?;
            if session.variables.is_empty() {
                println!("session \"{env_name}\" is empty");
            } else {
                for var in &session.variables {
                    let value = if var.secret { "***" } else { var.value.as_str() };
                    println!("{}={}", var.key, value);
                }
            }
        }

        SessionCommand::Clear { env, all } => {
            if all {
                storage.clear_all_sessions()?;
                println!("cleared all sessions");
            } else {
                let env_name = env.as_deref().unwrap_or("global");
                storage.clear_session(env_name)?;
                println!("cleared session \"{env_name}\"");
            }
        }
    }

    Ok(())
}
