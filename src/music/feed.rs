//! The YouTube Music feed: what is playing, and how to drive it.
//!
//! Pairs [`smtc`] with [`session`], and remembers which session was last settled on so commands
//! reach the player the strip is showing.


use super::{session, smtc};

use anyhow::Result;

pub use smtc::{Command, PlaybackStatus, Snapshot};

/// The current YouTube Music state, as the strip wants it.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub enum State {
    /// No YouTube Music session on the machine — nothing to draw.
    #[default]
    Absent,
    /// A session exists and has a track in it.
    Track(Snapshot),
}

impl State {
    pub fn snapshot(&self) -> Option<&Snapshot> {
        match self {
            Self::Track(snapshot) => Some(snapshot),
            Self::Absent => None,
        }
    }
}

/// Reads YouTube Music's session and sends it commands.
pub struct Ytm {
    smtc: smtc::Smtc,
    /// An app id pinned in config, which overrides the built-in matching.
    pinned: Option<String>,
    /// The app id the last [`Ytm::read`] settled on; commands address this rather than re-deciding.
    current_app_id: Option<String>,
}

impl Ytm {
    pub fn new(pinned: Option<String>) -> Result<Self> {
        Ok(Self {
            smtc: smtc::Smtc::new()?,
            pinned,
            current_app_id: None,
        })
    }

    /// Read the current state and track position, remembering which session they came from. Picks
    /// on the cheap [`smtc::Brief`]s and reads only the chosen session in full.
    pub fn read(&mut self) -> Result<(State, Option<smtc::Timeline>)> {
        let pinned = self.pinned.clone();
        let reading = self.smtc.read_current(|briefs| match pinned.as_deref() {
            // A pinned id is an exact instruction: use that session or none.
            Some(pinned) => briefs
                .iter()
                .position(|b| b.app_id.eq_ignore_ascii_case(pinned)),
            None => session::pick(briefs, |b| b.app_id.as_str(), |b| b.status.is_playing()),
        })?;

        let Some(reading) = reading else {
            self.current_app_id = None;
            return Ok((State::Absent, None));
        };

        self.current_app_id = Some(reading.snapshot.app_id.clone());
        // No title yet = a track change in flight; the app id is kept so buttons stay live.
        let state = if reading.snapshot.has_track() {
            State::Track(reading.snapshot)
        } else {
            State::Absent
        };
        Ok((state, reading.timeline))
    }

    /// Send a transport command to the session the last [`Ytm::read`] found. `Ok(false)` means no
    /// session or a refusal: the click did nothing, which is not an error.
    pub fn send(&self, command: Command) -> Result<bool> {
        let Some(app_id) = self.current_app_id.as_deref() else {
            return Ok(false);
        };
        self.smtc.send(app_id, command)
    }

    /// The app id of the followed session; for a PWA a real AUMID usable with `shell:AppsFolder\<aumid>`.
    pub fn current_app_id(&self) -> Option<&str> {
        self.current_app_id.as_deref()
    }

    /// Every session on the machine, for `--music-probe` (shows the real app id to pin).
    #[cfg(feature = "dev")]
    pub fn all_sessions(&self) -> Result<Vec<Snapshot>> {
        self.smtc.sessions()
    }
}
