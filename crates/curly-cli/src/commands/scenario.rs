//! `curly scenario` — run a multi-request orchestration (M6, FR-25). Design
//! fully worked through in `docs/ROADMAP.md` before any of this was written;
//! creating/editing a scenario is JSON-only for v1 (hand-edit
//! `.curly/scenarios/<name>.json` — see `docs/ROADMAP.md`'s "Decided:
//! authoring shape"), so this only has `list`/`show`/`run`, no `add`/`edit`.

use anyhow::Result;
use clap::{Args, Subcommand};
use curly_core::scenario::{self, StepOutcome};
use curly_core::storage::Storage;

use crate::one_shot::parse_kv;

#[derive(Subcommand, Debug)]
pub enum ScenarioCommand {
    /// List all scenarios
    List,
    /// Show one scenario's name, imports, and step count
    Show { name: String },
    /// Run a scenario
    Run(RunArgs),
    /// Delete a scenario
    Delete { name: String },
}

#[derive(Args, Debug)]
pub struct RunArgs {
    pub name: String,

    /// Environment to use for {{variable}} substitution across every step
    /// (also selects which environment's session extracted variables are
    /// read from / written to for every step — same as `curly run --env`)
    #[arg(long = "env")]
    pub env: Option<String>,

    /// Add or override a variable for this run, e.g. --var id=42
    /// (repeatable, highest precedence, applies to every step)
    #[arg(long = "var")]
    pub var: Vec<String>,
}

pub async fn run(command: ScenarioCommand, storage: &Storage) -> Result<()> {
    match command {
        ScenarioCommand::List => {
            let names = storage.list_scenarios()?;
            if names.is_empty() {
                println!("no scenarios yet — create one at .curly/scenarios/<name>.json (see docs/ROADMAP.md)");
            } else {
                for name in names {
                    println!("{name}");
                }
            }
            Ok(())
        }

        ScenarioCommand::Show { name } => {
            let scenario = storage.load_scenario(&name)?;
            println!("{}", scenario.name);
            if scenario.imports.is_empty() {
                println!("  (no imports)");
            } else {
                for (alias, collection) in &scenario.imports {
                    println!("  {alias} -> {collection}");
                }
            }
            println!("  {} top-level step(s)", scenario.requests.items.len());
            Ok(())
        }

        ScenarioCommand::Delete { name } => {
            storage.delete_scenario(&name)?;
            println!("deleted scenario \"{name}\"");
            Ok(())
        }

        ScenarioCommand::Run(args) => run_scenario(args, storage).await,
    }
}

async fn run_scenario(args: RunArgs, storage: &Storage) -> Result<()> {
    let scenario = storage.load_scenario(&args.name)?;

    let mut overrides = Vec::with_capacity(args.var.len());
    for raw in &args.var {
        overrides.push(parse_kv(raw, "--var")?);
    }

    let client = reqwest::Client::new();
    let result = scenario::run(&scenario, storage, &client, args.env.as_deref(), &overrides).await?;

    for step in &result.steps {
        match step.status {
            Some(status) => println!("{:<7} {} -> {status}", step.method, step.url),
            None => println!("{:<7} {} -> (no response: {})", step.method, step.url, step.error.as_deref().unwrap_or("?")),
        }
        if !step.extracted.is_empty() {
            println!("  ~ extracted {}", step.extracted.join(", "));
        }
    }

    match result.outcome {
        StepOutcome::Ok => {
            println!("scenario \"{}\" completed", args.name);
            Ok(())
        }
        StepOutcome::HttpFailure => {
            // Only ever surfaces here if `scenario::run`'s own top-level
            // sequence/parallel returned it directly with nothing to
            // convert it to a Halted -- practically unreachable (the top
            // level is always a sequence/parallel, which always resolves
            // an unforgiven HttpFailure into a Halted), kept only so this
            // match is exhaustive without a wildcard hiding a real variant.
            anyhow::bail!("scenario \"{}\" ended with an unhandled HTTP failure", args.name)
        }
        StepOutcome::Halted(reason) => {
            anyhow::bail!("scenario \"{}\" halted: {reason}", args.name)
        }
    }
}
