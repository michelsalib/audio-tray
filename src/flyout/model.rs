//! The flyout's display model: plain data for what the panel shows and the outcome flags.
//! No Win32/COM handles (those stay on the controller), so tests can build it.

use crate::audio::wasapi::WasapiBackend;
use crate::audio::{DeviceId, Flow};
use crate::config::Config;
use crate::icons::IconId;

pub(super) struct DeviceRow {
    pub id: DeviceId,
    pub label: String,
    pub icon: IconId,
    pub selected: bool,
    pub battery: Option<u8>, // battery 0..=100, if the device reports one (see crate::batteries)
    /// The device container, which its battery is found by.
    pub container: Option<String>,
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
    /// A video: with no cover, a video glyph.
    pub video: bool,
    /// The cover as a ready sprite, [`ART_DIP`] square at the panel's scale.
    pub art: Option<Vec<u8>>,
}

/// One app in the mixer.
#[derive(Clone, PartialEq)]
pub(super) struct AppRow {
    pub app: String,
    pub level: f32,
    pub muted: bool,
    /// The app's icon as a ready sprite ([`app_icon`]); `None` draws the generic glyph.
    pub icon: Option<std::sync::Arc<Vec<u8>>>,
}

impl AppRow {
    pub(super) fn of(session: &crate::hub::AppSession, icon_px: u32) -> Self {
        Self { app: session.app.clone(), level: session.volume, muted: session.muted, icon: app_icon(session.pid, icon_px) }
    }
}

/// The icon of process `pid`'s program, `px` square, as a sprite: extracted once per program and
/// size, since the hub re-lists the sessions on every change.
fn app_icon(pid: u32, px: u32) -> Option<std::sync::Arc<Vec<u8>>> {
    use std::collections::HashMap;
    type Cache = HashMap<(String, u32), Option<std::sync::Arc<Vec<u8>>>>;
    thread_local! {
        static CACHE: std::cell::RefCell<Cache> = Default::default();
    }
    let exe = crate::win::process_exe(pid)?;
    CACHE.with(|cache| {
        cache
            .borrow_mut()
            .entry((exe.clone(), px))
            .or_insert_with(|| {
                let png = crate::image::exe_icon_png(&exe, px).ok()?;
                crate::image::sprite(&png, px, 0.0).ok().map(std::sync::Arc::new)
            })
            .clone()
    })
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

/// What the panel gives up when it would not fit the screen, in this order: the apps list folds
/// to its header, then Now Playing shows one card at a time, then (the user having opened the
/// apps list) the list shows fewer rows, scrolled with the wheel.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub(super) struct Fit {
    pub apps_folded: bool,
    /// One Now Playing card ([`Model::page`]) with a pager under it.
    pub carousel: bool,
    /// App rows shown at once from [`Model::apps_scroll`]; `None` shows them all.
    pub apps_rows: Option<usize>,
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
    pub update: Option<String>, // staged update's version, if any → the footer's restart button
    pub notice: Option<&'static str>, // taskbar integration trouble, one line above the footer
    /// The user opened (`Some(true)`) or folded (`Some(false)`) the apps list; `None` leaves it
    /// to [`Fit`].
    pub apps_open: Option<bool>,
    /// What the panel gives up to fit the screen, set by [`super::layout::fit`].
    pub fit: Fit,
    /// The Now Playing card a carousel shows.
    pub page: usize,
    /// The first app row shown while the list shows fewer rows than it has ([`Fit::apps_rows`]).
    pub apps_scroll: usize,
    /// The Batteries section: every connected device that reports one, when it is shown.
    pub batteries: Vec<crate::hub::Battery>,
    // outcome, accumulated while the flyout is open
    pub quit: bool,
    pub restart: bool,
    /// Open the settings window once the flyout has closed.
    pub settings: bool,
}

impl Model {
    pub(super) fn new(groups: Vec<Group>, update: Option<String>) -> Self {
        Model {
            expanded: vec![false; groups.len()],
            now_playing: Vec::new(),
            apps: Vec::new(),
            voice: None,
            groups,
            update,
            notice: None,
            apps_open: None,
            fit: Fit::default(),
            page: 0,
            apps_scroll: 0,
            batteries: Vec::new(),
            quit: false,
            restart: false,
            settings: false,
        }
    }
}

/// The Now Playing card's art size, in DIPs.
pub(super) const ART_DIP: f32 = 52.0;

/// The hub's media, sessions and call as the flyout shows them: a Now Playing card per session,
/// paused ones too, the same ones the pill lists. A card already shown (`old`) keeps its place.
/// `art_px` is the cover's size at the panel's scale; decoding is skipped when `old` already
/// holds a track's sprite.
pub(super) fn from_hub(state: &crate::hub::State, art_px: u32, icon_px: u32, old: &[NowPlaying]) -> (Vec<NowPlaying>, Vec<AppRow>, Option<Voice>) {
    let now_playing = state.all_media().into_iter().take(crate::hub::MAX_SHOWN).map(|m| {
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
            video: m.video,
            art,
        }
    }).collect();
    // Cards already shown keep their place; one whose session went away stays, paused, so the
    // next card never slides under the pointer while the panel is open. The same track coming
    // back from another source (Edge's SMTC session giving way to the extension's tab) takes
    // that card over rather than show twice.
    let mut fresh: Vec<NowPlaying> = now_playing;
    let mut now_playing: Vec<NowPlaying> = Vec::with_capacity(old.len() + fresh.len());
    for o in old {
        let new_source = |n: &NowPlaying| !old.iter().any(|o| o.app_id == n.app_id) && crate::hub::names::same_title(&n.title, &o.title);
        let card = match fresh.iter().position(|n| n.app_id == o.app_id).or_else(|| fresh.iter().position(new_source)) {
            Some(i) => fresh.remove(i),
            None => NowPlaying { playing: false, ..o.clone() },
        };
        now_playing.push(card);
    }
    now_playing.extend(fresh);
    let apps = state
        .sessions
        .iter()
        .map(|s| AppRow::of(s, icon_px))
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

/// The batteries as the hub has them, or (the `--flyout` preview, no tray) as read now. None while
/// they are not followed.
pub(super) fn batteries_now(config: &Config) -> Vec<crate::hub::Battery> {
    if !config.batteries.enabled {
        return Vec::new();
    }
    match crate::tray::hub_snapshot() {
        Some((_, state)) => state.batteries,
        None => crate::batteries::read(),
    }
}

/// The devices the Batteries section lists: all of them, when it is shown.
pub(super) fn in_section(config: &Config, batteries: Vec<crate::hub::Battery>) -> Vec<crate::hub::Battery> {
    if config.batteries.enabled && config.batteries.flyout {
        batteries
    } else {
        Vec::new()
    }
}

/// Bring the open panel's levels in line with the hub's: the section and the device rows.
pub(super) fn follow_batteries(model: &mut Model, config: &Config, batteries: &[crate::hub::Battery]) {
    model.batteries = in_section(config, batteries.to_vec());
    for group in &mut model.groups {
        for row in &mut group.devices {
            if let Some(container) = &row.container {
                row.battery = batteries.iter().find(|b| b.id.eq_ignore_ascii_case(container)).map(|b| b.level);
            }
        }
    }
}

/// Read the current output + input state into display groups. Groups with no devices are
/// omitted (e.g. a machine with no microphone shows no Input section). A device row carries its
/// battery from `batteries`, matched by container.
pub(super) fn build_groups(backend: &WasapiBackend, config: &Config, batteries: &[crate::hub::Battery]) -> Vec<Group> {
    let battery_of = |container: &Option<String>| -> Option<u8> {
        let c = container.as_ref()?;
        batteries.iter().find(|b| b.id.eq_ignore_ascii_case(c)).map(|b| b.level)
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
                DeviceRow { id: d.id, label: d.friendly_name, icon, selected, battery, container: d.container_id }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::{Event, Media, State};

    fn media(app_id: &str, title: &str, playing: bool) -> Media {
        Media {
            app_id: app_id.into(),
            app: "Twitch".into(),
            title: title.into(),
            artist: "otplol_".into(),
            playing,
            cover: None,
            timeline: None,
            skip: false,
            video: true,
        }
    }

    fn cards(state: &State, old: &[NowPlaying]) -> Vec<NowPlaying> {
        from_hub(state, 52, 20, old).0
    }

    /// Edge's SMTC session is shown first, then the extension's tab for the same stream arrives
    /// and the hub hides the SMTC one: the open panel's card follows the tab, not shown twice.
    #[test]
    fn a_track_moving_to_another_source_keeps_one_card() {
        let mut state = State::default();
        state.apply(Event::Media(vec![media("MSEdge", "WORLDS - T1 vs KC", true)]));
        let open = cards(&state, &[]);
        assert_eq!(open.len(), 1);

        state.apply(Event::BrowserMedia { host: Some(7), media: vec![media("browser:7:2:0", "WORLDS - T1 vs KC", true)] });
        let now = cards(&state, &open);
        let ids: Vec<&str> = now.iter().map(|c| c.app_id.as_str()).collect();
        assert_eq!(ids, ["browser:7:2:0"]);
        assert!(now[0].playing);
    }

    /// A session that went away with nothing to replace it stays, paused, in its place.
    #[test]
    fn a_vanished_session_stays_paused_while_the_panel_is_open() {
        let mut state = State::default();
        state.apply(Event::Media(vec![media("A", "One", true), media("B", "Two", true)]));
        let open = cards(&state, &[]);
        state.apply(Event::Media(vec![media("B", "Two", true)]));
        let now = cards(&state, &open);
        let shown: Vec<(&str, bool)> = now.iter().map(|c| (c.app_id.as_str(), c.playing)).collect();
        assert_eq!(shown, [("A", false), ("B", true)]);
    }
}
