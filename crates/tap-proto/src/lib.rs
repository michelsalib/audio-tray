//! What audio-tray and its TAP (inside `explorer.exe`) must agree on. The exe and DLL can be
//! different builds (RELEASING.md), so values are never renumbered, only added.
//!
//! Message ids share the app's receiver window with app-internal ones: `WM_APP + 1..=19` and
//! `+ 30..=39` are the app's own (audio and tray), `+ 20..=29` and `+ 40..` are this crate's.

/// `WM_APP`, spelled out so this crate needs no Windows bindings.
const WM_APP: u32 = 0x8000;

/// The TAP's class id, matched by its `DllGetClassObject`.
pub const CLSID_TAP: u128 = 0xb3e9_2816_117d_476f_936e_06ed_52b2_e55d;
/// The XAML Diagnostics endpoint — shared with TranslucentTB/Windhawk, hence single-consumer.
pub const ENDPOINT_NAME: &str = "VisualDiagConnection1";
/// The TAP's file name, next to `audio-tray.exe`.
pub const TAP_DLL: &str = "audio_tray_tap.dll";

/// Class of the TAP's control window (in Explorer), found by the app with `EnumWindows`.
pub const CONTROL_CLASS: &str = "AudioTrayTapControl";
/// Class of the app's receiver window, found by the TAP with `EnumWindows`.
pub const RECEIVER_CLASS: &str = "AudioTrayTaskbarIpc";

/// TAP → app: a gesture on the strip; `wParam` is an action code below.
pub const WM_TASKBAR_ACTION: u32 = WM_APP + 20;
/// App → TAP: put the taskbar back. `wParam`: the requesting owner's pid, 0 = unconditional.
pub const WM_TAP_REVERT: u32 = WM_APP + 21;
/// App-internal: Explorer restarted (re-posted `TaskbarCreated`).
pub const WM_TASKBAR_RESTARTED: u32 = WM_APP + 22;
/// App → TAP: redraw the strip. `wParam` output, `lParam` input, each a packed glyph (below).
pub const WM_TAP_RESTYLE: u32 = WM_APP + 23;
/// TAP or wheel hook → app: a scroll over a button; `wParam` flow code, `lParam` signed delta.
pub const WM_TASKBAR_SCROLL: u32 = WM_APP + 24;
/// App-internal: the music feed's progress-bar value for the tray thread.
pub const WM_MUSIC_PROGRESS: u32 = WM_APP + 25;
/// TAP-internal: wire the hover preview's transport buttons now.
pub const WM_TAP_WIRE_TRANSPORT: u32 = WM_APP + 26;
/// TAP-internal: re-pin the music tile's indicators now.
pub const WM_TAP_REPIN: u32 = WM_APP + 27;
/// App → TAP: the window the app last put a taskbar progress bar on (`wParam`, 0 = none), so a
/// revert after the app dies clears exactly that window.
pub const WM_TAP_PROGRESS_WINDOW: u32 = WM_APP + 28;
/// App → TAP: the now-playing state file was rewritten; re-read it.
pub const WM_TAP_MUSIC_CHANGED: u32 = WM_APP + 29;
/// TAP → app: which parts of the taskbar integration are not working (`wParam`, `STATUS_*` bits;
/// 0 = all well). Posted when it changes, after a start-up grace period.
pub const WM_TASKBAR_STATUS: u32 = WM_APP + 40;

/// Our buttons are not on the taskbar (the notification icon was not found or not decorated).
pub const STATUS_NO_STRIP: usize = 1 << 0;
/// Moving the icons next to wifi failed and was given up.
pub const STATUS_REORDER_FAILED: usize = 1 << 1;
/// Taskbar elements the TAP relies on are missing on this Windows build.
pub const STATUS_SHAPE_CHANGED: usize = 1 << 2;

/// Restyle packing: the codepoint in the low 24 bits, then these flags.
pub const RESTYLE_GLYPH_MASK: usize = 0x00FF_FFFF;
pub const RESTYLE_MUTED: usize = 1 << 24;
pub const RESTYLE_RECORDING: usize = 1 << 25;

/// Action codes in [`WM_TASKBAR_ACTION`]'s `wParam`; music starts at 10, away from audio.
pub const ACTION_CYCLE_OUTPUT: usize = 1;
pub const ACTION_CYCLE_INPUT: usize = 2;
pub const ACTION_OPEN_PANEL: usize = 3;
pub const ACTION_MUSIC_PREVIOUS: usize = 10;
pub const ACTION_MUSIC_PLAY_PAUSE: usize = 11;
pub const ACTION_MUSIC_NEXT: usize = 12;

/// Flow codes in [`WM_TASKBAR_SCROLL`]'s `wParam`.
pub const FLOW_OUTPUT: usize = 0;
pub const FLOW_INPUT: usize = 1;

/// Handover `WM_COPYDATA` tag ("ATH1") and its results; 0 means a TAP without the handler.
pub const HANDOVER_MAGIC: usize = 0x4154_4831;
pub const HANDOVER_ACCEPTED: isize = 1;
pub const HANDOVER_DECLINED: isize = 2;

/// Plane-15 PUA markers (Segoe Fluent owns the BMP PUA) for earbuds the TAP draws as vectors.
pub const GLYPH_WIRELESS_EARBUDS: char = '\u{F0001}';
pub const GLYPH_ROUND_EARBUDS: char = '\u{F0002}';

/// The now-playing state file in `%TEMP%`, written by the app and re-read by the TAP.
pub const MUSIC_STATE_FILE: &str = "audio-tray-music.txt";

/// Version of the app ↔ TAP contract (init-data keys, messages, state file). A loaded TAP adopts a
/// new app on a handover when this matches and the DLL file is the same, instead of requiring the
/// identical app version. Bump it on any change an older TAP or app would misread.
pub const PROTOCOL: u32 = 2;

/// Encodes init data: `key=value` pairs joined by `;`. Values escape `%`, `;` and `=` as `%25`,
/// `%3B` and `%3D`, so a tile name or install path containing them survives; anything else is
/// written as is (so ordinary payloads read the same to an older parser).
pub fn encode_init<'a>(pairs: impl IntoIterator<Item = (&'a str, String)>) -> String {
    pairs
        .into_iter()
        .map(|(key, value)| format!("{key}={}", escape_value(&value)))
        .collect::<Vec<_>>()
        .join(";")
}

fn escape_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '%' => out.push_str("%25"),
            ';' => out.push_str("%3B"),
            '=' => out.push_str("%3D"),
            c => out.push(c),
        }
    }
    out
}

fn unescape_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(at) = rest.find('%') {
        out.push_str(&rest[..at]);
        let code = rest.get(at + 1..at + 3);
        match code {
            Some("25") => out.push('%'),
            Some("3B") | Some("3b") => out.push(';'),
            Some("3D") | Some("3d") => out.push('='),
            // Not one of ours: keep it verbatim.
            _ => {
                out.push('%');
                rest = &rest[at + 1..];
                continue;
            }
        }
        rest = &rest[at + 3..];
    }
    out.push_str(rest);
    out
}

/// Every `(key, value)` in init data, keys and values trimmed and values unescaped.
pub fn init_pairs(data: &str) -> impl Iterator<Item = (&str, String)> {
    data.split(';')
        .filter_map(|pair| pair.split_once('='))
        .map(|(key, value)| (key.trim(), unescape_value(value.trim())))
}

/// One value out of init data; `None` when absent or empty.
pub fn init_value(data: &str, wanted: &str) -> Option<String> {
    init_pairs(data)
        .find(|(key, _)| *key == wanted)
        .map(|(_, value)| value)
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_data_round_trips_awkward_values() {
        let tile = "Rock; Roll = 100%".to_string();
        let path = r"C:\Users\a=b;c\audio_tray_tap.dll".to_string();
        let data = encode_init([("tile", tile.clone()), ("tap", path.clone()), ("pid", "7".into())]);
        assert_eq!(init_value(&data, "tile"), Some(tile));
        assert_eq!(init_value(&data, "tap"), Some(path));
        assert_eq!(init_value(&data, "pid").as_deref(), Some("7"));
        assert_eq!(init_value(&data, "missing"), None);
    }

    /// An ordinary payload is byte-identical to the old unescaped format, so an older TAP reads it.
    #[test]
    fn plain_values_are_not_escaped() {
        let data = encode_init([("tooltip", "Audio Tray".into()), ("out", "E767".into())]);
        assert_eq!(data, "tooltip=Audio Tray;out=E767");
    }

    #[test]
    fn empty_values_read_as_absent_and_stray_percents_survive() {
        assert_eq!(init_value("tile=;pid=1", "tile"), None);
        assert_eq!(init_value("tile=50%off", "tile").as_deref(), Some("50%off"));
    }

    #[test]
    fn message_ids_are_distinct() {
        let ids = [
            WM_TASKBAR_ACTION, WM_TAP_REVERT, WM_TASKBAR_RESTARTED, WM_TAP_RESTYLE, WM_TASKBAR_SCROLL,
            WM_MUSIC_PROGRESS, WM_TAP_WIRE_TRANSPORT, WM_TAP_REPIN, WM_TAP_PROGRESS_WINDOW,
            WM_TAP_MUSIC_CHANGED, WM_TASKBAR_STATUS,
        ];
        for (i, a) in ids.iter().enumerate() {
            assert!(ids[i + 1..].iter().all(|b| a != b));
        }
    }

    #[test]
    fn a_glyph_survives_the_restyle_flags() {
        assert_eq!(GLYPH_ROUND_EARBUDS as usize & !RESTYLE_GLYPH_MASK, 0);
    }
}
