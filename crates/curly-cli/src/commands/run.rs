//! `curly run <collection>/<request-name>` — execute a saved request (FR-12).
//!
//! Variable scope, low to high precedence: a "global" environment (always
//! merged in if it exists, silently skipped if it doesn't — see DESIGN.md's
//! "global scope plus per-environment overrides" in FR-4), then `--env`'s
//! environment, then `--var key=value` overrides.

use std::collections::BTreeMap;

use anyhow::{anyhow, Result};
use clap::Args;
use curly_core::storage::{self, Storage};
use curly_core::substitution;

use crate::args::{ConnectionArgs, OutputArgs};
use crate::execute;
use crate::one_shot::parse_kv;

#[derive(Args, Debug)]
pub struct RunArgs {
    /// "collection/request-name"
    pub target: String,

    /// Environment to use for {{variable}} substitution
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

    for raw in &args.var {
        let (key, value) = parse_kv(raw, "--var")?;
        variables.insert(key, value);
    }

    Ok(variables)
}

pub async fn run(args: RunArgs) -> Result<()> {
    let (collection_name, request_name) = storage::parse_run_target(&args.target)?;

    let storage_handle = Storage::default_location()?;
    let collection = storage_handle.load_collection(collection_name)?;
    let saved = collection.find_request(request_name).ok_or_else(|| {
        anyhow!("collection \"{collection_name}\" has no request named \"{request_name}\"")
    })?;

    let variables = merged_variables(&storage_handle, &args)?;
    let resolved = substitution::resolve(saved, &variables)?;
    let request = storage::into_request(&resolved)?;

    execute::run(&request, &args.connection, &args.output).await
}
