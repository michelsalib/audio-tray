//! Which SMTC session is YouTube Music.
//!
//! There is no "app identity" field in SMTC beyond the User Model ID, and what
//! that string looks like depends entirely on *how* YouTube Music is being run.
//! Measured on Windows 11 26200, with a Chromium app-mode window standing in for
//! an installed PWA:
//!
//! ```text
//! MSEdge.localhost_/.edgeprofile.Default
//! ^^^^^^ ^^^^^^^^^^^^ ^^^^^^^^^^^^^^^^^^
//! browser  origin       profile
//! ```
//!
//! So an **installed PWA carries its origin in the id**, which is the case worth
//! having: `music.youtube.com` appears verbatim and the match is exact and
//! unambiguous. A plain browser tab does not — it reports bare `Chrome` or
//! `MSEdge`, indistinguishable from any other tab in the same browser, which is
//! why [`Match::Browser`] exists as a separate, weaker verdict rather than being
//! folded in with the certain ones.
//!
//! The patterns are matched case-insensitively as substrings. That is deliberately
//! loose: the exact shape of a PWA's id varies across Chromium versions and
//! channels, and a missed match means the app shows nothing at all — a far worse
//! failure than an over-broad one, which at worst picks up a YouTube Music tab the
//! user did want.

/// App-id fragments that mean "this is YouTube Music", in confidence order.
///
/// Not a config knob — these are facts about how the players identify themselves.
/// The user-facing escape hatch is an explicit app id in the config, handled by
/// [`classify_with_override`].
const CERTAIN: &[&str] = &[
    // Chromium PWA or `--app=` window: the origin is in the id.
    "music.youtube.com",
    // th-ch/youtube-music and YTMDesktop, which register their own AUMIDs.
    "youtube-music",
    "youtube music",
    "ytmdesktop",
];

/// Browsers whose bare id could be a YouTube Music tab, or could be anything else.
const BROWSERS: &[&str] = &["chrome", "msedge", "firefox", "brave", "opera", "vivaldi"];

/// How confident we are that a given app id is YouTube Music.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Match {
    /// The id names YouTube Music outright, or the user pinned this id in config.
    Certain,
    /// A bare browser id. It *might* be a YouTube Music tab; nothing in SMTC can
    /// say. Only ever used when no [`Match::Certain`] session exists.
    Browser,
    /// Something else entirely — Spotify, a video, a game.
    No,
}

/// Classify one app id.
pub fn classify(app_id: &str) -> Match {
    let id = app_id.to_ascii_lowercase();
    if CERTAIN.iter().any(|needle| id.contains(needle)) {
        return Match::Certain;
    }
    // Bare browser only — an id that merely *starts* with a browser name but
    // carries some other origin is that other site, not a YouTube Music tab.
    if BROWSERS.iter().any(|b| id == *b) {
        return Match::Browser;
    }
    Match::No
}

/// [`classify`], with the config's pinned app id taking precedence.
///
/// The override exists because the certain-patterns list cannot be exhaustive: a
/// PWA id is a Chromium implementation detail, and `audio-tray --music-probe` prints the
/// real one so it can be pinned when the guess misses.
#[allow(dead_code)] // `Ytm::read` applies the pin itself; this is the same rule for other callers.
pub fn classify_with_override(app_id: &str, pinned: Option<&str>) -> Match {
    if let Some(pinned) = pinned {
        return if app_id.eq_ignore_ascii_case(pinned) {
            Match::Certain
        } else {
            Match::No
        };
    }
    classify(app_id)
}

/// Pick the YouTube Music session out of a set of snapshots.
///
/// Preference order, and each step is there for a reason met in testing:
///
/// 1. A [`Match::Certain`] session that is **playing** — with two browser profiles
///    open on YouTube Music, the one making sound is the one the strip should
///    follow.
/// 2. Any [`Match::Certain`] session — so a paused track still shows, rather than
///    the strip emptying the moment you pause.
/// 3. A playing [`Match::Browser`] session, only if nothing certain exists. This is
///    the plain-tab case, and it is last because it is a guess.
pub fn pick<S, F>(snapshots: &[S], app_id: F, playing: impl Fn(&S) -> bool) -> Option<&S>
where
    F: Fn(&S) -> &str,
{
    let certain = |s: &&S| classify(app_id(s)) == Match::Certain;

    snapshots
        .iter()
        .find(|s| certain(s) && playing(s))
        .or_else(|| snapshots.iter().find(certain))
        .or_else(|| {
            snapshots
                .iter()
                .find(|s| classify(app_id(s)) == Match::Browser && playing(s))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pwa_and_desktop_ids_are_certain() {
        // The shape measured on 26200, with the real origin substituted in.
        assert_eq!(
            classify("MSEdge.music.youtube.com_/.edgeprofile.Default"),
            Match::Certain
        );
        assert_eq!(classify("Chrome.music.youtube.com__.Default"), Match::Certain);
        assert_eq!(classify("com.github.th-ch.youtube-music"), Match::Certain);
    }

    #[test]
    fn bare_browser_is_only_a_maybe() {
        assert_eq!(classify("Chrome"), Match::Browser);
        assert_eq!(classify("MSEdge"), Match::Browser);
    }

    #[test]
    fn another_site_in_the_same_browser_is_not_a_match() {
        // The reason `Browser` is an equality test and not a prefix test.
        assert_eq!(classify("MSEdge.open.spotify.com_/.Default"), Match::No);
    }

    #[test]
    fn unrelated_players_are_rejected() {
        assert_eq!(classify("Spotify.exe"), Match::No);
        assert_eq!(classify("Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic"), Match::No);
    }

    #[test]
    fn a_pinned_id_wins_and_excludes_everything_else() {
        let pinned = Some("Weird.Custom.Build");
        assert_eq!(classify_with_override("Weird.Custom.Build", pinned), Match::Certain);
        // Even a normally-certain id loses to an explicit pin — the pin is the
        // user saying "this one, not whatever you would have guessed".
        assert_eq!(classify_with_override("music.youtube.com", pinned), Match::No);
    }

    #[test]
    fn playing_certain_session_beats_paused_one() {
        let sessions = [("MSEdge.music.youtube.com_/.A", false), ("Chrome.music.youtube.com_/.B", true)];
        let picked = pick(&sessions, |s| s.0, |s| s.1).unwrap();
        assert_eq!(picked.0, "Chrome.music.youtube.com_/.B");
    }

    #[test]
    fn paused_certain_session_still_shows() {
        let sessions = [("MSEdge.music.youtube.com_/.A", false)];
        assert!(pick(&sessions, |s| s.0, |s| s.1).is_some());
    }

    #[test]
    fn bare_browser_only_wins_when_nothing_certain_exists() {
        let with_certain = [("Chrome", true), ("MSEdge.music.youtube.com_/.A", false)];
        assert_eq!(pick(&with_certain, |s| s.0, |s| s.1).unwrap().0, "MSEdge.music.youtube.com_/.A");

        let only_browser = [("Chrome", true)];
        assert_eq!(pick(&only_browser, |s| s.0, |s| s.1).unwrap().0, "Chrome");

        // A paused bare browser is not enough to guess on.
        let paused_browser = [("Chrome", false)];
        assert!(pick(&paused_browser, |s| s.0, |s| s.1).is_none());
    }
}
