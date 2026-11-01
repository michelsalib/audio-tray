//! A thin, safe skin over `Windows.Media.Control` (SMTC): reading and commanding sessions.
//!
//! Knows nothing about YouTube Music; picking its session is [`super::session`]'s job. All calls
//! block on WinRT async operations, so this must run on an MTA thread. A missing cover is a normal
//! state (Chromium publishes art only for fetched `http(s)` artwork). [`Smtc::watch`] reports
//! changes as they happen, so nothing needs to poll.

use std::cell::RefCell;
use std::sync::Arc;

use anyhow::{Context, Result};
use windows::Foundation::TypedEventHandler;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session,
    GlobalSystemMediaTransportControlsSessionManager as SessionManager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as WinRtPlaybackStatus,
};
use windows::Storage::Streams::DataReader;

/// What a session is doing; SMTC's `Closed`/`Opened`/`Changing` all fold into `Unknown`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum PlaybackStatus {
    #[default]
    Unknown,
    Stopped,
    Paused,
    Playing,
}

impl PlaybackStatus {
    fn from_winrt(status: WinRtPlaybackStatus) -> Self {
        match status {
            WinRtPlaybackStatus::Playing => Self::Playing,
            WinRtPlaybackStatus::Paused => Self::Paused,
            WinRtPlaybackStatus::Stopped => Self::Stopped,
            _ => Self::Unknown,
        }
    }

    pub fn is_playing(self) -> bool {
        self == Self::Playing
    }
}

/// Everything worth drawing about one session, as a plain comparable value.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Snapshot {
    /// The app's User Model ID, which session matching keys on.
    pub app_id: String,
    pub title: String,
    pub artist: String,
    pub status: PlaybackStatus,
    /// Cover art bytes as published (PNG or JPEG: read the magic). `None` if none published.
    pub cover: Option<Vec<u8>>,
}

/// The cheap part of a session (no cross-process call): enough to choose one.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Brief {
    pub app_id: String,
    pub status: PlaybackStatus,
}

/// One poll's answer: the chosen session and its timeline, from one enumeration.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Reading {
    pub snapshot: Snapshot,
    pub timeline: Option<Timeline>,
}

impl Snapshot {
    /// Whether there is a title (it is briefly empty during a track change).
    pub fn has_track(&self) -> bool {
        !self.title.trim().is_empty()
    }
}

/// Where the track is, as the app reports it, in SMTC units (100 ns ticks; `last_updated` is a
/// Windows `DateTime`). `position` is a checkpoint as of `last_updated`, not a live clock.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Timeline {
    pub start: i64,
    pub end: i64,
    pub position: i64,
    pub last_updated: i64,
}

impl Timeline {
    #[cfg(feature = "dev")]
    const TICKS_PER_SECOND: f64 = 10_000_000.0;

    /// Track length in seconds, or `None` when the app publishes no end.
    #[cfg(feature = "dev")]
    pub fn duration_seconds(self) -> Option<f64> {
        let span = self.end - self.start;
        (span > 0).then(|| span as f64 / Self::TICKS_PER_SECOND)
    }

    #[cfg(feature = "dev")]
    pub fn position_seconds(self) -> f64 {
        (self.position - self.start).max(0) as f64 / Self::TICKS_PER_SECOND
    }

    /// How far through the track it is at `now`, 0.0–1.0, or `None` with no track length. While
    /// playing, the time since `last_updated` is added to the checkpoint.
    pub fn fraction_at(self, now: i64, playing: bool) -> Option<f64> {
        let span = self.end - self.start;
        if span <= 0 {
            return None;
        }
        let mut position = self.position - self.start;
        if playing && self.last_updated > 0 {
            position += (now - self.last_updated).max(0);
        }
        Some((position as f64 / span as f64).clamp(0.0, 1.0))
    }
}

/// Now, in the epoch SMTC timestamps use: 100 ns ticks since 1601.
pub fn now_ticks() -> i64 {
    /// 1601-01-01 to 1970-01-01, in 100 ns ticks.
    const UNIX_EPOCH_IN_TICKS: i64 = 116_444_736_000_000_000;
    let since_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    UNIX_EPOCH_IN_TICKS + (since_unix.as_nanos() / 100) as i64
}

/// A transport command. One toggle, not `Play`/`Pause`, so SMTC resolves the current state.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Command {
    TogglePlayPause,
    Next,
    Previous,
}

/// What an SMTC event says changed. Handlers run on the WinRT pool, so they only report it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Change {
    /// A session came or went: call [`Smtc::watch_sessions`] to follow the new set.
    Sessions,
    /// Title, artist or cover: call [`Smtc::properties_changed`] so the cover is read again.
    Properties,
    /// Playback status or timeline.
    Playback,
}

/// Receives every [`Change`], from any thread.
pub type Notify = Arc<dyn Fn(Change) + Send + Sync>;

/// One session's handlers, removed on drop.
struct Subscription {
    session: Session,
    playback: i64,
    properties: i64,
    timeline: i64,
}

impl Subscription {
    fn new(session: Session, notify: &Notify) -> Result<Self> {
        Ok(Self {
            playback: session.PlaybackInfoChanged(&handler(notify, Change::Playback))?,
            properties: session.MediaPropertiesChanged(&handler(notify, Change::Properties))?,
            timeline: session.TimelinePropertiesChanged(&handler(notify, Change::Playback))?,
            session,
        })
    }
}

/// An event handler that reports `change`, whatever the event's arguments.
fn handler<S, A>(notify: &Notify, change: Change) -> TypedEventHandler<S, A>
where
    S: windows::core::RuntimeType + 'static,
    A: windows::core::RuntimeType + 'static,
{
    let notify = notify.clone();
    TypedEventHandler::new(move |_, _| {
        notify(change);
        Ok(())
    })
}

impl Drop for Subscription {
    fn drop(&mut self) {
        // A session that is already gone takes its handlers with it.
        let _ = self.session.RemovePlaybackInfoChanged(self.playback);
        let _ = self.session.RemoveMediaPropertiesChanged(self.properties);
        let _ = self.session.RemoveTimelinePropertiesChanged(self.timeline);
    }
}

/// The SMTC session manager, requested once (it brokers to the shell) and held open.
pub struct Smtc {
    manager: SessionManager,
    covers: RefCell<CoverCache>,
    /// Set by [`Smtc::watch`]; `None` for the one-shot dev readers.
    notify: Option<Notify>,
    sessions_changed: Option<i64>,
    subscriptions: RefCell<Vec<Subscription>>,
}

impl Smtc {
    pub fn new() -> Result<Self> {
        let manager = SessionManager::RequestAsync()
            .context("SMTC RequestAsync")?
            .get()
            .context("awaiting the SMTC session manager")?;
        Ok(Self {
            manager,
            covers: Default::default(),
            notify: None,
            sessions_changed: None,
            subscriptions: Default::default(),
        })
    }

    /// Report every change to `notify`: sessions coming and going, and each session's playback,
    /// properties and timeline.
    pub fn watch(&mut self, notify: Notify) -> Result<()> {
        let token = self
            .manager
            .SessionsChanged(&handler(&notify, Change::Sessions))
            .context("subscribing to SessionsChanged")?;
        self.sessions_changed = Some(token);
        self.notify = Some(notify);
        self.watch_sessions()
    }

    /// Subscribe to the sessions there are now, dropping the old subscriptions. Call after
    /// [`Change::Sessions`]; does nothing before [`Smtc::watch`].
    pub fn watch_sessions(&self) -> Result<()> {
        let Some(notify) = &self.notify else {
            return Ok(());
        };
        let mut subscriptions = self.subscriptions.borrow_mut();
        subscriptions.clear();
        for session in &self.manager.GetSessions().context("GetSessions")? {
            // Sessions dying mid-enumeration is routine; their SessionsChanged follows.
            if let Ok(subscription) = Subscription::new(session, notify) {
                subscriptions.push(subscription);
            }
        }
        Ok(())
    }

    /// The followed track's title, artist or cover changed: read the cover again next time.
    pub fn properties_changed(&self) {
        self.covers.borrow_mut().stale = true;
    }

    /// Every session, read in full. Expensive (a round-trip and artwork read each): `--music-probe` only.
    #[cfg(feature = "dev")]
    pub fn sessions(&self) -> Result<Vec<Snapshot>> {
        let sessions = self.manager.GetSessions().context("GetSessions")?;
        let mut out = Vec::new();
        for session in &sessions {
            // Sessions dying mid-enumeration is routine; skip them.
            if let Ok(snapshot) = read_session(&session) {
                out.push(snapshot);
            }
        }
        Ok(out)
    }

    /// One poll's reading: one `GetSessions`, a cheap [`Brief`] per session, and a full read only of
    /// the session whose index `choose` returns.
    pub fn read_current<F>(&self, choose: F) -> Result<Option<Reading>>
    where
        F: FnOnce(&[Brief]) -> Option<usize>,
    {
        let sessions: Vec<Session> = self
            .manager
            .GetSessions()
            .context("GetSessions")?
            .into_iter()
            .collect();
        let briefs: Vec<Brief> = sessions.iter().map(read_brief).collect();
        let Some(index) = choose(&briefs) else {
            return Ok(None);
        };
        let session = sessions.get(index).context("chosen session is out of range")?;

        let mut snapshot = Snapshot {
            app_id: briefs[index].app_id.clone(),
            status: briefs[index].status,
            ..Default::default()
        };
        read_properties_into(session, &mut snapshot, Some(&mut self.covers.borrow_mut()));
        Ok(Some(Reading {
            snapshot,
            timeline: read_timeline(session),
        }))
    }

    /// Send `command` to the session owned by `app_id` (never "the current session", which may be
    /// another player).
    pub fn send(&self, app_id: &str, command: Command) -> Result<bool> {
        let session = self
            .find(app_id)?
            .with_context(|| format!("no SMTC session for {app_id}"))?;
        dispatch(&session, command)
    }

    fn find(&self, app_id: &str) -> Result<Option<Session>> {
        let sessions = self.manager.GetSessions().context("GetSessions")?;
        for session in &sessions {
            let id = session
                .SourceAppUserModelId()
                .map(|s| s.to_string())
                .unwrap_or_default();
            if id == app_id {
                return Ok(Some(session));
            }
        }
        Ok(None)
    }
}

impl Drop for Smtc {
    fn drop(&mut self) {
        if let Some(token) = self.sessions_changed {
            let _ = self.manager.RemoveSessionsChanged(token);
        }
    }
}

/// Issue one command and wait. The `bool` means "the app accepted it", not "the state changed".
fn dispatch(session: &Session, command: Command) -> Result<bool> {
    let accepted = match command {
        Command::TogglePlayPause => session.TryTogglePlayPauseAsync()?.get()?,
        Command::Next => session.TrySkipNextAsync()?.get()?,
        Command::Previous => session.TrySkipPreviousAsync()?.get()?,
    };
    Ok(accepted)
}

/// Read one session into a [`Snapshot`], in full; each unreadable field degrades to its default.
#[cfg(feature = "dev")]
fn read_session(session: &Session) -> Result<Snapshot> {
    let brief = read_brief(session);
    let mut snapshot = Snapshot {
        app_id: brief.app_id,
        status: brief.status,
        ..Default::default()
    };
    read_properties_into(session, &mut snapshot, None);
    Ok(snapshot)
}

/// The cheap half: owner and playback status, no async call, safe to run over every session.
fn read_brief(session: &Session) -> Brief {
    let app_id = session
        .SourceAppUserModelId()
        .map(|s| s.to_string())
        .unwrap_or_default();
    let mut brief = Brief {
        app_id,
        ..Default::default()
    };
    if let Ok(info) = session.GetPlaybackInfo() {
        if let Ok(status) = info.PlaybackStatus() {
            brief.status = PlaybackStatus::from_winrt(status);
        }
    }
    brief
}

/// The expensive half: title, artist and cover (an async round-trip to the owning app).
fn read_properties_into(session: &Session, snapshot: &mut Snapshot, covers: Option<&mut CoverCache>) {
    if let Ok(op) = session.TryGetMediaPropertiesAsync() {
        if let Ok(props) = op.get() {
            snapshot.title = props.Title().map(|s| s.to_string()).unwrap_or_default();
            snapshot.artist = props.Artist().map(|s| s.to_string()).unwrap_or_default();
            snapshot.cover = match covers {
                Some(covers) => covers.cover(CoverKey::of(snapshot), || read_thumbnail(&props)),
                None => read_thumbnail(&props),
            };
        }
    }
}

/// Which track a cover belongs to.
#[derive(Clone, PartialEq, Eq, Debug)]
struct CoverKey {
    app_id: String,
    title: String,
    artist: String,
}

impl CoverKey {
    fn of(snapshot: &Snapshot) -> Self {
        Self { app_id: snapshot.app_id.clone(), title: snapshot.title.clone(), artist: snapshot.artist.clone() }
    }
}

/// The current track's cover, so a reading does not reopen and decode the thumbnail stream.
///
/// Re-read on a new track, after a properties change marks it `stale` (players can publish the new
/// title before its art), and while there is none; reused otherwise.
#[derive(Default)]
struct CoverCache {
    key: Option<CoverKey>,
    stale: bool,
    bytes: Option<Vec<u8>>,
}

impl CoverCache {
    fn cover(&mut self, key: CoverKey, read: impl FnOnce() -> Option<Vec<u8>>) -> Option<Vec<u8>> {
        if self.key.as_ref() != Some(&key) || self.stale || self.bytes.is_none() {
            self.key = Some(key);
            self.stale = false;
            self.bytes = read();
        }
        self.bytes.clone()
    }
}

/// A session's timeline, or `None` when it publishes none.
fn read_timeline(session: &Session) -> Option<Timeline> {
    let properties = session.GetTimelineProperties().ok()?;
    Some(Timeline {
        start: properties.StartTime().map(|s| s.Duration).unwrap_or(0),
        end: properties.EndTime().map(|s| s.Duration).unwrap_or(0),
        position: properties.Position().map(|s| s.Duration).unwrap_or(0),
        last_updated: properties
            .LastUpdatedTime()
            .map(|t| t.UniversalTime)
            .unwrap_or(0),
    })
}

/// Pull the cover art out of a session's properties, or `None` (a normal state).
fn read_thumbnail(
    props: &windows::Media::Control::GlobalSystemMediaTransportControlsSessionMediaProperties,
) -> Option<Vec<u8>> {
    let reference = props.Thumbnail().ok()?;
    let stream = reference.OpenReadAsync().ok()?.get().ok()?;
    let size = stream.Size().ok()?;
    if size == 0 {
        return None;
    }
    // The size comes from another process: cap it before allocating.
    const MAX_COVER: u64 = 8 * 1024 * 1024;
    if size > MAX_COVER {
        return None;
    }

    let reader = DataReader::CreateDataReader(&stream).ok()?;
    reader.LoadAsync(size as u32).ok()?.get().ok()?;
    let mut bytes = vec![0u8; size as usize];
    reader.ReadBytes(&mut bytes).ok()?;
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECOND: i64 = 10_000_000;

    /// A 275 s track with a position published `published_ago_s` before `now`.
    fn reading(position_s: i64, published_ago_s: i64, now: i64) -> Timeline {
        Timeline {
            start: 0,
            end: 275 * SECOND,
            position: position_s * SECOND,
            last_updated: now - published_ago_s * SECOND,
        }
    }

    #[test]
    fn a_stale_position_is_carried_forward_while_playing() {
        let now = 134_304_952_968_951_060;
        let timeline = reading(100, 10, now);
        let stale = timeline.fraction_at(now, false).unwrap();
        let live = timeline.fraction_at(now, true).unwrap();
        assert!((stale - 100.0 / 275.0).abs() < 1e-6, "{stale}");
        assert!((live - 110.0 / 275.0).abs() < 1e-6, "{live}");
    }

    #[test]
    fn a_paused_position_is_taken_as_published() {
        let now = 134_304_952_968_951_060;
        let timeline = reading(158, 30, now);
        assert_eq!(
            timeline.fraction_at(now, false),
            timeline.fraction_at(now + 60 * SECOND, false)
        );
    }

    #[test]
    fn interpolation_stops_at_the_end_of_the_track() {
        let now = 134_304_952_968_951_060;
        let timeline = reading(270, 600, now);
        assert_eq!(timeline.fraction_at(now, true), Some(1.0));
    }

    #[test]
    fn no_timeline_means_no_bar() {
        let empty = Timeline::default();
        assert_eq!(empty.fraction_at(now_ticks(), true), None);
    }

    #[test]
    fn now_is_in_the_same_epoch_as_the_timestamps() {
        // A real reading from 2026-08-06; "now" must be within a few years of it.
        let measured = 134_304_952_968_951_060;
        let now = now_ticks();
        let years = (now - measured).abs() as f64 / (SECOND as f64 * 86_400.0 * 365.0);
        assert!(years < 5.0, "now_ticks is {years} years from a real reading");
    }

    fn key(title: &str) -> CoverKey {
        CoverKey { app_id: "YTM!App".into(), title: title.into(), artist: "Artist".into() }
    }

    #[test]
    fn a_cover_is_read_once_per_track() {
        let mut cache = CoverCache::default();
        let mut reads = 0;
        for _ in 0..10 {
            let got = cache.cover(key("A"), || {
                reads += 1;
                Some(vec![1, 2, 3])
            });
            assert_eq!(got, Some(vec![1, 2, 3]));
        }
        assert_eq!(reads, 1);
        assert_eq!(cache.cover(key("B"), || Some(vec![2])), Some(vec![2]));
    }

    #[test]
    fn a_properties_change_reads_the_cover_again() {
        let mut cache = CoverCache::default();
        assert_eq!(cache.cover(key("A"), || Some(vec![1])), Some(vec![1]));
        cache.stale = true;
        assert_eq!(cache.cover(key("A"), || Some(vec![9])), Some(vec![9]));
        assert_eq!(cache.cover(key("A"), || panic!("not stale")), Some(vec![9]));
    }

    #[test]
    fn no_cover_keeps_asking() {
        let mut cache = CoverCache::default();
        for _ in 0..5 {
            assert_eq!(cache.cover(key("A"), || None), None);
        }
        assert_eq!(cache.cover(key("A"), || Some(vec![7])), Some(vec![7]));
    }
}
