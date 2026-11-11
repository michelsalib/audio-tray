//! App names as people say them, from the identifiers Windows hands out: SMTC app ids (AUMIDs,
//! bare exe names), consent-store keys (exe paths, package family names) and process paths.

use super::VoiceApp;

/// Known identifiers and their names, matched as case-insensitive substrings, first match wins.
const KNOWN: &[(&str, &str)] = &[
    ("music.youtube.com", "YouTube Music"),
    ("youtube-music", "YouTube Music"),
    ("ytmdesktop", "YouTube Music"),
    ("spotify", "Spotify"),
    ("discord", "Discord"),
    ("msteams", "Teams"),
    ("teams.exe", "Teams"),
    ("zoom", "Zoom"),
    ("slack", "Slack"),
    ("whatsapp", "WhatsApp"),
    ("telegram", "Telegram"),
    ("signal", "Signal"),
    ("skype", "Skype"),
    ("webex", "Webex"),
    ("obs64", "OBS"),
    ("zunemusic", "Media Player"),
    ("microsoft.media.player", "Media Player"),
    ("vlc", "VLC"),
    ("mpc-hc", "MPC-HC"),
    ("mpc-be", "MPC-BE"),
    ("msedge", "Edge"),
    ("chrome", "Chrome"),
    ("firefox", "Firefox"),
    ("brave", "Brave"),
    ("opera", "Opera"),
    ("vivaldi", "Vivaldi"),
    ("soundrecorder", "Sound Recorder"),
];

/// Apps that record because they are calls or meetings: their recording is not an alert even
/// when no integration reports a call.
const MEETING_APPS: &[&str] = &["Discord", "Teams", "Zoom", "Slack", "WhatsApp", "Telegram", "Signal", "Skype", "Webex"];

/// A display name for an app identifier.
pub fn app_name(id: &str) -> String {
    let lower = id.to_ascii_lowercase();
    if let Some((_, name)) = KNOWN.iter().find(|(needle, _)| lower.contains(needle)) {
        return name.to_string();
    }
    // A path or AUMID: its last segment, without the extension or package suffix.
    let last = id.rsplit(['\\', '/', '#']).next().unwrap_or(id);
    let last = last.split('!').next().unwrap_or(last);
    let last = last.strip_suffix(".exe").or_else(|| last.strip_suffix(".EXE")).unwrap_or(last);
    let last = last.split('_').next().unwrap_or(last);
    if last.is_empty() {
        id.to_string()
    } else {
        last.to_string()
    }
}

/// Players and sites that play video, by name ([`app_name`], [`site_name`]): what they play is a video
/// whatever their session says.
const VIDEO: &[&str] = &[
    "MPC-HC", "MPC-BE", "YouTube", "Twitch", "Kick", "Netflix", "Prime Video", "Disney+", "Vimeo", "Dailymotion", "Crunchyroll",
];

/// Whether the player or site named `name` plays video.
pub fn plays_video(name: &str) -> bool {
    VIDEO.contains(&name)
}

/// Browsers, whose media session names the browser rather than the site playing in it.
const BROWSERS: &[&str] = &["Edge", "Chrome", "Firefox", "Brave", "Opera", "Vivaldi"];

/// Sites recognised by name in a browser window's title.
const SITES: &[&str] = &[
    "Twitch", "YouTube", "Kick", "Netflix", "Prime Video", "Disney+", "SoundCloud", "Deezer", "Spotify",
    "Vimeo", "Dailymotion", "Bandcamp", "Apple Music", "Tidal", "Crunchyroll",
];

/// Hosts and their names, for media the browser extension reports.
const HOSTS: &[(&str, &str)] = &[
    ("music.youtube.com", "YouTube Music"),
    ("youtube.com", "YouTube"),
    ("youtu.be", "YouTube"),
    ("twitch.tv", "Twitch"),
    ("kick.com", "Kick"),
    ("netflix.com", "Netflix"),
    ("primevideo.com", "Prime Video"),
    ("disneyplus.com", "Disney+"),
    ("soundcloud.com", "SoundCloud"),
    ("spotify.com", "Spotify"),
    ("deezer.com", "Deezer"),
    ("vimeo.com", "Vimeo"),
    ("dailymotion.com", "Dailymotion"),
    ("bandcamp.com", "Bandcamp"),
    ("crunchyroll.com", "Crunchyroll"),
];

/// A site's name from a tab's host name: a known one, else the domain's name capitalised
/// (`www.example.org` → `Example`).
pub fn site_name(host: &str) -> String {
    let host = host.to_ascii_lowercase();
    if let Some((_, name)) = HOSTS.iter().find(|(domain, _)| host == *domain || host.ends_with(&format!(".{domain}"))) {
        return name.to_string();
    }
    let labels: Vec<&str> = host.split('.').filter(|l| !l.is_empty()).collect();
    let name = match labels.len() {
        0 => return "Browser".to_string(),
        1 => labels[0],
        n => labels[n - 2],
    };
    let mut chars = name.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

pub fn is_browser(name: &str) -> bool {
    BROWSERS.contains(&name)
}

/// Apps whose own media session is followed, by name ([`app_name`]); add a row to follow another.
/// Others that register one (Teams' ringer, Riot Client's launcher) would sit paused in the pill.
const PLAYERS: &[&str] = &["YouTube Music", "MPC-HC", "MPC-BE"];

/// Whether the app named `name` is a player whose session is followed: one of [`PLAYERS`], or a
/// browser, for the sites playing in it (the browser extension narrows those to the followed ones).
pub fn is_player(name: &str) -> bool {
    PLAYERS.contains(&name) || is_browser(name)
}

/// The site a browser session plays from, read from the title of the browser window showing it
/// (`(3) Nynyhon - Twitch et 1 page supplémentaire - Personnel – Microsoft Edge`). `None` when
/// no window mentions the session's title or artist (the tab is not the active one).
pub fn site_in_window(window_title: &str, title: &str, artist: &str) -> Option<String> {
    let lower = window_title.to_lowercase();
    let mentions = |needle: &str| !needle.trim().is_empty() && lower.contains(&needle.to_lowercase());
    if !mentions(title) && !mentions(artist) {
        return None;
    }
    if let Some(site) = SITES.iter().find(|site| lower.contains(&site.to_lowercase())) {
        return Some(site.to_string());
    }
    // Otherwise the segment after the one naming the session, without Edge's localised
    // "and N more pages" ("et 1 page supplémentaire", "und 2 weitere Seiten"…).
    let segments: Vec<&str> = window_title.split(" - ").collect();
    let at = segments.iter().position(|s| mentions_in(s, title) || mentions_in(s, artist))?;
    let next = segments.get(at + 1)?.trim();
    let words: Vec<&str> = next.split_whitespace().collect();
    let cut = words.windows(2).position(|w| w[1].chars().all(|c| c.is_ascii_digit())).unwrap_or(words.len());
    let site = words[..cut].join(" ");
    (!site.is_empty() && site.chars().count() <= 24 && !site.contains("Microsoft")).then_some(site)
}

/// Whether two sources' titles name the same track: equal but for case and spacing, since a
/// browser's SMTC session and its tab's page do not always agree on those. Empty never matches.
pub fn same_title(a: &str, b: &str) -> bool {
    let words = |s: &str| s.split_whitespace().map(str::to_lowercase).collect::<Vec<_>>();
    let a = words(a);
    !a.is_empty() && a == words(b)
}

fn mentions_in(segment: &str, needle: &str) -> bool {
    !needle.trim().is_empty() && segment.to_lowercase().contains(&needle.to_lowercase())
}

pub fn is_meeting_app(name: &str) -> bool {
    MEETING_APPS.contains(&name)
}

/// Whether a display name is the given voice app.
pub fn is_voice_app(name: &str, app: VoiceApp) -> bool {
    name == app.label()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_match_whatever_their_case_and_spacing() {
        assert!(same_title("WORLDS - T1 vs KC", "worlds -  T1 vs KC "));
        assert!(!same_title("WORLDS - T1 vs KC", "WORLDS - T1 vs G2"));
        assert!(!same_title(" ", ""), "no title is not a match");
    }

    #[test]
    fn names_come_from_every_kind_of_identifier() {
        assert_eq!(app_name("music.youtube.com-5929F88E_vezhnr0wkvrcy!App"), "YouTube Music");
        assert_eq!(app_name(r"C:\Users\me\AppData\Local\Discord\app-1.0.9\Discord.exe"), "Discord");
        assert_eq!(app_name("C:#Tools#recorder.exe"), "recorder");
        assert_eq!(app_name("Microsoft.WindowsSoundRecorder_8wekyb3d8bbwe"), "Sound Recorder");
        assert_eq!(app_name("Contoso.Notes_abc123"), "Contoso.Notes");
        assert_eq!(app_name("MSEdge"), "Edge");
        assert_eq!(app_name("mpc-hc64.exe"), "MPC-HC");
    }

    #[test]
    fn only_players_sessions_are_followed() {
        let followed = |id: &str| is_player(&app_name(id));
        assert!(followed("music.youtube.com-5929F88E_vezhnr0wkvrcy!App"));
        assert!(followed("mpc-hc64.exe"));
        assert!(followed("MSEdge"));
        assert!(followed("Chrome"));
        assert!(!followed("MSTeams_8wekyb3d8bbwe!MSTeams"));
        assert!(!followed("Riot Client.exe"));
        assert!(!followed("Spotify.exe"));
    }

    #[test]
    fn a_tabs_site_comes_from_its_host() {
        assert_eq!(site_name("www.twitch.tv"), "Twitch");
        assert_eq!(site_name("m.youtube.com"), "YouTube");
        assert_eq!(site_name("music.youtube.com"), "YouTube Music");
        assert_eq!(site_name("radio.example.org"), "Example");
        assert_eq!(site_name(""), "Browser");
    }

    #[test]
    fn a_browser_sessions_site_comes_from_its_window_title() {
        let edge = "(3) Nynyhon - Twitch et 1 page supplémentaire - Personnel – Microsoft\u{200b} Edge";
        assert_eq!(site_in_window(edge, "On continue la percé avec les Viking", "Nynyhon").as_deref(), Some("Twitch"));
        assert_eq!(site_in_window("(2) Lofi beats - YouTube - Microsoft Edge", "Lofi beats", "").as_deref(), Some("YouTube"));
        assert_eq!(
            site_in_window("Some talk - Conf TV and 2 more pages - Work - Microsoft Edge", "Some talk", "Ana").as_deref(),
            Some("Conf TV")
        );
        assert_eq!(site_in_window("Mail - Outlook", "Some talk", "Ana"), None, "a window not showing it");
    }
}
