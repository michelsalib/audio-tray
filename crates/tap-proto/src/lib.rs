//! What audio-tray and its TAP (inside `explorer.exe`) must agree on. Both sides build from these
//! constants, so they cannot drift; the exe and the DLL can still be *different builds* (see
//! RELEASING.md), so values here are never renumbered, only added.

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
/// App → TAP: redraw the strip. `wParam` output, `lParam` input, each a glyph packed with
/// [`RESTYLE_MUTED`] / [`RESTYLE_RECORDING`].
pub const WM_TAP_RESTYLE: u32 = WM_APP + 23;
/// TAP or wheel hook → app: a scroll over a button; `wParam` a flow code, `lParam` the signed
/// wheel delta in `WHEEL_DELTA` units.
pub const WM_TASKBAR_SCROLL: u32 = WM_APP + 24;
/// App-internal: the music feed's progress-bar value for the tray thread.
pub const WM_MUSIC_PROGRESS: u32 = WM_APP + 25;
/// TAP-internal: wire the hover preview's transport buttons now.
pub const WM_TAP_WIRE_TRANSPORT: u32 = WM_APP + 26;
/// TAP-internal: re-pin the music tile's indicators now.
pub const WM_TAP_REPIN: u32 = WM_APP + 27;

/// Restyle packing: the codepoint in the low 24 bits, then these flags.
pub const RESTYLE_GLYPH_MASK: usize = 0x00FF_FFFF;
pub const RESTYLE_MUTED: usize = 1 << 24;
pub const RESTYLE_RECORDING: usize = 1 << 25;

/// Action codes in [`WM_TASKBAR_ACTION`]'s `wParam`. The music ones start at 10, far from the
/// audio ones, so an off-by-one cannot turn a play click into a device switch.
pub const ACTION_CYCLE_OUTPUT: usize = 1;
pub const ACTION_CYCLE_INPUT: usize = 2;
pub const ACTION_OPEN_PANEL: usize = 3;
pub const ACTION_MUSIC_PREVIOUS: usize = 10;
pub const ACTION_MUSIC_PLAY_PAUSE: usize = 11;
pub const ACTION_MUSIC_NEXT: usize = 12;

/// Flow codes in [`WM_TASKBAR_SCROLL`]'s `wParam`.
pub const FLOW_OUTPUT: usize = 0;
pub const FLOW_INPUT: usize = 1;

/// Handover: `COPYDATASTRUCT::dwData` tagging a new owner's init payload ("ATH1"), sent to the
/// control window as `WM_COPYDATA`, and the results it returns. 0 means a TAP without the handler.
pub const HANDOVER_MAGIC: usize = 0x4154_4831;
pub const HANDOVER_ACCEPTED: isize = 1;
pub const HANDOVER_DECLINED: isize = 2;

/// Plane-15 private-use markers for the two earbud icons Segoe Fluent has no glyph for; the TAP
/// draws them as vectors. Outside the BMP PUA, which Segoe Fluent itself occupies.
pub const GLYPH_WIRELESS_EARBUDS: char = '\u{F0001}';
pub const GLYPH_ROUND_EARBUDS: char = '\u{F0002}';

/// The now-playing state file in `%TEMP%`, written by the app and re-read by the TAP.
pub const MUSIC_STATE_FILE: &str = "audio-tray-music.txt";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_ids_are_distinct() {
        let ids = [
            WM_TASKBAR_ACTION, WM_TAP_REVERT, WM_TASKBAR_RESTARTED, WM_TAP_RESTYLE, WM_TASKBAR_SCROLL,
            WM_MUSIC_PROGRESS, WM_TAP_WIRE_TRANSPORT, WM_TAP_REPIN,
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
