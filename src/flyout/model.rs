//! The flyout's display model: plain data for what the panel shows and the outcome flags.
//! No Win32/COM handles (those stay on the controller), so tests can build it.

use crate::audio::wasapi::WasapiBackend;
use crate::audio::{DeviceId, Flow};
use crate::config::Config;
use crate::icons::IconId;

use super::layout::View;

pub(super) struct DeviceRow {
    pub id: DeviceId,
    pub label: String,
    pub icon: IconId,
    pub selected: bool,
    pub battery: Option<u8>, // Bluetooth battery 0..=100, if the device reports one
}

pub(super) struct Group {
    pub flow: Flow,
    pub title: &'static str,
    pub default_id: Option<DeviceId>,
    pub level: f32, // 0.0..=1.0 of the default endpoint
    pub muted: bool,
    pub peak: f32, // smoothed live peak level 0.0..=1.0 of the default endpoint (activity glow)
    /// An app has the microphone open (any endpoint; see [`crate::audio::mic`]). Input only.
    pub recording: bool,
    pub devices: Vec<DeviceRow>,
}

/// A Now Playing card: one media session the pill shows.
#[derive(Clone, PartialEq)]
pub(super) struct NowPlaying {
    /// Which session the buttons command.
    pub app_id: String,
    pub title: String,
    pub artist: String,
    pub app: String,
    pub playing: bool,
    /// A stream with no end: no previous/next.
    pub live: bool,
    /// Previous/next reach the player.
    pub skip: bool,
    /// The cover as a ready sprite, [`ART_DIP`] square at the panel's scale.
    pub art: Option<Vec<u8>>,
}

/// One app in the mixer.
#[derive(Clone, PartialEq)]
pub(super) struct AppRow {
    pub app: String,
    pub level: f32,
    pub muted: bool,
}

/// The voice panel: a call an integration reports, or a meeting app holding the microphone.
#[derive(Clone, PartialEq)]
pub(super) struct Voice {
    pub app: Option<crate::hub::VoiceApp>,
    pub title: String,
    /// (name, speaking, muted), at most a handful.
    pub members: Vec<(String, bool, bool)>,
    pub muted: bool,
}

/// The settings screen's values, copied from the config when the panel opens.
#[derive(Clone, Default)]
pub(super) struct Settings {
    pub tile: crate::config::TileStyle,
    pub buttons: bool,
    pub pill: bool,
    pub devices: bool,
    pub size: crate::config::PillSize,
    pub alerts: bool,
    pub browser: bool,
    pub discord: bool,
    pub discord_id: Option<String>,
    /// The secret's last characters, when there is one: it is never shown whole.
    pub discord_secret: Option<String>,
}

impl Settings {
    pub(super) fn of(config: &Config) -> Self {
        Self {
            tile: config.music.tile_style,
            buttons: config.strip.enabled,
            pill: config.pill.enabled,
            devices: config.pill.devices,
            size: config.pill.size,
            alerts: config.pill.listening_alerts,
            browser: config.browser.enabled,
            discord: config.discord.enabled,
            discord_id: config.discord.client_id.clone(),
            discord_secret: config.discord.client_secret.as_deref().map(|s| {
                let tail: String = s.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
                format!("••••{tail}")
            }),
        }
    }
}

/// Everything the flyout paints, plus the outcome flags the caller reads back when it
/// closes. No live audio handles — see the module docs.
pub(super) struct Model {
    pub groups: Vec<Group>,
    /// Per group: show every device (`true`) or only the selected one.
    pub expanded: Vec<bool>,
    pub now_playing: Vec<NowPlaying>,
    pub apps: Vec<AppRow>,
    pub voice: Option<Voice>,
    pub settings: Settings,
    pub view: View, // which screen is shown (main panel / an icon picker)
    pub update: Option<String>, // staged update's version, if any → the footer's restart button
    pub notice: Option<&'static str>, // taskbar integration trouble, one line above the footer
    /// Which integrations are connected, by name (the hub's), for the connectors' status.
    pub integrations: Vec<(&'static str, bool)>,
    /// A connector row's one-off message in place of its value (a paste that found nothing).
    pub hint: Option<(super::layout::SettingKind, &'static str)>,
    /// Why Discord's RPC last failed, in a few words (a missing redirect, a wrong secret).
    pub discord_problem: Option<&'static str>,
    // outcome, accumulated while the flyout is open
    pub config_changed: bool,
    pub quit: bool,
    pub restart: bool,
    pub taskbar_changed: bool,
}

impl Model {
    pub(super) fn new(groups: Vec<Group>, update: Option<String>) -> Self {
        Model {
            expanded: vec![false; groups.len()],
            now_playing: Vec::new(),
            apps: Vec::new(),
            voice: None,
            settings: Settings::default(),
            groups,
            view: View::Main,
            update,
            notice: None,
            integrations: Vec::new(),
            hint: None,
            discord_problem: None,
            config_changed: false,
            quit: false,
            restart: false,
            taskbar_changed: false,
        }
    }
}

/// The Now Playing card's art size, in DIPs.
pub(super) const ART_DIP: f32 = 52.0;

/// The hub's media, sessions and call as the flyout shows them: a Now Playing card per playing
/// session, else for the first paused one. A card already shown (`old`) stays while its session
/// lives, paused or not: a card vanishing on pause would slide the next one under the pointer.
/// `art_px` is the cover's size at the panel's scale; decoding is skipped when `old` already
/// holds a track's sprite.
pub(super) fn from_hub(state: &crate::hub::State, art_px: u32, old: &[NowPlaying]) -> (Vec<NowPlaying>, Vec<AppRow>, Option<Voice>) {
    let all = state.all_media();
    let mut shown: Vec<&crate::hub::Media> = all
        .iter()
        .copied()
        .filter(|m| m.playing || old.iter().any(|o| o.app_id == m.app_id))
        .take(crate::hub::MAX_SHOWN)
        .collect();
    if shown.is_empty() {
        shown.extend(all.first().copied());
    }
    let now_playing = shown.into_iter().map(|m| {
        let same_track = old.iter().find(|o| o.title == m.title && o.artist == m.artist && o.app == m.app && o.art.is_some());
        let art = match same_track {
            Some(old) => old.art.clone(),
            None => m
                .cover
                .as_ref()
                .and_then(|bytes| crate::image::sprite(bytes, art_px, art_px as f32 * 0.15).ok()),
        };
        NowPlaying {
            app_id: m.app_id.clone(),
            title: m.title.clone(),
            artist: m.artist.clone(),
            app: m.app.clone(),
            playing: m.playing,
            live: m.length_seconds().is_none(),
            skip: m.skip,
            art,
        }
    }).collect();
    // Cards already shown keep their place; one whose session went away stays, paused, so the
    // next card never slides under the pointer while the panel is open.
    let fresh: Vec<NowPlaying> = now_playing;
    let mut now_playing: Vec<NowPlaying> = old
        .iter()
        .map(|o| fresh.iter().find(|n| n.app_id == o.app_id).cloned().unwrap_or_else(|| NowPlaying { playing: false, ..o.clone() }))
        .collect();
    now_playing.extend(fresh.into_iter().filter(|n| !old.iter().any(|o| o.app_id == n.app_id)));
    let apps = state
        .sessions
        .iter()
        .map(|s| AppRow { app: s.app.clone(), level: s.volume, muted: s.muted })
        .collect();
    let voice = match state.call() {
        Some((app, call)) => Some(Voice {
            app: Some(app),
            title: match &call.channel {
                Some(channel) => format!("{} · {channel}", app.label()),
                None => app.label().to_string(),
            },
            members: call.members.iter().map(|m| (m.name.clone(), m.speaking, m.muted)).collect(),
            muted: call.self_muted,
        }),
        None => state
            .mic_users
            .iter()
            .map(|u| crate::hub::names::app_name(u))
            .find(|name| crate::hub::names::is_meeting_app(name))
            .map(|name| Voice {
                app: None,
                title: format!("{name} is using the microphone"),
                members: Vec::new(),
                muted: state.endpoints.input.as_ref().is_some_and(|e| e.muted),
            }),
    };
    (now_playing, apps, voice)
}

/// Read the current output + input state into display groups. Groups with no devices are
/// omitted (e.g. a machine with no microphone shows no Input section).
pub(super) fn build_groups(backend: &WasapiBackend, config: &Config) -> Vec<Group> {
    // Bluetooth battery levels keyed by ContainerId — enumerated once for all devices.
    let batteries = crate::audio::battery::levels();
    let battery_of = |container: &Option<String>| -> Option<u8> {
        let c = container.as_ref()?;
        batteries
            .iter()
            .find(|(id, _)| id.eq_ignore_ascii_case(c))
            .map(|(_, pct)| *pct)
    };

    let mut groups = Vec::new();
    for (flow, title) in [(Flow::Output, "Output"), (Flow::Input, "Input")] {
        let devices = backend.enumerate_flow(flow).unwrap_or_default();
        if devices.is_empty() {
            continue;
        }
        let default_id = backend.default_of(flow).ok().flatten();
        let (level, muted) = match &default_id {
            Some(id) => (
                backend.volume_of(id).unwrap_or(0.0),
                backend.is_muted(id).unwrap_or(false),
            ),
            None => (0.0, false),
        };
        let rows = devices
            .into_iter()
            .map(|d| {
                let icon = config.icon_of(&d);
                let selected = default_id.as_ref() == Some(&d.id);
                let battery = battery_of(&d.container_id);
                DeviceRow { id: d.id, label: d.friendly_name, icon, selected, battery }
            })
            .collect();
        // Only the input side can be "recording"; an output group's dot would be nonsense.
        let recording = flow == Flow::Input && crate::audio::mic::in_use();
        groups.push(Group {
            flow,
            title,
            default_id,
            level,
            muted,
            peak: 0.0,
            recording,
            devices: rows,
        });
    }
    groups
}
