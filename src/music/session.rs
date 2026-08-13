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
//! **A bare browser id is never followed on its own**, and that is the outcome of trying the
//! alternative. It used to be a last-resort guess — with no YouTube Music session anywhere, a
//! playing browser session was taken to be a YouTube Music tab — and what that actually does is
//! put a YouTube video's title and thumbnail in the strip whenever the player is closed, which is
//! wrong far more often than it is right.
//!
//! Nothing in the session can rescue the guess; measured on 26200 against a plain YouTube video in
//! Edge, every field that looks like it should separate music from video does not:
//!
//! ```text
//! app id  MSEdge      the same id every other tab in that browser reports
//! kind    Music       Chromium says Music for a video too — see smtc::MediaKind
//! album   <empty>     and YouTube Music does not always publish one either
//! ```
//!
//! So the escape hatch is explicit rather than inferred: a user who really does run YouTube Music
//! as a plain tab pins `MSEdge` (or `Chrome`) as `app_id` in the config, which is the same rule
//! this used to apply silently — but chosen, and only on the machine that wants it.
//!
//! **The same question is asked of a window**, by [`window_is_player`], and it had to be: the
//! progress bar and the thumbnail toolbar are put on an HWND, and the HWND used to be found by
//! title alone. A browser window shows its active tab's title, so a YouTube Music tab makes a plain
//! Edge window answer to "the YouTube Music window" — and the toolbar, which has no removal call,
//! then stays under that browser's hover preview long after the tab has moved on. Measured on 26200:
//! the PWA window and the browser window are the same `msedge.exe`, the same process id and the same
//! `Chrome_WidgetWin_1` class, and the only field that separates them is the shell's own id for the
//! window.
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
///
/// Doubles as the executable list [`is_browser_process`] matches on, which is deliberate: the two
/// questions are the same one asked of a session and of a window.
const BROWSERS: &[&str] = &["chrome", "msedge", "firefox", "brave", "opera", "vivaldi"];

/// How confident we are that a given app id is YouTube Music.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Match {
    /// The id names YouTube Music outright, or the user pinned this id in config.
    Certain,
    /// A bare browser id. It *might* be a YouTube Music tab; nothing in SMTC can
    /// say. Never followed by [`pick`] — it names an id worth *offering* to pin, and
    /// `--music-probe` is where that offer is made.
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

/// Whether an executable name is a browser's — `msedge.exe`, `chrome.exe`, and the rest.
///
/// The fallback half of [`window_is_player`], for a window that publishes no identity of its own.
pub fn is_browser_process(exe: &str) -> bool {
    let exe = exe.to_ascii_lowercase();
    BROWSERS
        .iter()
        .any(|browser| exe == format!("{browser}.exe"))
}

/// Whether a **window** is the player's own, rather than a browser window showing the same title.
///
/// **The title cannot answer this, and that is not a subtlety — it is measured.** On 26200 an
/// installed PWA and a plain tab are windows of the *same* `msedge.exe`, same process id, same
/// `Chrome_WidgetWin_1` class, and the browser's title reads `YouTube Music …` whenever that tab is
/// the active one. The one field that separates them is the shell's own identity for the window,
/// the `PKEY_AppUserModel_ID` its taskbar button is grouped under:
///
/// ```text
/// PWA window      music.youtube.com-5929F88E_vezhnr0wkvrcy!App   Certain
/// browser window  MSEdge.UserData.Profile1                       No
/// ```
///
/// So a window is the player's when its id is [`Match::Certain`] — the same verdict the session
/// side follows, on the same string kind — and nothing weaker. Note that a browser window's id is
/// not even [`Match::Browser`]: the profile suffix makes it fail the bare-id test, which is exactly
/// what [`classify`] means by "some other origin in the same browser".
///
/// `app_id` of `None` means the window publishes no id at all, which is the normal case for a
/// plain Win32 or Electron player (th-ch/youtube-music, YTMDesktop) — there the title is all there
/// is, and it has already matched by the time this is asked. The process is the guard on *that*
/// path: a browser that published nothing must still not be followed, and the identity failing to
/// read for any other reason must not quietly reopen the hole this closes.
///
/// A pinned `music.app_id` deliberately has no say here. It names a *session*, and the session it
/// names when a user runs the player as a plain tab is a browser's — whose window plays everything
/// else that browser plays. The strip follows it; the buttons and the progress bar, which land on a
/// window and cannot be taken off one, do not.
pub fn window_is_player(app_id: Option<&str>, process: Option<&str>) -> bool {
    match app_id {
        Some(app_id) => classify(app_id) == Match::Certain,
        None => !process.is_some_and(is_browser_process),
    }
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
///
/// And nothing else: with no certain session on the machine the answer is **none**, not the
/// closest thing available. See the module note — the third step this used to have followed any
/// playing browser session, which is how a YouTube video ends up in the strip with the player
/// closed.
///
/// Returns an **index**, not a reference: the caller reads the chosen session in full afterwards,
/// and it needs to know *which* of the live session objects to read rather than getting a borrow of
/// the cheap description it picked from.
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

    #[test]
    fn a_pinned_id_wins_and_excludes_everything_else() {
        let pinned = Some("Weird.Custom.Build");
        assert_eq!(classify_with_override("Weird.Custom.Build", pinned), Match::Certain);
        // Even a normally-certain id loses to an explicit pin — the pin is the
        // user saying "this one, not whatever you would have guessed".
        assert_eq!(classify_with_override("music.youtube.com", pinned), Match::No);
    }

    /// The index `pick` returns has to address the *same* slice the caller passed, because that is
    /// what it then reads in full — an off-by-one here would draw one session's art under another's
    /// title.
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

    /// **The bug this rule exists for.** With YouTube Music closed, the only session on the machine
    /// is whatever else the browser is playing — a video, a stream, an autoplaying page — and it
    /// reports the same bare `MSEdge` a YouTube Music tab would. Following it puts a video's title
    /// and thumbnail in the strip; the strip stays empty instead.
    #[test]
    fn a_bare_browser_session_is_never_followed() {
        let only_browser = [("MSEdge", true)];
        assert_eq!(picked(&only_browser), None);

        let with_certain = [("MSEdge", true), ("MSEdge.music.youtube.com_/.A", false)];
        assert_eq!(picked(&with_certain), Some("MSEdge.music.youtube.com_/.A"));
    }

    /// The way back for someone who really does run the player as a plain tab: pin the id, and the
    /// override makes it certain. This is the old fallback, opted into.
    #[test]
    fn a_pinned_browser_id_is_how_a_plain_tab_is_followed() {
        assert_eq!(classify_with_override("MSEdge", Some("MSEdge")), Match::Certain);
    }

    /// **The bug the window rule exists for**, in the two ids measured on 26200 — one `msedge.exe`,
    /// two windows, and only this string telling them apart. The browser window used to pass on its
    /// title alone, which is how prev/play/next ended up under Edge's hover preview with the PWA
    /// closed and the strip showing nothing.
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

    /// A player that publishes no window identity — an Electron build, or any plain Win32 one — is
    /// still followed on its title, because that is all such a window offers.
    #[test]
    fn a_window_with_no_identity_falls_back_to_the_title() {
        assert!(window_is_player(None, Some("youtube-music.exe")));
    }

    /// And the guard on that fallback: a browser whose window published nothing must not slip
    /// through it. This is also what holds if the identity ever fails to read at all.
    #[test]
    fn a_browser_with_no_window_identity_is_still_not_the_player() {
        assert!(!window_is_player(None, Some("msedge.exe")));
        assert!(!window_is_player(None, Some("Chrome.exe")));
        // Neither field readable: nothing says browser, so the title stands — the old behaviour,
        // kept for the players it was right about.
        assert!(window_is_player(None, None));
    }

    /// The index is into the slice as given, not into some filtered subsequence — the case that
    /// would break if `pick` ever grew a `filter` before its `position`.
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
