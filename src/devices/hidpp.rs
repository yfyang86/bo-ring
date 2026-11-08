//! Native Logitech HID++ 2.0 button diversion on macOS (IOKit via hidapi).
//!
//! Port of the Linux `/dev/hidraw*` engine. Differences on macOS:
//! * devices are found through hidapi's IOKit backend by vendor (046d) and the
//!   HID++ vendor collection (usage page 0xFF43 over Bluetooth LE, 0xFF00
//!   usage 0x0002 on Bolt/Unifying receivers);
//! * Bluetooth LE only carries **long** reports (0x11), device index 0xFF;
//! * devices are opened in *shared* mode (never seize the mouse);
//! * diverted controls are **restored on shutdown** (`undivert_all`), so a
//!   stopped daemon never leaves a dead button behind.
//!
//! Needs the Input Monitoring permission.

use hidapi::{HidApi, HidDevice};
use std::collections::{HashMap, HashSet};
use std::ffi::CString;
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::devices::codes::*;

pub const REPORT_ID_SHORT: u8 = 0x10;
pub const REPORT_ID_LONG: u8 = 0x11;
pub const DEV_INDEX_WIRELESS_OR_BT: u8 = 0xFF;
/// Software IDs rotate per request (0x2..=0xF). On macOS every open handle to a
/// HID device receives *every* input report, so concurrent requesters (e.g. a
/// battery poll while discovery runs) would otherwise steal each other's ROOT
/// replies, which do not echo the queried feature id. 0x0 is reserved for
/// notifications; 0x1 is left for other tools.
static NEXT_SW_ID: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0x2);

fn next_sw_id() -> u8 {
    use std::sync::atomic::Ordering;
    let mut cur = NEXT_SW_ID.load(Ordering::Relaxed);
    loop {
        let next = if cur >= 0xF { 0x2 } else { cur + 1 };
        match NEXT_SW_ID.compare_exchange_weak(cur, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return cur,
            Err(actual) => cur = actual,
        }
    }
}

pub const FEATURE_ROOT: u16 = 0x0000;
pub const FEATURE_UNIFIED_BATTERY: u16 = 0x1004;
pub const FEATURE_BATTERY_STATUS: u16 = 0x1000;
pub const FEATURE_REPROG_CONTROLS_V4: u16 = 0x1B04;
pub const FEATURE_REPROG_CONTROLS: u16 = 0x1B00;

/// Physical Control IDs (CIDs) on Logitech devices
pub const CID_GESTURE_BUTTON: u16 = 0x00C3; // Thumb rest gesture button (MX Master series)
pub const CID_SMARTSHIFT_BUTTON: u16 = 0x00C4; // Mode-shift / top button behind wheel
pub const CID_APP_SWITCH_BUTTON: u16 = 0x00D0; // Hidden thumb gesture button (M720 Triathlon, M590)
pub const CID_DPI_SWITCH_BUTTON: u16 = 0x00FD; // Top DPI switch button (MX Vertical, Lift)
pub const CID_PRECISION_BUTTON: u16 = 0x00ED; // Trackball precision mode button (MX Ergo)
pub const CID_HAPTIC_PANEL: u16 = 0x01A0; // MX Master 4 haptic "Actions Ring" thumb panel

/// setCidReporting flags (HID++ 0x1B04): bit0 divert, bit1 divert-valid.
/// Verified on an MX Master 4 over BLE by reading back with getCidReporting.
pub const FLAG_DIVERT_ON: u8 = 0x03;
pub const FLAG_DIVERT_OFF: u8 = 0x02;

const USAGE_PAGE_HIDPP_BLE: u16 = 0xFF43;
const USAGE_PAGE_HIDPP_RECEIVER: u16 = 0xFF00;

#[derive(Clone, Debug)]
pub struct LogitechHidDevice {
    pub path: PathBuf,
    pub name: String,
    pub vid_pid: Option<String>,
    pub dev_index: u8,
    pub reprog_feature_index: u8,
    pub is_bluetooth: bool,
}

// hidapi allows a single HidApi instance per process.
static HIDAPI: Mutex<Option<HidApi>> = Mutex::new(None);

fn with_api<T>(f: impl FnOnce(&mut HidApi) -> T) -> Option<T> {
    let mut guard = HIDAPI.lock().ok()?;
    if guard.is_none() {
        match HidApi::new() {
            Ok(api) => *guard = Some(api),
            Err(e) => {
                eprintln!("⚠️ hidapi init failed: {e}");
                return None;
            }
        }
    } else if let Some(api) = guard.as_mut() {
        let _ = api.refresh_devices();
    }
    guard.as_mut().map(f)
}

fn open_path(path: &PathBuf) -> Option<HidDevice> {
    let c = CString::new(path.to_string_lossy().as_bytes()).ok()?;
    with_api(|api| api.open_path(&c).ok()).flatten()
}

/// Build a long HID++ request with software id `sw_id`.
pub fn long_request_sw(dev_idx: u8, feature_idx: u8, function: u8, params: &[u8], sw_id: u8) -> [u8; 20] {
    let mut req = [0u8; 20];
    req[0] = REPORT_ID_LONG;
    req[1] = dev_idx;
    req[2] = feature_idx;
    req[3] = ((function & 0x0F) << 4) | (sw_id & 0x0F);
    for (i, b) in params.iter().take(16).enumerate() {
        req[4 + i] = *b;
    }
    req
}

/// Build a long HID++ request (software id 0xE, for fire-and-forget writes).
pub fn long_request(dev_idx: u8, feature_idx: u8, function: u8, params: &[u8]) -> [u8; 20] {
    long_request_sw(dev_idx, feature_idx, function, params, 0x0E)
}

/// Drain any queued input reports.
pub fn flush(dev: &HidDevice) {
    let mut buf = [0u8; 64];
    while let Ok(n) = dev.read_timeout(&mut buf, 0) {
        if n == 0 {
            break;
        }
    }
}

/// Synchronous request/response (matching device, feature and function/sw-id).
pub fn call(dev: &HidDevice, dev_idx: u8, feature_idx: u8, function: u8, params: &[u8], timeout_ms: u64) -> Option<[u8; 20]> {
    let req = long_request_sw(dev_idx, feature_idx, function, params, next_sw_id());
    dev.write(&req).ok()?;
    let start = Instant::now();
    let mut resp = [0u8; 64];
    while start.elapsed() < Duration::from_millis(timeout_ms) {
        let left = timeout_ms.saturating_sub(start.elapsed().as_millis() as u64).max(1) as i32;
        match dev.read_timeout(&mut resp, left) {
            Ok(n) if n >= 4 => {
                if (resp[0] == REPORT_ID_LONG || resp[0] == REPORT_ID_SHORT) && resp[1] == dev_idx {
                    // HID++ 2.0 error: [id, dev, 0xFF, feat, func, code]
                    if resp[2] == 0xFF && resp[3] == feature_idx && resp[4] == req[3] {
                        return None;
                    }
                    if resp[2] == feature_idx && resp[3] == req[3] {
                        let mut out = [0u8; 20];
                        out[..n.min(20)].copy_from_slice(&resp[..n.min(20)]);
                        return Some(out);
                    }
                }
            }
            Ok(_) => {}
            Err(_) => return None,
        }
    }
    None
}

/// ROOT.getFeature → runtime feature index.
pub fn query_feature_index(dev: &HidDevice, dev_idx: u8, feature_id: u16) -> Option<u8> {
    let r = call(dev, dev_idx, 0x00, 0x0, &[(feature_id >> 8) as u8, (feature_id & 0xFF) as u8], 300)?;
    (r[4] != 0).then_some(r[4])
}

pub fn is_cid_diverted(dev: &HidDevice, dev_idx: u8, reprog_feature_index: u8, cid: u16) -> bool {
    call(dev, dev_idx, reprog_feature_index, 0x2, &[(cid >> 8) as u8, (cid & 0xFF) as u8], 300)
        .map(|r| r[6] & 0x01 != 0)
        .unwrap_or(false)
}

pub fn set_cid_diverted(dev: &HidDevice, dev_idx: u8, reprog_feature_index: u8, cid: u16, divert: bool) -> bool {
    let flags = if divert { FLAG_DIVERT_ON } else { FLAG_DIVERT_OFF };
    let params = [(cid >> 8) as u8, (cid & 0xFF) as u8, flags, 0, 0];
    if call(dev, dev_idx, reprog_feature_index, 0x3, &params, 400).is_some() {
        return true;
    }
    divert && is_cid_diverted(dev, dev_idx, reprog_feature_index, cid)
}

/// Battery (level %, charging) via UNIFIED_BATTERY (0x1004), else BATTERY_STATUS (0x1000).
pub fn battery_status(dev: &HidDevice, dev_idx: u8) -> Option<(u8, bool)> {
    if let Some(idx) = query_feature_index(dev, dev_idx, FEATURE_UNIFIED_BATTERY) {
        // get_status (fn 1): [soc %, level flags, charging status]
        let r = call(dev, dev_idx, idx, 0x1, &[], 400)?;
        let charging = matches!(r[6], 1 | 2 | 3);
        if r[4] > 0 && r[4] <= 100 {
            return Some((r[4], charging));
        }
    }
    if let Some(idx) = query_feature_index(dev, dev_idx, FEATURE_BATTERY_STATUS) {
        let r = call(dev, dev_idx, idx, 0x0, &[], 400)?;
        let charging = matches!(r[6], 1 | 2 | 3);
        if r[4] > 0 && r[4] <= 100 {
            return Some((r[4], charging));
        }
    }
    None
}

/// Candidate HID++ interfaces: (path, product name, vid:pid, is_bluetooth, is_receiver).
fn hidpp_interfaces() -> Vec<(PathBuf, String, String, bool, bool)> {
    with_api(|api| {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for d in api.device_list() {
            if d.vendor_id() != 0x046D {
                continue;
            }
            let ble = d.usage_page() == USAGE_PAGE_HIDPP_BLE;
            let receiver_long = d.usage_page() == USAGE_PAGE_HIDPP_RECEIVER && d.usage() == 0x0002;
            if !ble && !receiver_long {
                continue;
            }
            let path = PathBuf::from(d.path().to_string_lossy().to_string());
            if !seen.insert(path.clone()) {
                continue;
            }
            let name = d.product_string().unwrap_or("Logitech Device").to_string();
            if name.to_lowercase().contains("keyboard") {
                continue;
            }
            let vid_pid = format!("{:04x}:{:04x}", d.vendor_id(), d.product_id());
            let is_bt = matches!(d.bus_type(), hidapi::BusType::Bluetooth) || ble;
            out.push((path, name, vid_pid, is_bt, receiver_long));
        }
        out
    })
    .unwrap_or_default()
}

/// Discovers connected Logitech mice that support reprogrammable controls,
/// skipping already-monitored paths.
pub fn discover_logitech_mice(ignored_paths: &HashSet<PathBuf>) -> Vec<LogitechHidDevice> {
    let mut devices = Vec::new();
    for (path, name, vid_pid, is_bt, is_receiver) in hidpp_interfaces() {
        if ignored_paths.contains(&path) {
            continue;
        }
        let Some(dev) = open_path(&path) else { continue };
        let candidates: Vec<u8> = if is_receiver { vec![1, 2, 3, 4, 5, 6] } else { vec![DEV_INDEX_WIRELESS_OR_BT] };
        for dev_idx in candidates {
            flush(&dev);
            let feat = query_feature_index(&dev, dev_idx, FEATURE_REPROG_CONTROLS_V4)
                .or_else(|| query_feature_index(&dev, dev_idx, FEATURE_REPROG_CONTROLS));
            if let Some(feat_idx) = feat {
                devices.push(LogitechHidDevice {
                    path: path.clone(),
                    name: name.clone(),
                    vid_pid: Some(vid_pid.clone()),
                    dev_index: dev_idx,
                    reprog_feature_index: feat_idx,
                    is_bluetooth: is_bt,
                });
                break;
            }
        }
    }
    devices
}

/// Maps a Logitech Control ID (CID) to the standard Bo-Ring button code.
pub fn cid_to_button_code(cid: u16) -> u16 {
    match cid {
        CID_GESTURE_BUTTON => BTN_RING_CODE,
        CID_SMARTSHIFT_BUTTON => BTN_SMARTSHIFT_CODE,
        CID_APP_SWITCH_BUTTON => BTN_RING_CODE,
        CID_DPI_SWITCH_BUTTON => BTN_SMARTSHIFT_CODE,
        CID_PRECISION_BUTTON => BTN_GESTURES_CODE,
        CID_HAPTIC_PANEL => BTN_RING_CODE,
        0x0053 => BTN_BACK_CODE,
        0x0056 => BTN_FORWARD_CODE,
        0x0052 => BTN_MIDDLE_CODE,
        other => other,
    }
}

/// (CID, button code) pairs to divert for a mouse, from its device profile.
pub fn resolve_cids_for_mouse(dev_name: &str, vid_pid: Option<&str>) -> Vec<(u16, u16)> {
    let registry = crate::devices::DeviceRegistry::new();
    let mut cids = registry
        .match_device(dev_name, vid_pid)
        .map(|p| p.config.diverted_cids())
        .unwrap_or_default();
    if cids.is_empty() {
        cids.push((CID_GESTURE_BUTTON, BTN_RING_CODE));
    }
    cids
}

/// Parse the pressed CIDs of a divertedButtonsEvent for our device/feature.
pub fn parse_diverted_buttons(buf: &[u8], dev_idx: u8, feat_idx: u8) -> Option<HashSet<u16>> {
    if buf.len() < 12 || buf[0] != REPORT_ID_LONG || buf[1] != dev_idx || buf[2] != feat_idx || buf[3] != 0x00 {
        return None;
    }
    let mut set = HashSet::new();
    for i in 0..4 {
        let o = 4 + i * 2;
        let cid = ((buf[o] as u16) << 8) | buf[o + 1] as u16;
        if cid != 0 {
            set.insert(cid);
        }
    }
    Some(set)
}

// Everything we diverted, so it can be restored on shutdown.
static DIVERTED: Mutex<Vec<(PathBuf, u8, u8, Vec<u16>)>> = Mutex::new(Vec::new());

/// Restore every control this process diverted (call on shutdown).
pub fn undivert_all() {
    let entries = std::mem::take(&mut *DIVERTED.lock().unwrap());
    for (path, dev_idx, feat, cids) in entries {
        if let Some(dev) = open_path(&path) {
            for cid in cids {
                if set_cid_diverted(&dev, dev_idx, feat, cid, false) {
                    println!("↩️ Restored CID 0x{:04X} on {:?}", cid, path);
                }
            }
        }
    }
}

/// A physical press/release from a diverted control.
#[derive(Debug, Clone)]
pub struct HidppButtonEvent {
    pub dev_name: String,
    pub vid_pid: Option<String>,
    pub code: u16,
    pub value: i32,
}

fn run_device_loop(
    mouse: LogitechHidDevice,
    tracker: Arc<Mutex<HashSet<PathBuf>>>,
    mut on_event: impl FnMut(u16, u16, i32),
    verbose: bool,
) {
    let Some(dev) = open_path(&mouse.path) else {
        tracker.lock().unwrap().remove(&mouse.path);
        return;
    };
    let cids = resolve_cids_for_mouse(&mouse.name, mouse.vid_pid.as_deref());
    let mut diverted = Vec::new();
    for &(cid, code) in &cids {
        flush(&dev);
        if set_cid_diverted(&dev, mouse.dev_index, mouse.reprog_feature_index, cid, true) {
            diverted.push(cid);
            if verbose {
                println!("🎯 Diverted CID 0x{:04X} -> Button Code {} on {}", cid, code, mouse.name);
            }
        } else if verbose {
            println!("⚠️ No diversion ACK for CID 0x{:04X} on {} (not present?)", cid, mouse.name);
        }
    }
    if !diverted.is_empty() {
        let mut lock = DIVERTED.lock().unwrap();
        lock.retain(|e| e.0 != mouse.path);
        lock.push((mouse.path.clone(), mouse.dev_index, mouse.reprog_feature_index, diverted));
    }
    let cid_map: HashMap<u16, u16> = cids.into_iter().collect();
    let mut previous: HashSet<u16> = HashSet::new();
    let mut buf = [0u8; 64];
    loop {
        match dev.read_timeout(&mut buf, 500) {
            Ok(0) => {}
            Ok(n) => {
                if let Some(current) = parse_diverted_buttons(&buf[..n], mouse.dev_index, mouse.reprog_feature_index) {
                    for &cid in current.difference(&previous) {
                        let code = cid_map.get(&cid).copied().unwrap_or_else(|| cid_to_button_code(cid));
                        on_event(cid, code, 1);
                    }
                    for &cid in previous.difference(&current) {
                        let code = cid_map.get(&cid).copied().unwrap_or_else(|| cid_to_button_code(cid));
                        on_event(cid, code, 0);
                    }
                    previous = current;
                }
            }
            Err(e) => {
                eprintln!("🔌 HID++ device disconnected / read error {:?}: {}", mouse.path, e);
                break;
            }
        }
    }
    tracker.lock().unwrap().remove(&mouse.path);
    DIVERTED.lock().unwrap().retain(|e| e.0 != mouse.path);
}

/// Daemon: discover Logitech mice, divert their profile CIDs, stream events.
pub fn start_hidpp_manager(tx: mpsc::Sender<HidppButtonEvent>, diverted_devices: Arc<Mutex<HashSet<PathBuf>>>) {
    thread::spawn(move || loop {
        let ignored = diverted_devices.lock().unwrap().clone();
        for mouse in discover_logitech_mice(&ignored) {
            if !diverted_devices.lock().unwrap().insert(mouse.path.clone()) {
                continue;
            }
            println!(
                "✨ Logitech HID++ 2.0 device: {} ({:?}, {}, DevIdx 0x{:02X}, ReprogFeat 0x{:02X})",
                mouse.name,
                mouse.vid_pid,
                if mouse.is_bluetooth { "Bluetooth" } else { "receiver/USB" },
                mouse.dev_index,
                mouse.reprog_feature_index
            );
            let tx = tx.clone();
            let tracker = Arc::clone(&diverted_devices);
            thread::spawn(move || {
                let name = mouse.name.clone();
                let vid_pid = mouse.vid_pid.clone();
                run_device_loop(
                    mouse,
                    tracker,
                    |cid, code, value| {
                        println!(
                            "🔘 [HID++ {}] Control 0x{:04X} -> Code {} on {}",
                            if value == 1 { "Press" } else { "Release" },
                            cid,
                            code,
                            name
                        );
                        let _ = tx.send(HidppButtonEvent { dev_name: name.clone(), vid_pid: vid_pid.clone(), code, value });
                    },
                    true,
                );
            });
        }
        thread::sleep(Duration::from_millis(1500));
    });
}

/// Config UI fallback listener (when the daemon is not running).
pub fn attach_hidpp_gui_listener(
    ctx: eframe::egui::Context,
    tx: mpsc::Sender<crate::devices::MouseInputEvent>,
    diverted_devices: Arc<Mutex<HashSet<PathBuf>>>,
) {
    thread::spawn(move || loop {
        let ignored = diverted_devices.lock().unwrap().clone();
        for mouse in discover_logitech_mice(&ignored) {
            if !diverted_devices.lock().unwrap().insert(mouse.path.clone()) {
                continue;
            }
            let tx = tx.clone();
            let ctx = ctx.clone();
            let tracker = Arc::clone(&diverted_devices);
            thread::spawn(move || {
                let name = mouse.name.clone();
                let vid_pid = mouse.vid_pid.clone();
                run_device_loop(
                    mouse,
                    tracker,
                    |_cid, code, value| {
                        if value == 1 {
                            let _ = tx.send(crate::devices::MouseInputEvent {
                                code,
                                raw_code: code,
                                dev_name: Some(name.clone()),
                                vid_pid: vid_pid.clone(),
                            });
                            ctx.request_repaint();
                        }
                    },
                    false,
                );
            });
        }
        thread::sleep(Duration::from_millis(1500));
    });
}

/// First reachable Logitech HID++ device's battery (level, charging).
pub fn query_battery() -> Option<(u8, bool)> {
    for (path, _name, _vp, _bt, is_receiver) in hidpp_interfaces() {
        let Some(dev) = open_path(&path) else { continue };
        let idxs: Vec<u8> = if is_receiver { vec![1, 2, 3, 4, 5, 6] } else { vec![DEV_INDEX_WIRELESS_OR_BT] };
        for i in idxs {
            flush(&dev);
            if let Some(b) = battery_status(&dev, i) {
                return Some(b);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cid_to_button_code_mappings() {
        assert_eq!(cid_to_button_code(CID_GESTURE_BUTTON), BTN_RING_CODE);
        assert_eq!(cid_to_button_code(CID_SMARTSHIFT_BUTTON), BTN_SMARTSHIFT_CODE);
        assert_eq!(cid_to_button_code(CID_APP_SWITCH_BUTTON), BTN_RING_CODE);
        assert_eq!(cid_to_button_code(CID_DPI_SWITCH_BUTTON), BTN_SMARTSHIFT_CODE);
        assert_eq!(cid_to_button_code(CID_PRECISION_BUTTON), BTN_GESTURES_CODE);
        assert_eq!(cid_to_button_code(CID_HAPTIC_PANEL), BTN_RING_CODE);
        assert_eq!(cid_to_button_code(0x0053), BTN_BACK_CODE);
        assert_eq!(cid_to_button_code(0x0056), BTN_FORWARD_CODE);
        assert_eq!(cid_to_button_code(0x0052), BTN_MIDDLE_CODE);
        assert_eq!(cid_to_button_code(0x1234), 0x1234);
    }

    #[test]
    fn test_sw_ids_rotate_and_skip_notification_id() {
        let ids: Vec<u8> = (0..30).map(|_| next_sw_id()).collect();
        assert!(ids.iter().all(|&i| (0x2..=0xF).contains(&i)));
        assert!(ids.windows(2).all(|w| w[0] != w[1]));
    }

    #[test]
    fn test_long_request_framing() {
        let r = long_request(0xFF, 0x0D, 0x3, &[0x01, 0xA0, FLAG_DIVERT_ON]);
        assert_eq!(&r[..7], &[0x11, 0xFF, 0x0D, 0x3E, 0x01, 0xA0, 0x03]);
        assert_eq!(r.len(), 20);
    }

    #[test]
    fn test_parse_diverted_button_report() {
        let mut report = [0u8; 20];
        report[0] = REPORT_ID_LONG;
        report[1] = 0xFF;
        report[2] = 0x0D;
        report[4] = 0x01;
        report[5] = 0xA0;
        report[6] = 0x00;
        report[7] = 0xC3;
        let set = parse_diverted_buttons(&report, 0xFF, 0x0D).unwrap();
        assert!(set.contains(&0x01A0) && set.contains(&0x00C3) && set.len() == 2);
        // A reply to our own request (sw id 0xE) is not an event.
        report[3] = 0x3E;
        assert!(parse_diverted_buttons(&report, 0xFF, 0x0D).is_none());
        // Wrong feature index.
        report[3] = 0x00;
        assert!(parse_diverted_buttons(&report, 0xFF, 0x0B).is_none());
    }

    #[test]
    fn test_resolve_cids_for_mouse() {
        let cids = resolve_cids_for_mouse("Wireless Mouse MX Master 2s", Some("046d:b019"));
        assert_eq!(cids, vec![(0x00C3, 278), (0x00C4, 280)]);

        let cids_m720 = resolve_cids_for_mouse("Logitech M720 Triathlon", Some("046d:b015"));
        assert_eq!(cids_m720, vec![(0x00D0, 278)]);

        // macOS: the MX Master 4's haptic panel and gesture button are both
        // silent natively, so both are diverted (plus SmartShift).
        let cids_mx4 = resolve_cids_for_mouse("MX Master 4", Some("046d:b042"));
        assert_eq!(cids_mx4, vec![(0x00C3, 281), (0x01A0, 278), (0x00C4, 280)]);

        let fallback = resolve_cids_for_mouse("Unknown Logitech Mouse", None);
        assert_eq!(fallback, vec![(0x00C3, 278)]);
    }
}
