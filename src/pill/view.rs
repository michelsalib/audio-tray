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
    /// The grey sliver.
    Idle,
    /// The art in its progress ring; `card` while the pointer is on it.
    Music { media: Media, card: bool },
    /// The microphone is live in a call: faces (when an integration sends them), a mic.
    Call {
        app: Option<VoiceApp>,
        /// The app holding the microphone, when no integration names the call.
        holder: Option<String>,
        members: Vec<Member>,
        muted: bool,
    },
    /// A one-line banner: warnings, alerts and notices.
    Banner { level: Level, glyph: char, text: String, action: Action },
}

/// What clicking a state does.
#[derive(Clone, PartialEq, Debug)]
pub enum Action {
    /// Unmute: in the voice app when it reported the mute, else the default microphone.
    Unmute(Option<VoiceApp>),
    /// Mute the default microphone and acknowledge the alert.
    MuteAndAcknowledge(Notice),
    /// Acknowledge, and open the link.
    Open(Notice, String),
    /// Acknowledge only.
    Dismiss(Notice),
}

impl View {
    pub fn level(&self) -> Level {
        match self {
            View::Hidden | View::Idle => Level::Quiet,
            View::Music { card, .. } => {
                if *card {
                    Level::Normal
                } else {
                    Level::Quiet
                }
            }
            View::Call { .. } => Level::Normal,
            View::Banner { level, .. } => *level,
        }
    }

    /// Whether clicks pass through to whatever is underneath.
    pub fn click_through(&self) -> bool {
        self.level() == Level::Quiet
    }
}

/// Segoe Fluent Icons glyphs the banners use.
pub const GLYPH_MIC_OFF: char = '\u{F781}';
pub const GLYPH_MIC: char = '\u{E720}';
pub const GLYPH_SPEAKERS: char = '\u{E7F5}';
pub const GLYPH_LIVE: char = '\u{E714}';

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
}

/// The state to show.
pub fn decide(state: &State, inputs: Inputs) -> View {
    if !inputs.enabled {
        return View::Hidden;
    }
    let view = loudest(state, inputs);
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
    if let Some(view) = call_view(state) {
        return view;
    }
    if let Some(notice @ Notice::WentLive { channel, title, url }) = newest(state, |n| matches!(n, Notice::WentLive { .. })) {
        let text = if title.is_empty() { format!("{channel} is live") } else { format!("{channel} is live · {title}") };
        return View::Banner { level: Level::Normal, glyph: GLYPH_LIVE, text, action: Action::Open(notice.clone(), url.clone()) };
    }
    match &state.media {
        // Paused media still opens on hover (from the sliver), so it can be resumed from here.
        Some(media) if media.playing || inputs.hover => View::Music { media: media.clone(), card: inputs.hover },
        _ => View::Idle,
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

fn call_view(state: &State) -> Option<View> {
    let (app, muted) = call_mute(state)?;
    // "While mic is live": a call an integration reports, with nobody holding the mic, is
    // shown only while someone in it speaks.
    let (voice, members) = match state.call() {
        Some((voice, Call { members, .. })) => (Some(voice), members.clone()),
        None => (None, Vec::new()),
    };
    if !state.mic_live() && !members.iter().any(|m| m.speaking) {
        return None;
    }
    Some(View::Call { app: voice.or(app), holder: meeting_holder(state), members, muted })
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
        }
    }

    fn on() -> Inputs {
        Inputs { enabled: true, listening_alerts: true, ..Default::default() }
    }

    fn muted_input(state: &mut State, muted: bool) {
        state.apply(Event::Endpoints(Endpoints {
            output: None,
            input: Some(Endpoint { id: DeviceId("mic".into()), name: "Mic".into(), muted }),
            active_outputs: vec![],
        }));
    }

    #[test]
    fn music_shows_while_playing_and_the_sliver_otherwise() {
        let mut state = State::default();
        assert_eq!(decide(&state, on()), View::Idle);
        state.apply(Event::Media(Some(media(true))));
        assert!(matches!(decide(&state, on()), View::Music { card: false, .. }));
        assert!(matches!(decide(&state, Inputs { hover: true, ..on() }), View::Music { card: true, .. }));
        state.apply(Event::Media(Some(media(false))));
        assert_eq!(decide(&state, on()), View::Idle);
        assert!(
            matches!(decide(&state, Inputs { hover: true, ..on() }), View::Music { card: true, .. }),
            "paused music still opens on hover, to be resumed"
        );
    }

    #[test]
    fn a_meeting_app_on_the_mic_is_a_call_and_beats_music() {
        let mut state = State::default();
        state.apply(Event::Media(Some(media(true))));
        state.apply(Event::MicUsers(vec!["MSTeams_8wekyb3d8bbwe".into()]));
        match decide(&state, on()) {
            View::Call { holder, muted, .. } => {
                assert_eq!(holder.as_deref(), Some("Teams"));
                assert!(!muted);
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
        assert!(matches!(decide(&state, talking), View::Call { .. }));
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
        state.apply(Event::Media(Some(media(true))));
        let fullscreen = Inputs { fullscreen: true, ..on() };
        assert_eq!(decide(&state, fullscreen), View::Hidden);
        state.apply(Event::MicUsers(vec![r"C:\x\recorder.exe".into()]));
        assert!(matches!(decide(&state, fullscreen), View::Banner { level: Level::Alert, .. }));
        assert_eq!(decide(&state, Inputs { listening_alerts: false, ..fullscreen }), View::Hidden);
    }

    #[test]
    fn quiet_states_let_clicks_through() {
        assert!(View::Idle.click_through());
        assert!(View::Music { media: media(true), card: false }.click_through());
        assert!(!View::Music { media: media(true), card: true }.click_through());
    }
}
