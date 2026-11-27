//! The init data (`key=value;` pairs, [`tap_proto::encode_init`]): what `SetSite` and a handover
//! hand the TAP, applied to its switches, and whether a handover's app may adopt this TAP.

use std::sync::atomic::Ordering;

use crate::{battery, clock, decorate, ipc, lifecycle, lock, log, music, reorder};
use tap_proto::{init_value, key};

/// Apply an init payload: what to decorate, what to draw, and whom to report to. Shared by
/// `SetSite` and [`crate::hand_over`]; touches no XAML, so it is safe in any message handler.
pub(crate) fn configure(data: &str) {
    log::set_verbose(init_value(data, key::DEBUG).as_deref() == Some("1"));
    *lock(&crate::TARGET_TOOLTIP) = init_value(data, key::TOOLTIP).unwrap_or_default();
    *lock(&crate::STRIP) = Some(decorate::StripState::parse(data));
    // `tile=<app name>` names the taskbar button for the music tile; absent disables that half.
    music::tile::set_host(init_value(data, key::TILE));
    if let Some(width) = init_value(data, key::STRIP).and_then(|w| w.parse().ok()) {
        music::layout::set_content_width(width);
    }
    if let Some(hwnd) = init_value(data, key::HWND).and_then(|h| h.parse::<isize>().ok()) {
        ipc::set_receiver(hwnd);
    }
    // Whoever asked for the strip is also who we put it away for.
    lifecycle::watch_owner(init_value(data, key::PID));
    // Absent (an older app) means on, as before the switch existed.
    reorder::set_enabled(init_value(data, key::REORDER).as_deref() != Some("0"));
    crate::BUTTONS.store(init_value(data, key::BUTTONS).as_deref() != Some("0"), Ordering::SeqCst);
    // Absent (an older app, or the calendar off) leaves the clock to the shell. The calendar (and
    // maybe the bell), or the bell alone, the clock's taps the shell's.
    let clock = init_value(data, key::CLOCK);
    clock::set_calendar(clock.as_deref() == Some(tap_proto::CLOCK_CALENDAR));
    clock::set_enabled(clock.as_deref().is_some_and(|c| c == tap_proto::CLOCK_CALENDAR || c == tap_proto::CLOCK_BELL_ONLY));
    // Absent (an older app, or the batteries off) leaves the battery icon to the shell.
    battery::set_enabled(init_value(data, key::BATTERY).as_deref() == Some("1"));
    // A new owner gets a fresh grace period and its own copy of the status.
    *lock(&crate::STOOD_UP) = Some(std::time::Instant::now());
    crate::LAST_STATUS.store(usize::MAX, Ordering::SeqCst);
}

/// Whether a new audio-tray may adopt this already-loaded TAP: the same app/TAP protocol (or, from
/// an app that predates it, the same app version), and the same DLL file it would have injected.
/// A routine update keeps the protocol, so it does not restart Explorer: the updated TAP loads
/// with the next Explorer. Anything else needs a fresh Explorer.
pub(crate) fn compatible(data: &str, own_path: Option<&str>) -> core::result::Result<(), String> {
    let protocol = init_value(data, key::PROTO).and_then(|p| p.parse::<u32>().ok());
    let version = init_value(data, key::VER);
    if protocol != Some(tap_proto::PROTOCOL) && version.as_deref() != Some(crate::APP_VERSION) {
        return Err(format!(
            "exe is v{} (protocol {}), TAP is v{} (protocol {})",
            version.unwrap_or_default(),
            protocol.map_or_else(|| "none".to_string(), |p| p.to_string()),
            crate::APP_VERSION,
            tap_proto::PROTOCOL
        ));
    }
    if let (Some(wanted), Some(own)) = (init_value(data, key::TAP), own_path) {
        if !wanted.eq_ignore_ascii_case(own) {
            return Err(format!("exe wants {wanted}, TAP is {own}"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::compatible;
    use crate::APP_VERSION;

    #[test]
    fn handover_needs_the_same_version_and_dll() {
        let own = r"C:\App\audio_tray_tap.dll";
        let ok = format!(r"ver={APP_VERSION};tap=c:\app\AUDIO_TRAY_TAP.dll;pid=1");
        assert!(compatible(&ok, Some(own)).is_ok());
        // No path to compare (older payload, or the module path unreadable) still matches on version.
        assert!(compatible(&format!("ver={APP_VERSION}"), Some(own)).is_ok());
        assert!(compatible(&ok, None).is_ok());
        assert!(compatible("ver=0.0.1;pid=1", Some(own)).is_err());
        // An exe that predates the handover sends no version at all.
        assert!(compatible("pid=1", Some(own)).is_err());
        let other = format!(r"ver={APP_VERSION};tap=D:\dev\audio_tray_tap.dll");
        assert!(compatible(&other, Some(own)).is_err());
    }

    /// An update keeps the protocol: another version is adopted without an Explorer restart, but
    /// still only from the same DLL path, and never across a protocol change.
    #[test]
    fn handover_across_versions_needs_the_same_protocol() {
        let own = r"C:\App\audio_tray_tap.dll";
        let proto = tap_proto::PROTOCOL;
        let newer = format!(r"ver=99.0.0;proto={proto};tap=C:\App\audio_tray_tap.dll");
        assert!(compatible(&newer, Some(own)).is_ok());
        let changed = format!(r"ver=99.0.0;proto={};tap=C:\App\audio_tray_tap.dll", proto + 1);
        assert!(compatible(&changed, Some(own)).is_err());
        let elsewhere = format!(r"ver=99.0.0;proto={proto};tap=D:\dev\audio_tray_tap.dll");
        assert!(compatible(&elsewhere, Some(own)).is_err());
    }
}
