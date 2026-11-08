pub mod codes;
pub mod hidpp;
pub mod model;
pub mod registry;

pub use codes::*;
pub use hidpp::*;
pub use model::*;
pub use registry::*;

use eframe::egui;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;

use crate::i18n::tr;

#[derive(Clone, Debug, PartialEq)]
pub enum ConnectionType {
    UsbReceiver,
    Bluetooth,
    UsbCable,
}

#[derive(Clone, Debug)]
pub struct MouseDeviceInfo {
    pub name: String,
    pub connection_type: ConnectionType,
    pub battery_level: u8,
    pub is_charging: bool,
    pub channel_count: usize,
    pub detected_profile_id: Option<String>,
    pub vid_pid: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MouseInputEvent {
    pub code: u16,
    pub raw_code: u16,
    pub dev_name: Option<String>,
    pub vid_pid: Option<String>,
}

static LAST_KNOWN_BATTERY_LEVEL: AtomicU8 = AtomicU8::new(60);

/// macOS: is a Logitech mouse attached over a USB cable (not a receiver)?
pub fn check_usb_direct_mouse_cable_connected() -> bool {
    enumerate_hid_mice().iter().any(|m| m.bus == "usb" && !m.name.to_lowercase().contains("receiver") && m.vendor == 0x046d)
}

/// Battery via HID++ (UNIFIED_BATTERY 0x1004 / BATTERY_STATUS 0x1000).
pub fn query_hidpp_battery_status() -> Option<(u8, bool)> {
    hidpp::query_battery()
}

pub fn scan_battery_level() -> (u8, bool) {
    if let Some((level, charging)) = query_hidpp_battery_status() {
        LAST_KNOWN_BATTERY_LEVEL.store(level, Ordering::Relaxed);
        return (level, charging || check_usb_direct_mouse_cable_connected());
    }
    (LAST_KNOWN_BATTERY_LEVEL.load(Ordering::Relaxed), check_usb_direct_mouse_cable_connected())
}

#[derive(Clone, Debug)]
pub struct HidMouse {
    pub path: PathBuf,
    pub name: String,
    pub vendor: u16,
    pub product: u16,
    /// "bluetooth" | "usb" | "other"
    pub bus: &'static str,
}

/// Pointer devices (HID usage page 1 / usage 2) known to IOKit.
pub fn enumerate_hid_mice() -> Vec<HidMouse> {
    let Ok(api) = hidapi_snapshot() else { return Vec::new() };
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for d in api {
        if d.0 != 0x0001 || d.1 != 0x0002 {
            continue;
        }
        if !seen.insert(d.2.clone()) {
            continue;
        }
        out.push(HidMouse { path: PathBuf::from(&d.2), name: d.3, vendor: d.4, product: d.5, bus: d.6 });
    }
    out
}

/// (usage_page, usage, path, product, vid, pid, bus) for every HID interface.
fn hidapi_snapshot() -> Result<Vec<(u16, u16, String, String, u16, u16, &'static str)>, ()> {
    let api = hidapi::HidApi::new().map_err(|_| ())?;
    Ok(api
        .device_list()
        .map(|d| {
            let bus = match d.bus_type() {
                hidapi::BusType::Bluetooth => "bluetooth",
                hidapi::BusType::Usb => "usb",
                _ => "other",
            };
            (
                d.usage_page(),
                d.usage(),
                d.path().to_string_lossy().to_string(),
                d.product_string().unwrap_or("Unknown device").to_string(),
                d.vendor_id(),
                d.product_id(),
                bus,
            )
        })
        .collect())
}

pub fn enumerate_input_device_names() -> Vec<(PathBuf, String, Option<String>, bool)> {
    enumerate_hid_mice()
        .into_iter()
        .map(|m| {
            let vp = Some(format!("{:04x}:{:04x}", m.vendor, m.product));
            (m.path, m.name, vp, true)
        })
        .collect()
}

pub fn scan_mouse_device() -> Option<MouseDeviceInfo> {
    let mice: Vec<HidMouse> = enumerate_hid_mice()
        .into_iter()
        .filter(|m| {
            let n = m.name.to_lowercase();
            !n.contains("keyboard") && !n.contains("trackpad")
        })
        .collect();
    if mice.is_empty() {
        return None;
    }
    // Prefer a Logitech device, then any named mouse.
    let best = mice.iter().find(|m| m.vendor == 0x046d).unwrap_or(&mice[0]).clone();
    let vid_pid = Some(format!("{:04x}:{:04x}", best.vendor, best.product));
    let connection_type = if best.name.to_lowercase().contains("receiver") {
        ConnectionType::UsbReceiver
    } else if best.bus == "bluetooth" {
        ConnectionType::Bluetooth
    } else if best.bus == "usb" {
        ConnectionType::UsbCable
    } else {
        ConnectionType::Bluetooth
    };
    let (battery_level, is_charging) = scan_battery_level();
    let detected_profile_id = DeviceRegistry::new()
        .match_device(&best.name, vid_pid.as_deref())
        .map(|p| p.config.id.clone());
    Some(MouseDeviceInfo {
        name: best.name,
        connection_type,
        battery_level,
        is_charging,
        channel_count: mice.len(),
        detected_profile_id,
        vid_pid,
    })
}

pub fn attach_new_mouse_listeners(
    ctx: egui::Context,
    tx: mpsc::Sender<MouseInputEvent>,
    listened_paths: Arc<Mutex<HashSet<PathBuf>>>,
) {
    let sock_path = crate::platform::ipc::daemon_socket_path();
    let tx_ipc = tx.clone();
    let ctx_ipc = ctx.clone();

    // 1. Preferred: events relayed by the running daemon over its Unix socket.
    let daemon_up = std::os::unix::net::UnixStream::connect(&sock_path).is_ok();
    thread::spawn(move || {
        use std::io::{BufRead, BufReader};
        use std::os::unix::net::UnixStream;
        use crate::platform::ipc::DaemonIpcEvent;

        if let Ok(stream) = UnixStream::connect(&sock_path) {
            println!("📡 Config UI connected to daemon IPC socket at {:?}", sock_path);
            let reader = BufReader::new(stream);
            for line in reader.lines().map_while(Result::ok) {
                if let Some(event) = DaemonIpcEvent::parse_line(&line) {
                    let (code_opt, raw_opt, vid_pid, dev_name) = match event {
                        DaemonIpcEvent::ButtonPressed { raw_code, normalized_code, vid_pid, dev_name } => {
                            (Some(normalized_code), Some(raw_code), vid_pid, dev_name)
                        }
                        DaemonIpcEvent::ButtonCode(code) => (Some(code), Some(code), None, None),
                        _ => (None, None, None, None),
                    };
                    if let (Some(code), Some(raw_code)) = (code_opt, raw_opt) {
                        if is_supported_button_code(raw_code) || is_supported_button_code(code) {
                            let _ = tx_ipc.send(MouseInputEvent { code, raw_code, dev_name, vid_pid });
                            ctx_ipc.request_repaint();
                        }
                    }
                }
            }
        }
    });

    // The daemon already owns the tap and the HID++ diversion: don't compete.
    if daemon_up {
        return;
    }

    // 2. Fallback: an observe-only event tap for the standard extra buttons.
    {
        let mut lock = listened_paths.lock().unwrap();
        if lock.insert(PathBuf::from("cgeventtap")) {
            let (btx, brx) = mpsc::channel::<crate::platform::event_tap::ButtonEvent>();
            crate::platform::event_tap::start_listen_only_tap(btx);
            let tx_tap = tx.clone();
            let ctx_tap = ctx.clone();
            let name = scan_mouse_device().map(|m| (m.name, m.vid_pid));
            thread::spawn(move || {
                for ev in brx {
                    if ev.value == 1 && is_supported_button_code(ev.code) {
                        let _ = tx_tap.send(MouseInputEvent {
                            code: normalize_button_code(ev.code),
                            raw_code: ev.code,
                            dev_name: name.as_ref().map(|n| n.0.clone()),
                            vid_pid: name.as_ref().and_then(|n| n.1.clone()),
                        });
                        ctx_tap.request_repaint();
                    }
                }
            });
        }
    }

    // 3. Fallback: direct HID++ diversion for Logitech special controls.
    let diverted_hidpp_paths = Arc::new(Mutex::new(HashSet::new()));
    hidpp::attach_hidpp_gui_listener(ctx, tx, diverted_hidpp_paths);
}

pub fn normalize_button_code(code: u16) -> u16 {
    if code == BTN_MIDDLE_CODE {
        BTN_MIDDLE_CODE
    } else if code == BTN_FORWARD_CODE || BTN_FORWARD_BT_ALIASES.contains(&code) {
        BTN_FORWARD_CODE
    } else if code == BTN_BACK_CODE || BTN_BACK_BT_ALIASES.contains(&code) {
        BTN_BACK_CODE
    } else if code == BTN_GESTURES_CODE || BTN_GESTURES_BT_ALIASES.contains(&code) {
        BTN_GESTURES_CODE
    } else if code == BTN_RING_CODE || BTN_RING_BT_ALIASES.contains(&code) {
        BTN_RING_CODE
    } else if code == BTN_SMARTSHIFT_CODE {
        BTN_SMARTSHIFT_CODE
    } else if code == BTN_HAPTIC_CODE {
        BTN_HAPTIC_CODE
    } else {
        code
    }
}

pub fn is_supported_button_code(code: u16) -> bool {
    // Primary left and right clicks must NEVER be remapped or captured
    if code == BTN_LEFT_CODE || code == BTN_RIGHT_CODE {
        return false;
    }
    let norm = normalize_button_code(code);
    matches!(norm, BTN_RING_CODE | BTN_GESTURES_CODE | BTN_FORWARD_CODE | BTN_BACK_CODE | BTN_MIDDLE_CODE | BTN_SMARTSHIFT_CODE | BTN_HAPTIC_CODE)
        || (256..=350).contains(&code)
        || (704..=745).contains(&code)
        || BTN_BACK_BT_ALIASES.contains(&code)
        || BTN_FORWARD_BT_ALIASES.contains(&code)
        || BTN_GESTURES_BT_ALIASES.contains(&code)
        || BTN_RING_BT_ALIASES.contains(&code)
}

pub fn get_button_name_lang(code: u16, lang: &str) -> String {
    let norm = normalize_button_code(code);
    match norm {
        BTN_RING_CODE => tr(lang, "btn_thumb_ring").to_string(),
        BTN_GESTURES_CODE => tr(lang, "btn_gestures").to_string(),
        BTN_FORWARD_CODE => tr(lang, "btn_next").to_string(),
        BTN_BACK_CODE => tr(lang, "btn_back").to_string(),
        BTN_MIDDLE_CODE => tr(lang, "btn_middle_wheel").to_string(),
        BTN_SMARTSHIFT_CODE => tr(lang, "btn_smartshift").to_string(),
        BTN_HAPTIC_CODE => tr(lang, "btn_haptic").to_string(),
        _ => format!("{} {}", tr(lang, "button_code_prefix"), code),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_button_code() {
        assert_eq!(normalize_button_code(BTN_RING_CODE), BTN_RING_CODE);
        assert_eq!(normalize_button_code(172), BTN_RING_CODE);
        assert_eq!(normalize_button_code(149), BTN_RING_CODE);
        assert_eq!(normalize_button_code(257), BTN_RING_CODE);
        assert_eq!(normalize_button_code(999), BTN_RING_CODE);

        assert_eq!(normalize_button_code(BTN_GESTURES_CODE), BTN_GESTURES_CODE);
        assert_eq!(normalize_button_code(120), BTN_GESTURES_CODE);
        assert_eq!(normalize_button_code(171), BTN_GESTURES_CODE);
        assert_eq!(normalize_button_code(204), BTN_GESTURES_CODE);

        assert_eq!(normalize_button_code(BTN_FORWARD_CODE), BTN_FORWARD_CODE);
        assert_eq!(normalize_button_code(159), BTN_FORWARD_CODE);

        assert_eq!(normalize_button_code(BTN_BACK_CODE), BTN_BACK_CODE);
        assert_eq!(normalize_button_code(158), BTN_BACK_CODE);

        assert_eq!(normalize_button_code(BTN_MIDDLE_CODE), BTN_MIDDLE_CODE);
        assert_eq!(normalize_button_code(9999), 9999);
    }

    #[test]
    fn test_is_supported_button_code() {
        assert!(!is_supported_button_code(BTN_LEFT_CODE));
        assert!(!is_supported_button_code(BTN_RIGHT_CODE));
        assert!(is_supported_button_code(BTN_RING_CODE));
        assert!(is_supported_button_code(BTN_GESTURES_CODE));
        assert!(is_supported_button_code(BTN_FORWARD_CODE));
        assert!(is_supported_button_code(BTN_BACK_CODE));
        assert!(is_supported_button_code(BTN_MIDDLE_CODE));
        assert!(is_supported_button_code(172)); // BT alias for ring
        assert!(is_supported_button_code(704)); // BTN_TRIGGER_HAPPY
        assert!(!is_supported_button_code(100)); // KEY_RIGHTALT should not be matched
    }
}
