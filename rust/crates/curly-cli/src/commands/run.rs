//! `curly run <collection>/<request-name>` — execute a saved request (FR-12).
//!
//! Variable scope, low to high precedence: a "global" environment (always
//! merged in if it exists, silently skipped if it doesn't — see DESIGN.md's
//! "global scope plus per-environment overrides" in FR-4), then `--env`'s
//! environment, then `--env`'s **session** (machine-written by this
//! request's own `extract` rules on a previous run — see
//! `curly_core::extraction` — or another request's), then `--var
//! key=value` overrides.

use std::collections::BTreeMap;

use anyhow::{anyhow, Result};
use clap::Args;
use curly_core::extraction;
use curly_core::storage::{self, Storage};
use curly_core::substitution;

use crate::args::{ConnectionArgs, OutputArgs};
use crate::execute;
use crate::one_shot::parse_kv;

#[derive(Args, Debug)]
pub struct RunArgs {
    /// "collection/request-name"
    pub target: String,

    /// Environment to use for {{variable}} substitution (also selects which
    /// environment's session extracted variables are read from / written to)
    #[arg(long = "env")]
    pub env: Option<String>,

    /// Add or override a variable for this run, e.g. --var id=42 (repeatable, highest precedence)
    #[arg(long = "var")]
    pub var: Vec<String>,

    #[command(flatten)]
    pub connection: ConnectionArgs,

    #[command(flatten)]
    pub output: OutputArgs,
}

/// The environment name used to scope this run's session — `--env`'s value,
/// or "global" if none was given. Distinct from the "global" *environment*,
/// which is always merged in regardless of this.
fn session_scope(args: &RunArgs) -> &str {
    args.env.as_deref().unwrap_or("global")
}

fn merged_variables(storage: &Storage, args: &RunArgs) -> Result<BTreeMap<String, String>> {
    let mut variables = BTreeMap::new();

    if let Some(global) = storage.load_environment_opt("global")? {
        for var in global.variables {
            variables.insert(var.key, var.value);
        }
    }

    if let Some(env_name) = &args.env {
        let env = storage.load_environment(env_name)?;
        for var in env.variables {
            variables.insert(var.key, var.value);
        }
    }

    let session = storage.load_session(session_scope(args))?;
    for var in session.variables {
        variables.insert(var.key, var.value);
    }

    for raw in &args.var {
        let (key, value) = parse_kv(raw, "--var")?;
        variables.insert(key, value);
    }

    Ok(variables)
}

pub async fn run(args: RunArgs, storage: &Storage) -> Result<()> {
    let (collection_name, request_name) = storage::parse_run_target(&args.target)?;

    let collection = storage.load_collection(collection_name)?;
    let saved = collection.find_request(request_name).ok_or_else(|| {
        anyhow!("collection \"{collection_name}\" has no request named \"{request_name}\"")
    })?;
    let extract_rules = saved.extract.clone();

    let variables = merged_variables(storage, &args)?;
    let resolved = substitution::resolve(saved, &variables)?;
    let request = storage::into_request(&resolved)?;

    let session_env = session_scope(&args).to_string();

    execute::run(&request, &args.connection, &args.output, storage, |response| {
        // Only extract from a successful response — a failed login has no
        // token to pull out, and that's an expected absence, not a bug.
        if !(200..300).contains(&response.status) || extract_rules.is_empty() {
            return Ok(());
        }

        let extracted = extraction::apply(&extract_rules, response, &variables)?;
        let mut session = storage.load_session(&session_env)?;
        for var in &extracted {
            session.set(var.name.as_str(), var.value.as_str(), var.secret);
        }
        storage.save_session(&session)?;

        if args.output.verbose {
            for var in &extracted {
                eprintln!("~ extracted {} into session \"{session_env}\"", var.name);
            }
        }

        Ok(())
    })
    .await
}
