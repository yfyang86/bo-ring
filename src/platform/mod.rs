//! macOS platform layer. Same module surface as the Linux build (so the
//! config UI, ring and daemon code are shared), different internals:
//!
//! | Linux                         | macOS (this port)                              |
//! |-------------------------------|------------------------------------------------|
//! | evdev grab + uinput passthrough | `CGEventTap` (absorb remapped buttons) — `event_tap` |
//! | uinput key injection          | `CGEvent` posting — `input_emitter`            |
//! | /dev/hidraw HID++             | IOKit via hidapi — `devices::hidpp`            |
//! | GNOME/KDE/Hyprland/Sway/X11   | `NSWorkspace.frontmostApplication` — `window`  |
//! | systemd / XDG autostart       | launchd LaunchAgents — `autostart`             |
//! | udev rules                    | Accessibility + Input Monitoring — `permissions` |
pub mod accent_color;
pub mod autostart;
pub mod event_tap;
pub mod input_emitter;
pub mod ipc;
pub mod menubar;
pub mod overlay;
pub mod permissions;
pub mod uninstall;
pub mod window;
