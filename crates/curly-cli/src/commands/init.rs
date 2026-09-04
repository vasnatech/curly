//! `curly init` — create a project-local `./.curly` storage directory, the
//! same idea as `git init`. Once created, `Storage::resolve` auto-detects it
//! from anywhere under the current directory, so collections created here
//! can be committed alongside a project's own repo instead of living only
//! in the OS-wide default location.
//!
//! Environments are gitignored by default alongside history, not just
//! committed with collections: they typically hold real credentials/tokens
//! (`--secret` only masks them in `env show`, it doesn't encrypt them at
//! rest — see DESIGN.md's tech-choices table), so treating them like a
//! project's own `.env` file (never committed, `.env.example` documents the
//! shape) is the same, more careful default. Collections hold `{{variable}}`
//! *names*, never values, so they're safe to share.

use std::fs;

use anyhow::{Context, Result};
use curly_core::storage::PROJECT_LOCAL_DIR_NAME;

pub fn run() -> Result<()> {
    let cwd = std::env::current_dir().context("failed to determine current directory")?;
    let root = cwd.join(PROJECT_LOCAL_DIR_NAME);

    if root.is_dir() {
        println!("curly project already initialized at {}", root.display());
        return Ok(());
    }

    fs::create_dir_all(&root).with_context(|| format!("failed to create {}", root.display()))?;
    let gitignore = root.join(".gitignore");
    fs::write(&gitignore, "environments/\nhistory/\n")
        .with_context(|| format!("failed to write {}", gitignore.display()))?;

    println!("initialized curly project at {}", root.display());
    println!("collections created here will be committed with the project;");
    println!("environments/ and history/ are gitignored by default (they can hold real");
    println!("credentials/tokens and response bodies) — each teammate sets their own via");
    println!("`curly env set`, the same way you'd fill in a project's .env from .env.example.");
    Ok(())
}
