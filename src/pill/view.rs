//! What the pill shows, decided from the hub state alone: pure, so the precedence rules are
//! tested without a window. Louder states win: an alert over a warning over a call over music.

use crate::hub::{Call, Media, Member, Notice, State, VoiceApp};

/// How loud a state is; sets the colours and whether clicks pass through.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Level {
    Quiet,
    Normal,
    Warn,
    Alert,
}

/// One state from the brief's table, with what it draws.
#[derive(Clone, PartialEq, Debug)]
pub enum View {
    /// A fullscreen app runs (and nothing is red), or the pill is off.
    Hidden,
    /// Nothing playing: the grey sliver, or the call's and the devices' pills when there are any.
    Idle { call: Option<CallPill>, devices: Option<Devices> },
    /// A column of pills: the art of each playing session in its progress ring, then the call,
    /// then the devices, each shown when there is one. On hover (`card`), a card with a row per
    /// session (and the call's).
    Music { media: Vec<Media>, card: bool, call: Option<CallPill>, devices: Option<Devices> },
    /// A one-line banner: warnings, alerts and notices.
    Banner { level: Level, glyph: char, text: String, action: Action },
}

/// A call, as its own pill beside the others: faces (when an integration sends them), a mic.
#[derive(Clone, PartialEq, Debug)]
pub struct CallPill {
    pub app: Option<VoiceApp>,
    /// The app holding the microphone, when no integration names the call.
    pub holder: Option<String>,
    pub channel: Option<String>,
    pub members: Vec<Member>,
    pub muted: bool,
}

/// One default device on the pill.
#[derive(Clone, PartialEq, Debug)]
pub struct Device {
    pub glyph: char,
    pub name: String,
    pub muted: bool,
    /// An app has the microphone open (input only).
    pub recording: bool,
}

/// The output and input on the pill, clicked to switch to the next device.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Devices {
    pub output: Option<Device>,
    pub input: Option<Device>,
}

impl Devices {
    fn of(state: &State) -> Option<Self> {
        let device = |e: &crate::hub::Endpoint, recording: bool| Device { glyph: e.glyph, name: e.name.clone(), muted: e.muted, recording };
        let devices = Self {
            output: state.endpoints.output.as_ref().map(|e| device(e, false)),
            input: state.endpoints.input.as_ref().map(|e| device(e, state.mic_live())),
        };
        (devices.output.is_some() || devices.input.is_some()).then_some(devices)
    }
}

/// What clicking a state does.
#[derive(Clone, PartialEq, Debug)]
pub enum Action {
    /// Unmute: in the voice app when it reported the mute, else the default microphone.
    Unmute(Option<VoiceApp>),
    /// Mute the default microphone and acknowledge the alert.
    MuteAndAcknowledge(Notice),
    /// Acknowledge only.
    Dismiss(Notice),
}

impl View {
    pub fn level(&self) -> Level {
        match self {
            View::Hidden | View::Idle { call: None, .. } => Level::Quiet,
            View::Idle { call: Some(_), .. } => Level::Normal,
            View::Music { card, call, .. } => {
                if *card || call.is_some() {
                    Level::Normal
                } else {
                    Level::Quiet
                }
            }
            View::Banner { level, .. } => *level,
        }
    }

    /// Whether clicks pass through to whatever is underneath: quiet states, unless they carry the
    /// devices (which are buttons).
    pub fn click_through(&self) -> bool {
        let devices = matches!(self, View::Idle { devices: Some(_), .. } | View::Music { devices: Some(_), .. });
        self.level() == Level::Quiet && !devices
    }
}

/// Segoe Fluent Icons glyphs the banners use.
pub const GLYPH_MIC_OFF: char = '\u{F781}';
pub const GLYPH_MIC: char = '\u{E720}';
pub const GLYPH_SPEAKERS: char = '\u{E7F5}';

/// Everything besides the hub state that the view depends on.
#[derive(Clone, Copy, Default)]
pub struct Inputs {
    pub enabled: bool,
    pub fullscreen: bool,
    /// The pointer is on the music pill or its card.
    pub hover: bool,
    /// The microphone picks up speech (only measured while muted in a call).
    pub talking: bool,
    pub listening_alerts: bool,
    /// Show the output and input on the quiet states.
    pub devices: bool,
    /// How much shows at rest (`Hidden`: everything short of a warning collapses to the sliver).
    pub size: crate::config::PillSize,
}

/// The state to show.
pub fn decide(state: &State, inputs: Inputs) -> View {
    if !inputs.enabled {
        return View::Hidden;
    }
    let view = loudest(state, inputs);
    // Compact: the sliver until hovered; warnings and alerts still speak up.
    if inputs.size == crate::config::PillSize::Hidden && !inputs.hover && matches!(view.level(), Level::Quiet | Level::Normal) {
        return if inputs.fullscreen { View::Hidden } else { View::Idle { call: None, devices: None } };
    }
    // A fullscreen app hides everything but red.
    if inputs.fullscreen && view.level() != Level::Alert {
        return View::Hidden;
    }
    view
}

fn loudest(state: &State, inputs: Inputs) -> View {
    // Red: an app listening outside a call, newest first.
    if inputs.listening_alerts {
        if let Some(notice @ Notice::Listening { app, .. }) = newest(state, |n| matches!(n, Notice::Listening { .. })) {
            return View::Banner {
                level: Level::Alert,
                glyph: GLYPH_MIC,
                text: format!("{app} is listening · click to mute"),
                action: Action::MuteAndAcknowledge(notice.clone()),
            };
        }
    }
    // Amber: talking while muted.
    if let Some((app, muted)) = call_mute(state) {
        if muted && inputs.talking {
            let name = app.map_or("Windows", VoiceApp::label);
            return View::Banner {
                level: Level::Warn,
                glyph: GLYPH_MIC_OFF,
                text: format!("You're muted in {name} · click to unmute"),
                action: Action::Unmute(app),
            };
        }
    }
    if let Some(notice @ Notice::AudioMoved { to, from }) = newest(state, |n| matches!(n, Notice::AudioMoved { .. })) {
        return View::Banner {
            level: Level::Warn,
            glyph: GLYPH_SPEAKERS,
            text: format!("Now on {to} · {from} disconnected"),
            action: Action::Dismiss(notice.clone()),
        };
    }

    // On hover the card lists every session (paused ones too, to resume them from here); quiet,
    // the pill shows the playing ones.
    let media: Vec<Media> = if inputs.hover {
        state.all_media().into_iter().take(crate::hub::MAX_SHOWN).cloned().collect()
    } else {
        state.playing().cloned().collect()
    };
    let devices = if inputs.devices { Devices::of(state) } else { None };
    let call = call_pill(state);
    if media.is_empty() {
        View::Idle { call, devices }
    } else {
        View::Music { media, card: inputs.hover, call, devices }
    }
}

fn newest(state: &State, want: impl Fn(&Notice) -> bool) -> Option<&Notice> {
    state.notices.iter().rev().map(|(n, _)| n).find(|n| want(n))
}

/// Whether the user is in a call, and muted: as the voice app says, else the default microphone.
pub fn call_mute(state: &State) -> Option<(Option<VoiceApp>, bool)> {
    if let Some((app, call)) = state.call() {
        let endpoint = state.endpoints.input.as_ref().is_some_and(|e| e.muted);
        return Some(if call.self_muted { (Some(app), true) } else { (None, endpoint) });
    }
    if meeting_holder(state).is_some() {
        return Some((None, state.endpoints.input.as_ref().is_some_and(|e| e.muted)));
    }
    None
}

/// The meeting app holding the microphone, when one does.
fn meeting_holder(state: &State) -> Option<String> {
    state
        .mic_users
        .iter()
        .map(|user| crate::hub::names::app_name(user))
        .find(|name| crate::hub::names::is_meeting_app(name))
}

/// The call's pill: a call an integration reports (for as long as the user is in it), or a
/// meeting app holding the microphone.
fn call_pill(state: &State) -> Option<CallPill> {
    let (app, muted) = call_mute(state)?;
    let (voice, channel, members) = match state.call() {
        Some((voice, Call { channel, members, .. })) => (Some(voice), channel.clone(), members.clone()),
        None => (None, None, Vec::new()),
    };
    Some(CallPill { app: voice.or(app), holder: meeting_holder(state), channel, members, muted })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::{Endpoint, Endpoints, Event};
    use crate::audio::DeviceId;

    fn media(playing: bool) -> Media {
        Media {
            app_id: "Spotify.exe".into(),
            app: "Spotify".into(),
            title: "Song".into(),
            artist: "Band".into(),
            playing,
            cover: None,
            timeline: None,
            skip: true,
        }
    }

    fn on() -> Inputs {
        Inputs { enabled: true, listening_alerts: true, size: crate::config::PillSize::Big, ..Default::default() }
    }

    fn muted_input(state: &mut State, muted: bool) {
        state.apply(Event::Endpoints(Endpoints {
            output: None,
            input: Some(Endpoint { id: DeviceId("mic".into()), name: "Mic".into(), muted, glyph: '\u{E720}' }),
            active_outputs: vec![],
        }));
    }

    #[test]
    fn music_shows_while_playing_and_the_sliver_otherwise() {
        let mut state = State::default();
        assert_eq!(decide(&state, on()), View::Idle { call: None, devices: None });
        state.apply(Event::Media(vec![media(true)]));
        assert!(matches!(decide(&state, on()), View::Music { card: false, .. }));
        assert!(matches!(decide(&state, Inputs { hover: true, ..on() }), View::Music { card: true, .. }));
        state.apply(Event::Media(vec![media(false)]));
        assert_eq!(decide(&state, on()), View::Idle { call: None, devices: None });
        assert!(
            matches!(decide(&state, Inputs { hover: true, ..on() }), View::Music { card: true, .. }),
            "paused music still opens on hover, to be resumed"
        );
    }

    #[test]
    fn every_playing_session_shows_and_the_card_lists_the_paused_too() {
        let mut state = State::default();
        let twitch = Media { app_id: "MSEdge".into(), app: "Twitch".into(), ..media(true) };
        let paused = Media { app_id: "Spotify.exe".into(), ..media(false) };
        state.apply(Event::Media(vec![media(true), twitch, paused]));
        match decide(&state, on()) {
            View::Music { media, card: false, .. } => assert_eq!(media.len(), 2, "both playing sessions"),
            other => panic!("{other:?}"),
        }
        match decide(&state, Inputs { hover: true, ..on() }) {
            View::Music { media, card: true, .. } => assert_eq!(media.len(), 3, "the paused one too, to resume it"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_meeting_app_on_the_mic_adds_a_call_pill_beside_the_music() {
        let mut state = State::default();
        state.apply(Event::Media(vec![media(true)]));
        state.apply(Event::MicUsers(vec!["MSTeams_8wekyb3d8bbwe".into()]));
        match decide(&state, on()) {
            View::Music { call: Some(call), .. } => {
                assert_eq!(call.holder.as_deref(), Some("Teams"));
                assert!(!call.muted);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn talking_while_muted_warns_only_when_muted() {
        let mut state = State::default();
        state.apply(Event::MicUsers(vec!["Discord.exe".into()]));
        muted_input(&mut state, false);
        let talking = Inputs { talking: true, ..on() };
        assert!(matches!(decide(&state, talking), View::Idle { call: Some(_), .. }));
        muted_input(&mut state, true);
        assert!(matches!(decide(&state, talking), View::Banner { level: Level::Warn, action: Action::Unmute(None), .. }));
    }

    #[test]
    fn an_integrations_self_mute_names_the_app_to_unmute() {
        let mut state = State::default();
        state.apply(Event::MicUsers(vec!["Discord.exe".into()]));
        state.apply(Event::Call { app: VoiceApp::Discord, call: Some(Call { self_muted: true, ..Default::default() }) });
        match decide(&state, Inputs { talking: true, ..on() }) {
            View::Banner { text, action, .. } => {
                assert_eq!(text, "You're muted in Discord · click to unmute");
                assert_eq!(action, Action::Unmute(Some(VoiceApp::Discord)));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn fullscreen_hides_all_but_red() {
        let mut state = State::default();
        state.apply(Event::Media(vec![media(true)]));
        let fullscreen = Inputs { fullscreen: true, ..on() };
        assert_eq!(decide(&state, fullscreen), View::Hidden);
        state.apply(Event::MicUsers(vec![r"C:\x\recorder.exe".into()]));
        assert!(matches!(decide(&state, fullscreen), View::Banner { level: Level::Alert, .. }));
        assert_eq!(decide(&state, Inputs { listening_alerts: false, ..fullscreen }), View::Hidden);
    }

    #[test]
    fn compact_is_the_sliver_until_hovered_but_warnings_still_show() {
        let mut state = State::default();
        state.apply(Event::Media(vec![media(true)]));
        let compact = Inputs { size: crate::config::PillSize::Hidden, devices: true, ..on() };
        assert_eq!(decide(&state, compact), View::Idle { call: None, devices: None });
        assert!(matches!(decide(&state, Inputs { hover: true, ..compact }), View::Music { card: true, .. }));
        state.apply(Event::MicUsers(vec![r"C:\x\recorder.exe".into()]));
        assert!(matches!(decide(&state, compact), View::Banner { level: Level::Alert, .. }));
    }

    #[test]
    fn the_devices_ride_the_quiet_states() {
        let mut state = State::default();
        muted_input(&mut state, true);
        match decide(&state, Inputs { devices: true, ..on() }) {
            View::Idle { devices: Some(devices), .. } => assert!(devices.input.is_some_and(|d| d.muted)),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn quiet_states_let_clicks_through() {
        assert!(View::Idle { call: None, devices: None }.click_through());
        assert!(!View::Idle { call: None, devices: Some(Devices::default()) }.click_through(), "the devices are buttons");
        assert!(View::Music { media: vec![media(true)], card: false, call: None, devices: None }.click_through());
        assert!(!View::Music { media: vec![media(true)], card: true, call: None, devices: None }.click_through());
    }
}
