//! Which SMTC session, and which window, is YouTube Music.
//!
//! An installed PWA carries its origin in its app id (`music.youtube.com`), so it is matched with
//! certainty; a plain tab reports a bare `MSEdge`/`Chrome` like every other tab and is **never
//! followed** (it put YouTube videos in the strip). Users who run it as a tab pin `music.app_id`.
//! Windows are judged by the shell's app id, never by title alone. Patterns are case-insensitive
//! substrings: a missed match shows nothing, which is worse than an over-broad one.

/// App-id fragments that mean "this is YouTube Music". Not a config knob: a pinned `music.app_id`
/// replaces this matching entirely (see [`crate::music::feed::Ytm`]).
const CERTAIN: &[&str] = &[
    // Chromium PWA or `--app=` window: the origin is in the id.
    "music.youtube.com",
    // th-ch/youtube-music and YTMDesktop, which register their own AUMIDs.
    "youtube-music",
    "youtube music",
    "ytmdesktop",
];

/// Browsers whose bare id could be a YouTube Music tab, or anything else. Also the executable
/// list for [`is_browser_process`].
const BROWSERS: &[&str] = &["chrome", "msedge", "firefox", "brave", "opera", "vivaldi"];

/// How confident we are that a given app id is YouTube Music.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Match {
    /// The id names YouTube Music outright, or the user pinned this id in config.
    Certain,
    /// A bare browser id: maybe a YouTube Music tab. Never followed by [`pick`]; only offered
    /// for pinning by `--music-probe`.
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
    // Bare browser only: a browser prefix with another origin is that other site.
    if BROWSERS.iter().any(|b| id == *b) {
        return Match::Browser;
    }
    Match::No
}

/// Whether an executable name is a browser's (`msedge.exe`, …); the fallback of [`window_is_player`].
pub fn is_browser_process(exe: &str) -> bool {
    let exe = exe.to_ascii_lowercase();
    BROWSERS
        .iter()
        .any(|browser| exe == format!("{browser}.exe"))
}

/// Whether a window (already matched on title) is the player's own rather than a browser window
/// showing the same title. `app_id` is the window's `PKEY_AppUserModel_ID` and must be
/// [`Match::Certain`]; with none published (Win32/Electron players) any non-browser process passes.
/// A pinned `music.app_id` deliberately has no say: it names a session, not a window.
pub fn window_is_player(app_id: Option<&str>, process: Option<&str>) -> bool {
    match app_id {
        Some(app_id) => classify(app_id) == Match::Certain,
        None => !process.is_some_and(is_browser_process),
    }
}

/// Pick the YouTube Music session: a playing [`Match::Certain`] one, else any certain one, else none.
/// Returns an index into `snapshots` so the caller can read that live session in full.
pub fn pick<S, F>(snapshots: &[S], app_id: F, playing: impl Fn(&S) -> bool) -> Option<usize>
where
    F: Fn(&S) -> &str,
{
    let certain = |s: &S| classify(app_id(s)) == Match::Certain;

    snapshots
        .iter()
        .position(|s| certain(s) && playing(s))
        .or_else(|| snapshots.iter().position(certain))
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

    fn picked(sessions: &[(&'static str, bool)]) -> Option<&'static str> {
        pick(sessions, |s| s.0, |s| s.1).map(|index| sessions[index].0)
    }

    #[test]
    fn playing_certain_session_beats_paused_one() {
        let sessions = [("MSEdge.music.youtube.com_/.A", false), ("Chrome.music.youtube.com_/.B", true)];
        assert_eq!(picked(&sessions), Some("Chrome.music.youtube.com_/.B"));
    }

    #[test]
    fn paused_certain_session_still_shows() {
        let sessions = [("MSEdge.music.youtube.com_/.A", false)];
        assert!(picked(&sessions).is_some());
    }

    /// With the player closed, a browser playing a video must not land in the strip.
    #[test]
    fn a_bare_browser_session_is_never_followed() {
        let only_browser = [("MSEdge", true)];
        assert_eq!(picked(&only_browser), None);

        let with_certain = [("MSEdge", true), ("MSEdge.music.youtube.com_/.A", false)];
        assert_eq!(picked(&with_certain), Some("MSEdge.music.youtube.com_/.A"));
    }

    /// The two window ids measured on 26200: same `msedge.exe`, only this string tells them apart.
    #[test]
    fn a_browser_window_is_not_the_player() {
        assert!(window_is_player(
            Some("music.youtube.com-5929F88E_vezhnr0wkvrcy!App"),
            Some("msedge.exe")
        ));
        assert!(!window_is_player(
            Some("MSEdge.UserData.Profile1"),
            Some("msedge.exe")
        ));
    }

    #[test]
    fn a_window_with_no_identity_falls_back_to_the_title() {
        assert!(window_is_player(None, Some("youtube-music.exe")));
    }

    #[test]
    fn a_browser_with_no_window_identity_is_still_not_the_player() {
        assert!(!window_is_player(None, Some("msedge.exe")));
        assert!(!window_is_player(None, Some("Chrome.exe")));
        // Neither field readable: nothing says browser, so the title stands.
        assert!(window_is_player(None, None));
    }

    /// The index is into the slice as given, not a filtered subsequence.
    #[test]
    fn the_index_addresses_the_original_slice() {
        let sessions = [
            ("Spotify.exe", true),
            ("MSEdge.open.spotify.com_/.Default", true),
            ("Chrome.music.youtube.com_/.B", true),
        ];
        assert_eq!(pick(&sessions, |s| s.0, |s| s.1), Some(2));
    }
}
