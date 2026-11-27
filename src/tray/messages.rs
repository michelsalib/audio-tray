//! The receiver window's own messages and timers: their ids, which of them the queue takes, and
//! how their parameters are packed. (The TAP's are in `tap_proto`; every one shares the window.)

use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer, WM_APP, WM_TIMER};

use crate::audio::{mic, notify, sessions};
use crate::taskbar::{WM_MUSIC_PROGRESS, WM_TASKBAR_ACTION, WM_TASKBAR_RESTARTED, WM_TASKBAR_SCROLL};

/// Apply the scroll folded so far; self-posted once per batch ([`super::ScrollTotals`]).
pub(super) const WM_APPLY_SCROLL: u32 = WM_APP + 30;
/// A click on the plain icon, from tray-icon's handler: `wParam` 1 = left, 2 = right; `lParam` the
/// anchor (icon centre x, icon top), packed by [`pack_point`].
pub(super) const WM_TRAY_CLICK: u32 = WM_APP + 31;
/// A default endpoint's volume or mute changed, from anywhere (media keys, another app). Only a
/// mute change matters to the icon and the strip; see [`super::Tray::check_mutes`].
pub(super) const WM_ENDPOINT_VOLUME: u32 = WM_APP + 32;
/// Check that the icon survived an Explorer restart (`wParam`: attempt number); see
/// [`super::Tray::ensure_icon`].
pub(super) const WM_ICON_CHECK: u32 = WM_APP + 33;
/// From the wheel hook: a wheel over the taskbar missed the icon slot, which may have moved;
/// re-read it. (`WM_APP + 40` is the TAP's.)
pub(super) const WM_ICON_RECT_CHECK: u32 = WM_APP + 41;
/// From the mouse hook: a middle click on the strip (`wParam`: the flow code); see
/// [`super::clicks::on_middle_click`]. (`WM_APP + 42` is the calendar's.)
pub(super) const WM_STRIP_MIDDLE: u32 = WM_APP + 43;
/// From the mouse hook: a middle click on the bell; see [`super::clicks::on_bell_middle_click`].
pub(super) const WM_BELL_MIDDLE: u32 = WM_APP + 44;
/// A banner's word on an app listening (`crate::calendar::listening`): `wParam` 1, the microphone
/// muted (a click), 0, only the alert dismissed (a right click).
pub(crate) const WM_BANNER_LISTENING: u32 = WM_APP + 45;
/// The display changed (resolution, scale, work area): posted by [`super::wndproc`] so it is
/// handled with the queue; the banners move.
pub(super) const WM_DISPLAY_CHANGED: u32 = WM_APP + 46;
/// This PC's charge, its power source or the energy saver changed (`WM_POWERBROADCAST`, posted
/// by [`super::wndproc`]): Windows' battery icon and the batteries' card follow.
pub(super) const WM_POWER_CHANGED: u32 = WM_APP + 47;
/// Tell the battery icons again (the TAP reported in, or was reloaded).
pub(super) const WM_BATTERY_SYNC: u32 = WM_APP + 48;
/// [`WM_TRAY_CLICK`]'s `wParam` for a right click on Audio Tray's own battery icon: its menu.
pub(super) const TRAY_CLICK_BATTERY_MENU: usize = 3;

/// The messages [`super::handle`] takes, queued and handled in order: one list, which the routing
/// and the distinctness test both read (one handled but missing here was dropped by `DefWindowProc`).
pub(super) const QUEUED: [u32; 23] = [
    notify::WM_AUDIO_REFRESH,
    mic::WM_MIC_CHANGED,
    WM_TASKBAR_ACTION,
    WM_TASKBAR_RESTARTED,
    WM_TASKBAR_SCROLL,
    WM_MUSIC_PROGRESS,
    WM_APPLY_SCROLL,
    WM_TRAY_CLICK,
    WM_STRIP_MIDDLE,
    WM_BELL_MIDDLE,
    WM_BANNER_LISTENING,
    WM_DISPLAY_CHANGED,
    WM_POWER_CHANGED,
    WM_BATTERY_SYNC,
    WM_ENDPOINT_VOLUME,
    WM_ICON_CHECK,
    WM_ICON_RECT_CHECK,
    crate::hub::WM_HUB_EVENTS,
    sessions::WM_SESSIONS_CHANGED,
    crate::pill::WM_PILL_ACTION,
    crate::settings::WM_SETTINGS,
    crate::settings::WM_OPEN_SETTINGS,
    crate::calendar::WM_CALENDAR,
];

/// The receiver window's timer for the calendar's next moment ([`crate::calendar::schedule`]).
pub(super) const CALENDAR_TIMER: usize = 0x4341;
/// The receiver window's timer for drawing the closed panel ahead ([`crate::calendar::prewarm`]),
/// once a burst of notifications has settled.
pub(super) const PREWARM_TIMER: usize = 0x5057;
/// The receiver window's timer for telling Windows' battery icon again ([`super::battery`]).
pub(super) const BATTERY_RETRY_TIMER: usize = 0x4254;

/// Every timer on the receiver window, queued as [`QUEUED`] messages are: these, and the scroll
/// readout's (`crate::osd`, which arms its own).
const TIMERS: [usize; 4] = [crate::osd::TIMER_ID, CALENDAR_TIMER, PREWARM_TIMER, BATTERY_RETRY_TIMER];

/// Whether the receiver window's procedure queues this message for [`super::handle`].
pub(super) fn is_ours(msg: u32, wparam: usize) -> bool {
    QUEUED.contains(&msg) || (msg == WM_TIMER && TIMERS.contains(&wparam))
}

/// Arm (or re-arm) one of the receiver window's timers to fire in `ms`. Nothing before the window
/// exists.
pub(super) fn set_timer(id: usize, ms: u32) {
    if let Some(window) = crate::taskbar::receiver() {
        unsafe { SetTimer(Some(window), id, ms, None) };
    }
}

/// Disarm one of the receiver window's timers (they repeat until killed).
pub(super) fn kill_timer(id: usize) {
    if let Some(window) = crate::taskbar::receiver() {
        let _ = unsafe { KillTimer(Some(window), id) };
    }
}

/// Two signed 16-bit screen coordinates in one `LPARAM` (multi-monitor coordinates can be negative).
pub(super) fn pack_point(x: i32, y: i32) -> isize {
    (x as u16 as u32 | (y as u16 as u32) << 16) as isize
}

pub(super) fn unpack_point(packed: isize) -> (i32, i32) {
    ((packed & 0xFFFF) as u16 as i16 as i32, ((packed >> 16) & 0xFFFF) as u16 as i16 as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every message the receiver window handles, the TAP's included: a shared id would route one
    /// as the other (a status report swallowed scroll-to-volume before this test existed).
    #[test]
    fn receiver_message_ids_are_distinct() {
        // The queued ones, then those the TAP's receiver answers at once (in `taskbar`).
        let direct = [
            tap_proto::WM_TASKBAR_STATUS,
            tap_proto::WM_TASKBAR_CLOCK_HOVER,
            tap_proto::WM_TASKBAR_CLOCK_RECT,
            tap_proto::WM_TASKBAR_BATTERY_HOVER,
            tap_proto::WM_TAP_PANEL,
        ];
        let ids: Vec<u32> = QUEUED.iter().chain(&direct).copied().collect();
        for (i, a) in ids.iter().enumerate() {
            assert!(ids[i + 1..].iter().all(|b| a != b), "0x{a:x} is used twice");
        }
    }

    #[test]
    fn timer_ids_are_distinct() {
        for (i, a) in TIMERS.iter().enumerate() {
            assert!(TIMERS[i + 1..].iter().all(|b| a != b), "timer 0x{a:x} is used twice");
        }
    }

    #[test]
    fn points_survive_packing_including_negative_coordinates() {
        for (x, y) in [(0, 0), (1919, 1032), (-1280, 1400), (-1, -32768), (32767, 5)] {
            assert_eq!(unpack_point(pack_point(x, y)), (x, y));
        }
    }
}
