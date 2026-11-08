//! Autostart integration for macOS (launchd), keeping the Linux function names
//! so the Settings tab is unchanged:
//!
//! | Linux name            | macOS meaning                                              |
//! |-----------------------|------------------------------------------------------------|
//! | `*_systemd_*`         | LaunchAgent `dev.bo-ring.daemon` running `bo-ring --daemon` (KeepAlive) |
//! | `*_xdg_autostart*`    | LaunchAgent `dev.bo-ring.config` opening the config UI at login |
//! | `*_gnome_menu*`       | `~/Applications/Bo-Ring.app` launcher (Spotlight / Launchpad) |

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

pub const DAEMON_LABEL: &str = "dev.bo-ring.daemon";
pub const CONFIG_LABEL: &str = "dev.bo-ring.config";

pub fn get_exe_path() -> PathBuf {
    env::current_exe().unwrap_or_else(|_| PathBuf::from("bo-ring"))
}

pub fn get_home_dir() -> PathBuf {
    env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/tmp"))
}

fn launch_agents_dir() -> PathBuf {
    get_home_dir().join("Library/LaunchAgents")
}

fn uid() -> u32 {
    unsafe { libc::getuid() }
}

fn domain() -> String {
    format!("gui/{}", uid())
}

pub fn log_dir() -> PathBuf {
    get_home_dir().join("Library/Logs/bo-ring")
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn agent_plist(label: &str, args: &[&str], keep_alive: bool, log_name: &str) -> String {
    let exe = get_exe_path().to_string_lossy().to_string();
    let mut argv = format!("        <string>{}</string>\n", xml_escape(&exe));
    for a in args {
        argv.push_str(&format!("        <string>{}</string>\n", xml_escape(a)));
    }
    let logs = log_dir();
    let _ = fs::create_dir_all(&logs);
    let log = xml_escape(&logs.join(log_name).to_string_lossy());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key><string>{label}</string>
    <key>ProgramArguments</key>
    <array>
{argv}    </array>
    <key>RunAtLoad</key><true/>
    <key>KeepAlive</key><{ka}/>
    <key>ProcessType</key><string>Interactive</string>
    <key>LimitLoadToSessionType</key><string>Aqua</string>
    <key>StandardOutPath</key><string>{log}</string>
    <key>StandardErrorPath</key><string>{log}</string>
</dict>
</plist>
"#,
        ka = if keep_alive { "true" } else { "false" }
    )
}

fn launchctl(args: &[&str]) -> Result<String, String> {
    let out = Command::new("launchctl").args(args).output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn is_loaded(label: &str) -> bool {
    launchctl(&["print", &format!("{}/{}", domain(), label)]).is_ok()
}

fn is_running(label: &str) -> bool {
    launchctl(&["print", &format!("{}/{}", domain(), label)])
        .map(|s| s.contains("state = running"))
        .unwrap_or(false)
}

fn install_agent(label: &str, args: &[&str], keep_alive: bool, log_name: &str) -> Result<(), String> {
    let dir = launch_agents_dir();
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{label}.plist"));
    fs::write(&path, agent_plist(label, args, keep_alive, log_name)).map_err(|e| e.to_string())?;
    let _ = launchctl(&["bootout", &format!("{}/{}", domain(), label)]);
    launchctl(&["bootstrap", &domain(), &path.to_string_lossy()])?;
    let _ = launchctl(&["enable", &format!("{}/{}", domain(), label)]);
    Ok(())
}

fn remove_agent(label: &str) -> Result<(), String> {
    let _ = launchctl(&["bootout", &format!("{}/{}", domain(), label)]);
    let path = launch_agents_dir().join(format!("{label}.plist"));
    if path.exists() {
        fs::remove_file(&path).map_err(|e| e.to_string())?;
    }
    Ok(())
}

// ==========================================
// 1. Background daemon (LaunchAgent)
// ==========================================

pub fn systemd_service_path() -> PathBuf {
    launch_agents_dir().join(format!("{DAEMON_LABEL}.plist"))
}

pub fn is_systemd_installed() -> bool {
    systemd_service_path().exists()
}

pub fn is_systemd_enabled() -> bool {
    is_systemd_installed() && is_loaded(DAEMON_LABEL)
}

pub fn is_systemd_active() -> bool {
    is_running(DAEMON_LABEL)
}

pub fn enable_systemd_service() -> Result<(), String> {
    install_agent(DAEMON_LABEL, &["--daemon"], true, "daemon.log")
}

pub fn disable_systemd_service() -> Result<(), String> {
    remove_agent(DAEMON_LABEL)
}

pub fn restart_systemd_service_if_active() {
    if is_systemd_active() {
        let _ = launchctl(&["kickstart", "-k", &format!("{}/{}", domain(), DAEMON_LABEL)]);
    }
}

/// GNOME-only on Linux; nothing to do on macOS.
pub fn ensure_gnome_show_desktop_keybinding() {}

// ==========================================
// 2. Open the configurator at login
// ==========================================

pub fn xdg_autostart_path() -> PathBuf {
    launch_agents_dir().join(format!("{CONFIG_LABEL}.plist"))
}

pub fn is_xdg_autostart_installed() -> bool {
    xdg_autostart_path().exists()
}

pub fn enable_xdg_autostart() -> Result<(), String> {
    // Write the plist only (no bootstrap) so it takes effect at next login
    // instead of opening the window right now.
    let dir = launch_agents_dir();
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    fs::write(xdg_autostart_path(), agent_plist(CONFIG_LABEL, &[], false, "config.log")).map_err(|e| e.to_string())
}

pub fn disable_xdg_autostart() -> Result<(), String> {
    remove_agent(CONFIG_LABEL)
}

// ==========================================
// 3. Applications-folder launcher
// ==========================================

pub fn gnome_menu_path() -> PathBuf {
    get_home_dir().join("Applications/Bo-Ring.app")
}

pub fn gnome_system_menu_path() -> PathBuf {
    PathBuf::from("/Applications/Bo-Ring.app")
}

pub fn is_gnome_menu_system() -> bool {
    gnome_system_menu_path().exists()
}

/// Create a minimal `Bo-Ring.app` that launches this binary's config UI.
pub fn install_gnome_menu() -> Result<(), String> {
    let app = gnome_menu_path();
    let macos = app.join("Contents/MacOS");
    fs::create_dir_all(&macos).map_err(|e| e.to_string())?;
    let exe = get_exe_path().to_string_lossy().replace('\'', "'\\''");
    let script = format!("#!/bin/sh\nexec '{exe}' \"$@\"\n");
    let launcher = macos.join("bo-ring-launcher");
    fs::write(&launcher, script).map_err(|e| e.to_string())?;
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&launcher, fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    let plist = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>Bo-Ring</string>
    <key>CFBundleIdentifier</key><string>dev.bo-ring.launcher</string>
    <key>CFBundleExecutable</key><string>bo-ring-launcher</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>0.2.0</string>
    <key>LSMinimumSystemVersion</key><string>11.0</string>
</dict>
</plist>
"#;
    fs::write(app.join("Contents/Info.plist"), plist).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn remove_gnome_menu() -> Result<(), String> {
    let app = gnome_menu_path();
    if app.exists() {
        fs::remove_dir_all(&app).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Linux checked /dev/uinput; on macOS synthesizing keys needs Accessibility.
pub fn check_uinput_access() -> bool {
    super::permissions::accessibility_granted()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_plist_contents() {
        let p = agent_plist(DAEMON_LABEL, &["--daemon"], true, "daemon.log");
        assert!(p.contains("<string>dev.bo-ring.daemon</string>"));
        assert!(p.contains("<string>--daemon</string>"));
        assert!(p.contains("<key>KeepAlive</key><true/>"));
        assert!(p.contains("daemon.log"));
    }
}
