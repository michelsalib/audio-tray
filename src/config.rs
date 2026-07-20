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
}
