//! Mouse-button capture on macOS — replaces the Linux evdev exclusive grab +
//! uinput passthrough.
//!
//! A session-level `CGEventTap` watches `otherMouseDown/Up` (middle, back,
//! forward and any extra buttons). For a button the user remapped, the event is
//! **absorbed** (retyped to `Null`) and forwarded to the daemon; everything else
//! passes through untouched, so there is no passthrough device to maintain.
//! Left/right clicks are never intercepted.
//!
//! Requires the Accessibility permission (tap creation fails without it).

use core_foundation::base::TCFType;
use core_foundation::runloop::{kCFRunLoopCommonModes, CFRunLoop};
use core_graphics::event::{
    CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventType, EventField,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::{mpsc, Arc, RwLock};
use std::thread;
use std::time::Duration;

use super::input_emitter::BORING_EVENT_TAG;

/// A physical button transition: (Linux-compatible code, 1 = press / 0 = release).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ButtonEvent {
    pub code: u16,
    pub value: i32,
}

/// macOS button number → the evdev code the shared config uses.
/// (0 left=272, 1 right=273, 2 middle=274, 3 back/side=275, 4 forward/extra=276, n → 272+n)
pub fn mac_button_to_code(button: i64) -> u16 {
    (272 + button.clamp(0, 200)) as u16
}

// Only the daemon's absorbing tap is re-enabled through this handle; a
// listen-only tap is never disabled for slowness.
static TAP_PORT: AtomicPtr<std::ffi::c_void> = AtomicPtr::new(std::ptr::null_mut());

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventTapEnable(tap: *mut std::ffi::c_void, enable: bool);
}

/// Spawn the tap thread. `should_absorb(code)` is consulted synchronously for
/// every press/release (keep it cheap: a map lookup).
pub fn start_event_tap(
    tx: mpsc::Sender<ButtonEvent>,
    remapped: Arc<RwLock<HashMap<u16, crate::config::ButtonActionConfig>>>,
) {
    start_tap(tx, remapped, false);
}

/// Observe-only tap (never absorbs) — the Config UI's fallback listener when
/// the daemon is not running.
pub fn start_listen_only_tap(tx: mpsc::Sender<ButtonEvent>) {
    start_tap(tx, Arc::new(RwLock::new(HashMap::new())), true);
}

fn start_tap(
    tx: mpsc::Sender<ButtonEvent>,
    remapped: Arc<RwLock<HashMap<u16, crate::config::ButtonActionConfig>>>,
    listen_only: bool,
) {
    thread::spawn(move || {
        loop {
            let tx_cb = tx.clone();
            let remapped_cb = Arc::clone(&remapped);
            let tap = CGEventTap::new(
                CGEventTapLocation::Session,
                CGEventTapPlacement::HeadInsertEventTap,
                if listen_only { CGEventTapOptions::ListenOnly } else { CGEventTapOptions::Default },
                vec![CGEventType::OtherMouseDown, CGEventType::OtherMouseUp],
                move |_proxy, etype, event| {
                    match etype {
                        CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput => {
                            let port = TAP_PORT.load(Ordering::SeqCst);
                            if !port.is_null() {
                                unsafe { CGEventTapEnable(port, true) };
                                eprintln!("⚠️ Event tap was disabled by the system; re-enabled");
                            }
                            return None;
                        }
                        CGEventType::OtherMouseDown | CGEventType::OtherMouseUp => {}
                        _ => return None,
                    }
                    // Our own synthesized clicks (e.g. a slot sending BTN_BACK) pass through.
                    if event.get_integer_value_field(EventField::EVENT_SOURCE_USER_DATA) == BORING_EVENT_TAG {
                        return None;
                    }
                    let button = event.get_integer_value_field(EventField::MOUSE_EVENT_BUTTON_NUMBER);
                    let code = mac_button_to_code(button);
                    let value = if matches!(etype, CGEventType::OtherMouseDown) { 1 } else { 0 };
                    let raw = code;
                    let norm = crate::devices::normalize_button_code(code);
                    let absorb = remapped_cb
                        .read()
                        .map(|m| m.contains_key(&raw) || m.contains_key(&norm))
                        .unwrap_or(false);
                    let _ = tx_cb.send(ButtonEvent { code, value });
                    if absorb && !listen_only {
                        // Swallow the original click so the focused app never sees it.
                        event.set_type(CGEventType::Null);
                    }
                    None
                },
            );

            match tap {
                Ok(tap) => {
                    if !listen_only {
                        TAP_PORT.store(tap.mach_port.as_concrete_TypeRef() as *mut _, Ordering::SeqCst);
                    }
                    let source = tap
                        .mach_port
                        .create_runloop_source(0)
                        .expect("event tap runloop source");
                    let rl = CFRunLoop::get_current();
                    unsafe { rl.add_source(&source, kCFRunLoopCommonModes) };
                    tap.enable();
                    println!(
                        "🖱️ CGEventTap active ({}): middle / back / forward / extra mouse buttons",
                        if listen_only { "listen-only" } else { "capturing" }
                    );
                    CFRunLoop::run_current();
                    TAP_PORT.store(std::ptr::null_mut(), Ordering::SeqCst);
                }
                Err(()) => {
                    eprintln!(
                        "⚠️ Could not create the mouse event tap — grant Accessibility to bo-ring \
                         (System Settings → Privacy & Security → Accessibility). Retrying in 5 s…"
                    );
                    thread::sleep(Duration::from_secs(5));
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::codes::*;

    #[test]
    fn test_mac_button_mapping() {
        assert_eq!(mac_button_to_code(0), BTN_LEFT_CODE);
        assert_eq!(mac_button_to_code(1), BTN_RIGHT_CODE);
        assert_eq!(mac_button_to_code(2), BTN_MIDDLE_CODE);
        assert_eq!(mac_button_to_code(3), BTN_BACK_CODE);
        assert_eq!(mac_button_to_code(4), BTN_FORWARD_CODE);
        assert_eq!(mac_button_to_code(5), BTN_GESTURES_CODE);
    }
}
