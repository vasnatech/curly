//! Curly's GUI (M4, FR-17..24) — an `egui`/`eframe` desktop app, calling
//! straight into `curly-core` the same way `curly-cli` does (no business
//! logic here, per DESIGN.md §1). Linked into the same `curly` binary as
//! the CLI, launched via `curly-cli::main` when invoked with no arguments
//! or via `curly gui` (DESIGN.md §3).

mod app;
mod project;

pub use app::CurlyApp;
pub use project::Project;

/// Decode the embedded window icon (a `{ }` mark — see assets/icon.svg for
/// the editable source) into the raw RGBA `egui` wants. Baked into the
/// binary via `include_bytes!` rather than loaded from disk at runtime, so
/// the icon always matches whatever binary is running — no missing-file
/// case to handle, no install-time asset to lose track of.
fn window_icon() -> egui::IconData {
    let bytes = include_bytes!("../assets/icon-256.png");
    let image = image::load_from_memory(bytes)
        .expect("bundled window icon (assets/icon-256.png) failed to decode")
        .into_rgba8();
    let (width, height) = image.dimensions();
    egui::IconData {
        rgba: image.into_raw(),
        width,
        height,
    }
}

/// Reverse-DNS app id, matching the `.desktop` file's own name — Wayland
/// (and desktop environments generally) resolve a running window's
/// dock/taskbar/alt-tab icon by matching this against an installed
/// `Icon=`/`StartupWMClass` entry, not the in-window icon hint set below
/// (which X11 window managers, Windows, and macOS use directly, but GNOME's
/// Wayland session does not). No `.desktop` file is installed by this repo
/// yet — that's packaging (DESIGN.md M5), not core app code — so on GNOME
/// specifically, expect a generic icon until one is.
const APP_ID: &str = "tech.vasnatech.curly";

/// Launch the GUI. Blocks until the window is closed.
pub fn run() -> anyhow::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_icon(window_icon())
            .with_app_id(APP_ID),
        ..Default::default()
    };
    eframe::run_native(
        "Curly",
        options,
        Box::new(|cc| Ok(Box::new(CurlyApp::new(cc)))),
    )
    .map_err(|e| anyhow::anyhow!("failed to launch GUI: {e}"))
}
