//! Do not disturb: through Windows' own focus sessions (`Windows.UI.Shell.FocusSessionManager`)
//! where Windows allows them, else by switching Windows' Do not disturb itself ([`dnd`]).
//!
//! Starting a focus session is a limited-access feature: on some machines Windows refuses it to
//! an unpackaged app (`com.microsoft.windows.focussessionmanager.1 is not available`). Do not
//! disturb is then set through the shell's own state (WNF), which is what its Quick Settings
//! toggle writes, and Audio Tray ends it itself when its time is up.
//!
//! Audio Tray ends only what it started; Do not disturb the user turned on stays theirs.

use std::cell::Cell;

use windows::Foundation::DateTime;
use windows::UI::Shell::FocusSessionManager;

/// Why Audio Tray keeps things quiet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// The focus timer, until its end.
    Timer,
    /// A calendar event, or a call.
    Quiet,
    /// Switched on by hand, until switched off.
    Manual,
}

/// How it was done, so it is undone the same way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum How {
    Session,
    Dnd,
}

thread_local! {
    /// What Audio Tray started: why, until when (seconds since the epoch), and how.
    static OURS: Cell<Option<(Why, i64, How)>> = const { Cell::new(None) };
}

fn manager() -> Option<FocusSessionManager> {
    FocusSessionManager::IsSupported().ok().filter(|s| *s)?;
    FocusSessionManager::GetDefault().ok()
}

fn date_time(secs: i64) -> DateTime {
    DateTime { UniversalTime: (secs + 11_644_473_600) * 10_000_000 }
}

fn session_active() -> bool {
    manager().and_then(|m| m.IsFocusActive().ok()).unwrap_or(false)
}

/// Whether Windows keeps notifications quiet: a focus session, or Do not disturb (anyone's).
pub fn active() -> bool {
    session_active() || dnd::on()
}

/// What Audio Tray holds, if it still runs. Do not disturb it set past its time is ended here.
pub fn ours() -> Option<(Why, i64)> {
    let (why, until, how) = OURS.with(Cell::get)?;
    let running = match how {
        How::Session => session_active(),
        How::Dnd => dnd::on(),
    };
    if !running || super::time::now() >= until {
        if running && how == How::Dnd {
            dnd::set(false);
        }
        OURS.with(|o| o.set(None));
        return None;
    }
    Some((why, until))
}

/// Keep things quiet until `until` (seconds since the epoch), unless they already are and not by
/// Audio Tray. `false` when Windows refused both ways.
pub fn start(why: Why, until: i64) -> bool {
    if active() && ours().is_none() {
        // The user's own session or Do not disturb already keeps things quiet.
        return false;
    }
    let session = manager().map(|m| m.TryStartFocusSession2(date_time(until)));
    let how = match session {
        Some(Ok(_)) => How::Session,
        refused => {
            if let Some(Err(e)) = refused {
                println!("calendar: no focus session here ({e}); setting Do not disturb instead");
            }
            if !dnd::set(true) {
                return false;
            }
            How::Dnd
        }
    };
    OURS.with(|o| o.set(Some((why, until, how))));
    true
}

/// End what Audio Tray started for `why` (another reason's, or the user's, stays).
pub fn stop(why: Why) {
    let Some((held, _, how)) = OURS.with(Cell::get) else { return };
    if held != why {
        return;
    }
    match how {
        How::Session => {
            if let Some(manager) = manager() {
                let _ = manager.DeactivateFocus();
            }
        }
        How::Dnd => {
            dnd::set(false);
        }
    }
    OURS.with(|o| o.set(None));
}

/// Do not disturb, switched by hand from the notification centre: off if Windows keeps things
/// quiet (whoever started it), else on until switched off.
pub fn toggle() -> bool {
    if active() {
        if session_active() {
            if let Some(manager) = manager() {
                let _ = manager.DeactivateFocus();
            }
        }
        if dnd::on() {
            dnd::set(false);
        }
        OURS.with(|o| o.set(None));
        return false;
    }
    start(Why::Manual, i64::MAX)
}

/// Windows' Do not disturb, as its Quick Settings toggle sets it: the shell's
/// `WNF_SHEL_QUIETHOURS_ACTIVE_PROFILE_CHANGED` state (0 off, 1 priority only, 2 alarms only).
pub mod dnd {
    use core::ffi::c_void;

    const STATE: u64 = 0x0D83_063E_A3BF_1C75;
    /// Priority only: what Do not disturb is on Windows 11 (priority notifications still come).
    const ON: u32 = 1;

    #[link(name = "ntdll", kind = "raw-dylib")]
    unsafe extern "system" {
        fn NtQueryWnfStateData(name: *const u64, type_id: *const c_void, scope: *const c_void, stamp: *mut u32, buffer: *mut c_void, size: *mut u32) -> i32;
        fn NtUpdateWnfStateData(name: *const u64, buffer: *const c_void, length: u32, type_id: *const c_void, scope: *const c_void, matching: u32, check: u32) -> i32;
    }

    fn profile() -> Option<u32> {
        let mut value = 0u32;
        let mut size = 4u32;
        let mut stamp = 0u32;
        let status = unsafe { NtQueryWnfStateData(&STATE, core::ptr::null(), core::ptr::null(), &mut stamp, (&mut value as *mut u32).cast(), &mut size) };
        (status >= 0 && size >= 4).then_some(value)
    }

    /// Whether Do not disturb (any profile) is on.
    pub fn on() -> bool {
        profile().is_some_and(|p| p != 0)
    }

    /// Switch it; `false` when Windows refused.
    pub fn set(on: bool) -> bool {
        let value: u32 = if on { ON } else { 0 };
        let status = unsafe { NtUpdateWnfStateData(&STATE, (&value as *const u32).cast(), 4, core::ptr::null(), core::ptr::null(), 0, 0) };
        if status < 0 {
            eprintln!("calendar: Windows would not switch Do not disturb (0x{status:08x})");
        }
        status >= 0
    }
}
