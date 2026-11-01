//! The hub: every source reports facts as one [`Event`] type, and the surfaces render the
//! [`State`] those events fold into. A surface never knows which source produced a fact
//! (`SelfMuted` comes from Discord or Teams alike), and a source never knows who listens.
//!
//! Sources run on any thread and call [`emit`]; events queue here and the tray thread is woken
//! once ([`WM_HUB_EVENTS`]) to fold them all with [`take`]. Nothing here polls.

pub mod names;

use std::sync::{Arc, Mutex};

use crate::audio::DeviceId;

/// Posted (coalesced) to the tray's message window when events are queued.
pub const WM_HUB_EVENTS: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 34;

/// A voice app that can report a call. Integrations are optional; each is one variant.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum VoiceApp {
    Discord,
    Teams,
}

impl VoiceApp {
    pub fn label(self) -> &'static str {
        match self {
            VoiceApp::Discord => "Discord",
            VoiceApp::Teams => "Teams",
        }
    }
}

/// The media session worth showing: any app, a playing one first.
#[derive(Clone, PartialEq, Debug)]
pub struct Media {
    pub app_id: String,
    /// The app as people say it ("YouTube Music", "Spotify").
    pub app: String,
    pub title: String,
    pub artist: String,
    pub playing: bool,
    /// PNG or JPEG bytes as published, shared because every surface keeps a copy.
    pub cover: Option<Arc<Vec<u8>>>,
    /// The timeline checkpoint, in SMTC ticks (see [`crate::music::smtc::Timeline`]).
    pub timeline: Option<crate::music::smtc::Timeline>,
}

impl Media {
    /// Track length in seconds; `None` for a live stream or a player that publishes no end.
    pub fn length_seconds(&self) -> Option<f64> {
        let t = self.timeline?;
        let span = t.end - t.start;
        (span > 0).then(|| span as f64 / 1e7)
    }

    /// How far through the track it is now, 0.0–1.0, interpolated while playing.
    pub fn fraction_now(&self) -> Option<f64> {
        self.timeline?.fraction_at(crate::music::smtc::now_ticks(), self.playing)
    }
}

/// One default endpoint, as the surfaces show it.
#[derive(Clone, PartialEq, Debug)]
pub struct Endpoint {
    pub id: DeviceId,
    pub name: String,
    pub muted: bool,
}

/// The default endpoints and which outputs exist, so a vanished default can be told from a switch.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Endpoints {
    pub output: Option<Endpoint>,
    pub input: Option<Endpoint>,
    pub active_outputs: Vec<DeviceId>,
}

/// One app's audio session on the default output (the mixer's row).
#[derive(Clone, PartialEq, Debug)]
pub struct AppSession {
    /// Stable across reads: the session instance identifier.
    pub key: String,
    pub app: String,
    pub pid: u32,
    pub volume: f32,
    pub muted: bool,
}

/// Someone in a call.
#[derive(Clone, PartialEq, Debug)]
pub struct Member {
    pub id: String,
    pub name: String,
    /// PNG or JPEG bytes, when the integration sent a face.
    pub avatar: Option<Arc<Vec<u8>>>,
    pub speaking: bool,
    pub muted: bool,
    /// This member is the user.
    pub me: bool,
}

/// A call as an integration reports it.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Call {
    /// The channel or meeting name, when known.
    pub channel: Option<String>,
    /// Who is there; empty when the integration cannot tell (Teams).
    pub members: Vec<Member>,
    pub self_muted: bool,
    pub self_deafened: bool,
}

/// Every fact any source can report.
#[derive(Clone, PartialEq, Debug)]
pub enum Event {
    /// The media session to show, or none.
    Media(Option<Media>),
    /// The apps holding the microphone (exe paths or package names), from the consent store.
    MicUsers(Vec<String>),
    /// The default endpoints, after any endpoint change.
    Endpoints(Endpoints),
    /// The apps playing on the default output.
    AppSessions(Vec<AppSession>),
    /// A voice app joined (`Some`) or left (`None`) a call; also sent on any change in it.
    Call { app: VoiceApp, call: Option<Call> },
    /// The user muted or unmuted themselves in a voice app.
    SelfMuted { app: VoiceApp, muted: bool },
    /// A followed channel went live (Twitch).
    WentLive { channel: String, title: String, url: String },
    /// An integration connected (`true`) or went away (`false`), for the flyout's status.
    Integration { name: &'static str, connected: bool },
}

/// A short-lived notice the pill shows on top of the steady state.
#[derive(Clone, PartialEq, Debug)]
pub enum Notice {
    /// The default output moved because the old one went away.
    AudioMoved { to: String, from: String },
    /// An app started recording outside a known call; stays until acknowledged.
    Listening { app: String, user: String },
    WentLive { channel: String, title: String, url: String },
}

impl Notice {
    /// How long it shows; `None` until acknowledged.
    pub fn lifetime(&self) -> Option<std::time::Duration> {
        match self {
            Notice::AudioMoved { .. } => Some(std::time::Duration::from_secs(5)),
            Notice::Listening { .. } => None,
            Notice::WentLive { .. } => Some(std::time::Duration::from_secs(8)),
        }
    }
}

/// Everything the surfaces render, folded from events.
#[derive(Default, Clone)]
pub struct State {
    pub media: Option<Media>,
    pub mic_users: Vec<String>,
    pub endpoints: Endpoints,
    pub sessions: Vec<AppSession>,
    pub calls: Vec<(VoiceApp, Call)>,
    pub integrations: Vec<(&'static str, bool)>,
    /// Shown newest last; each with when it started.
    pub notices: Vec<(Notice, std::time::Instant)>,
}

impl State {
    /// Fold one event in. Returns whether anything a surface shows changed.
    pub fn apply(&mut self, event: Event) -> bool {
        match event {
            Event::Media(media) => replace(&mut self.media, media),
            Event::MicUsers(users) => {
                let started: Vec<String> =
                    users.iter().filter(|u| !self.mic_users.contains(u)).cloned().collect();
                let changed = replace(&mut self.mic_users, users);
                // Released: its alert has nothing left to warn about.
                let held = self.mic_users.clone();
                self.notices.retain(|(n, _)| !matches!(n, Notice::Listening { user, .. } if !held.contains(user)));
                for user in started {
                    if !self.in_known_call(&user) {
                        self.notices.push((
                            Notice::Listening { app: names::app_name(&user), user },
                            std::time::Instant::now(),
                        ));
                    }
                }
                changed
            }
            Event::Endpoints(endpoints) => {
                if let (Some(old), Some(new)) = (&self.endpoints.output, &endpoints.output) {
                    if old.id != new.id && !endpoints.active_outputs.contains(&old.id) {
                        self.push_notice(Notice::AudioMoved { to: new.name.clone(), from: old.name.clone() });
                    }
                }
                replace(&mut self.endpoints, endpoints)
            }
            Event::AppSessions(sessions) => replace(&mut self.sessions, sessions),
            Event::Call { app, call } => {
                let at = self.calls.iter().position(|(a, _)| *a == app);
                match (at, call) {
                    (Some(i), Some(call)) => {
                        let changed = self.calls[i].1 != call;
                        self.calls[i].1 = call;
                        changed
                    }
                    (Some(i), None) => {
                        self.calls.remove(i);
                        true
                    }
                    (None, Some(call)) => {
                        // A call explains a recording that started before the integration said so.
                        self.notices.retain(|(n, _)| !matches!(n, Notice::Listening { app: a, .. } if names::is_voice_app(a, app)));
                        self.calls.push((app, call));
                        true
                    }
                    (None, None) => false,
                }
            }
            Event::SelfMuted { app, muted } => match self.calls.iter_mut().find(|(a, _)| *a == app) {
                Some((_, call)) => {
                    let changed = call.self_muted != muted;
                    call.self_muted = muted;
                    if let Some(me) = call.members.iter_mut().find(|m| m.me) {
                        me.muted = muted;
                    }
                    changed
                }
                None => false,
            },
            Event::WentLive { channel, title, url } => {
                self.push_notice(Notice::WentLive { channel, title, url });
                true
            }
            Event::Integration { name, connected } => {
                let entry = self.integrations.iter_mut().find(|(n, _)| *n == name);
                match entry {
                    Some((_, was)) => std::mem::replace(was, connected) != connected,
                    None => {
                        self.integrations.push((name, connected));
                        true
                    }
                }
            }
        }
    }

    fn push_notice(&mut self, notice: Notice) {
        self.notices.retain(|(n, _)| std::mem::discriminant(n) != std::mem::discriminant(&notice));
        self.notices.push((notice, std::time::Instant::now()));
    }

    /// A recording by `user` that a reported call accounts for, or by a known voice/meeting app
    /// (whose integration may simply be off).
    fn in_known_call(&self, user: &str) -> bool {
        let app = names::app_name(user);
        names::is_meeting_app(&app) || self.calls.iter().any(|(voice, _)| names::is_voice_app(&app, *voice))
    }

    /// Drop timed-out notices; returns whether any went, and when the next one is due.
    pub fn expire(&mut self, now: std::time::Instant) -> (bool, Option<std::time::Duration>) {
        let before = self.notices.len();
        self.notices.retain(|(n, at)| n.lifetime().is_none_or(|life| now < *at + life));
        let next = self
            .notices
            .iter()
            .filter_map(|(n, at)| n.lifetime().map(|life| (*at + life).saturating_duration_since(now)))
            .min();
        (self.notices.len() != before, next)
    }

    /// Acknowledge (dismiss) the notice of the same kind as `notice`.
    pub fn acknowledge(&mut self, notice: &Notice) {
        self.notices.retain(|(n, _)| n != notice);
    }

    /// The call the pill shows: the one with someone in it, Discord before Teams.
    pub fn call(&self) -> Option<(VoiceApp, &Call)> {
        self.calls.iter().map(|(app, call)| (*app, call)).next()
    }

    /// Whether the microphone is live for the user: an app holds it.
    pub fn mic_live(&self) -> bool {
        !self.mic_users.is_empty()
    }
}

fn replace<T: PartialEq>(slot: &mut T, value: T) -> bool {
    if *slot == value {
        return false;
    }
    *slot = value;
    true
}

/// Events waiting for the tray thread.
static QUEUE: Mutex<Vec<Event>> = Mutex::new(Vec::new());

/// Coalesces [`WM_HUB_EVENTS`]: one post wakes the tray for every event queued before it drains.
pub(crate) static WAKE: crate::tray::Wake = crate::tray::Wake::new(WM_HUB_EVENTS);

/// Report a fact, from any thread. Free when nothing listens (the dev CLI modes).
pub fn emit(event: Event) {
    QUEUE.lock().unwrap_or_else(|p| p.into_inner()).push(event);
    WAKE.post();
}

/// Everything queued since the last call. Tray thread; re-arms the wake first so an event
/// emitted while folding is never missed.
pub fn take() -> Vec<Event> {
    WAKE.take();
    std::mem::take(&mut *QUEUE.lock().unwrap_or_else(|p| p.into_inner()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn endpoint(id: &str) -> Endpoint {
        Endpoint { id: DeviceId(id.into()), name: id.into(), muted: false }
    }

    fn endpoints(output: &str, active: &[&str]) -> Event {
        Event::Endpoints(Endpoints {
            output: Some(endpoint(output)),
            input: None,
            active_outputs: active.iter().map(|id| DeviceId(id.to_string())).collect(),
        })
    }

    #[test]
    fn a_vanished_default_is_announced_but_a_switch_is_not() {
        let mut state = State::default();
        state.apply(endpoints("Headphones", &["Headphones", "Speakers"]));
        state.apply(endpoints("Speakers", &["Headphones", "Speakers"]));
        assert!(state.notices.is_empty(), "a switch between present devices is not news");
        state.apply(endpoints("Headphones", &["Headphones", "Speakers"]));
        state.apply(endpoints("Speakers", &["Speakers"]));
        assert!(matches!(
            &state.notices[..],
            [(Notice::AudioMoved { to, from }, _)] if to == "Speakers" && from == "Headphones"
        ));
    }

    #[test]
    fn a_recording_outside_a_call_raises_an_alert_until_released() {
        let mut state = State::default();
        state.apply(Event::MicUsers(vec![r"C:\Tools\recorder.exe".into()]));
        assert!(matches!(&state.notices[..], [(Notice::Listening { app, .. }, _)] if app == "recorder"));
        state.apply(Event::MicUsers(vec![]));
        assert!(state.notices.is_empty());
    }

    #[test]
    fn a_meeting_app_recording_is_not_an_alert() {
        let mut state = State::default();
        state.apply(Event::MicUsers(vec![r"C:\Users\me\AppData\Local\Discord\app-1.0\Discord.exe".into()]));
        state.apply(Event::MicUsers(vec![r"C:\Users\me\AppData\Local\Discord\app-1.0\Discord.exe".into(), "MSTeams_8wekyb3d8bbwe".into()]));
        assert!(state.notices.is_empty(), "{:?}", state.notices);
    }

    #[test]
    fn self_mute_lands_on_the_call_and_on_me() {
        let mut state = State::default();
        let me = Member { id: "1".into(), name: "Me".into(), avatar: None, speaking: false, muted: false, me: true };
        state.apply(Event::Call { app: VoiceApp::Discord, call: Some(Call { members: vec![me], ..Default::default() }) });
        assert!(state.apply(Event::SelfMuted { app: VoiceApp::Discord, muted: true }));
        let (_, call) = state.call().unwrap();
        assert!(call.self_muted && call.members[0].muted);
        assert!(!state.apply(Event::SelfMuted { app: VoiceApp::Teams, muted: true }), "no Teams call to mute");
    }

    #[test]
    fn timed_notices_expire_and_report_the_next_deadline() {
        let mut state = State::default();
        let t0 = std::time::Instant::now();
        state.notices.push((Notice::AudioMoved { to: "a".into(), from: "b".into() }, t0));
        state.notices.push((Notice::Listening { app: "x".into(), user: "x".into() }, t0));
        let (gone, next) = state.expire(t0 + std::time::Duration::from_secs(1));
        assert!(!gone);
        assert_eq!(next, Some(std::time::Duration::from_secs(4)));
        let (gone, next) = state.expire(t0 + std::time::Duration::from_secs(6));
        assert!(gone);
        assert_eq!(next, None, "the alert waits for acknowledgement");
        assert_eq!(state.notices.len(), 1);
    }
}
