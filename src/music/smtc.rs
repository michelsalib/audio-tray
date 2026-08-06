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

/// Which transport commands the app says it will honour right now.
///
/// Worth respecting rather than always drawing every button: YouTube Music
/// disables "previous" at the head of a queue, and a control that visibly does
/// nothing reads as a broken strip.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Capabilities {
    pub can_play: bool,
    pub can_pause: bool,
    pub can_skip_next: bool,
    pub can_skip_previous: bool,
}

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
    pub capabilities: Capabilities,
    /// Cover art bytes, as published (PNG or JPEG — read the magic, don't assume).
    /// `None` whenever the app publishes no artwork; see the module note.
    pub cover: Option<Vec<u8>>,
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
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Command {
    Play,
    Pause,
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
    /// Returned as snapshots keyed by app id, because that is all the caller can
    /// safely keep — see [`Snapshot`].
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

    /// The session Windows considers current — what a media key would reach.
    ///
    /// **Not used as a fallback, deliberately.** Falling back to "whatever is current" is how a
    /// click on a YouTube Music strip pauses MPC-HC. Kept for `--probe`-style diagnosis, where the
    /// question *is* which session would win.
    #[allow(dead_code)]
    pub fn current(&self) -> Result<Option<Snapshot>> {
        match self.manager.GetCurrentSession() {
            Ok(session) => Ok(Some(read_session(&session)?)),
            // No current session is an error rather than a null on this API.
            Err(_) => Ok(None),
        }
    }

    /// The timeline of the session owned by `app_id`, if it publishes one.
    ///
    /// Separate from [`Snapshot`] rather than a field of it, until it is known to be worth drawing:
    /// `GetTimelineProperties` is a second cross-process read per poll, and a Chromium session that
    /// publishes nothing but zeroes would make the strip pay for it every 100 ms for no picture.
    pub fn timeline(&self, app_id: &str) -> Result<Option<Timeline>> {
        let Some(session) = self.find(app_id)? else {
            return Ok(None);
        };
        let properties = session
            .GetTimelineProperties()
            .context("GetTimelineProperties")?;
        Ok(Some(Timeline {
            start: properties.StartTime().map(|s| s.Duration).unwrap_or(0),
            end: properties.EndTime().map(|s| s.Duration).unwrap_or(0),
            position: properties.Position().map(|s| s.Duration).unwrap_or(0),
            last_updated: properties
                .LastUpdatedTime()
                .map(|t| t.UniversalTime)
                .unwrap_or(0),
        }))
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
        Command::Play => session.TryPlayAsync()?.get()?,
        Command::Pause => session.TryPauseAsync()?.get()?,
        Command::TogglePlayPause => session.TryTogglePlayPauseAsync()?.get()?,
        Command::Next => session.TrySkipNextAsync()?.get()?,
        Command::Previous => session.TrySkipPreviousAsync()?.get()?,
    };
    Ok(accepted)
}

/// Read one session into a [`Snapshot`].
///
/// Every field is independently fallible and every failure degrades to a default
/// rather than failing the whole read: a session mid-track-change routinely has
/// properties that are briefly unavailable, and losing the strip for a moment
/// would be worse than showing a blank artist.
fn read_session(session: &Session) -> Result<Snapshot> {
    let app_id = session
        .SourceAppUserModelId()
        .map(|s| s.to_string())
        .unwrap_or_default();

    let mut snapshot = Snapshot {
        app_id,
        ..Default::default()
    };

    if let Ok(info) = session.GetPlaybackInfo() {
        if let Ok(status) = info.PlaybackStatus() {
            snapshot.status = PlaybackStatus::from_winrt(status);
        }
        if let Ok(controls) = info.Controls() {
            snapshot.capabilities = Capabilities {
                can_play: controls.IsPlayEnabled().unwrap_or(false),
                can_pause: controls.IsPauseEnabled().unwrap_or(false),
                can_skip_next: controls.IsNextEnabled().unwrap_or(false),
                can_skip_previous: controls.IsPreviousEnabled().unwrap_or(false),
            };
        }
    }

    // The properties are a single async round-trip to the owning app, so a wedged
    // player shows up here as a slow read. Treated as "nothing to draw yet".
    if let Ok(op) = session.TryGetMediaPropertiesAsync() {
        if let Ok(props) = op.get() {
            snapshot.title = props.Title().map(|s| s.to_string()).unwrap_or_default();
            snapshot.artist = props.Artist().map(|s| s.to_string()).unwrap_or_default();
            snapshot.album = props.AlbumTitle().map(|s| s.to_string()).unwrap_or_default();
            snapshot.cover = read_thumbnail(&props);
        }
    }

    Ok(snapshot)
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
