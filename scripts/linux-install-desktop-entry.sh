#!/usr/bin/env bash
# Install a .desktop entry + icon so Linux desktop environments — GNOME
# Shell/Wayland in particular — show curly's real icon in the dock/Alt-Tab
# switcher/Activities overview instead of a generic fallback.
#
# Why this is needed at all: eframe/winit's in-window icon hint (set in
# crates/curly-gui/src/lib.rs) is what X11 window managers and the
# Windows/macOS taskbar use directly, but GNOME's Wayland session instead
# resolves a running window's dock icon by matching its Wayland app-id
# against an installed .desktop file's Icon=/name — with no such file
# installed, there's nothing to match, hence the generic icon.
#
# This is dev/local convenience, not proper packaging (see docs/DESIGN.md's
# M5 milestone for that) — it points straight at this checkout's release
# binary, so re-run it if you move the repo or rebuild elsewhere.
#
# The app id below must match APP_ID in crates/curly-gui/src/lib.rs — that's
# the string the desktop environment actually matches on.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(dirname "$SCRIPT_DIR")"
BINARY="$REPO_ROOT/target/release/curly"
APP_ID="tech.vasnatech.curly"

if [ ! -x "$BINARY" ]; then
  echo "Release binary not found — building it first (cargo build --release)..." >&2
  (cd "$REPO_ROOT" && cargo build --release)
fi

ICON_DIR_PNG="$HOME/.local/share/icons/hicolor/256x256/apps"
ICON_DIR_SVG="$HOME/.local/share/icons/hicolor/scalable/apps"
APPS_DIR="$HOME/.local/share/applications"

mkdir -p "$ICON_DIR_PNG" "$ICON_DIR_SVG" "$APPS_DIR"
cp "$REPO_ROOT/crates/curly-gui/assets/icon-256.png" "$ICON_DIR_PNG/$APP_ID.png"
cp "$REPO_ROOT/crates/curly-gui/assets/icon.svg" "$ICON_DIR_SVG/$APP_ID.svg"

cat > "$APPS_DIR/$APP_ID.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Curly
Comment=A curl-like CLI / Postman-like GUI REST client
Exec=$BINARY gui
Icon=$APP_ID
Terminal=false
Categories=Development;Utility;
StartupWMClass=$APP_ID
EOF

command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$APPS_DIR" || true
command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -f "$HOME/.local/share/icons/hicolor" >/dev/null 2>&1 || true

echo "Installed $APPS_DIR/$APP_ID.desktop"
echo "Restart curly's GUI (if it's already running) for your desktop environment to pick up the association."
echo "To remove: rm '$APPS_DIR/$APP_ID.desktop' '$ICON_DIR_PNG/$APP_ID.png' '$ICON_DIR_SVG/$APP_ID.svg'"
