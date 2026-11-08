//! System accent colour on macOS: `defaults read -g AppleAccentColor`.
//! Absent key = the default (multicolor/blue); -1 = graphite.

pub fn detect_os_accent_color() -> Option<String> {
    let out = std::process::Command::new("defaults")
        .args(["read", "-g", "AppleAccentColor"])
        .output()
        .ok()?;
    let raw = if out.status.success() { String::from_utf8_lossy(&out.stdout).trim().to_string() } else { String::new() };
    Some(accent_index_to_hex(&raw).to_string())
}

/// Map the AppleAccentColor index to macOS's accent hex values.
pub fn accent_index_to_hex(raw: &str) -> &'static str {
    match raw.trim() {
        "-1" => "#8C8C8C", // graphite
        "0" => "#FF5257",  // red
        "1" => "#F7821B",  // orange
        "2" => "#FFC600",  // yellow
        "3" => "#62BA46",  // green
        "5" => "#A550A7",  // purple
        "6" => "#F74F9E",  // pink
        _ => "#007AFF",    // 4 / unset: blue (multicolor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_accent_mapping() {
        assert_eq!(accent_index_to_hex(""), "#007AFF");
        assert_eq!(accent_index_to_hex("4"), "#007AFF");
        assert_eq!(accent_index_to_hex("-1"), "#8C8C8C");
        assert_eq!(accent_index_to_hex("3\n"), "#62BA46");
    }
}
