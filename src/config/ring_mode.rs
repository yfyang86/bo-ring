//! Which ring the overlay shows — switchable at runtime from the menu-bar icon
//! or `bo-ring --ring-mode`.
//!
//! * `auto`    — follow the frontmost app (per-app rings; the upstream behaviour)
//! * `default` — always the main ring
//! * `app:<app_id>` — always the profile whose `app_id` field equals `<app_id>`
//!
//! Stored in its own file (`~/.config/bo-ring/ring-mode`) rather than in
//! `config.toml`, so the configurator's Save (which rewrites `config.toml` from
//! its in-memory copy) can never revert a switch made from the menu bar.

use super::{AppProfileConfig, Config};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum RingMode {
    #[default]
    Auto,
    Default,
    Profile(String),
}

impl RingMode {
    pub fn parse(s: &str) -> RingMode {
        let t = s.trim();
        match t.to_ascii_lowercase().as_str() {
            "" | "auto" | "app" | "follow" => RingMode::Auto,
            "default" | "main" | "global" => RingMode::Default,
            _ => match t.strip_prefix("app:") {
                Some(id) if !id.trim().is_empty() => RingMode::Profile(id.trim().to_string()),
                _ => RingMode::Auto,
            },
        }
    }

    pub fn as_config_str(&self) -> String {
        match self {
            RingMode::Auto => "auto".into(),
            RingMode::Default => "default".into(),
            RingMode::Profile(id) => format!("app:{id}"),
        }
    }
}

pub fn mode_path() -> PathBuf {
    Config::config_dir().join("ring-mode")
}

pub fn load() -> RingMode {
    std::fs::read_to_string(mode_path()).map(|s| RingMode::parse(&s)).unwrap_or_default()
}

pub fn save(mode: &RingMode) -> std::io::Result<()> {
    let path = mode_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, format!("{}\n", mode.as_config_str()))
}

/// Resolve a user-typed name (CLI): `auto`, `default`, a profile label
/// ("Web Browsers"), one of its app ids ("vscode", "com.apple.Safari"), or `app:<app_id>`.
pub fn resolve_name(name: &str, profiles: &[AppProfileConfig]) -> Option<RingMode> {
    let n = name.trim();
    match n.to_ascii_lowercase().as_str() {
        "auto" | "follow" | "app" => return Some(RingMode::Auto),
        "default" | "main" | "global" => return Some(RingMode::Default),
        _ => {}
    }
    if let Some(id) = n.strip_prefix("app:") {
        let id = id.trim();
        return profiles.iter().any(|p| p.app_id == id).then(|| RingMode::Profile(id.to_string()));
    }
    profiles
        .iter()
        .find(|p| p.label.eq_ignore_ascii_case(n) || super::matches_app_id(&p.app_id, n))
        .map(|p| RingMode::Profile(p.app_id.clone()))
}

/// Human-readable description, using the profile's label for pinned rings.
pub fn describe(mode: &RingMode, profiles: &[AppProfileConfig]) -> String {
    match mode {
        RingMode::Auto => "follow frontmost app".to_string(),
        RingMode::Default => "default ring".to_string(),
        RingMode::Profile(id) => match profiles.iter().find(|p| &p.app_id == id) {
            Some(p) => format!("pinned: {}", p.label),
            None => "pinned profile missing → default ring".to_string(),
        },
    }
}

/// Apply the mode to the ring's inputs: (profiles to match against, app to match).
pub fn apply(
    mode: &RingMode,
    profiles: Vec<AppProfileConfig>,
    initial_app: Option<String>,
) -> (Vec<AppProfileConfig>, Option<String>) {
    match mode {
        RingMode::Auto => (profiles, initial_app),
        RingMode::Default => (Vec::new(), Some("Global / Desktop".to_string())),
        RingMode::Profile(id) => match profiles.into_iter().find(|p| &p.app_id == id) {
            Some(p) => {
                // Match on the profile's first app id so the ring selects it.
                let first = p.app_id.split(';').map(str::trim).find(|s| !s.is_empty()).unwrap_or("").to_string();
                (vec![p], Some(first))
            }
            // Pinned profile no longer exists: fall back to the main ring.
            None => (Vec::new(), Some("Global / Desktop".to_string())),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(app_id: &str, label: &str) -> AppProfileConfig {
        AppProfileConfig {
            app_id: app_id.into(),
            label: label.into(),
            translations: Default::default(),
            logo_path: String::new(),
            ring_menu: None,
        }
    }

    #[test]
    fn parse_and_roundtrip() {
        for m in [RingMode::Auto, RingMode::Default, RingMode::Profile("code; vscode".into())] {
            assert_eq!(RingMode::parse(&m.as_config_str()), m);
        }
        assert_eq!(RingMode::parse(" DEFAULT\n"), RingMode::Default);
        assert_eq!(RingMode::parse("garbage"), RingMode::Auto);
        assert_eq!(RingMode::parse("app:"), RingMode::Auto);
    }

    #[test]
    fn resolve_names() {
        let ps = vec![profile("code; vscode; com.microsoft.vscode", "VS Code"), profile("firefox; safari", "Web Browsers")];
        assert_eq!(resolve_name("auto", &ps), Some(RingMode::Auto));
        assert_eq!(resolve_name("default", &ps), Some(RingMode::Default));
        assert_eq!(resolve_name("vs code", &ps), Some(RingMode::Profile(ps[0].app_id.clone())));
        assert_eq!(resolve_name("com.microsoft.VSCode", &ps), Some(RingMode::Profile(ps[0].app_id.clone())));
        assert_eq!(resolve_name("safari", &ps), Some(RingMode::Profile(ps[1].app_id.clone())));
        assert_eq!(resolve_name("nope", &ps), None);
    }

    #[test]
    fn apply_modes() {
        let ps = vec![profile("code; vscode", "VS Code"), profile("firefox", "Web Browsers")];
        let (p, app) = apply(&RingMode::Auto, ps.clone(), Some("com.microsoft.VSCode".into()));
        assert_eq!((p.len(), app.as_deref()), (2, Some("com.microsoft.VSCode")));
        let (p, app) = apply(&RingMode::Default, ps.clone(), Some("com.microsoft.VSCode".into()));
        assert!(p.is_empty() && app.as_deref() == Some("Global / Desktop"));
        let (p, app) = apply(&RingMode::Profile("firefox".into()), ps.clone(), Some("com.microsoft.VSCode".into()));
        assert_eq!((p.len(), app.as_deref()), (1, Some("firefox")));
        assert!(super::super::matches_app_id(&p[0].app_id, app.as_deref().unwrap()));
        let (p, _) = apply(&RingMode::Profile("gone".into()), ps, None);
        assert!(p.is_empty());
    }
}
