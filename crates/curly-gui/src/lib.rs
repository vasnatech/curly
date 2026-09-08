//! Curly's GUI (M4, FR-17..24) — an `egui`/`eframe` desktop app, calling
//! straight into `curly-core` the same way `curly-cli` does (no business
//! logic here, per DESIGN.md §1). Linked into the same `curly` binary as
//! the CLI, launched via `curly-cli::main` when invoked with no arguments
//! or via `curly gui` (DESIGN.md §3).

mod app;

pub use app::CurlyApp;

/// Launch the GUI. Blocks until the window is closed.
pub fn run() -> anyhow::Result<()> {
    let options = eframe::NativeOptions::default();
    eframe::run_native(
        "Curly",
        options,
        Box::new(|cc| Ok(Box::new(CurlyApp::new(cc)))),
    )
    .map_err(|e| anyhow::anyhow!("failed to launch GUI: {e}"))
}
