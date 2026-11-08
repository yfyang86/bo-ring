//! macOS privacy permissions Bo-Ring needs.
//!
//! * **Accessibility** — the daemon's `CGEventTap` that captures/absorbs extra
//!   mouse buttons, and posting synthetic key events (replaces uinput).
//! * **Input Monitoring** — opening the mouse's HID++ interface through IOKit
//!   (replaces /dev/hidraw access).
//!
//! Grants attach to the binary's code signature (or, when run from a terminal,
//! to the terminal app).

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrustedWithOptions(options: core_foundation::dictionary::CFDictionaryRef) -> bool;
    static kAXTrustedCheckOptionPrompt: core_foundation::string::CFStringRef;
}

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOHIDCheckAccess(request: u32) -> u32;
    fn IOHIDRequestAccess(request: u32) -> bool;
}

const IOHID_REQUEST_TYPE_LISTEN_EVENT: u32 = 1;
const IOHID_ACCESS_TYPE_GRANTED: u32 = 0;

pub fn accessibility_granted() -> bool {
    super::input_emitter::check_post_access()
}

/// Shows the system "allow Accessibility" prompt if not yet trusted.
pub fn prompt_accessibility() -> bool {
    use core_foundation::base::TCFType;
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::string::CFString;
    unsafe {
        let key = CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt);
        let dict = CFDictionary::from_CFType_pairs(&[(key, CFBoolean::true_value())]);
        AXIsProcessTrustedWithOptions(dict.as_concrete_TypeRef())
    }
}

pub fn input_monitoring_granted() -> bool {
    unsafe { IOHIDCheckAccess(IOHID_REQUEST_TYPE_LISTEN_EVENT) == IOHID_ACCESS_TYPE_GRANTED }
}

pub fn request_input_monitoring() -> bool {
    unsafe { IOHIDRequestAccess(IOHID_REQUEST_TYPE_LISTEN_EVENT) }
}

pub fn open_accessibility_settings() {
    let _ = std::process::Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
        .spawn();
}

pub fn open_input_monitoring_settings() {
    let _ = std::process::Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent")
        .spawn();
}

/// (all granted, human-readable status) — shown in the Settings tab.
pub fn status_summary(lang: &str) -> (bool, String) {
    let ax = accessibility_granted();
    let im = input_monitoring_granted();
    let t = |k: &str| crate::i18n::tr(lang, k).to_string();
    let s = format!(
        "{}: {} · {}: {}",
        t("perm_accessibility"),
        if ax { "✔" } else { "✘" },
        t("perm_input_monitoring"),
        if im { "✔" } else { "✘" }
    );
    (ax && im, s)
}

/// Prompt for whatever is missing and open the relevant System Settings pane.
pub fn request_missing() -> Result<String, String> {
    let mut opened = false;
    if !input_monitoring_granted() {
        request_input_monitoring();
        open_input_monitoring_settings();
        opened = true;
    }
    if !accessibility_granted() {
        prompt_accessibility();
        if !opened {
            open_accessibility_settings();
        }
        opened = true;
    }
    if opened {
        Ok("🔐 Enable Bo-Ring in System Settings → Privacy & Security, then restart the daemon.".into())
    } else {
        Ok("✅ All permissions granted.".into())
    }
}
