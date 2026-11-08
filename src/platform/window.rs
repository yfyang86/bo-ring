//! Active application detection on macOS — replaces the GNOME / KDE /
//! Hyprland / Sway / X11 strategies with `NSWorkspace.frontmostApplication`.
//!
//! The detected id is the app's **bundle identifier** (e.g.
//! `org.mozilla.firefox`, `com.microsoft.VSCode`). `matches_app_id` compares
//! the last dot component case-insensitively, so presets like `firefox` and
//! `vscode` keep matching; macOS-specific ids are added to the presets.

use objc2_app_kit::{NSRunningApplication, NSWorkspace};
use objc2_foundation::NSString;

/// Bundle id (falls back to the localized name) of the frontmost application.
pub fn detect_active_window() -> String {
    frontmost_app()
        .map(|(bundle, name, _)| if bundle.is_empty() { name } else { bundle })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Global / Desktop".to_string())
}

/// (bundle id, localized name, pid) of the frontmost application.
pub fn frontmost_app() -> Option<(String, String, i32)> {
    unsafe {
        let ws = NSWorkspace::sharedWorkspace();
        let app = ws.frontmostApplication()?;
        Some(describe(&app))
    }
}

fn describe(app: &NSRunningApplication) -> (String, String, i32) {
    unsafe {
        let bundle = app.bundleIdentifier().map(|s| s.to_string()).unwrap_or_default();
        let name = app.localizedName().map(|s| s.to_string()).unwrap_or_default();
        (bundle, name, app.processIdentifier())
    }
}

/// Human-readable name for a bundle id ("org.mozilla.firefox" → "Firefox").
pub fn display_name_for(app_id: &str) -> Option<String> {
    if !app_id.contains('.') {
        return None;
    }
    unsafe {
        let apps = NSRunningApplication::runningApplicationsWithBundleIdentifier(&NSString::from_str(app_id));
        if let Some(app) = apps.firstObject() {
            return app.localizedName().map(|s| s.to_string());
        }
    }
    None
}

/// Re-activate the application with this pid (restores the user's target app
/// after the ring overlay — a separate process — took focus).
pub fn activate_pid(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    unsafe {
        match NSRunningApplication::runningApplicationWithProcessIdentifier(pid) {
            Some(app) => {
                #[allow(deprecated)]
                app.activateWithOptions(objc2_app_kit::NSApplicationActivationOptions::NSApplicationActivateIgnoringOtherApps)
            }
            None => false,
        }
    }
}

/// On Linux/GNOME this focused the window under the cursor through a shell
/// extension. On macOS the daemon restores the previously frontmost app.
pub fn focus_window_under_cursor() -> bool {
    let pid = super::overlay::last_target_pid();
    activate_pid(pid)
}

/// Diagnostics shown in the Debug tab.
pub fn run_window_detection_diagnostics() -> String {
    let mut log = String::new();
    log.push_str("🔍 --- WINDOW DETECTION DIAGNOSTICS (macOS) ---\n\n");
    log.push_str("Strategy: NSWorkspace.frontmostApplication\n");
    match frontmost_app() {
        Some((bundle, name, pid)) => {
            log.push_str(&format!("• Bundle id: {bundle}\n• Name: {name}\n• PID: {pid}\n"));
        }
        None => log.push_str("• No frontmost application reported\n"),
    }
    log.push_str(&format!(
        "• Accessibility (post events / tap buttons): {}\n",
        if super::input_emitter::check_post_access() { "granted" } else { "MISSING" }
    ));
    log.push_str(&format!(
        "• Input Monitoring (HID++): {}\n",
        if super::permissions::input_monitoring_granted() { "granted" } else { "MISSING" }
    ));
    log.push_str(&format!("\n🎯 GLOBAL DETECTED RESULT: '{}'\n", detect_active_window()));
    log
}

/// Kept for API compatibility with the Linux Debug tab.
pub fn run_kde_diagnostics() -> String {
    "KDE diagnostics are not applicable on macOS.\n".to_string()
}

/// The Linux build offered a GNOME Shell extension here; the macOS Settings
/// card shows the privacy permissions instead (Accessibility + Input Monitoring).
pub fn check_gnome_extension_status_lang(lang: &str) -> (bool, String) {
    super::permissions::status_summary(lang)
}

pub fn install_or_update_gnome_extension_lang(_lang: &str) -> Result<String, String> {
    super::permissions::request_missing()
}
