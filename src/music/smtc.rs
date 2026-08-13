//! A thin, safe skin over `Windows.Media.Control` — the Global System Media
//! Transport Controls (GSMTC, or just SMTC).
//!
//! This is the same feed the volume-OSD and the lock screen read, and it is how
//! YouTube Music publishes what it is playing: the web app calls the Media Session
//! API, Chromium forwards that to SMTC, and the session shows up here alongside
//! Spotify and everything else on the machine.
//!
//! Nothing in this module knows what YouTube Music *is* — picking its session out
//! of the list is [`super::session`]'s job. This layer only knows how to read a
//! session and how to command one.
//!
//! Two things measured on Windows 11 26200 that shape the code below:
//!
//! * **Cover art is only published for artwork the browser actually fetched.** A
//!   `data:` URL in `MediaMetadata.artwork` produces a session with *no*
//!   thumbnail at all; an `http(s)` URL produces one. Nothing here can work
//!   around that — it is decided in Chromium — so a missing cover is a normal
//!   state, not an error. YouTube Music serves real URLs, so in practice it has
//!   one.
//! * **The thumbnail is not the artwork as published.** Chromium re-encodes and
//!   downscales it; a 256×256 PNG came back as 544 bytes. So this is art to draw
//!   small, which is exactly what a taskbar section wants.

use anyhow::{Context, Result};
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session,
    GlobalSystemMediaTransportControlsSessionManager as SessionManager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as WinRtPlaybackStatus,
};
use windows::Media::MediaPlaybackType;
use windows::Storage::Streams::DataReader;

/// What a session is doing, reduced to the states a transport control cares about.
///
/// SMTC's own enum has six values; `Closed`, `Opened` and `Changing` are all
/// "there is a session but it is not telling us anything useful yet", and folding
/// them into one state keeps the UI from flickering through them during a track
/// change.
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

/// What kind of media the session says it is carrying.
///
/// Read from `GetPlaybackInfo`, so it costs nothing beyond the status that is already read there.
///
/// **Do not use this to tell music from video in a browser — it cannot.** It looks exactly like the
/// field that would separate a YouTube Music tab from a YouTube video, and it was added to try
/// that; measured on 26200, a plain YouTube video playing in Edge reports `Music`. Chromium sets
/// the type once for its whole SMTC integration, because that is what lets it publish title,
/// artist and album at all. Kept because it is free, `--music-probe` prints it, and the next person
/// to have this idea should be able to see the answer without wiring it up again.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum MediaKind {
    #[default]
    Unknown,
    Music,
    Video,
    Image,
}

impl MediaKind {
    fn from_winrt(kind: MediaPlaybackType) -> Self {
        match kind {
            MediaPlaybackType::Music => Self::Music,
            MediaPlaybackType::Video => Self::Video,
            MediaPlaybackType::Image => Self::Image,
            _ => Self::Unknown,
        }
    }
}

// There is deliberately no `Capabilities` here. `GetPlaybackInfo`'s `Controls` says which
// transport commands the app will honour, and it was read into every snapshot and brief for a
// strip that was going to grey the buttons out — but the buttons are the shell's thumbnail
// toolbar now (see `super::thumbbar`), and it draws and enables them itself.

/// Everything worth drawing about one session, read in a single pass.
///
/// A snapshot rather than a live handle on purpose: the session object is
/// thread-affine COM, and the strip wants a plain value it can compare against
/// the last one to decide whether anything actually changed.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Snapshot {
    /// The app's User Model ID — `Chrome`/`MSEdge` for a plain tab, and an
    /// origin-qualified id for an installed PWA. This is what session matching
    /// keys on.
    pub app_id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub status: PlaybackStatus,
    pub kind: MediaKind,
    /// Cover art bytes, as published (PNG or JPEG — read the magic, don't assume).
    /// `None` whenever the app publishes no artwork; see the module note.
    pub cover: Option<Vec<u8>>,
}

/// What a session is, without asking it what it is playing.
///
/// The half of a session that is free to read — see [`Smtc::briefs`]. It carries exactly what
/// choosing a session needs and nothing that costs a cross-process call, which is what lets the
/// expensive read happen once per poll instead of once per session.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Brief {
    pub app_id: String,
    pub status: PlaybackStatus,
    pub kind: MediaKind,
}

/// One poll's answer: the session that won, and where it is in the track.
///
/// The two together because they come from the same enumeration — keeping them apart is what used to
/// cost a second `GetSessions` per poll for the timeline alone.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Reading {
    pub snapshot: Snapshot,
    pub timeline: Option<Timeline>,
}

impl Snapshot {
    /// Whether this looks like a session with something real in it.
    ///
    /// A session can exist with an empty title during a track change; drawing that
    /// gives a strip that blinks to blank between songs.
    pub fn has_track(&self) -> bool {
        !self.title.trim().is_empty()
    }
}

/// Where the track is, as the app reports it.
///
/// Every field is in the units SMTC uses — 100 ns ticks for the spans, and a Windows `DateTime` for
/// `last_updated` — because the interesting question about this data is not "how long is the song"
/// but **when was it last true**. A player publishes a position when something happens, not
/// continuously, so anything drawn from `position` alone is stale between updates and a progress bar
/// has to interpolate from `last_updated` while the status is `Playing`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Timeline {
    pub start: i64,
    pub end: i64,
    pub position: i64,
    pub last_updated: i64,
}

impl Timeline {
    const TICKS_PER_SECOND: f64 = 10_000_000.0;

    /// Track length in seconds, or `None` when the app publishes no end.
    pub fn duration_seconds(self) -> Option<f64> {
        let span = self.end - self.start;
        (span > 0).then(|| span as f64 / Self::TICKS_PER_SECOND)
    }

    pub fn position_seconds(self) -> f64 {
        (self.position - self.start).max(0) as f64 / Self::TICKS_PER_SECOND
    }

    /// How far through the track it is *now*, 0.0–1.0, or `None` when there is no track to be
    /// through.
    ///
    /// **`position` is a checkpoint, not a clock** — measured: over 6 s of playback it moved 1.2 s,
    /// because the player republishes it when something happens and then leaves it alone. So while
    /// the status is `Playing` the time since `last_updated` is added; paused, the checkpoint is the
    /// truth and nothing is added.
    ///
    /// `now` is a parameter rather than read here so the arithmetic is testable — the whole point of
    /// this function is what it does to a stale reading, and that cannot be tested against a clock
    /// that keeps moving.
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
///
/// From `SystemTime` plus the known offset rather than `GetSystemTimeAsFileTime`, which would pull in
/// another Windows feature for arithmetic the standard library already has.
pub fn now_ticks() -> i64 {
    /// 1601-01-01 to 1970-01-01, in 100 ns ticks.
    const UNIX_EPOCH_IN_TICKS: i64 = 116_444_736_000_000_000;
    let since_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    UNIX_EPOCH_IN_TICKS + (since_unix.as_nanos() / 100) as i64
}

impl Timeline {
    /// Whether there is anything here worth drawing.
    ///
    /// A session that publishes no timeline at all reads as all zeroes, which is the case this
    /// exists to distinguish — see `--timeline`.
    pub fn is_published(self) -> bool {
        self.end > self.start || self.position > 0
    }
}

/// A transport command, as the strip's buttons express it.
///
/// There is no `Play` or `Pause`: the strip has one button for both, and SMTC's own
/// `TryTogglePlayPauseAsync` is what keeps it honest when the session changed state behind us.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Command {
    TogglePlayPause,
    Next,
    Previous,
}

/// The SMTC session manager, held open for the life of the app.
///
/// `RequestAsync` is a real cost (it brokers to the shell), so this is resolved
/// once and reused rather than per poll.
pub struct Smtc {
    manager: SessionManager,
}

impl Smtc {
    pub fn new() -> Result<Self> {
        let manager = SessionManager::RequestAsync()
            .context("SMTC RequestAsync")?
            .get()
            .context("awaiting the SMTC session manager")?;
        Ok(Self { manager })
    }

    /// Every session Windows currently knows about, newest state each time.
    ///
    /// **The expensive one, and only `--music-probe` should want it.** Reading a session in full
    /// means an async round-trip to the owning app plus a decode of its artwork; doing that for every
    /// session is what [`Smtc::read_current`] exists to avoid. Kept because diagnosis genuinely does
    /// want every session's title, and pays for it once.
    pub fn sessions(&self) -> Result<Vec<Snapshot>> {
        let sessions = self.manager.GetSessions().context("GetSessions")?;
        let mut out = Vec::new();
        for session in &sessions {
            // One unreadable session must not blank the whole strip: apps come and
            // go mid-enumeration, and a session that died between the list and the
            // read is routine rather than exceptional.
            if let Ok(snapshot) = read_session(&session) {
                out.push(snapshot);
            }
        }
        Ok(out)
    }

    /// One poll's worth of reading, in a single enumeration.
    ///
    /// **The shape exists to stop paying for sessions nobody asked about.** Before this, a poll read
    /// every session in full — an async properties call and a complete artwork decode *each* — and
    /// then threw all but one away, then enumerated a second time for the timeline. Per second, on a
    /// machine with three media sessions, that was two `GetSessions`, three cross-process round trips
    /// and three image decodes to draw one strip.
    ///
    /// Now: one `GetSessions`, a local `GetPlaybackInfo` per session, and then the properties, the
    /// artwork and the timeline for the **one** session `choose` returns.
    ///
    /// `choose` gets the briefs and returns an index into them, so the picking rule stays in
    /// [`super::session`] where it is tested, and this stays the part that knows about WinRT.
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
            kind: briefs[index].kind,
            ..Default::default()
        };
        read_properties_into(session, &mut snapshot);
        Ok(Some(Reading {
            snapshot,
            timeline: read_timeline(session),
        }))
    }

    /// The timeline of the session owned by `app_id`, if it publishes one.
    ///
    /// **Not on the poll path** — [`Smtc::read_current`] returns the timeline from the enumeration it
    /// already did, because asking separately meant a second `GetSessions` every second. This is for
    /// `--music-timeline`, which asks about one session by name and has nothing else in flight.
    pub fn timeline(&self, app_id: &str) -> Result<Option<Timeline>> {
        let Some(session) = self.find(app_id)? else {
            return Ok(None);
        };
        Ok(read_timeline(&session))
    }

    /// Send `command` to the session owned by `app_id`.
    ///
    /// Deliberately addressed by app id rather than to "the current session": the
    /// point of this app is to drive YouTube Music, and the current session is
    /// whatever played last — a video in another tab would happily swallow the
    /// click.
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

/// Issue one command and wait for the app's answer.
///
/// The `bool` is SMTC's own "did the app accept it", which is not the same as "the
/// state changed" — a `TryPlayAsync` against an already-playing session returns
/// true and does nothing. Callers use it to detect a session that has gone deaf,
/// not to confirm the new state.
fn dispatch(session: &Session, command: Command) -> Result<bool> {
    let accepted = match command {
        Command::TogglePlayPause => session.TryTogglePlayPauseAsync()?.get()?,
        Command::Next => session.TrySkipNextAsync()?.get()?,
        Command::Previous => session.TrySkipPreviousAsync()?.get()?,
    };
    Ok(accepted)
}

/// Read one session into a [`Snapshot`], in full.
///
/// Every field is independently fallible and every failure degrades to a default
/// rather than failing the whole read: a session mid-track-change routinely has
/// properties that are briefly unavailable, and losing the strip for a moment
/// would be worse than showing a blank artist.
fn read_session(session: &Session) -> Result<Snapshot> {
    let brief = read_brief(session);
    let mut snapshot = Snapshot {
        app_id: brief.app_id,
        status: brief.status,
        kind: brief.kind,
        ..Default::default()
    };
    read_properties_into(session, &mut snapshot);
    Ok(snapshot)
}

/// The cheap half: who the session belongs to and what it is doing.
///
/// No async call and no artwork, so this costs the same whether the machine has one media session or
/// six — which is what makes it safe to run over all of them on every poll.
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
        // An `IReference` that is null — the app published no type at all — reads as an error here,
        // which is the `Unknown` default.
        if let Ok(kind) = info.PlaybackType().and_then(|t| t.Value()) {
            brief.kind = MediaKind::from_winrt(kind);
        }
    }
    brief
}

/// The expensive half: what is playing, and the picture of it.
///
/// The properties are a single async round-trip to the owning app, so a wedged
/// player shows up here as a slow read. Treated as "nothing to draw yet".
fn read_properties_into(session: &Session, snapshot: &mut Snapshot) {
    if let Ok(op) = session.TryGetMediaPropertiesAsync() {
        if let Ok(props) = op.get() {
            snapshot.title = props.Title().map(|s| s.to_string()).unwrap_or_default();
            snapshot.artist = props.Artist().map(|s| s.to_string()).unwrap_or_default();
            snapshot.album = props.AlbumTitle().map(|s| s.to_string()).unwrap_or_default();
            snapshot.cover = read_thumbnail(&props);
        }
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

/// Pull the cover art out of a session's properties, or `None`.
///
/// Never an error: no artwork is the normal state for plenty of players, and the
/// strip has a placeholder for it.
fn read_thumbnail(
    props: &windows::Media::Control::GlobalSystemMediaTransportControlsSessionMediaProperties,
) -> Option<Vec<u8>> {
    let reference = props.Thumbnail().ok()?;
    let stream = reference.OpenReadAsync().ok()?.get().ok()?;
    let size = stream.Size().ok()?;
    // A zero-length stream is a player that advertised artwork and then had none;
    // `ReadBytes` against it is legal and yields nothing useful.
    if size == 0 {
        return None;
    }
    // Cover art is small by construction (Chromium downscales it), but the size
    // comes from another process, so it still gets a ceiling rather than being
    // trusted into an allocation.
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

    /// The measured shape of a real reading: a 275.7 s track, a position published a few seconds
    /// ago, and the question of what to draw *now*.
    fn reading(position_s: i64, published_ago_s: i64, now: i64) -> Timeline {
        Timeline {
            start: 0,
            end: 275 * SECOND,
            position: position_s * SECOND,
            last_updated: now - published_ago_s * SECOND,
        }
    }

    /// **The whole point of the interpolation.** A position published 10 s ago is 10 s stale while
    /// the track is playing, and drawing it directly is a bar that sits still and then jumps.
    #[test]
    fn a_stale_position_is_carried_forward_while_playing() {
        let now = 134_304_952_968_951_060;
        let timeline = reading(100, 10, now);
        let stale = timeline.fraction_at(now, false).unwrap();
        let live = timeline.fraction_at(now, true).unwrap();
        assert!((stale - 100.0 / 275.0).abs() < 1e-6, "{stale}");
        assert!((live - 110.0 / 275.0).abs() < 1e-6, "{live}");
    }

    /// Paused, the checkpoint *is* the truth — carrying it forward would show a bar creeping along
    /// under a player that is not playing.
    #[test]
    fn a_paused_position_is_taken_as_published() {
        let now = 134_304_952_968_951_060;
        let timeline = reading(158, 30, now);
        assert_eq!(
            timeline.fraction_at(now, false),
            timeline.fraction_at(now + 60 * SECOND, false)
        );
    }

    /// A reading left behind by a track that finished must not run past the end of the bar.
    #[test]
    fn interpolation_stops_at_the_end_of_the_track() {
        let now = 134_304_952_968_951_060;
        let timeline = reading(270, 600, now);
        assert_eq!(timeline.fraction_at(now, true), Some(1.0));
    }

    /// A session with no timeline at all reads as zeroes, and a zero-length track has no fraction to
    /// draw — the caller clears the bar rather than showing an empty one.
    #[test]
    fn no_timeline_means_no_bar() {
        let empty = Timeline::default();
        assert_eq!(empty.fraction_at(now_ticks(), true), None);
        assert!(!empty.is_published());
    }

    /// The epoch has to match SMTC's, or every interpolation is out by 369 years.
    #[test]
    fn now_is_in_the_same_epoch_as_the_timestamps() {
        // The real reading measured on 2026-08-06; "now" must be within a few years of it rather
        // than in 1601 or 1970.
        let measured = 134_304_952_968_951_060;
        let now = now_ticks();
        let years = (now - measured).abs() as f64 / (SECOND as f64 * 86_400.0 * 365.0);
        assert!(years < 5.0, "now_ticks is {years} years from a real reading");
    }
}
