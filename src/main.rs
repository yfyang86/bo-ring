mod config;
mod config_ui;
pub mod devices;
pub mod platform;
pub mod utils;

pub use platform::input_emitter as keyboard;
pub use platform::ipc::daemon_socket_path;
pub use platform::{autostart, uninstall, window as active_window};
pub use utils::{color, i18n, icon_loader, path};

mod ring_menu;

use config::{ButtonActionConfig, Config};
use config_ui::run_config_gui;
use devices::{is_supported_button_code, normalize_button_code, BTN_LEFT_CODE, BTN_RIGHT_CODE};
use platform::ipc::{broadcast_button_event, DaemonIpcCommand, IpcClients};
use ring_menu::{execute_action_direct, run_ring_menu_window};
use std::collections::HashSet;
use std::env;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

static RING_BUTTON_HELD: AtomicBool = AtomicBool::new(false);

/// A physical button transition from any source (event tap or HID++).
struct InputEvent {
    dev_name: String,
    vid_pid: Option<String>,
    code: u16,
    value: i32,
}

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.iter().any(|arg| arg == "--uninstall") {
        uninstall::run_uninstall();
        return;
    }

    // Ring Menu mode: `bo-ring --ring [--app <bundle id>]`
    if args.iter().any(|arg| arg == "--ring") {
        println!("🚀 Launching Ring Menu...");
        let initial_app = args.windows(2).find(|w| w[0] == "--app").map(|w| w[1].clone());
        let config = Config::load_or_default();
        let lang = crate::utils::i18n::resolve_language_code(&config.general.language);
        run_ring_menu_window(config.ring_menu, config.app_profiles, initial_app, lang);
        return;
    }

    // `bo-ring --install-agent` — install + start the daemon LaunchAgent.
    if args.iter().any(|arg| arg == "--install-agent") {
        match autostart::enable_systemd_service() {
            Ok(()) => println!("✅ LaunchAgent {} installed and started.", autostart::DAEMON_LABEL),
            Err(e) => {
                eprintln!("❌ LaunchAgent install failed: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    // `bo-ring --toggle` — ask the running daemon to toggle the ring (bind to any hotkey tool).
    if args.iter().any(|arg| arg == "--toggle") {
        if !platform::ipc::send_command_to_daemon(&DaemonIpcCommand::ToggleRing) {
            eprintln!("⚠️ Daemon not running (start it with `bo-ring --daemon`).");
            std::process::exit(1);
        }
        return;
    }

    if !args.iter().any(|arg| arg == "--daemon") {
        println!("🎛️ Launching Configuration Menu...");
        autostart::ensure_daemon_running_from_bundle();
        let config = Config::load_or_default();
        run_config_gui(config);
        return;
    }

    run_daemon();
}

fn run_daemon() {
    println!("=== Bo-Ring (macOS) - Daemon & Actions Ring ===");
    install_shutdown_handler();
    keyboard::init_layout_detection();
    ring_menu::cleanup_stale_ring_pid();

    if !platform::permissions::accessibility_granted() {
        println!("🔐 Accessibility permission missing — prompting (needed to capture buttons & send shortcuts).");
        platform::permissions::prompt_accessibility();
    }
    if !platform::permissions::input_monitoring_granted() {
        println!("🔐 Input Monitoring permission missing — prompting (needed for Logitech HID++ buttons).");
        platform::permissions::request_input_monitoring();
    }

    let config = Config::load_or_default();
    println!("✅ Configuration ready:");
    println!("   - Target device: {}", config.general.device_name);
    println!("   - Configured buttons: {}", config.buttons.len());
    println!("   - Ring Menu slots: {}", config.ring_menu.items.len());

    let button_mappings = Arc::new(std::sync::RwLock::new(config.buttons));
    let (tx, rx) = mpsc::channel::<InputEvent>();

    // Standard extra buttons (middle / back / forward / …) via CGEventTap.
    let (tap_tx, tap_rx) = mpsc::channel::<platform::event_tap::ButtonEvent>();
    platform::event_tap::start_event_tap(tap_tx, Arc::clone(&button_mappings));
    {
        let tx = tx.clone();
        thread::spawn(move || {
            let name = devices::scan_mouse_device().map(|m| (m.name, m.vid_pid)).unwrap_or(("Mouse".into(), None));
            for ev in tap_rx {
                let _ = tx.send(InputEvent { dev_name: name.0.clone(), vid_pid: name.1.clone(), code: ev.code, value: ev.value });
            }
        });
    }

    // Logitech special controls (gesture button, haptic panel, SmartShift…) via HID++.
    let (hid_tx, hid_rx) = mpsc::channel::<devices::HidppButtonEvent>();
    let diverted_hidpp = Arc::new(Mutex::new(HashSet::new()));
    devices::hidpp::start_hidpp_manager(hid_tx, Arc::clone(&diverted_hidpp));
    {
        let tx = tx.clone();
        thread::spawn(move || {
            for ev in hid_rx {
                let _ = tx.send(InputEvent { dev_name: ev.dev_name, vid_pid: ev.vid_pid, code: ev.code, value: ev.value });
            }
        });
    }
    start_wake_listener(Arc::clone(&diverted_hidpp));

    println!("\n========================================================");
    println!(">>> Daemon active (event tap + HID++ with reconnect & wake recovery).");
    println!(">>> Press the configured Action Ring button to open the menu.");
    println!("========================================================\n");

    let ipc_clients = start_ipc_server(Arc::clone(&button_mappings));

    while let Ok(ev) = rx.recv() {
        let raw_code = ev.code;
        let code = normalize_button_code(raw_code);
        let is_primary_click = raw_code == BTN_LEFT_CODE || raw_code == BTN_RIGHT_CODE;
        let mappings_guard = button_mappings.read().unwrap();
        let is_remapped = !is_primary_click && (mappings_guard.contains_key(&raw_code) || mappings_guard.contains_key(&code));

        if is_supported_button_code(raw_code) {
            broadcast_button_event(&ipc_clients, raw_code, code, ev.value, ev.vid_pid.as_deref(), Some(&ev.dev_name));
        }

        if !is_remapped {
            // Not remapped: the event tap already let the click through.
            continue;
        }
        let action = mappings_guard.get(&raw_code).or_else(|| mappings_guard.get(&code)).cloned();
        drop(mappings_guard);

        if matches!(action, Some(ButtonActionConfig::ShowRingMenu)) {
            RING_BUTTON_HELD.store(ev.value == 1, Ordering::SeqCst);
        }
        if ev.value == 1 {
            if let Some(action) = action {
                println!("🔘 Button pressed (Raw {} -> Normalized {}) -> {:?}", raw_code, code, action);
                match action {
                    ButtonActionConfig::ShowRingMenu => toggle_ring_menu(),
                    _ => execute_action_direct(&action),
                }
            }
        }
    }
}

/// SIGTERM / SIGINT / SIGHUP: restore diverted Logitech controls, remove the
/// socket, exit. Signals are blocked in every thread and consumed by one
/// `sigwait` thread, so the cleanup runs in normal (non-handler) context.
fn install_shutdown_handler() {
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        for sig in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
            libc::sigaddset(&mut set, sig);
        }
        libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
        let set_copy = set;
        thread::spawn(move || {
            let mut sig: libc::c_int = 0;
            libc::sigwait(&set_copy, &mut sig);
            println!("\n🛑 Signal {} received — restoring diverted controls…", sig);
            devices::hidpp::undivert_all();
            let _ = std::fs::remove_file(daemon_socket_path());
            std::process::exit(0);
        });
    }
}

/// After sleep/wake the mouse may have power-cycled and lost its diversion.
/// Detect wake by watching for a wall-clock jump and force re-discovery.
fn start_wake_listener(diverted_hidpp: Arc<Mutex<HashSet<std::path::PathBuf>>>) {
    thread::spawn(move || {
        let mut last = std::time::SystemTime::now();
        loop {
            thread::sleep(Duration::from_secs(5));
            let now = std::time::SystemTime::now();
            if now.duration_since(last).map(|d| d > Duration::from_secs(30)).unwrap_or(false) {
                println!("☀️ System wake detected — re-applying HID++ diversion.");
                ring_menu::cleanup_stale_ring_pid();
                diverted_hidpp.lock().unwrap().clear();
            }
            last = now;
        }
    });
}

fn execute_action_daemon_delayed(action: &ButtonActionConfig) {
    let resolved = config::resolve_action(action, &[]);
    match &resolved {
        ButtonActionConfig::ShowRingMenu | ButtonActionConfig::ActionRef { .. } => {}
        ButtonActionConfig::Command { .. } => execute_action_direct(&resolved),
        ButtonActionConfig::KeyCombo { keys } => {
            let keys_to_send = keys.clone();
            let is_media_key = keys.iter().any(|k| {
                let norm = k.trim().to_lowercase();
                norm.contains("volume") || norm.contains("mute") || norm.contains("play") || norm.contains("song")
            });
            if is_media_key {
                keyboard::send_key_combo(&keys_to_send);
            } else {
                thread::spawn(move || {
                    // The ring overlay (a separate process) took focus; hand it
                    // back to the app the user summoned the ring over.
                    active_window::focus_window_under_cursor();
                    thread::sleep(Duration::from_millis(180));
                    keyboard::send_key_combo(&keys_to_send);
                });
            }
        }
    }
}

fn start_ipc_server(
    button_mappings: Arc<std::sync::RwLock<std::collections::HashMap<u16, ButtonActionConfig>>>,
) -> IpcClients {
    let sock_path = daemon_socket_path();
    let _ = std::fs::remove_file(&sock_path);

    let clients: IpcClients = Arc::new(Mutex::new(Vec::new()));
    let clients_clone = Arc::clone(&clients);

    if let Ok(listener) = std::os::unix::net::UnixListener::bind(&sock_path) {
        println!("📡 Unix IPC Socket server active at {:?}", sock_path);
        let button_mappings_ipc = Arc::clone(&button_mappings);
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                use std::io::{BufRead, BufReader, Write};
                let is_held = RING_BUTTON_HELD.load(Ordering::SeqCst);
                    let init_msg = if is_held { "STATE:HELD\n" } else { "STATE:RELEASED\n" };

                    if let Ok(mut writer_stream) = stream.try_clone() {
                        let _ = writer_stream.write_all(init_msg.as_bytes());
                        let mut lock = clients_clone.lock().unwrap();
                        lock.push(writer_stream);
                    }

                    // Spawn reader thread for incoming EXEC and CONFIG commands
                    let button_mappings_client = Arc::clone(&button_mappings_ipc);
                    thread::spawn(move || {
                        let reader = BufReader::new(stream);
                        for line in reader.lines().map_while(Result::ok) {
                            if let Some(cmd) = DaemonIpcCommand::parse_wire_line(&line) {
                                match cmd {
                                    DaemonIpcCommand::ShowRing | DaemonIpcCommand::ToggleRing => {
                                        toggle_ring_menu();
                                    }
                                    DaemonIpcCommand::HideRing => {
                                        let pid_path = ring_menu::ring_pid_file_path();
                                        if let Ok(content) = std::fs::read_to_string(&pid_path) {
                                            if let Ok(pid) = content.trim().parse::<i32>() {
                                                ring_menu::terminate_ring_process(pid);
                                            }
                                        }
                                        ring_menu::cleanup_stale_ring_pid();
                                    }
                                    DaemonIpcCommand::ExecCmd(c) => {
                                        let action = ButtonActionConfig::Command { cmd: c };
                                        execute_action_direct(&action);
                                    }
                                    DaemonIpcCommand::ExecKey(keys) => {
                                        let action = ButtonActionConfig::KeyCombo { keys };
                                        execute_action_daemon_delayed(&action);
                                    }
                                    DaemonIpcCommand::ReloadConfig => {
                                        println!("🔄 Received CONFIG:RELOAD. Reloading configuration from disk...");
                                        let new_cfg = Config::load_or_default();
                                        if let Ok(mut lock) = button_mappings_client.write() {
                                            *lock = new_cfg.buttons;
                                            println!("✅ Configuration reloaded: {} button mappings active", lock.len());
                                        }
                                    }
                                }
                            }
                        }
                    });
            }
            let _ = std::fs::remove_file(sock_path);
        });
    }

    clients
}

fn toggle_ring_menu() {
    let pid_path = ring_menu::ring_pid_file_path();
    let current_pid = std::process::id() as i32;
    if pid_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&pid_path) {
            if let Ok(pid) = content.trim().parse::<i32>() {
                if pid != current_pid {
                    let is_alive = platform::overlay::is_pid_alive(pid);
                    if is_alive && ring_menu::terminate_ring_process(pid) {
                        println!("⏹️ Closed active Ring Menu (PID {})...", pid);
                        let _ = std::fs::remove_file(&pid_path);
                        return;
                    }
                }
            }
        }
        let _ = std::fs::remove_file(&pid_path);
    }

    let active_app = active_window::detect_active_window();
    if let Some((_, _, pid)) = active_window::frontmost_app() {
        platform::overlay::set_last_target_pid(pid);
    }

    println!("✨ Opening Ring Menu! (Detected active app: {})", active_app);
    let exe = env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("bo-ring"));
    let mut cmd = std::process::Command::new(&exe);
    cmd.arg("--ring");
    if !active_app.is_empty() {
        cmd.arg("--app").arg(&active_app);
    }

    match cmd.spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(e) => {
            eprintln!("⚠️ Failed to launch Ring Menu process: {}", e);
        }
    }
}


