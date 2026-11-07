//! Persisted per-device icon choices (plan §6). Keyed by the stable endpoint id string,
//! never the friendly name. Stored as TOML under `%APPDATA%\AudioTray\config.toml`.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::icons::IconId;

/// Unknown sections (e.g. a legacy `[taskbar]`) must be ignored, not rejected: a failed
/// parse falls back to defaults and loses the user's icon choices.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// endpoint id string -> chosen built-in icon
    pub icons: HashMap<String, IconId>,
    /// The YouTube Music half of the app — see [`crate::music`].
    pub music: Music,
    /// What the taskbar buttons change in Windows' own tray — see [`crate::taskbar`]. Not called
    /// `[taskbar]`: a legacy section of that name may still sit in old files with other keys.
    pub strip: Strip,
    /// The floating edge pill — see [`crate::pill`].
    pub pill: Pill,
    /// The Discord integration: its local RPC as the user's own application, and the Vencord
    /// plugin's pipe.
    pub discord: Discord,
    /// The Teams integration: Teams' local third-party app API.
    pub teams: Teams,
    /// Every tab playing media, through the browser extension in `integrations/browser`.
    pub browser: Browser,
    /// MPC-HC's session: followed, or left out.
    pub mpc: Mpc,
    /// A keyboard layout per physical keyboard — see [`crate::keyboards`].
    pub keyboards: Keyboards,
}

/// Switching the keyboard layout by which keyboard is typed on. Off by default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Keyboards {
    pub enabled: bool,
    /// Confirm each switch on the pill, like a new song.
    pub show_readout: bool,
    /// One per keyboard; a keyboard not listed switches nothing.
    pub assigned: Vec<AssignedKeyboard>,
}

impl Default for Keyboards {
    fn default() -> Self {
        Self { enabled: false, show_readout: true, assigned: Vec::new() }
    }
}

/// A keyboard and the layout it types in.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AssignedKeyboard {
    /// Its Raw Input interface path, stable across reconnects (the session's handle is not).
    pub device_path: String,
    /// What the user calls it.
    pub name: String,
    /// The full layout handle, in hex (`0409040C`): never only the language. A layout uninstalled
    /// since leaves the entry inert, not broken.
    pub hkl: String,
}

impl AssignedKeyboard {
    pub fn hkl(&self) -> Option<u64> {
        let digits = self.hkl.trim().trim_start_matches("0x").trim_start_matches("0X");
        u64::from_str_radix(digits, 16).ok()
    }
}

/// The floating pill on a screen's edge.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Pill {
    pub enabled: bool,
    /// Which edge of the screen it hugs.
    pub edge: PillEdge,
    /// Where along that edge: its start (the top, or the left of the top edge), middle or end.
    pub align: PillAlign,
    /// The display, by its device name (`\\.\DISPLAY2`). `None`, or a display no longer
    /// connected, is the one at the chosen edge of the desktop (see [`crate::pill`]).
    pub monitor: Option<String>,
    /// The red "is listening" alert when an app records outside a known call.
    pub listening_alerts: bool,
    /// The output and input devices on the pill, clicked to switch like the taskbar buttons.
    pub devices: bool,
    /// How much the pill shows at rest; hovering the edge always opens the full card, and warnings
    /// and alerts always show in full.
    pub size: PillSize,
    /// A new song shows its title for a moment, as a device change does.
    pub announce_tracks: bool,
}

/// How much the pill shows at rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PillSize {
    /// Only the sliver.
    Hidden,
    /// The same column at about half the size: small covers and rings, small device icons.
    Small,
    /// Covers with progress rings, LIVE badges, device buttons.
    #[default]
    Big,
}

/// The screen edge the pill hugs. Not the bottom: that is the taskbar's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PillEdge {
    #[default]
    Left,
    Right,
    Top,
}

impl PillEdge {
    /// The pill runs down the edge (a column), not along the top (a row).
    pub fn vertical(self) -> bool {
        self != PillEdge::Top
    }
}

/// Where along its edge the pill sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PillAlign {
    Start,
    #[default]
    Center,
    End,
}

impl Default for Pill {
    fn default() -> Self {
        Self {
            enabled: true,
            edge: PillEdge::Left,
            align: PillAlign::Center,
            monitor: None,
            listening_alerts: true,
            devices: true,
            size: PillSize::Big,
            announce_tracks: false,
        }
    }
}

/// How the music tile draws into the player's taskbar button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TileStyle {
    /// The button stays the shell's.
    #[default]
    Off,
    /// The cover, with the shell's progress bar under it.
    Cover,
    /// The cover, the title and artist, and the progress bar.
    Full,
}

/// Discord: through its local RPC when `client_id` and `client_secret` name your application,
/// and through `\\.\pipe\audio-tray`, which the Vencord plugin in `integrations/vencord` connects to.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Discord {
    pub enabled: bool,
    /// Your own Discord application (discord.com/developers), for Discord's local RPC: its voice
    /// scopes are granted to the application's owner and testers. Without them, only the Vencord
    /// plugin's pipe is served.
    pub client_id: Option<String>,
    /// Kept in the Credential Manager ([`crate::secrets`]), never written here; read from an
    /// older file once, and moved on the next save.
    #[serde(skip_serializing)]
    pub client_secret: Option<String>,
}

impl Default for Discord {
    fn default() -> Self {
        Self { enabled: true, client_id: None, client_secret: None }
    }
}

/// MPC-HC, followed through its own Windows media controls ([`crate::integrations::mpc`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Mpc {
    /// Off: its session is left out, whatever MPC-HC shares.
    pub enabled: bool,
}

impl Default for Mpc {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// The browser extension's native messaging host is registered (per user) while this is on.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Browser {
    pub enabled: bool,
    /// Follow every tab that plays, or only those of `sites`.
    pub every_site: bool,
    /// The sites followed when not every one is, by the keys of
    /// [`crate::integrations::browser::SITES`] ("youtube", "twitch").
    pub sites: Vec<String>,
}

impl Default for Browser {
    fn default() -> Self {
        Self { enabled: true, every_site: true, sites: vec!["youtube".into(), "twitch".into()] }
    }
}

/// Teams' local API must also be turned on in Teams (Settings > Privacy > Manage API); the first
/// connection asks for approval inside Teams, which then hands over the `token` (kept in the
/// Credential Manager).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Teams {
    pub enabled: bool,
    /// Kept in the Credential Manager, like [`Discord::client_secret`].
    #[serde(skip_serializing)]
    pub token: Option<String>,
}

/// Switches for each edit the taskbar integration makes to the shell. Injecting at all is opt-in
/// (`enabled`); once on, every edit is on by default, and turning one off is also the way to rule
/// it out when the taskbar misbehaves.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Strip {
    /// Draw the buttons into the taskbar by injecting into Explorer. Off: a plain tray icon.
    pub enabled: bool,
    /// Move the notification icons (our buttons among them) between the keyboard-layout indicator
    /// and wifi. Off: they stay where Windows puts them.
    pub next_to_wifi: bool,
    /// Hide Windows' volume icon, which the output button duplicates.
    pub hide_windows_volume: bool,
    /// Hide Windows' "microphone in use" icon, which the input button's red dot replaces.
    pub hide_windows_mic: bool,
    /// Hide Windows' keyboard-layout indicator ("FRA"). Off by default: unlike the two above it is
    /// not replaced by anything; Win+Space still switches. Needs the TAP (buttons or music tile).
    pub hide_windows_language: bool,
    /// What a left click on the output or input button does.
    pub left_click: LeftClick,
    /// Switching devices (a button's or the pill's click) stops once on the last device, muted,
    /// before going back to the first.
    pub mute_in_cycle: bool,
}

impl Default for Strip {
    fn default() -> Self {
        Self {
            enabled: false,
            next_to_wifi: true,
            hide_windows_volume: true,
            hide_windows_mic: true,
            hide_windows_language: false,
            left_click: LeftClick::Switch,
            mute_in_cycle: true,
        }
    }
}

/// A left click on a taskbar audio button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LeftClick {
    /// Step that flow to its next device (with one muted stop before the wrap, unless
    /// [`Strip::mute_in_cycle`] is off).
    #[default]
    Switch,
    /// Mute or unmute that flow's default device.
    Mute,
    /// Open Audio Tray's panel, as a right click does.
    Panel,
}

/// Following YouTube Music and drawing it into the taskbar. On by default: it does nothing
/// without a YouTube Music media session.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Music {
    /// Follow the player at all. `false` turns the whole feature off, progress bar included.
    pub enabled: bool,
    /// Draw the strip into this app's own taskbar button, matched on its name. Empty means no
    /// strip; the feed and the progress bar still work.
    pub tile: String,
    /// What the tile shows. Drawing it injects into Explorer, so it is opt-in.
    pub tile_style: TileStyle,
    /// Pin the SMTC app id instead of guessing it (`--music-probe` prints it). Required for a
    /// plain browser tab, which is never followed otherwise; see [`crate::music::session`].
    pub app_id: Option<String>,
}

impl Default for Music {
    fn default() -> Self {
        Self {
            enabled: true,
            tile: "YouTube Music".to_string(),
            tile_style: TileStyle::Off,
            app_id: None,
        }
    }
}

impl Config {
    /// Whether anything is drawn into the taskbar (the audio buttons or the music tile), which is
    /// what injecting into Explorer is for.
    pub fn injects(&self) -> bool {
        self.strip.enabled || self.tile_shown()
    }

    /// Whether the music tile is drawn.
    pub fn tile_shown(&self) -> bool {
        self.music.enabled && self.music.tile_style != TileStyle::Off && !self.music.tile.is_empty()
    }
    /// `%APPDATA%\AudioTray\config.toml`.
    pub fn path() -> Result<PathBuf> {
        let dirs = directories::ProjectDirs::from("", "", "AudioTray")
            .context("resolve %APPDATA% config directory")?;
        Ok(dirs.config_dir().join("config.toml"))
    }

    /// Load config, falling back to defaults if it is missing or unreadable — a bad config file
    /// must never prevent the tray from starting.
    pub fn load() -> Self {
        let mut config = match Self::path() {
            Ok(path) => Self::load_from(&path),
            Err(e) => {
                eprintln!("config: using defaults ({e:#})");
                Self::default()
            }
        };
        // The secrets live in the Credential Manager; one still in an older file wins until saved.
        if config.discord.client_secret.is_none() {
            config.discord.client_secret = crate::secrets::get(crate::secrets::DISCORD_SECRET);
        }
        if config.teams.token.is_none() {
            config.teams.token = crate::secrets::get(crate::secrets::TEAMS_TOKEN);
        }
        config
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
        match Self::parse(&text) {
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

    /// Parse a config file, bringing one written by an older version forward.
    fn parse(text: &str) -> std::result::Result<Self, toml::de::Error> {
        let mut config: Self = toml::from_str(text)?;
        let raw: toml::Table = toml::from_str(text)?;
        let has = |section: &str, key: &str| raw.get(section).and_then(|s| s.get(key)).is_some();
        // Versions before these switches drew the buttons and the tile whenever they ran, and every
        // version since writes the keys: a file without them is an upgrade, which keeps both.
        if !has("strip", "enabled") {
            config.strip.enabled = true;
        }
        if !has("music", "tile_style") && !config.music.tile.is_empty() {
            config.music.tile_style = TileStyle::Full;
        }
        Ok(config)
    }

    pub fn save(&self) -> Result<()> {
        crate::secrets::set(crate::secrets::DISCORD_SECRET, self.discord.client_secret.as_deref());
        crate::secrets::set(crate::secrets::TEAMS_TOKEN, self.teams.token.as_deref());
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

    /// The icon to draw for a device: the user's choice, else the form-factor default.
    /// Every surface (tray, strip, flyout) must resolve icons through this.
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

    /// TOML wants plain values before tables; a misplaced field fails `save()` at runtime.
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

    #[test]
    fn an_upgrade_keeps_the_taskbar_buttons_and_the_tile() {
        let old = "[music]\nenabled = true\ntile = \"YouTube Music\"\n\n[strip]\nnext_to_wifi = true\n";
        let cfg = Config::parse(old).expect("parse");
        assert!(cfg.strip.enabled, "older versions always drew the buttons");
        assert_eq!(cfg.music.tile_style, TileStyle::Full);
        let chosen = "[music]\ntile = \"YouTube Music\"\ntile_style = \"off\"\n\n[strip]\nenabled = false\n";
        let cfg = Config::parse(chosen).expect("parse");
        assert!(!cfg.strip.enabled, "a choice made in Settings stays");
        assert_eq!(cfg.music.tile_style, TileStyle::Off);
    }

    #[test]
    fn the_pill_stays_where_it_was_unless_placed() {
        let cfg = Config::parse("[pill]\nsize = \"small\"\n").expect("parse");
        assert_eq!((cfg.pill.edge, cfg.pill.align, cfg.pill.monitor), (PillEdge::Left, PillAlign::Center, None));
        let mut placed = Config::default();
        placed.pill.edge = PillEdge::Top;
        placed.pill.align = PillAlign::End;
        placed.pill.monitor = Some(r"\\.\DISPLAY2".into());
        let back: Config = toml::from_str(&toml::to_string_pretty(&placed).unwrap()).unwrap();
        assert_eq!((back.pill.edge, back.pill.align), (PillEdge::Top, PillAlign::End));
        assert_eq!(back.pill.monitor.as_deref(), Some(r"\\.\DISPLAY2"));
    }

    /// Configs from when the strip was opt-in carry a `[taskbar]` section; it must be ignored.
    #[test]
    fn legacy_config_with_a_taskbar_section_still_loads() {
        let legacy = "[icons]\n\"{0.0.0.00000000}.{abc}\" = \"Speakers\"\n\n[taskbar]\nenabled = true\n";
        let cfg: Config = toml::from_str(legacy).expect("deserialize legacy");
        assert_eq!(cfg.icon_for("{0.0.0.00000000}.{abc}"), Some(IconId::Speakers));
    }
}
