//! Overlay window helpers for macOS: per-user runtime paths, the ring PID
//! file, and the transparent full-screen viewport placed on the display under
//! the cursor.

use eframe::egui;
use objc2_app_kit::NSScreen;
use objc2_foundation::MainThreadMarker;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, Ordering};

/// Per-user runtime directory. Uses `confstr(_CS_DARWIN_USER_TEMP_DIR)` rather
/// than `$TMPDIR`, because launchd agents may run without `TMPDIR` set and the
/// daemon, ring and config UI must agree on the socket / PID file location.
pub fn runtime_dir() -> PathBuf {
    let mut buf = vec![0u8; 1024];
    let n = unsafe { libc::confstr(libc::_CS_DARWIN_USER_TEMP_DIR, buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
    if n > 1 && (n as usize) <= buf.len() {
        buf.truncate(n as usize - 1);
        if let Ok(s) = String::from_utf8(buf) {
            return PathBuf::from(s);
        }
    }
    std::env::temp_dir()
}

pub fn ring_pid_file_path() -> PathBuf {
    runtime_dir().join("bo-ring-ring.pid")
}

/// Global cursor position in Quartz coordinates (origin top-left of the main display, points).
pub fn global_cursor_pos() -> (f32, f32) {
    use core_graphics::event::CGEvent;
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
    CGEventSource::new(CGEventSourceStateID::CombinedSessionState)
        .ok()
        .and_then(|s| CGEvent::new(s).ok())
        .map(|e| {
            let p = e.location();
            (p.x as f32, p.y as f32)
        })
        .unwrap_or((0.0, 0.0))
}

/// Frame (x, y, w, h) in top-left global points of the screen containing the
/// cursor, as (full frame, visible frame).
fn screen_under_cursor() -> Option<((f32, f32, f32, f32), (f32, f32, f32, f32))> {
    let mtm = MainThreadMarker::new()?;
    let (cx, cy) = global_cursor_pos();
    let screens = NSScreen::screens(mtm);
    let primary_h = unsafe { screens.firstObject() }.map(|s| s.frame().size.height as f32)?;
    let flip = |r: objc2_foundation::NSRect| {
        let x = r.origin.x as f32;
        let w = r.size.width as f32;
        let h = r.size.height as f32;
        let y = primary_h - (r.origin.y as f32 + h);
        (x, y, w, h)
    };
    let mut best = None;
    for s in screens.iter() {
        let f = flip(s.frame());
        if cx >= f.0 && cx < f.0 + f.2 && cy >= f.1 && cy < f.1 + f.3 {
            best = Some((f, flip(s.visibleFrame())));
            break;
        }
    }
    best.or_else(|| unsafe { screens.firstObject() }.map(|s| (flip(s.frame()), flip(s.visibleFrame()))))
}

static OVERLAY_ORIGIN: Mutex<(f32, f32)> = Mutex::new((0.0, 0.0));

/// The ring centre in overlay-window coordinates: the real cursor position.
/// (Replaces the Wayland "wait for a motion event after a uinput nudge" dance.)
pub fn ring_center_in_window() -> egui::Pos2 {
    let (cx, cy) = global_cursor_pos();
    let (ox, oy) = *OVERLAY_ORIGIN.lock().unwrap();
    egui::pos2(cx - ox, cy - oy)
}

pub fn get_work_area_size() -> Option<(f32, f32)> {
    screen_under_cursor().map(|(_, v)| (v.2, v.3))
}

pub fn build_ring_overlay_viewport() -> egui::ViewportBuilder {
    // Cover the whole display under the cursor (menu bar and Dock included) so
    // the ring can be centred on the pointer anywhere.
    let (x, y, w, h) = screen_under_cursor().map(|(f, _)| f).unwrap_or((0.0, 0.0, 1440.0, 900.0));
    *OVERLAY_ORIGIN.lock().unwrap() = (x, y);
    println!("📐 Overlay frame: {}x{} at ({}, {})", w, h, x, y);

    egui::ViewportBuilder::default()
        .with_app_id("bo-ring-overlay")
        .with_position([x, y])
        .with_inner_size([w, h])
        .with_decorations(false)
        .with_resizable(false)
        .with_always_on_top()
        .with_transparent(true)
        .with_taskbar(false)
}

/// PID of the app that was frontmost when the ring was summoned (daemon side),
/// so the chosen shortcut can be delivered back to it.
static LAST_TARGET_PID: AtomicI32 = AtomicI32::new(0);

pub fn set_last_target_pid(pid: i32) {
    LAST_TARGET_PID.store(pid, Ordering::SeqCst);
}

pub fn last_target_pid() -> i32 {
    LAST_TARGET_PID.load(Ordering::SeqCst)
}

/// Executable path of a pid (libproc).
pub fn pid_path(pid: i32) -> Option<String> {
    if pid <= 0 {
        return None;
    }
    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let n = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr() as *mut libc::c_void, buf.len() as u32) };
    if n <= 0 {
        return None;
    }
    buf.truncate(n as usize);
    String::from_utf8(buf).ok()
}

pub fn is_pid_alive(pid: i32) -> bool {
    pid > 0 && unsafe { libc::kill(pid, 0) } == 0
}

/// Guards against PID reuse: the process must be alive AND be a bo-ring binary.
pub fn is_pid_boring(pid: i32) -> bool {
    is_pid_alive(pid) && pid_path(pid).is_some_and(|p| p.contains("bo-ring"))
}

pub fn cleanup_stale_ring_pid() {
    let pid_path = ring_pid_file_path();
    if let Ok(content) = std::fs::read_to_string(&pid_path) {
        match content.trim().parse::<i32>() {
            Ok(pid) if is_pid_boring(pid) => {}
            Ok(pid) => {
                println!("🧹 Cleaning stale Ring Menu PID file (PID {} dead/reused)...", pid);
                let _ = std::fs::remove_file(&pid_path);
            }
            Err(_) => {
                let _ = std::fs::remove_file(&pid_path);
            }
        }
    }
}

/// Safely sends a POSIX signal to a specific process ID (`pid > 0` only —
/// 0 / -1 would target a process group or every process).
pub fn safe_kill(pid: i32, sig: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    unsafe { libc::kill(pid, sig) == 0 }
}

/// Terminates an active Bo-Ring overlay process after validating its identity.
pub fn terminate_ring_process(pid: i32) -> bool {
    if !is_pid_boring(pid) {
        return false;
    }
    safe_kill(pid, libc::SIGTERM)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_safe_kill_invalid_pid() {
        assert!(!safe_kill(0, libc::SIGTERM));
        assert!(!safe_kill(-1, libc::SIGTERM));
        assert!(!safe_kill(-100, libc::SIGTERM));
        assert!(!terminate_ring_process(0));
        assert!(!terminate_ring_process(-1));
    }

    #[test]
    fn test_pid_identity() {
        let me = std::process::id() as i32;
        assert!(is_pid_alive(me));
        assert!(pid_path(me).is_some());
        assert!(!is_pid_alive(0));
    }
}
