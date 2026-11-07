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

/// One media session, from any app.
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
    /// Previous and next reach the player (not for a live stream, nor a browser tab, whose page
    /// handles them out of reach).
    pub skip: bool,
    /// A video, not music: with no cover, it is drawn with a video glyph.
    pub video: bool,
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
    /// The device's chosen icon.
    pub icon: crate::icons::IconId,
}

/// An active device a flow can switch to.
#[derive(Clone, PartialEq, Debug)]
pub struct Choice {
    pub id: DeviceId,
    pub name: String,
    pub icon: crate::icons::IconId,
}

/// The default endpoints and the active devices of each flow, in Windows' order: the pill lists
/// them, and a vanished default is told from a switch.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Endpoints {
    pub output: Option<Endpoint>,
    pub input: Option<Endpoint>,
    pub outputs: Vec<Choice>,
    pub inputs: Vec<Choice>,
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
    /// The app's icon (PNG), shown where there are no faces to say whose call it is.
    pub icon: Option<std::sync::Arc<Vec<u8>>>,
}

/// Every fact any source can report.
#[derive(Clone, PartialEq, Debug)]
pub enum Event {
    /// Every media session with a track, in a stable order (Windows').
    Media(Vec<Media>),
    /// Every tab playing media in one browser (`host`: its messaging host's id, which its tabs'
    /// app ids carry), or `None` to clear every browser's.
    BrowserMedia { host: Option<u32>, media: Vec<Media> },
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
    /// An integration connected (`true`) or went away (`false`), for the flyout's status.
    Integration { name: &'static str, connected: bool },
    /// Why an integration cannot connect, in a few words the user can act on (`None`: fixed).
    IntegrationProblem { name: &'static str, problem: Option<&'static str> },
    /// The layout changed: switched because another keyboard was typed on, or by the user
    /// (Win+Space). `keyboard` is the name of the one that typed last, empty when it has none
    /// (see [`crate::keyboards`]).
    KeyboardSwitched { keyboard: String, layout: String },
}

/// A short-lived notice the pill shows on top of the steady state.
#[derive(Clone, PartialEq, Debug)]
pub enum Notice {
    /// The default output moved because the old one went away.
    AudioMoved { to: String, from: String },
    /// An app started recording outside a known call; stays until acknowledged.
    Listening { app: String, user: String },
    /// A playing session moved on to a new track (the pill shows it when asked to).
    NewTrack { app_id: String, title: String, artist: String },
    /// The layout is now `layout`, on `keyboard` (empty: no name to give).
    KeyboardSwitched { keyboard: String, layout: String },
}

impl Notice {
    /// How long it shows; `None` until acknowledged.
    pub fn lifetime(&self) -> Option<std::time::Duration> {
        match self {
            Notice::AudioMoved { .. } => Some(std::time::Duration::from_secs(5)),
            Notice::Listening { .. } => None,
            Notice::NewTrack { .. } => Some(std::time::Duration::from_secs(4)),
            // A glance while typing: short, and replaced by the next switch.
            Notice::KeyboardSwitched { .. } => Some(std::time::Duration::from_millis(1500)),
        }
    }
}

/// Everything the surfaces render, folded from events.
#[derive(Default, Clone)]
pub struct State {
    /// Every media session with a track, in a stable order (Windows').
    pub media: Vec<Media>,
    /// Every tab playing media, from the browser extension; see [`State::all_media`].
    pub browser: Vec<Media>,
    /// Apps whose SMTC session just went away, and when: a player drops its session for about a
    /// second on a track change, and its tab must not show alone meanwhile.
    pub gone: Vec<(String, std::time::Instant)>,
    pub mic_users: Vec<String>,
    pub endpoints: Endpoints,
    pub sessions: Vec<AppSession>,
    pub calls: Vec<(VoiceApp, Call)>,
    pub integrations: Vec<(&'static str, bool)>,
    /// Integrations that cannot connect, and why.
    pub problems: Vec<(&'static str, &'static str)>,
    /// The title last announced as a new song: a player reporting through two sources (YouTube
    /// Music's own session and its browser tab) changes song once, not twice.
    announced: Option<String>,
    /// Shown newest last; each with when it started.
    pub notices: Vec<(Notice, std::time::Instant)>,
}

impl State {
    /// Why integration `name` cannot connect, if it cannot.
    pub fn problem(&self, name: &str) -> Option<&'static str> {
        self.problems.iter().find(|(n, _)| *n == name).map(|(_, p)| *p)
    }

    /// Fold one event in. Returns whether anything a surface shows changed.
    pub fn apply(&mut self, event: Event) -> bool {
        match event {
            Event::Media(media) => {
                let now = std::time::Instant::now();
                self.gone.retain(|(_, at)| now.duration_since(*at) < APP_GRACE);
                for old in &self.media {
                    if !media.iter().any(|m| m.app_id == old.app_id) {
                        self.gone.push((old.app.clone(), now));
                    }
                }
                if let Some(notice) = new_track(&self.media, &media) {
                    self.announce(notice);
                }
                replace(&mut self.media, media)
            }
            Event::BrowserMedia { host, media } => {
                let mut next: Vec<Media> = match host {
                    // The other browsers' tabs stay, in place.
                    Some(host) => self.browser.iter().filter(|m| browser_host(&m.app_id) != Some(host)).cloned().collect(),
                    None => Vec::new(),
                };
                next.extend(media);
                if let Some(notice) = new_track(&self.browser, &next) {
                    self.announce(notice);
                }
                replace(&mut self.browser, next)
            }
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
                    if old.id != new.id && !endpoints.outputs.iter().any(|c| c.id == old.id) {
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
            Event::IntegrationProblem { name, problem } => {
                let before = self.problem(name);
                self.problems.retain(|(n, _)| *n != name);
                if let Some(problem) = problem {
                    self.problems.push((name, problem));
                }
                before != problem
            }
            Event::KeyboardSwitched { keyboard, layout } => {
                self.push_notice(Notice::KeyboardSwitched { keyboard, layout });
                true
            }
        }
    }

    /// A new song's notice, unless that title was just announced (through another source).
    fn announce(&mut self, notice: Notice) {
        let Notice::NewTrack { title, .. } = &notice else { return };
        if self.announced.as_ref() == Some(title) {
            return;
        }
        self.announced = Some(title.clone());
        self.push_notice(notice);
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

    /// The call the pill shows: the first one reported (two voice apps in calls at once is rare).
    pub fn call(&self) -> Option<(VoiceApp, &Call)> {
        self.calls.iter().map(|(app, call)| (*app, call)).next()
    }

    /// The sessions to show: the playing ones, at most `MAX_SHOWN`.
    pub fn playing(&self) -> impl Iterator<Item = &Media> {
        self.all_media().into_iter().filter(|m| m.playing).take(MAX_SHOWN)
    }

    /// Every media session, SMTC's and the browser extension's, in a stable order, each once.
    ///
    /// A browser's own SMTC session gives way to the extension's tab with the same title (one per
    /// tab, where SMTC has one per browser). An installed web app's session (YouTube Music runs in
    /// Edge) wins over its tab, since it takes previous/next; they are matched by app, not title,
    /// because titles differ for a moment on a track change, and the tab stays hidden for
    /// [APP_GRACE] after the app's session goes, since players drop it during a track change.
    pub fn all_media(&self) -> Vec<&Media> {
        let is_browser_session = |m: &Media| names::is_browser(&names::app_name(&m.app_id));
        let now = std::time::Instant::now();
        let app_has_session = |tab: &Media| {
            let same_app = |app: &str| !app.is_empty() && app == tab.app;
            self.media.iter().any(|m| !is_browser_session(m) && (same_app(&m.app) || names::same_title(&m.title, &tab.title)))
                || self.gone.iter().any(|(app, at)| same_app(app) && now.duration_since(*at) < APP_GRACE)
        };
        let smtc = self
            .media
            .iter()
            .filter(|m| !(is_browser_session(m) && self.browser.iter().any(|b| names::same_title(&b.title, &m.title))));
        let tabs = self.browser.iter().filter(|b| !app_has_session(b));
        smtc.chain(tabs).collect()
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

/// How long a tab stays hidden after its app's SMTC session went: longer than the music feed's
/// own bridge over a track change (`music::GAP`).
pub const APP_GRACE: std::time::Duration = std::time::Duration::from_secs(6);

/// How many media sessions the surfaces show at once.
pub const MAX_SHOWN: usize = 4;

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

/// A session already there and playing that moved on to another title: a new song (not a player
/// opening, nor a resume, nor the app starting).
fn new_track(old: &[Media], new: &[Media]) -> Option<Notice> {
    new.iter().filter(|m| m.playing && !m.title.trim().is_empty()).find_map(|m| {
        let before = old.iter().find(|o| o.app_id == m.app_id)?;
        (before.title != m.title).then(|| Notice::NewTrack { app_id: m.app_id.clone(), title: m.title.clone(), artist: m.artist.clone() })
    })
}

/// The browser (messaging host id) a tab's app id names: `browser:<host>:<tab>:<frame>`.
pub fn browser_host(app_id: &str) -> Option<u32> {
    app_id.strip_prefix(crate::integrations::browser::APP_ID_PREFIX)?.split(':').next()?.parse().ok()
}
#[cfg(test)]
mod tests {
    use super::*;

    fn endpoint(id: &str) -> Endpoint {
        Endpoint { id: DeviceId(id.into()), name: id.into(), muted: false, icon: crate::icons::IconId::Speakers }
    }

    fn endpoints(output: &str, active: &[&str]) -> Event {
        Event::Endpoints(Endpoints {
            output: Some(endpoint(output)),
            input: None,
            outputs: active.iter().map(|id| Choice { id: DeviceId(id.to_string()), name: id.to_string(), icon: crate::icons::IconId::Speakers }).collect(),
            inputs: Vec::new(),
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

    fn media(app_id: &str, title: &str) -> Media {
        Media {
            app_id: app_id.into(),
            app: String::new(),
            title: title.into(),
            artist: String::new(),
            playing: true,
            cover: None,
            timeline: None,
            skip: false,
            video: false,
        }
    }

    #[test]
    fn a_tab_and_its_smtc_session_show_once() {
        let mut state = State::default();
        state.apply(Event::Media(vec![
            media("music.youtube.com-5929F88E_vezhnr0wkvrcy!App", "Silence"),
            media("MSEdge", "WORLDS"),
        ]));
        state.apply(Event::BrowserMedia {
            host: Some(7),
            media: vec![media("browser:7:1:0", "Silence"), media("browser:7:2:0", "WORLDS"), media("browser:7:3:0", "Vikings")],
        });
        let ids: Vec<&str> = state.all_media().iter().map(|m| m.app_id.as_str()).collect();
        assert_eq!(ids, ["music.youtube.com-5929F88E_vezhnr0wkvrcy!App", "browser:7:2:0", "browser:7:3:0"]);
    }

    #[test]
    fn a_new_song_is_noticed_but_not_a_resume_or_a_player_opening() {
        let song = |title: &str, playing: bool| Media { playing, ..media("ytm", title) };
        let mut state = State::default();
        state.apply(Event::Media(vec![song("One", true)]));
        assert!(state.notices.is_empty(), "a player opening is not a new song");
        state.apply(Event::Media(vec![song("One", false)]));
        state.apply(Event::Media(vec![song("One", true)]));
        assert!(state.notices.is_empty(), "a resume is not a new song");
        state.apply(Event::Media(vec![song("Two", true)]));
        assert!(matches!(&state.notices[..], [(Notice::NewTrack { title, .. }, _)] if title == "Two"));
        // The same song through the player's browser tab is not a second one.
        let at = state.notices[0].1;
        state.apply(Event::BrowserMedia { host: Some(1), media: vec![Media { playing: true, ..media("browser:1:2:0", "One") }] });
        state.apply(Event::BrowserMedia { host: Some(1), media: vec![Media { playing: true, ..media("browser:1:2:0", "Two") }] });
        assert_eq!(state.notices.len(), 1);
        assert_eq!(state.notices[0].1, at, "not raised again");
    }

    #[test]
    fn each_browser_replaces_only_its_own_tabs() {
        let mut state = State::default();
        state.apply(Event::BrowserMedia { host: Some(1), media: vec![media("browser:1:4:0", "Edge stream")] });
        state.apply(Event::BrowserMedia { host: Some(2), media: vec![media("browser:2:9:0", "Chrome stream")] });
        state.apply(Event::BrowserMedia { host: Some(1), media: vec![] });
        let ids: Vec<&str> = state.browser.iter().map(|m| m.app_id.as_str()).collect();
        assert_eq!(ids, ["browser:2:9:0"], "Edge closing its tab leaves Chrome's");
        state.apply(Event::BrowserMedia { host: None, media: vec![] });
        assert!(state.browser.is_empty());
    }

    #[test]
    fn an_apps_tab_stays_hidden_through_a_track_change() {
        let app = |id: &str, title: &str| Media { app: "YouTube Music".into(), ..media(id, title) };
        let mut state = State::default();
        state.apply(Event::Media(vec![app("ytm!App", "Old song")]));
        // The tab already has the new title while the app still shows the old one…
        state.apply(Event::BrowserMedia { host: Some(1), media: vec![app("browser:1:5:0", "New song")] });
        assert_eq!(state.all_media().len(), 1, "matched by app, not title");
        // …then the app's session drops for a moment (past the feed's own bridge).
        state.apply(Event::Media(vec![]));
        assert!(state.all_media().is_empty(), "the tab does not show alone in the gap");
        state.apply(Event::Media(vec![app("ytm!App", "New song")]));
        assert_eq!(state.all_media()[0].app_id, "ytm!App");
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
