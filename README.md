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
# from the project root (this repository's root, or boring-mac/ in the monorepo)
./install.sh                    # builds, installs ~/.local/bin/bo-ring and assets
~/.local/bin/bo-ring --daemon   # first run: allow Accessibility + Input Monitoring
~/.local/bin/bo-ring            # the configurator (enable the LaunchAgent under Settings)
```

Permissions attach to the binary that asks for them. If you run it from a terminal,
macOS attributes them to the terminal app instead.

### Upgrading or reinstalling: remove and re-add the permissions

**Every time you install a new build**, remove Bo-Ring from both privacy lists and add it
again. Switching the existing entry off and on is **not** enough.

1. Quit the configurator and stop the daemon:
   `launchctl bootout gui/$(id -u)/dev.bo-ring.daemon`.
2. Replace `/Applications/Bo-Ring.app` with the new build and open it.
3. In **System Settings → Privacy & Security → Accessibility**, select **Bo-Ring**, click
   **–** to remove it, then click **+**, choose `/Applications/Bo-Ring.app`, and turn it
   on.
4. Do the same in **Privacy & Security → Input Monitoring**.
5. Restart the daemon: `launchctl kickstart -k gui/$(id -u)/dev.bo-ring.daemon`
   (or open Bo-Ring again).

Why: macOS ties each grant to the app's **code signature**, not its name or path. These
builds are ad-hoc signed, so every build has a new signature. The old entry stays in the
list, and can still show as *on*, but macOS no longer applies it to the new binary.
Toggling it only re-enables the stale grant; removing it and adding the app again
records the new signature. Without this, the daemon logs `Input Monitoring permission
missing` (the mouse buttons do nothing) and `Could not create the mouse event tap`.
Builds signed with a stable identity (`BORING_SIGN_IDENTITY`) keep their grants across
updates.

## Usage

### Open the ring
* **MX Master 4:** press the **haptic thumb panel** (the pad under your thumb). It sends
  button code 278, which is mapped to *Action Ring* by default.
* **Any other mouse:** map a button to *Action Ring* in the configurator (see below).
* **Keyboard or scripts:** run `/Applications/Bo-Ring.app/Contents/MacOS/bo-ring --toggle`
  from any hotkey tool (Raycast, Alfred, Karabiner, BetterTouchTool, a Shortcuts *Run Shell
  Script* action…). Running it again closes the ring.

The ring opens centered on the pointer, on whichever display the pointer is on.

### Pick a slot
| Input | Effect |
|---|---|
| Move the pointer toward a slot, then **click** | run the slot (or open its submenu) |
| **Scroll wheel** | cycle through the slots (Ring tab → *wheel navigation*) |
| **Middle click** | run the highlighted slot |
| **Right click** / **Esc** | leave a submenu / close the ring |
| Click outside the ring, or press the ring button again | close the ring |

**Trigger modes** (Ring tab → *hold* checkbox):
* **Hybrid** (default, checkbox on): a quick press keeps the ring open so you can click a
  slot; press, hold, hover a slot and release to run it immediately.
* **Click** (checkbox off): click to open, click a slot to run it, click the center to
  close.
* **Hold to release:** hold the button, hover a slot, release to run it. Set
  `trigger_mode = "HoldToRelease"` in `config.toml`; the UI doesn't offer this mode.

### Per-app rings
The ring follows the frontmost app: Safari, Chrome, Firefox, Brave, Arc, Edge, VS Code,
Cursor, Zed, Finder, Blender and GIMP have their own rings, and everything else gets the
default ring. Apps are matched by **bundle id** (e.g. `com.apple.Safari`). Add another app
with the **Ring** tab → app profiles, and put its bundle id in *App ID*. To find a bundle
id, run `osascript -e 'id of app "Notes"'`.

### Switch rings from the menu bar
The daemon puts a ring icon in the menu bar. Its **Ring to show** menu sets which ring
opens:
* **Follow Frontmost App** (default): the per-app ring for the active app, if one matches.
  The icon is a hollow ring.
* **Always Default Ring**: ignore the per-app rings. The icon is filled and labelled
  *Default*.
* **Always: <profile>**: pin one ring, e.g. *Web Browsers*, wherever you are. The icon
  shows the profile's name.

The menu also has **Open Ring**, **Settings…** and **Show Log**. The choice applies from
the next ring you open and is stored in `~/.config/bo-ring/ring-mode`, so saving in the
configurator never resets it. To script it:

```bash
bo-ring --ring-mode               # show the current mode
bo-ring --ring-mode default       # always the default ring
bo-ring --ring-mode "web browsers"   # pin a profile, by label or app id ("vscode", "com.apple.Safari")
bo-ring --ring-mode auto          # follow the frontmost app again
```

To hide the icon, set `menu_bar_icon = false` under `[general]` in `config.toml`.

### Remap mouse buttons
In the **Buttons** tab, click a button on the mouse picture, or press the physical button to
capture it, then choose an action:
* **Action Ring**: open the ring.
* **Keyboard shortcut**: e.g. `CMD` + `SHIFT` + `4`. Use `CMD`, `CTRL`, `ALT` (Option),
  `SHIFT`, `fn`. Media keys: `PlayPause`, `NextSong`, `VolumeUp`, `Mute`…
* **Command**: any shell command, e.g. `open -a Notes` or `open https://github.com`.
* **Folder**: open a folder in Finder.

Remapped buttons are swallowed, so the frontmost app never sees the original click.
Unmapped buttons, and left and right clicks, always pass through unchanged. On the
MX Master 4 the haptic panel (278), gesture button (281) and SmartShift (280) are captured
over HID++; the back, forward and middle buttons (275 / 276 / 274) through the event tap.

**Click Save** to apply. The background daemon restarts with the new configuration.

### Configurator tabs
| Tab | What it does |
|---|---|
| **Buttons** | map physical buttons to actions; see the detected mouse and its battery level |
| **Ring** | edit ring slots, icons (emoji or a PNG/WebP image), colors, submenus, per-app rings, trigger mode and wheel navigation |
| **Settings** | language; **Background Service** (LaunchAgent), *open at login*, an Applications launcher, the **privacy permission** status, import/export config |
| **Debug** | full diagnostics: frontmost-app detection and the Accessibility / Input Monitoring status |

### Command line
```bash
B=/Applications/Bo-Ring.app/Contents/MacOS/bo-ring
$B                  # configurator (also starts the daemon if it isn't running)
$B --daemon         # run the daemon in the foreground (logs to the terminal)
$B --toggle         # open / close the ring through the running daemon
$B --ring           # show the ring overlay on its own (no daemon needed)
$B --ring-mode default   # which ring to show: auto | default | <profile>
$B --install-agent  # install + start the daemon LaunchAgent
$B --uninstall      # remove LaunchAgents, config and logs (asks first)
```

### Files
| Path | Contents |
|---|---|
| `~/.config/bo-ring/rings/default.ring.toml` | **the main ring**: slots, submenus, actions. Edit this file by hand, not `config.toml` |
| `~/.config/bo-ring/rings/<app>.ring.toml` | per-app rings |
| `~/.config/bo-ring/ring-mode` | menu-bar ring choice: `auto`, `default` or `app:<app_id>` |
| `~/.config/bo-ring/config.toml` | general settings and **button mappings**. Its ring sections are a copy written on Save; nothing reads them back |
| `~/Library/LaunchAgents/dev.bo-ring.daemon.plist` | daemon autostart |
| `~/Library/Logs/bo-ring/daemon.log` | daemon log (button presses, HID++ diversion, permission warnings) |

### Troubleshooting
* **Nothing happens when I press the button.** Look at `daemon.log`. You should see
  `Diverted CID 0x01A0` when the daemon starts and `[HID++ Press]` for each press. If not,
  re-grant **Input Monitoring** and **Accessibility** to Bo-Ring (remove it with **–**, add
  it with **+**), then restart the daemon with
  `launchctl kickstart -k gui/$(id -u)/dev.bo-ring.daemon`. Every new ad-hoc build has a
  new signature, so macOS ignores permissions granted to an earlier build even when the
  switch still shows on.
* **The ring opens, but shortcuts don't reach the app.** That needs **Accessibility**.
* **A slot runs the wrong action, or an edit has no effect.** Edit
  `rings/default.ring.toml`, not `config.toml`. Close the configurator first, or its next
  Save writes its in-memory copy back. Only `*.toml` files in `rings/` are loaded, so keep
  backups elsewhere.
* **The side buttons aren't remapped.** The event tap needs **Accessibility**. A log that
  repeats `Could not create the mouse event tap` means it's missing.

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
