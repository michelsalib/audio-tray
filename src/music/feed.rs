//! The YouTube Music feed: what is playing, and how to drive it.
//!
//! [`smtc`] knows how to talk to Windows; [`session`] knows which session is
//! YouTube Music. This module is the pairing of the two that the rest of the app
//! uses, and it owns one piece of state neither of them has: *which* session we
//! settled on last time, so a command goes to the same player the strip is
//! currently showing rather than to whatever is momentarily current.


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
    #[allow(dead_code)] // The strip reads `State` by matching; this is for callers that do not.
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
    /// The app id the last [`Ytm::read`] settled on. Commands address this rather
    /// than re-deciding, so a click always reaches the player the user is looking
    /// at — even if another app became "current" in between.
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

    /// Read the current state and the track position together, remembering which session they came
    /// from.
    ///
    /// **One enumeration, and the artwork is read once.** The picking rule runs against the cheap
    /// [`smtc::Brief`]s and only the session it returns is read in full — which is the difference
    /// between one cross-process round-trip per poll and one *per session on the machine*.
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
        // A session that exists but has no title yet is a track change in flight. Keeping the
        // previous app id means the buttons stay live through it.
        let state = if reading.snapshot.has_track() {
            State::Track(reading.snapshot)
        } else {
            State::Absent
        };
        Ok((state, reading.timeline))
    }

    /// Send a transport command to the session the last [`Ytm::read`] found.
    ///
    /// Returns `Ok(false)` when there is nothing to command — no YouTube Music
    /// session, or one that refused. The caller treats that as "the click did
    /// nothing", not as an error worth surfacing.
    pub fn send(&self, command: Command) -> Result<bool> {
        let Some(app_id) = self.current_app_id.as_deref() else {
            return Ok(false);
        };
        self.smtc.send(app_id, command)
    }

    /// The app id of the session the strip is currently following.
    ///
    /// For an installed PWA this is a real AUMID (`<PackageFamilyName>!App`), which is exactly
    /// what `shell:AppsFolder\<aumid>` needs to bring the player forward.
    pub fn current_app_id(&self) -> Option<&str> {
        self.current_app_id.as_deref()
    }

    /// Where the followed session says it is in the track, if it says anything.
    pub fn timeline(&self, app_id: &str) -> Result<Option<smtc::Timeline>> {
        self.smtc.timeline(app_id)
    }

    /// Every session on the machine, for `--probe`.
    ///
    /// The point of exposing this is diagnosis: when the built-in matching misses
    /// an unusual YouTube Music build, this is what shows the real app id to pin.
    pub fn all_sessions(&self) -> Result<Vec<Snapshot>> {
        self.smtc.sessions()
    }

    /// Whether some *other* app holds a media session right now.
    ///
    /// Written as a guard for the media-key fallback and then **not used for that**, because it is
    /// not a sufficient one: a player can register for media keys without publishing an SMTC
    /// session at all, so "no foreign session" does not mean "the key will reach us". The fallback
    /// raises the player instead — see the cold-start branch in `main::handle_click`.
    ///
    /// Kept because it answers a question worth answering elsewhere: whether the strip is idle
    /// because nothing is playing, or because something *else* is.
    ///
    /// Errors count as "yes, something else is there" — failing closed is the safe direction for
    /// every caller this could have.
    #[allow(dead_code)] // Kept deliberately, and the doc comment above says why.
    pub fn foreign_session_exists(&self) -> bool {
        // Briefs, not full snapshots: the question is only about app ids, and reading every
        // session's artwork to answer it would cost more than the answer is worth.
        match self.smtc.briefs() {
            Ok(sessions) => sessions
                .iter()
                .any(|s| session::classify(&s.app_id) == session::Match::No),
            Err(_) => true,
        }
    }
}
