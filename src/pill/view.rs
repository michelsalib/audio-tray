//! What the pill shows, decided from the hub state alone: pure, so the precedence rules are
//! tested without a window. Louder states win: a warning over a call over music.
//! It is about audio activity alone: what plays and the calls. Everything else
//! (an app listening, batteries, devices, reminders) is a notification or another surface's.

use crate::hub::{CallSummary, Media, State, VoiceApp};

/// How loud a state is; sets the colours and whether clicks pass through.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Level {
    Quiet,
    Normal,
    Warn,
}

/// One state the pill can be in, with what it draws.
#[derive(Clone, PartialEq, Debug)]
pub enum View {
    /// A fullscreen app runs (and nothing is red), or the pill is off.
    Hidden,
    /// Nothing playing: the grey sliver, or the call's pill when there is one.
    Idle { call: Option<CallPill> },
    /// A column of pills: the art of each playing session in its progress ring, then the call when
    /// there is one. On hover (`card`), a card with a row per session (and the call's).
    Music { media: Vec<Media>, card: bool, call: Option<CallPill> },
    /// The pill's one banner: talking while muted in a call (the rest are notifications). A click
    /// unmutes, in the voice app `app` when it reported the mute, else the default microphone.
    MutedWarning { app: Option<VoiceApp> },
}

/// A call, as its own pill beside the others: faces (when an integration sends them), a mic.
pub type CallPill = CallSummary;

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
            View::MutedWarning { .. } => Level::Warn,
        }
    }

    /// Whether clicks pass through to whatever is underneath: the quiet states.
    pub fn click_through(&self) -> bool {
        self.level() == Level::Quiet
    }
}

/// Everything besides the hub state that the view depends on.
#[derive(Clone, Copy, Default)]
pub struct Inputs {
    pub enabled: bool,
    pub fullscreen: bool,
    /// The pointer is on the music pill or its card.
    pub hover: bool,
    /// The microphone picks up speech (only measured while muted in a call).
    pub talking: bool,
    /// How much shows at rest (`Hidden`: everything short of a warning collapses to the sliver).
    pub size: crate::config::PillSize,
}

/// The state to show.
pub fn decide(state: &State, inputs: Inputs) -> View {
    if !inputs.enabled {
        return View::Hidden;
    }
    let view = loudest(state, inputs);
    // Compact: the sliver until hovered; a warning still speaks up.
    if inputs.size == crate::config::PillSize::Hidden && !inputs.hover && matches!(view.level(), Level::Quiet | Level::Normal) {
        return if inputs.fullscreen { View::Hidden } else { View::Idle { call: None } };
    }
    // A fullscreen app hides it all (an app listening is a banner of Audio Tray's, not the pill's).
    if inputs.fullscreen {
        return View::Hidden;
    }
    view
}

fn loudest(state: &State, inputs: Inputs) -> View {
    // Amber: talking while muted.
    if let Some((app, muted)) = call_mute(state) {
        if muted && inputs.talking {
            return View::MutedWarning { app };
        }
    }

    // The column and the card list the same sessions in the same order (paused ones too, to resume
    // them; the column dims them), so a disc is the row it opens on. With nothing playing at all
    // the column gives way to the quiet sliver; hovering still lists them.
    let media: Vec<Media> = if inputs.hover || state.playing().next().is_some() {
        state.all_media().into_iter().take(crate::hub::MAX_SHOWN).cloned().collect()
    } else {
        Vec::new()
    };
    // The call's pill: a call an integration reports, or a meeting app holding the microphone.
    let call = state.call_summary();
    if media.is_empty() {
        View::Idle { call }
    } else {
        View::Music { media, card: inputs.hover, call }
    }
}

/// Whether the user is in a call, and muted: as the voice app says, else the default microphone.
pub fn call_mute(state: &State) -> Option<(Option<VoiceApp>, bool)> {
    state.call_summary().map(|call| call.mute())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::DeviceId;
    use crate::hub::{Endpoint, Endpoints, Event};

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
            video: false,
        }
    }

    fn on() -> Inputs {
        Inputs { enabled: true, size: crate::config::PillSize::Big, ..Default::default() }
    }

    fn muted_input(state: &mut State, muted: bool) {
        state.apply(Event::Endpoints(Endpoints {
            output: None,
            input: Some(Endpoint { id: DeviceId("mic".into()), name: "Mic".into(), muted, icon: crate::icons::IconId::Microphone }),
            ..Default::default()
        }));
    }

    #[test]
    fn music_shows_while_playing_and_the_sliver_otherwise() {
        let mut state = State::default();
        assert_eq!(decide(&state, on()), View::Idle { call: None });
        state.apply(Event::Media(vec![media(true)]));
        assert!(matches!(decide(&state, on()), View::Music { card: false, .. }));
        assert!(matches!(decide(&state, Inputs { hover: true, ..on() }), View::Music { card: true, .. }));
        state.apply(Event::Media(vec![media(false)]));
        assert_eq!(decide(&state, on()), View::Idle { call: None });
        assert!(
            matches!(decide(&state, Inputs { hover: true, ..on() }), View::Music { card: true, .. }),
            "paused music still opens on hover, to be resumed"
        );
    }

    #[test]
    fn the_column_lists_what_the_card_lists_once_something_plays() {
        let mut state = State::default();
        let twitch = Media { app_id: "MSEdge".into(), app: "Twitch".into(), ..media(true) };
        let paused = Media { app_id: "Spotify.exe".into(), ..media(false) };
        state.apply(Event::Media(vec![media(true), twitch, paused]));
        match decide(&state, on()) {
            View::Music { media, card: false, .. } => assert_eq!(media.len(), 3, "the paused one too, in the card's order"),
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
                assert!(!call.muted());
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
        let warning = decide(&state, talking);
        assert_eq!(warning, View::MutedWarning { app: None });
        assert_eq!(warning.level(), Level::Warn);
    }

    #[test]
    fn an_integrations_self_mute_names_the_app_to_unmute() {
        let mut state = State::default();
        state.apply(Event::MicUsers(vec!["Discord.exe".into()]));
        state.apply(Event::Call { app: VoiceApp::Discord, call: Some(crate::hub::Call { self_muted: true, ..Default::default() }) });
        assert_eq!(decide(&state, Inputs { talking: true, ..on() }), View::MutedWarning { app: Some(VoiceApp::Discord) });
        assert_eq!(super::super::draw::muted_text(Some(VoiceApp::Discord)), "You're muted in Discord · click to unmute");
    }

    #[test]
    fn fullscreen_hides_it_all_and_an_app_listening_is_never_the_pills() {
        let mut state = State::default();
        state.apply(Event::Media(vec![media(true)]));
        let fullscreen = Inputs { fullscreen: true, ..on() };
        assert_eq!(decide(&state, fullscreen), View::Hidden);
        // An app listening is a banner of Audio Tray's.
        state.apply(Event::MicUsers(vec![r"C:\x\recorder.exe".into()]));
        assert_eq!(decide(&state, fullscreen), View::Hidden);
        assert!(matches!(decide(&state, on()), View::Music { .. }));
    }

    #[test]
    fn compact_is_the_sliver_until_hovered_but_warnings_still_show() {
        let mut state = State::default();
        state.apply(Event::Media(vec![media(true)]));
        let compact = Inputs { size: crate::config::PillSize::Hidden, ..on() };
        assert_eq!(decide(&state, compact), View::Idle { call: None });
        assert!(matches!(decide(&state, Inputs { hover: true, ..compact }), View::Music { card: true, .. }));
        // Talking while muted in a call: the warning shows all the same.
        state.apply(Event::MicUsers(vec!["Discord.exe".into()]));
        muted_input(&mut state, true);
        assert!(matches!(decide(&state, Inputs { talking: true, ..compact }), View::MutedWarning { .. }));
    }

    #[test]
    fn quiet_states_let_clicks_through() {
        assert!(View::Idle { call: None }.click_through());
        assert!(View::Music { media: vec![media(true)], card: false, call: None }.click_through());
        assert!(!View::Music { media: vec![media(true)], card: true, call: None }.click_through());
    }
}
