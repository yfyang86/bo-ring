# Bo-Ring for macOS

macOS (Apple Silicon) port of [**Bo-Ring**](https://github.com/BazinFla/bo-ring) by
BazinFla: a radial **Action Ring** overlay and mouse-button remapper written in Rust and
egui. The pristine upstream (v0.2.0) is kept in [`../boring`](../boring) as the template.
This directory is the port.

Everything outside the platform layer is the upstream code: the egui configurator, ring
rendering and state machine, app profiles, presets, device profiles and i18n. The Linux
input stack is replaced by native macOS APIs.

## What changed for macOS

| Concern | Linux (upstream) | macOS (this port) |
|---|---|---|
| Capture extra mouse buttons | evdev exclusive grab + uinput passthrough | `CGEventTap` that absorbs only remapped buttons (`platform/event_tap.rs`) |
| Synthesize shortcuts / media keys | uinput virtual keyboard | `CGEvent` + `NSSystemDefined` media keys (`platform/input_emitter.rs`) |
| Logitech special buttons | HID++ over `/dev/hidraw*` | HID++ over IOKit via `hidapi`, shared open, long reports over BLE (`devices/hidpp.rs`) |
| Active app → contextual ring | GNOME ext / kdotool / hyprctl / swaymsg / xprop | `NSWorkspace.frontmostApplication` bundle id (`platform/window.rs`) |
| Ring position | maximized window + Wayland "nudge" to learn pointer | full-screen transparent window on the cursor's display, real cursor position (`platform/overlay.rs`) |
| Autostart | systemd user unit / XDG autostart | launchd LaunchAgents (`platform/autostart.rs`) |
| Permissions | udev `uaccess` rules | Accessibility + Input Monitoring (`platform/permissions.rs`) |
| Accent color | portal / gsettings / kdeglobals | `AppleAccentColor` |

Behavior changes and fixes:

* **MX Master 4.** On macOS the **haptic thumb panel** (CID `0x01A0`) and the gesture
  button (`0x00C3`) do nothing natively, so both are diverted over HID++. The panel maps
  to code 278, which opens the ring by default. Verified on hardware over Bluetooth.
* **Diverted controls are restored** when the daemon stops (SIGTERM/SIGINT), so a
  stopped daemon no longer leaves a dead button. Upstream leaves them diverted.
* **HID++ software IDs rotate per request.** On macOS every handle to a HID device sees
  every report, and ROOT replies don't say which feature was asked for, so concurrent
  requests used to steal each other's replies.
* **Presets use Mac shortcuts.** Ctrl→Cmd in app rings, but Ctrl+Tab is kept for browser
  tabs. Browser F-keys map to their Mac equivalents. Finder gets Cmd+I, Cmd+[ / Cmd+],
  Cmd+Up. `Super` → Mission Control, Print → Cmd+Shift+4, window snapping →
  macOS 15+ tiling (Fn+Ctrl+arrows). Commands use `open`, `pmset` and `osascript`.
* **App profiles match bundle ids.** Examples: `org.mozilla.firefox`,
  `com.google.Chrome`, `com.apple.Safari`, `com.microsoft.VSCode`, `com.apple.finder`.
  The ring shows the app's display name.
* **`bo-ring --toggle`** toggles the ring through the daemon, so any hotkey tool can
  open it. **`--install-agent`** installs and starts the LaunchAgent.

## Install

### DMG (recommended)

```bash
scripts/build-dmg.sh              # → dist/Bo-Ring-<version>-arm64.dmg (+ dist/Bo-Ring.app)
scripts/build-dmg.sh --universal  # arm64 + x86_64 (rustup target add x86_64-apple-darwin)
```

Open the DMG, drag **Bo-Ring** to Applications, and launch it. The first launch of an
ad-hoc signed build needs right-click → Open. Opening the app starts the background
daemon automatically (as a LaunchAgent, so it also runs at login). You can turn it off
under Settings → Background Service, and that choice is remembered. Allow Accessibility
and Input Monitoring for Bo-Ring when macOS asks.
Set `BORING_SIGN_IDENTITY="Developer ID Application: …"` to sign with a real identity.
The app bundle includes the device profiles, images and locales in
`Contents/Resources`, and its icon is rendered by `packaging/macos/make-icon.swift`.

### From source

You need Rust 1.85+ (edition 2024) and Xcode command-line tools.

```bash
cd boring-mac
./install.sh                    # builds, installs ~/.local/bin/bo-ring and assets
~/.local/bin/bo-ring --daemon   # first run: allow Accessibility + Input Monitoring
~/.local/bin/bo-ring            # the configurator (enable the LaunchAgent under Settings)
```

Permissions attach to the binary that asks for them. If you run it from a terminal,
macOS attributes them to the terminal app instead.

## Run / develop

```bash
cargo test                       # 45 tests (upstream + macOS-specific)
cargo run --release -- --daemon  # daemon: event tap + HID++ + IPC socket
cargo run --release              # configurator
cargo run --release -- --ring    # the ring overlay alone
cargo run --release -- --toggle  # ask the running daemon to toggle the ring
```

Runtime files (socket, ring PID) live in the per-user temp dir
(`confstr(_CS_DARWIN_USER_TEMP_DIR)`). Config stays at `~/.config/bo-ring/` (same layout
as Linux). Daemon logs from the LaunchAgent go to `~/Library/Logs/bo-ring/`.

## Known limitations

* Color emoji icons are drawn with egui's bundled monochrome NotoEmoji subset and Apple
  Symbols. egui cannot render Apple Color Emoji.
* The ring is a separate process (as upstream), so it briefly takes focus. The daemon
  re-activates the app you summoned it over before sending a shortcut.
* "Clipboard" in the default ring opens Spotlight, because macOS has no single-key
  clipboard history.
* Bolt/Unifying receivers are supported in code but untested. Testing was done over
  Bluetooth LE.

MIT, © 2026 BazinFla (upstream) and contributors. Not affiliated with Logitech.
