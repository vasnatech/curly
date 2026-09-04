//! `curly init` — create a project-local `./.curly` storage directory, the
//! same idea as `git init`. Once created, `Storage::resolve` auto-detects it
//! from anywhere under the current directory, so collections/environments
//! created there can be committed alongside a project's own repo instead of
//! living only in the OS-wide default location.

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
    fs::write(&gitignore, "history/\n")
        .with_context(|| format!("failed to write {}", gitignore.display()))?;

    println!("initialized curly project at {}", root.display());
    println!("collections and environments created here will be committed with the project;");
    println!("history/ is gitignored by default (response bodies aren't fully redacted).");
    Ok(())
}
