//! `curly init` — create a project-local `./.curly` storage directory, the
//! same idea as `git init`. Once created, `Storage::resolve` auto-detects it
//! from anywhere under the current directory, so collections created here
//! can be committed alongside a project's own repo instead of living only
//! in the OS-wide default location.
//!
//! The actual directory/`.gitignore` creation lives in
//! `curly_core::storage::Storage::init_project_local`, shared with
//! `curly-gui`'s "Open Project" flow (which offers to create one when the
//! chosen folder has none) — this module is just the CLI's messaging around
//! that shared call.
//!
//! Environments and sessions are gitignored by default alongside history,
//! not just committed with collections: they typically hold real
//! credentials/tokens (`--secret` only masks them in `env show`/`session
//! show`, it doesn't encrypt them at rest — see DESIGN.md's tech-choices
//! table), so treating them like a project's own `.env` file (never
//! committed, `.env.example` documents the shape) is the same, more careful
//! default. Collections hold `{{variable}}` *names*, never values, so
//! they're safe to share.

use anyhow::{Context, Result};
use curly_core::storage::Storage;

pub fn run() -> Result<()> {
    let cwd = std::env::current_dir().context("failed to determine current directory")?;
    let (storage, created) = Storage::init_project_local(&cwd)?;

    if !created {
        println!(
            "curly project already initialized at {}",
            storage.root().display()
        );
        return Ok(());
    }

    println!("initialized curly project at {}", storage.root().display());
    println!("collections created here will be committed with the project;");
    println!("environments/, session/, and history/ are gitignored by default (they can hold");
    println!("real credentials/tokens and response bodies) — each teammate sets their own via");
    println!("`curly env set`, the same way you'd fill in a project's .env from .env.example.");
    Ok(())
}
