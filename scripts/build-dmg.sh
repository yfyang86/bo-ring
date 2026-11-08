#!/usr/bin/env bash
# Build Bo-Ring.app and a distributable DMG for macOS.
#
#   scripts/build-dmg.sh               arm64 (Apple Silicon) build
#   scripts/build-dmg.sh --universal   arm64 + x86_64 universal binary
#                                      (needs: rustup target add x86_64-apple-darwin)
#
# Output: dist/Bo-Ring.app and dist/Bo-Ring-<version>-<arch>.dmg
#
# Signing: $BORING_SIGN_IDENTITY (e.g. "Developer ID Application: …") if set,
# otherwise ad-hoc. Ad-hoc builds run fine locally; on other Macs Gatekeeper
# requires right-click → Open (or `xattr -dr com.apple.quarantine`) the first time.
set -euo pipefail
cd "$(dirname "$0")/.."

APP_NAME="Bo-Ring"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
BUILD="$(git rev-list --count HEAD 2>/dev/null || echo 1)"
DIST="dist"
APP="$DIST/$APP_NAME.app"
UNIVERSAL=0
[ "${1:-}" = "--universal" ] && UNIVERSAL=1

echo "🔨 Building bo-ring ${VERSION}…"
if [ "$UNIVERSAL" = 1 ]; then
  cargo build --release --target aarch64-apple-darwin
  cargo build --release --target x86_64-apple-darwin
  mkdir -p target/universal
  lipo -create -output target/universal/bo-ring \
    target/aarch64-apple-darwin/release/bo-ring target/x86_64-apple-darwin/release/bo-ring
  BIN="target/universal/bo-ring"; ARCH="universal"
else
  cargo build --release
  BIN="target/release/bo-ring"; ARCH="$(uname -m)"
fi

echo "📦 Assembling ${APP}…"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/bo-ring"
sed -e "s/__VERSION__/$VERSION/" -e "s/__BUILD__/$BUILD/" packaging/macos/Info.plist > "$APP/Contents/Info.plist"
cp -R assets "$APP/Contents/Resources/assets"
cp -R locales "$APP/Contents/Resources/locales"
cp LICENSE "$APP/Contents/Resources/"

# App icon (rendered once, cached in target/).
ICNS="target/AppIcon.icns"
if [ ! -f "$ICNS" ] || [ packaging/macos/make-icon.swift -nt "$ICNS" ]; then
  echo "🎨 Rendering icon…"
  ICONSET="target/AppIcon.iconset"
  rm -rf "$ICONSET"; mkdir -p "$ICONSET"
  swift packaging/macos/make-icon.swift target/icon-1024.png >/dev/null
  for s in 16 32 128 256 512; do
    sips -z $s $s target/icon-1024.png --out "$ICONSET/icon_${s}x${s}.png" >/dev/null
    sips -z $((s*2)) $((s*2)) target/icon-1024.png --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
  done
  iconutil -c icns "$ICONSET" -o "$ICNS"
fi
cp "$ICNS" "$APP/Contents/Resources/AppIcon.icns"

echo "🔏 Signing…"
if [ -n "${BORING_SIGN_IDENTITY:-}" ]; then
  codesign --force --options runtime --timestamp --sign "$BORING_SIGN_IDENTITY" "$APP"
else
  codesign --force --sign - "$APP"
fi
codesign --verify --strict "$APP"

echo "💿 Creating DMG…"
DMG="$DIST/$APP_NAME-$VERSION-$ARCH.dmg"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
cat > "$STAGE/READ ME FIRST.txt" <<TXT
Bo-Ring $VERSION for macOS

1. Drag Bo-Ring into Applications, then open it (first time: right-click → Open).
2. Opening Bo-Ring starts its background daemon (runs at login; Settings → Background Service turns it off).
3. Allow Bo-Ring in System Settings → Privacy & Security → Accessibility and
   Input Monitoring when asked, then restart the service from Settings.

Toggle the ring from any hotkey tool:
  /Applications/Bo-Ring.app/Contents/MacOS/bo-ring --toggle
Uninstall:
  /Applications/Bo-Ring.app/Contents/MacOS/bo-ring --uninstall
TXT
rm -f "$DMG"
hdiutil create -volname "$APP_NAME $VERSION" -srcfolder "$STAGE" -fs HFS+ -format UDZO -imagekey zlib-level=9 -ov "$DMG" >/dev/null
[ -n "${BORING_SIGN_IDENTITY:-}" ] && codesign --force --sign "$BORING_SIGN_IDENTITY" "$DMG"

echo "✅ $DMG ($(du -h "$DMG" | cut -f1))"
