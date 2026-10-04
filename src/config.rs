//! Persisted per-device icon choices (plan §6). Keyed by the stable endpoint id string,
//! never the friendly name. Stored as TOML under `%APPDATA%\AudioTray\config.toml`.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::icons::IconId;

/// Unknown sections are ignored rather than rejected, which is what lets a config
/// written by an older build load unchanged — `[taskbar]`, the opt-in that used to
/// choose between the plain tray icon and the taskbar strip, is the case that
/// matters today. Rejecting it would throw away the user's icon choices along with
/// it.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// endpoint id string -> chosen built-in icon
    pub icons: HashMap<String, IconId>,
    /// The YouTube Music half of the app — see [`crate::music`].
    pub music: Music,
}

/// Following YouTube Music, and drawing it into the taskbar.
///
/// **On by default, which is defensible because it is invisible until it applies.** Nothing here
/// happens without a YouTube Music media session on the machine: no session, no state to publish, no
/// progress bar, no tile. A user who never opens the player never sees a difference.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Music {
    /// Follow the player at all. `false` turns the whole feature off, progress bar included.
    pub enabled: bool,
    /// Draw the strip into this app's own taskbar button, matched on its name.
    ///
    /// The app's *own* button, so the shell keeps doing the things it already does well: launching
    /// adds no second icon, minimising goes there, and dragging reorders it. Empty means no strip —
    /// the feed and the progress bar still work.
    pub tile: String,
    /// Pin the SMTC app id instead of guessing it.
    ///
    /// Needed when the built-in matching misses an unusual build — the id is a Chromium
    /// implementation detail, and `audio-tray --music-probe` prints the real one — and when
    /// YouTube Music runs as a **plain browser tab**, which reports a bare `MSEdge`/`Chrome` that
    /// is indistinguishable from any other tab. That one is never followed unless it is pinned
    /// here; see [`crate::music::session`].
    pub app_id: Option<String>,
}

impl Default for Music {
    fn default() -> Self {
        Self {
            enabled: true,
            tile: "YouTube Music".to_string(),
            app_id: None,
        }
    }
}

impl Config {
    /// `%APPDATA%\AudioTray\config.toml`.
    pub fn path() -> Result<PathBuf> {
        let dirs = directories::ProjectDirs::from("", "", "AudioTray")
            .context("resolve %APPDATA% config directory")?;
        Ok(dirs.config_dir().join("config.toml"))
    }

    /// Load config, falling back to defaults if it is missing or unreadable — a bad config file
    /// must never prevent the tray from starting.
    pub fn load() -> Self {
        match Self::path() {
            Ok(path) => Self::load_from(&path),
            Err(e) => {
                eprintln!("config: using defaults ({e:#})");
                Self::default()
            }
        }
    }

    /// A file that does not parse is renamed to `config.toml.bad` (replacing an older one) rather
    /// than left for the next [`Config::save`] to overwrite: the user's icon choices are in it.
    fn load_from(path: &std::path::Path) -> Self {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(e) => {
                eprintln!("config: could not read {} ({e}); using defaults", path.display());
                return Self::default();
            }
        };
        match toml::from_str(&text) {
            Ok(config) => config,
            Err(e) => {
                let bad = path.with_extension("toml.bad");
                match std::fs::rename(path, &bad) {
                    Ok(()) => eprintln!("config: {} is invalid ({e}); kept it as {} and using defaults", path.display(), bad.display()),
                    Err(err) => eprintln!("config: {} is invalid ({e}) and could not be set aside ({err}); using defaults", path.display()),
                }
                Self::default()
            }
        }
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

    /// The icon to draw for a device: the user's choice if there is one, otherwise the
    /// form-factor default.
    ///
    /// One place, because all three surfaces have to agree — the tray icon, the strip and the
    /// flyout's device rows. They did not when each resolved it for itself: the same speaker
    /// appeared as a laptop in one and a speaker in another.
    pub fn icon_of(&self, device: &crate::audio::Device) -> IconId {
        self.icon_for(&device.id.0).unwrap_or_else(|| {
            crate::icons::default_icon(device.form_factor, &device.friendly_name)
        })
    }

    pub fn set_icon(&mut self, device_id: String, icon: IconId) {
        self.icons.insert(device_id, icon);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// TOML requires plain values before tables, so adding a plain field after the
    /// icons map is exactly the kind of change that makes `save()` fail at runtime
    /// while still compiling. Round-trip it.
    #[test]
    fn round_trips_through_toml() {
        let mut cfg = Config::default();
        cfg.set_icon("{0.0.0.00000000}.{abc}".into(), IconId::Speakers);

        let text = toml::to_string_pretty(&cfg).expect("serialize");
        let back: Config = toml::from_str(&text).expect("deserialize");

        assert_eq!(back.icon_for("{0.0.0.00000000}.{abc}"), Some(IconId::Speakers));
    }

    #[test]
    fn a_corrupt_config_is_set_aside_not_overwritten() {
        let dir = std::env::temp_dir().join(format!("audio-tray-config-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "[icons
this is not toml").unwrap();

        let cfg = Config::load_from(&path);
        assert!(cfg.icons.is_empty(), "defaults");
        assert!(!path.exists(), "the bad file is moved out of the way");
        let bad = std::fs::read_to_string(dir.join("config.toml.bad")).unwrap();
        assert!(bad.contains("not toml"), "and kept intact");
        // Missing is plain defaults, with nothing renamed.
        assert!(Config::load_from(&path).icons.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Every config written while the taskbar strip was an opt-in still has a
    /// `[taskbar]` section. Loading must ignore it, not fail — a rejected parse
    /// falls back to defaults and silently drops the user's icon choices.
    #[test]
    fn legacy_config_with_a_taskbar_section_still_loads() {
        let legacy = "[icons]\n\"{0.0.0.00000000}.{abc}\" = \"Speakers\"\n\n[taskbar]\nenabled = true\n";
        let cfg: Config = toml::from_str(legacy).expect("deserialize legacy");
        assert_eq!(cfg.icon_for("{0.0.0.00000000}.{abc}"), Some(IconId::Speakers));
    }
}
