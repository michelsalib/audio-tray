//! Persisted per-device icon choices (plan §6). Keyed by the stable endpoint id string,
//! never the friendly name. Stored as TOML under `%APPDATA%\AudioTray\config.toml`.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::icons::IconId;

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// endpoint id string -> chosen built-in icon
    pub icons: HashMap<String, IconId>,
    /// Opt-in Explorer integration. See [`Taskbar`].
    pub taskbar: Taskbar,
}

/// The `[taskbar]` section: the experimental in-Explorer icon.
///
/// Off by default, and deliberately so. Enabling it injects a DLL into
/// `explorer.exe` (see `crate::taskbar`), which is unsupported by Windows, breaks
/// on Explorer updates, and conflicts with TranslucentTB / Windhawk. The plain
/// `Shell_NotifyIcon` tray entry is always registered regardless, so turning this
/// off — or having it fail — leaves the app behaving exactly as it always has.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Taskbar {
    pub enabled: bool,
}

impl Config {
    /// `%APPDATA%\AudioTray\config.toml`.
    pub fn path() -> Result<PathBuf> {
        let dirs = directories::ProjectDirs::from("", "", "AudioTray")
            .context("resolve %APPDATA% config directory")?;
        Ok(dirs.config_dir().join("config.toml"))
    }

    /// Load config, falling back to defaults if it's missing or unreadable — a bad
    /// config file must never prevent the tray from starting.
    pub fn load() -> Self {
        match Self::try_load() {
            Ok(cfg) => cfg,
            Err(e) => {
                eprintln!("config: using defaults ({e:#})");
                Self::default()
            }
        }
    }

    fn try_load() -> Result<Self> {
        let path = Self::path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        toml::from_str(&text).context("parse config.toml")
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create {}", parent.display()))?;
        }
        let text = toml::to_string_pretty(self).context("serialize config")?;
        std::fs::write(&path, text).with_context(|| format!("write {}", path.display()))?;
        Ok(())
    }

    /// User-assigned icon for a device, if any (overrides `default_icon`).
    pub fn icon_for(&self, device_id: &str) -> Option<IconId> {
        self.icons.get(device_id).copied()
    }

    pub fn set_icon(&mut self, device_id: String, icon: IconId) {
        self.icons.insert(device_id, icon);
    }

    /// Flip the opt-in Explorer integration, returning the new state.
    pub fn toggle_taskbar(&mut self) -> bool {
        self.taskbar.enabled = !self.taskbar.enabled;
        self.taskbar.enabled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// TOML requires plain values before tables, so adding a struct field after a
    /// map is exactly the kind of change that makes `save()` fail at runtime while
    /// still compiling. Round-trip it.
    #[test]
    fn round_trips_with_the_taskbar_section() {
        let mut cfg = Config::default();
        cfg.set_icon("{0.0.0.00000000}.{abc}".into(), IconId::Speakers);
        assert!(cfg.toggle_taskbar());

        let text = toml::to_string_pretty(&cfg).expect("serialize");
        let back: Config = toml::from_str(&text).expect("deserialize");

        assert!(back.taskbar.enabled);
        assert_eq!(back.icon_for("{0.0.0.00000000}.{abc}"), Some(IconId::Speakers));
    }

    /// A config written before the opt-in existed must still load, with the
    /// feature off — the default behaviour is the plain tray icon.
    #[test]
    fn legacy_config_without_taskbar_section_defaults_to_off() {
        let legacy = "[icons]\n\"{0.0.0.00000000}.{abc}\" = \"Speakers\"\n";
        let cfg: Config = toml::from_str(legacy).expect("deserialize legacy");
        assert!(!cfg.taskbar.enabled);
        assert_eq!(cfg.icon_for("{0.0.0.00000000}.{abc}"), Some(IconId::Speakers));
    }
}
