use std::fs;
use std::io::{self, Write};

use super::autostart;

/// Interactive uninstallation routine for Bo-Ring on macOS.
pub fn run_uninstall() {
    let home = autostart::get_home_dir();

    let files_to_remove = [
        home.join(".local/bin/bo-ring"),
        autostart::systemd_service_path(),
        autostart::xdg_autostart_path(),
    ];
    let dirs_to_remove = [
        home.join(".config/bo-ring"),
        home.join(".local/share/bo-ring"),
        autostart::gnome_menu_path(),
        autostart::log_dir(),
    ];

    println!("\n🗑️  Bo-Ring Uninstaller (macOS)\n");
    println!("The following will be removed:\n");
    for f in files_to_remove.iter().filter(|f| f.exists()) {
        println!("  • {}", f.display());
    }
    for d in dirs_to_remove.iter().filter(|d| d.exists()) {
        println!("  • {}/ (directory)", d.display());
    }
    println!();

    print!("\x1b[31mProceed with uninstallation? [y/N] \x1b[0m");
    let _ = io::stdout().flush();
    let mut input = String::new();
    if io::stdin().read_line(&mut input).is_err() || !input.trim().eq_ignore_ascii_case("y") {
        println!("Uninstallation cancelled.");
        return;
    }

    let _ = autostart::disable_systemd_service();
    let _ = autostart::disable_xdg_autostart();

    for f in files_to_remove.iter().filter(|f| f.exists()) {
        match fs::remove_file(f) {
            Ok(()) => println!("  ✗ Removed {}", f.display()),
            Err(e) => eprintln!("  ⚠️  Failed to remove {}: {}", f.display(), e),
        }
    }
    for d in dirs_to_remove.iter().filter(|d| d.exists()) {
        match fs::remove_dir_all(d) {
            Ok(()) => println!("  ✗ Removed {}/", d.display()),
            Err(e) => eprintln!("  ⚠️  Failed to remove {}: {}", d.display(), e),
        }
    }

    println!("\nℹ️  Remove Bo-Ring from System Settings → Privacy & Security → Accessibility");
    println!("   and Input Monitoring if you no longer need it.");
    println!("\n✅ Bo-Ring has been uninstalled.\n");
}
