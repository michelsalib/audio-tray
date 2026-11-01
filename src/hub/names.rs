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
    fn names_come_from_every_kind_of_identifier() {
        assert_eq!(app_name("music.youtube.com-5929F88E_vezhnr0wkvrcy!App"), "YouTube Music");
        assert_eq!(app_name(r"C:\Users\me\AppData\Local\Discord\app-1.0.9\Discord.exe"), "Discord");
        assert_eq!(app_name("C:#Tools#recorder.exe"), "recorder");
        assert_eq!(app_name("Microsoft.WindowsSoundRecorder_8wekyb3d8bbwe"), "Sound Recorder");
        assert_eq!(app_name("Contoso.Notes_abc123"), "Contoso.Notes");
        assert_eq!(app_name("MSEdge"), "Edge");
    }
}
