//! Do not disturb, through Windows' own focus sessions (`Windows.UI.Shell.FocusSessionManager`,
//! allowed for unpackaged apps): Windows silences notifications for as long as one runs, as when
//! a focus session is started from its own calendar.
//!
//! Audio Tray ends only the sessions it started; one the user started stays theirs.

use std::cell::Cell;

use windows::Foundation::DateTime;
use windows::UI::Shell::FocusSessionManager;

/// Why Audio Tray holds a focus session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// The focus timer, until its end.
    Timer,
    /// A calendar event, or a call.
    Quiet,
    /// Switched on by hand, until switched off.
    Manual,
}

thread_local! {
    /// The session Audio Tray started, why, and until when (seconds since the epoch).
    static OURS: Cell<Option<(Why, i64)>> = const { Cell::new(None) };
}

fn manager() -> Option<FocusSessionManager> {
    FocusSessionManager::IsSupported().ok().filter(|s| *s)?;
    FocusSessionManager::GetDefault().ok()
}

fn date_time(secs: i64) -> DateTime {
    DateTime { UniversalTime: (secs + 11_644_473_600) * 10_000_000 }
}

/// Whether Windows is in a focus session (anyone's).
pub fn active() -> bool {
    manager().and_then(|m| m.IsFocusActive().ok()).unwrap_or(false)
}

/// The session Audio Tray holds, if it still runs.
pub fn ours() -> Option<(Why, i64)> {
    let held = OURS.with(Cell::get)?;
    if !active() || super::time::now() >= held.1 {
        OURS.with(|o| o.set(None));
        return None;
    }
    Some(held)
}

/// Start a session until `until` (seconds since the epoch), unless one already runs that is not
/// ours. `false` when Windows refused.
pub fn start(why: Why, until: i64) -> bool {
    let Some(manager) = manager() else { return false };
    if manager.IsFocusActive().unwrap_or(false) && ours().is_none() {
        // The user's own session already keeps things quiet.
        return false;
    }
    match manager.TryStartFocusSession2(date_time(until)) {
        Ok(_) => {
            OURS.with(|o| o.set(Some((why, until))));
            true
        }
        Err(e) => {
            eprintln!("calendar: Windows would not start a focus session ({e})");
            false
        }
    }
}

/// End the session Audio Tray started for `why` (another reason's, or the user's, stays).
pub fn stop(why: Why) {
    if ours().is_some_and(|(w, _)| w == why) {
        if let Some(manager) = manager() {
            let _ = manager.DeactivateFocus();
        }
        OURS.with(|o| o.set(None));
    }
}

/// Do not disturb, switched by hand from the notification centre: off if Windows is in any focus
/// session (whoever started it), else on until switched off.
pub fn toggle() -> bool {
    let Some(manager) = manager() else { return false };
    if manager.IsFocusActive().unwrap_or(false) {
        let _ = manager.DeactivateFocus();
        OURS.with(|o| o.set(None));
        return false;
    }
    match manager.TryStartFocusSession() {
        Ok(_) => {
            OURS.with(|o| o.set(Some((Why::Manual, i64::MAX))));
            true
        }
        Err(e) => {
            eprintln!("calendar: Windows would not start a focus session ({e})");
            false
        }
    }
}
