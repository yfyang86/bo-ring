//! macOS input emitter — replaces the Linux `uinput` virtual device.
//!
//! Key combos are synthesized with `CGEvent` and posted to the HID event tap
//! (requires the Accessibility permission). Media keys are posted as
//! `NSSystemDefined` (subtype 8) events, like the hardware media keys.
//!
//! The public API mirrors the Linux module so the rest of the code base is
//! unchanged: `send_key_combo`, `init_virtual_keyboard`, `nudge_mouse`, …

use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation, CGKeyCode, CGMouseButton, CGEventType};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

static IS_AZERTY_LAYOUT: AtomicBool = AtomicBool::new(false);
static LAYOUT_CHECKED: AtomicBool = AtomicBool::new(false);

/// A resolved key: either a regular virtual key code, a modifier, a media key
/// (NX_KEYTYPE_*), or a mouse button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacKey {
    Code(CGKeyCode),
    Modifier(CGKeyCode, u64),
    Media(i64),
    Mouse(u32),
}

// kVK_* virtual key codes (Carbon HIToolbox/Events.h). These are positional
// (ANSI layout), so AZERTY remapping mirrors the Linux behaviour.
mod vk {
    pub const A: u16 = 0x00; pub const S: u16 = 0x01; pub const D: u16 = 0x02; pub const F: u16 = 0x03;
    pub const H: u16 = 0x04; pub const G: u16 = 0x05; pub const Z: u16 = 0x06; pub const X: u16 = 0x07;
    pub const C: u16 = 0x08; pub const V: u16 = 0x09; pub const B: u16 = 0x0B; pub const Q: u16 = 0x0C;
    pub const W: u16 = 0x0D; pub const E: u16 = 0x0E; pub const R: u16 = 0x0F; pub const Y: u16 = 0x10;
    pub const T: u16 = 0x11; pub const N1: u16 = 0x12; pub const N2: u16 = 0x13; pub const N3: u16 = 0x14;
    pub const N4: u16 = 0x15; pub const N6: u16 = 0x16; pub const N5: u16 = 0x17; pub const EQUAL: u16 = 0x18;
    pub const N9: u16 = 0x19; pub const N7: u16 = 0x1A; pub const MINUS: u16 = 0x1B; pub const N8: u16 = 0x1C;
    pub const N0: u16 = 0x1D; pub const RBRACKET: u16 = 0x1E; pub const O: u16 = 0x1F; pub const U: u16 = 0x20;
    pub const LBRACKET: u16 = 0x21; pub const I: u16 = 0x22; pub const P: u16 = 0x23; pub const RETURN: u16 = 0x24;
    pub const L: u16 = 0x25; pub const J: u16 = 0x26; pub const QUOTE: u16 = 0x27; pub const K: u16 = 0x28;
    pub const SEMICOLON: u16 = 0x29; pub const BACKSLASH: u16 = 0x2A; pub const COMMA: u16 = 0x2B;
    pub const SLASH: u16 = 0x2C; pub const N: u16 = 0x2D; pub const M: u16 = 0x2E; pub const PERIOD: u16 = 0x2F;
    pub const TAB: u16 = 0x30; pub const SPACE: u16 = 0x31; pub const GRAVE: u16 = 0x32; pub const DELETE: u16 = 0x33;
    pub const ESCAPE: u16 = 0x35; pub const COMMAND: u16 = 0x37; pub const SHIFT: u16 = 0x38;
    pub const OPTION: u16 = 0x3A; pub const CONTROL: u16 = 0x3B; pub const FUNCTION: u16 = 0x3F;
    pub const F5: u16 = 0x60; pub const F6: u16 = 0x61; pub const F7: u16 = 0x62; pub const F3: u16 = 0x63;
    pub const F8: u16 = 0x64; pub const F9: u16 = 0x65; pub const F11: u16 = 0x67; pub const F13: u16 = 0x69;
    pub const F10: u16 = 0x6D; pub const F12: u16 = 0x6F; pub const HOME: u16 = 0x73; pub const PAGEUP: u16 = 0x74;
    pub const FWD_DELETE: u16 = 0x75; pub const F4: u16 = 0x76; pub const END: u16 = 0x77; pub const F2: u16 = 0x78;
    pub const PAGEDOWN: u16 = 0x79; pub const F1: u16 = 0x7A; pub const LEFT: u16 = 0x7B; pub const RIGHT: u16 = 0x7C;
    pub const DOWN: u16 = 0x7D; pub const UP: u16 = 0x7E;
}

// NX_KEYTYPE_* (IOKit/hidsystem/ev_keymap.h)
const NX_KEYTYPE_SOUND_UP: i64 = 0;
const NX_KEYTYPE_SOUND_DOWN: i64 = 1;
const NX_KEYTYPE_MUTE: i64 = 7;
const NX_KEYTYPE_PLAY: i64 = 16;
const NX_KEYTYPE_NEXT: i64 = 17;
const NX_KEYTYPE_PREVIOUS: i64 = 18;

pub fn init_layout_detection() {
    if LAYOUT_CHECKED.load(Ordering::Relaxed) {
        return;
    }
    thread::spawn(|| {
        // The selected input sources are listed in com.apple.HIToolbox.
        let mut azerty = false;
        if let Ok(out) = std::process::Command::new("defaults")
            .args(["read", "com.apple.HIToolbox", "AppleSelectedInputSources"])
            .output()
        {
            let s = String::from_utf8_lossy(&out.stdout).to_lowercase();
            azerty = s.contains("french") || s.contains("belgian") || s.contains("azerty");
        }
        IS_AZERTY_LAYOUT.store(azerty, Ordering::Relaxed);
        LAYOUT_CHECKED.store(true, Ordering::Relaxed);
    });
}

pub fn is_azerty() -> bool {
    IS_AZERTY_LAYOUT.load(Ordering::Relaxed)
}

pub fn str_to_key(s: &str) -> Option<MacKey> {
    use MacKey::*;
    let n = s.trim().to_lowercase();
    let az = is_azerty();
    let key = match n.as_str() {
        // Modifiers. "super"/"win"/"meta" map to Command — the macOS equivalent.
        "ctrl" | "ctrl_l" | "ctrl_r" | "control" => Modifier(vk::CONTROL, CGEventFlags::CGEventFlagControl.bits()),
        "alt" | "alt_l" | "alt_r" | "opt" | "option" => Modifier(vk::OPTION, CGEventFlags::CGEventFlagAlternate.bits()),
        "shift" | "shift_l" | "shift_r" => Modifier(vk::SHIFT, CGEventFlags::CGEventFlagShift.bits()),
        "super" | "super_l" | "super_r" | "meta" | "win" | "cmd" | "command" => {
            Modifier(vk::COMMAND, CGEventFlags::CGEventFlagCommand.bits())
        }
        "fn" => Modifier(vk::FUNCTION, CGEventFlags::CGEventFlagSecondaryFn.bits()),

        // Media keys
        "volumeup" | "volume_up" | "volup" | "xf86audioraisevolume" => Media(NX_KEYTYPE_SOUND_UP),
        "volumedown" | "volume_down" | "voldown" | "xf86audiolowervolume" => Media(NX_KEYTYPE_SOUND_DOWN),
        "mute" | "volumemute" | "audiomute" | "xf86audiomute" => Media(NX_KEYTYPE_MUTE),
        "playpause" | "play" | "pause" | "audioplay" | "xf86audioplay" => Media(NX_KEYTYPE_PLAY),
        "nextsong" | "next" | "audionext" | "xf86audionext" => Media(NX_KEYTYPE_NEXT),
        "previoussong" | "prev" | "audioprev" | "xf86audioprev" => Media(NX_KEYTYPE_PREVIOUS),

        // Mouse buttons (CGEvent button numbers: 0 left, 1 right, 2 middle, 3 back, 4 forward)
        "btn_back" | "btn_side" | "back" => Mouse(3),
        "btn_forward" | "btn_extra" | "forward" => Mouse(4),
        "btn_left" => Mouse(0),
        "btn_right" => Mouse(1),
        "btn_middle" => Mouse(2),

        "space" => Code(vk::SPACE),
        "enter" | "return" => Code(vk::RETURN),
        "tab" => Code(vk::TAB),
        "escape" | "esc" => Code(vk::ESCAPE),
        "backspace" => Code(vk::DELETE),
        "delete" | "del" => Code(vk::FWD_DELETE),
        // No PrintScreen on a Mac keyboard — F13 is its traditional position.
        "print" | "printscreen" | "sysrq" | "prtscr" => Code(vk::F13),
        "left" | "arrowleft" => Code(vk::LEFT),
        "right" | "arrowright" => Code(vk::RIGHT),
        "up" | "arrowup" => Code(vk::UP),
        "down" | "arrowdown" => Code(vk::DOWN),
        "home" => Code(vk::HOME),
        "end" => Code(vk::END),
        "pageup" => Code(vk::PAGEUP),
        "pagedown" => Code(vk::PAGEDOWN),
        "-" | "minus" => Code(vk::MINUS),
        "=" | "equal" | "plus" => Code(vk::EQUAL),
        "[" | "bracketleft" => Code(vk::LBRACKET),
        "]" | "bracketright" => Code(vk::RBRACKET),
        ";" | "semicolon" => Code(vk::SEMICOLON),
        "'" | "apostrophe" => Code(vk::QUOTE),
        "\\" | "backslash" => Code(vk::BACKSLASH),
        "," | "comma" => Code(vk::COMMA),
        "." | "period" => Code(vk::PERIOD),
        "/" | "slash" => Code(vk::SLASH),
        "`" | "grave" => Code(vk::GRAVE),

        "a" => Code(if az { vk::Q } else { vk::A }),
        "b" => Code(vk::B), "c" => Code(vk::C), "d" => Code(vk::D), "e" => Code(vk::E),
        "f" => Code(vk::F), "g" => Code(vk::G), "h" => Code(vk::H), "i" => Code(vk::I),
        "j" => Code(vk::J), "k" => Code(vk::K), "l" => Code(vk::L),
        "m" => Code(if az { vk::SEMICOLON } else { vk::M }),
        "n" => Code(vk::N), "o" => Code(vk::O), "p" => Code(vk::P),
        "q" => Code(if az { vk::A } else { vk::Q }),
        "r" => Code(vk::R), "s" => Code(vk::S), "t" => Code(vk::T), "u" => Code(vk::U),
        "v" => Code(vk::V),
        "w" => Code(if az { vk::Z } else { vk::W }),
        "x" => Code(vk::X), "y" => Code(vk::Y),
        "z" => Code(if az { vk::W } else { vk::Z }),

        "0" => Code(vk::N0), "1" => Code(vk::N1), "2" => Code(vk::N2), "3" => Code(vk::N3),
        "4" => Code(vk::N4), "5" => Code(vk::N5), "6" => Code(vk::N6), "7" => Code(vk::N7),
        "8" => Code(vk::N8), "9" => Code(vk::N9),

        "f1" => Code(vk::F1), "f2" => Code(vk::F2), "f3" => Code(vk::F3), "f4" => Code(vk::F4),
        "f5" => Code(vk::F5), "f6" => Code(vk::F6), "f7" => Code(vk::F7), "f8" => Code(vk::F8),
        "f9" => Code(vk::F9), "f10" => Code(vk::F10), "f11" => Code(vk::F11), "f12" => Code(vk::F12),
        "f13" => Code(vk::F13),
        _ => return None,
    };
    Some(key)
}

/// Whether this process may post synthetic events (Accessibility permission).
pub fn check_post_access() -> bool {
    unsafe { AXIsProcessTrusted() }
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
}

fn source() -> Option<CGEventSource> {
    CGEventSource::new(CGEventSourceStateID::HIDSystemState).ok()
}

/// No virtual device is needed on macOS; kept for API compatibility.
pub fn init_virtual_keyboard() {}
pub fn reset_virtual_keyboard() {}

pub fn send_key_combo(keys: &[String]) {
    if keys.is_empty() {
        return;
    }
    let resolved: Vec<MacKey> = keys
        .iter()
        .filter_map(|k| {
            let r = str_to_key(k);
            if r.is_none() {
                eprintln!("⚠️ Unrecognized key in shortcut: '{}'", k);
            }
            r
        })
        .collect();
    if resolved.is_empty() {
        return;
    }
    if !check_post_access() {
        eprintln!("⚠️ Accessibility permission missing: cannot synthesize {:?}", keys);
        return;
    }
    println!("⌨️ Simulating keyboard shortcut via CGEvent: {:?}", keys);

    // Media keys stand alone.
    if let Some(MacKey::Media(k)) = resolved.iter().find(|k| matches!(k, MacKey::Media(_))) {
        post_media_key(*k);
        return;
    }
    // Mouse buttons (e.g. BTN_BACK) are clicks at the current pointer.
    if let Some(MacKey::Mouse(b)) = resolved.iter().find(|k| matches!(k, MacKey::Mouse(_))) {
        post_mouse_click(*b);
        return;
    }

    let mut flags = CGEventFlags::empty();
    let mut modifiers = Vec::new();
    let mut main_keys = Vec::new();
    for k in &resolved {
        match *k {
            MacKey::Modifier(code, f) => {
                flags |= CGEventFlags::from_bits_truncate(f);
                modifiers.push(code);
            }
            MacKey::Code(code) => main_keys.push(code),
            _ => {}
        }
    }
    let Some(src) = source() else { return };

    // Press modifiers, then keys (carrying the flags), then release in reverse.
    let mut held = CGEventFlags::empty();
    for &m in &modifiers {
        if let Ok(e) = CGEvent::new_keyboard_event(src.clone(), m, true) {
            held |= flag_for_modifier(m);
            e.set_flags(held);
            e.post(CGEventTapLocation::HID);
        }
    }
    for &k in &main_keys {
        if let Ok(e) = CGEvent::new_keyboard_event(src.clone(), k, true) {
            e.set_flags(flags);
            e.post(CGEventTapLocation::HID);
        }
    }
    thread::sleep(Duration::from_millis(30));
    for &k in main_keys.iter().rev() {
        if let Ok(e) = CGEvent::new_keyboard_event(src.clone(), k, false) {
            e.set_flags(flags);
            e.post(CGEventTapLocation::HID);
        }
    }
    for &m in modifiers.iter().rev() {
        held.remove(flag_for_modifier(m));
        if let Ok(e) = CGEvent::new_keyboard_event(src.clone(), m, false) {
            e.set_flags(held);
            e.post(CGEventTapLocation::HID);
        }
    }
}

fn flag_for_modifier(code: CGKeyCode) -> CGEventFlags {
    match code {
        vk::CONTROL => CGEventFlags::CGEventFlagControl,
        vk::OPTION => CGEventFlags::CGEventFlagAlternate,
        vk::SHIFT => CGEventFlags::CGEventFlagShift,
        vk::COMMAND => CGEventFlags::CGEventFlagCommand,
        vk::FUNCTION => CGEventFlags::CGEventFlagSecondaryFn,
        _ => CGEventFlags::empty(),
    }
}

/// Post a hardware-style media key (NSSystemDefined, subtype 8).
fn post_media_key(key: i64) {
    use objc2_app_kit::{NSEvent, NSEventModifierFlags, NSEventType};
    use objc2_foundation::NSPoint;
    for down in [true, false] {
        let flags: usize = if down { 0xA00 } else { 0xB00 };
        let data1 = ((key << 16) as isize) | flags as isize;
        unsafe {
            let ev = NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
                NSEventType::SystemDefined,
                NSPoint::new(0.0, 0.0),
                NSEventModifierFlags(flags),
                0.0,
                0,
                None,
                8,
                data1,
                -1,
            );
            if let Some(ev) = ev {
                // -[NSEvent CGEvent] isn't bound by objc2-app-kit 0.2; send it directly.
                let cg: *const std::ffi::c_void = objc2::msg_send![&*ev, CGEvent];
                if !cg.is_null() {
                    CGEventPost(0, cg);
                }
            }
        }
    }
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventPost(tap: u32, event: *const std::ffi::c_void);
}

fn post_mouse_click(button: u32) {
    let Some(src) = source() else { return };
    let pos = CGEvent::new(src.clone()).map(|e| e.location()).unwrap_or_default();
    let (down, up, btn) = match button {
        0 => (CGEventType::LeftMouseDown, CGEventType::LeftMouseUp, CGMouseButton::Left),
        1 => (CGEventType::RightMouseDown, CGEventType::RightMouseUp, CGMouseButton::Right),
        _ => (CGEventType::OtherMouseDown, CGEventType::OtherMouseUp, CGMouseButton::Center),
    };
    for t in [down, up] {
        if let Ok(e) = CGEvent::new_mouse_event(src.clone(), t, pos, btn) {
            // kCGMouseEventButtonNumber = 3
            e.set_integer_value_field(3, button as i64);
            // Tag so our own event tap passes it through untouched.
            e.set_integer_value_field(core_graphics::event::EventField::EVENT_SOURCE_USER_DATA, BORING_EVENT_TAG);
            e.post(CGEventTapLocation::HID);
        }
    }
}

/// Marker stored in kCGEventSourceUserData on events we synthesize, so the
/// daemon's event tap never re-captures them (avoids the BTN_BACK loop noted in
/// docs/architecture.md §5).
pub const BORING_EVENT_TAG: i64 = 0x0B0B_0B0B;

/// Linux needed a 1px uinput nudge to refresh stale Wayland pointer
/// coordinates. macOS reports the real cursor position, so this is a no-op.
pub fn nudge_mouse() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_str_to_key_mapping() {
        assert_eq!(str_to_key("CTRL"), Some(MacKey::Modifier(vk::CONTROL, CGEventFlags::CGEventFlagControl.bits())));
        assert_eq!(str_to_key("Super"), Some(MacKey::Modifier(vk::COMMAND, CGEventFlags::CGEventFlagCommand.bits())));
        assert_eq!(str_to_key("cmd"), str_to_key("Super"));
        assert_eq!(str_to_key("c"), Some(MacKey::Code(vk::C)));
        assert_eq!(str_to_key("C"), Some(MacKey::Code(vk::C)));
        assert_eq!(str_to_key("VolumeUp"), Some(MacKey::Media(NX_KEYTYPE_SOUND_UP)));
        assert_eq!(str_to_key("XF86AudioPlay"), Some(MacKey::Media(NX_KEYTYPE_PLAY)));
        assert_eq!(str_to_key("Left"), Some(MacKey::Code(vk::LEFT)));
        assert_eq!(str_to_key("BTN_BACK"), Some(MacKey::Mouse(3)));
        assert_eq!(str_to_key("UnknownKey123"), None);
    }
}
