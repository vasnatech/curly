//! `curly env` — manage environments (named {{variable}} sets, FR-4/FR-13).

use std::fs;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::Subcommand;
use curly_core::import::postman;
use curly_core::storage::{Environment, Storage};

use crate::one_shot::parse_kv;

#[derive(Subcommand, Debug)]
pub enum EnvCommand {
    /// List all environments
    List,
    /// Show one environment's variables (secret values masked)
    Show { name: String },
    /// Create the environment if needed, then set (or update) one variable
    Set {
        name: String,
        /// KEY=VALUE
        assignment: String,
        /// Mask this value in `env show` and never print it in -v/--verbose traces
        #[arg(long = "secret")]
        secret: bool,
    },
    /// Remove one variable from an environment
    Unset { name: String, key: String },
    /// Delete an environment and all its variables
    Delete { name: String },
    /// Import a Postman Environment or Globals export (FR-7 / M3)
    ImportPostman {
        /// Path to the exported environment/globals JSON file
        path: PathBuf,
        /// Name for the imported environment (defaults to the file's own name,
        /// or "global" for a Globals export — curly's own always-merged-in scope)
        #[arg(long = "as")]
        name: Option<String>,
    },
}

pub fn run(command: EnvCommand, storage: &Storage) -> Result<()> {
    match command {
        EnvCommand::List => {
            let names = storage.list_environments()?;
            if names.is_empty() {
                println!("no environments yet — create one with: curly env set <name> KEY=VALUE");
            } else {
                for name in names {
                    println!("{name}");
                }
            }
        }

        EnvCommand::Show { name } => {
            let env = storage.load_environment(&name)?;
            if env.variables.is_empty() {
                println!("environment \"{name}\" has no variables yet");
            } else {
                for var in &env.variables {
                    let value = if var.secret { "***" } else { var.value.as_str() };
                    println!("{}={}", var.key, value);
                }
            }
        }

        EnvCommand::Set {
            name,
            assignment,
            secret,
        } => {
            let (key, value) = parse_kv(&assignment, "env set")?;
            let mut env = storage
                .load_environment_opt(&name)?
                .unwrap_or_else(|| Environment::new(&name));
            env.set(key.clone(), value, secret);
            storage.save_environment(&env)?;
            println!("set {key} in environment \"{name}\"");
        }

        EnvCommand::Unset { name, key } => {
            let mut env = storage.load_environment(&name)?;
            if env.unset(&key) {
                storage.save_environment(&env)?;
                println!("removed {key} from environment \"{name}\"");
            } else {
                bail!("environment \"{name}\" has no variable named \"{key}\"");
            }
        }

        EnvCommand::Delete { name } => {
            storage.delete_environment(&name)?;
            println!("deleted environment \"{name}\"");
        }

        EnvCommand::ImportPostman { path, name } => {
            let json = fs::read_to_string(&path)
                .with_context(|| format!("failed to read {}", path.display()))?;
            let env = postman::import_environment(&json, name.as_deref())?;
            if storage.load_environment_opt(&env.name)?.is_some() {
                bail!(
                    "environment \"{}\" already exists (delete it first to re-import)",
                    env.name
                );
            }
            let count = env.variables.len();
            let name = env.name.clone();
            storage.save_environment(&env)?;
            println!("imported {count} variable(s) into environment \"{name}\"");
        }
    }

    Ok(())
}
