#!/usr/bin/env bash
# Bo-Ring for macOS — build & install into your home directory (no sudo).
#
#   ./install.sh                 build (release), install binary + assets
#   ./install.sh --enable-daemon …and start the background daemon at login (LaunchAgent)
#   ./install.sh --uninstall     run the interactive uninstaller
set -euo pipefail
cd "$(dirname "$0")"

BIN_DIR="$HOME/.local/bin"
SHARE_DIR="$HOME/.local/share/bo-ring"

if [ "${1:-}" = "--uninstall" ]; then
  exec "$BIN_DIR/bo-ring" --uninstall
fi

command -v cargo >/dev/null || { echo "❌ Rust is required: https://rustup.rs"; exit 1; }

echo "🔨 Building release binary…"
cargo build --release

echo "📦 Installing…"
mkdir -p "$BIN_DIR" "$SHARE_DIR"
# Stop a running daemon so the binary can be replaced.
launchctl bootout "gui/$(id -u)/dev.bo-ring.daemon" 2>/dev/null || true
pkill -f "$BIN_DIR/bo-ring --daemon" 2>/dev/null || true
install -m 755 target/release/bo-ring "$BIN_DIR/bo-ring"
# Stable identifier so macOS privacy grants survive reinstalls of the same build.
codesign --force --sign - --identifier dev.bo-ring "$BIN_DIR/bo-ring" 2>/dev/null || true
rm -rf "$SHARE_DIR/assets" "$SHARE_DIR/locales"
cp -R assets "$SHARE_DIR/assets"
cp -R locales "$SHARE_DIR/locales"

echo "✅ Installed $BIN_DIR/bo-ring"
case ":$PATH:" in *":$BIN_DIR:"*) ;; *) echo "ℹ️  Add $BIN_DIR to your PATH.";; esac

if [ "${1:-}" = "--enable-daemon" ]; then
  "$BIN_DIR/bo-ring" --install-agent
fi

cat <<MSG

Next steps
  1. Run the daemon once:   $BIN_DIR/bo-ring --daemon
     macOS will ask for Accessibility and Input Monitoring — allow both
     (System Settings → Privacy & Security), then restart the daemon.
  2. Configure:             $BIN_DIR/bo-ring
     Settings → Background Service enables the LaunchAgent (start at login).
  3. Toggle the ring from any hotkey tool:   $BIN_DIR/bo-ring --toggle
MSG
