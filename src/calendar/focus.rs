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

/// The latest end Windows takes: the last second of 9999 (a manual session runs "until switched
/// off", `i64::MAX`, which would overflow).
const LATEST: i64 = 253_402_300_799;

fn date_time(secs: i64) -> DateTime {
    DateTime { UniversalTime: super::time::to_filetime(secs.clamp(0, LATEST)) }
}

/// A file that says Do not disturb is Audio Tray's: it outlives a crash, so the next start can
/// end what this one left on.
fn marker() -> Option<std::path::PathBuf> {
    Some(std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("audio-tray").join("dnd-ours"))
}

fn mark(ours: bool) {
    let Some(path) = marker() else { return };
    if ours {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&path, b"");
    } else {
        let _ = std::fs::remove_file(&path);
    }
}

/// Follow whether Windows keeps notifications quiet, from now on (once): its focus state, which
/// its automatic rules switch too (a full-screen app, a duplicated display, a game), and Do not
/// disturb switched by hand. Each change has the tray look again, so the panel's moon and the
/// bell's cross always say the same as Windows.
pub fn watch() {
    use std::sync::atomic::{AtomicBool, Ordering};
    static WATCHING: AtomicBool = AtomicBool::new(false);
    if WATCHING.swap(true, Ordering::SeqCst) {
        return;
    }
    dnd::watch();
    let Some(manager) = manager() else { return };
    let handler = windows::Foundation::TypedEventHandler::<FocusSessionManager, windows_core::IInspectable>::new(|_, _| {
        crate::tray::post(super::WM_CALENDAR, super::QUIET, 0);
        Ok(())
    });
    match manager.IsFocusActiveChanged(&handler) {
        // The manager is kept for the process's life, and the subscription with it.
        Ok(_) => std::mem::forget(manager),
        Err(e) => eprintln!("calendar: could not follow Windows' focus ({e}); it is read when the panel or the clock next look"),
    }
}

/// At start: Do not disturb a previous run set and never ended (it quit or crashed) is ended.
pub fn recover() {
    if marker().is_some_and(|p| p.exists()) {
        if dnd::on() {
            println!("calendar: ending the Do not disturb a previous run left on");
            dnd::set(false);
        }
        mark(false);
    }
}

/// At exit: end what Audio Tray started on its own (a timer, a meeting's quiet). Do not disturb
/// switched on by hand stays on: an update or a restart must not turn it off behind the user's back.
pub fn release() {
    if let Some((why, ..)) = OURS.with(Cell::get).filter(|(why, ..)| *why != Why::Manual) {
        stop(why);
    }
}

fn session_active() -> bool {
    manager().and_then(|m| m.IsFocusActive().ok()).unwrap_or(false)
}

/// Whether Windows keeps notifications quiet: a focus session, or Do not disturb (anyone's).
pub fn active() -> bool {
    session_active() || dnd::on()
}

/// What Audio Tray holds, if it still runs. Do not disturb it set past its time is ended here.
pub fn still_ours() -> Option<(Why, i64)> {
    let (why, until, how) = OURS.with(Cell::get)?;
    let running = match how {
        How::Session => session_active(),
        How::Dnd => dnd::on(),
    };
    if !running || super::time::now() >= until {
        if running && how == How::Dnd {
            dnd::set(false);
        }
        if how == How::Dnd {
            mark(false);
        }
        OURS.with(|o| o.set(None));
        return None;
    }
    Some((why, until))
}

/// Keep things quiet until `until` (seconds since the epoch), unless they already are and not by
/// Audio Tray. `false` when Windows refused both ways.
pub fn start(why: Why, until: i64) -> bool {
    let held = still_ours();
    if active() && held.is_none() {
        // The user's own session or Do not disturb already keeps things quiet.
        return false;
    }
    if let Some((_, held_until, how)) = held.and(OURS.with(Cell::get)) {
        if how == How::Dnd || held_until == until {
            // Already quiet by Audio Tray, by Do not disturb (which Audio Tray ends itself): the new
            // reason and end take over.
            OURS.with(|o| o.set(Some((why, until, how))));
            if why == Why::Manual {
                // Kept past a restart (see [`release`]): no longer one for the next start to end.
                mark(false);
            }
            return true;
        }
        // A Windows session ends at its own time: one for the new end replaces it, or it would end
        // the new reason early (a timer started during a meeting's quiet).
        if let Some(manager) = manager() {
            let _ = manager.DeactivateFocus();
        }
        OURS.with(|o| o.set(None));
    }
    // By hand, "until switched off": Do not disturb itself, not a Windows focus session (which also
    // hides the taskbar's badges and shows a timer running to the year 9999).
    let session = if why == Why::Manual { None } else { manager().map(|m| m.TryStartFocusSession2(date_time(until))) };
    let how = match session {
        Some(Ok(_)) => How::Session,
        refused => {
            if let Some(Err(e)) = refused {
                println!("calendar: no focus session here ({e}); setting Do not disturb instead");
            }
            if !dnd::set(true) {
                return false;
            }
            // Audio Tray's own, for the next start to end after a crash; one switched on by hand
            // stays the user's.
            mark(why != Why::Manual);
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
            mark(false);
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
        if dnd::on() && !dnd::set(false) {
            // Windows refused: still on.
            return true;
        }
        mark(false);
        OURS.with(|o| o.set(None));
        return false;
    }
    start(Why::Manual, i64::MAX)
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_session_until_switched_off_ends_in_9999_not_in_an_overflow() {
        assert_eq!(super::date_time(i64::MAX).UniversalTime, crate::calendar::time::to_filetime(super::LATEST));
        assert_eq!(super::date_time(0).UniversalTime, 116_444_736_000_000_000);
    }
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

    #[link(name = "ntdll", kind = "raw-dylib")]
    unsafe extern "system" {
        fn RtlSubscribeWnfStateChangeNotification(
            subscription: *mut *mut c_void,
            name: u64,
            stamp: u32,
            callback: unsafe extern "system" fn(u64, u32, *const c_void, *const c_void, *const c_void, u32) -> i32,
            context: *const c_void,
            type_id: *const c_void,
            serialization_group: u32,
            unknown: u32,
        ) -> i32;
    }

    /// Called on a pool thread whenever Do not disturb changes, by anyone (Windows' Quick Settings
    /// too): the tray looks again (the bell's cross, the panel's moon).
    unsafe extern "system" fn changed(_name: u64, _stamp: u32, _type: *const c_void, _context: *const c_void, _buffer: *const c_void, _length: u32) -> i32 {
        crate::tray::post(super::super::WM_CALENDAR, super::super::QUIET, 0);
        0
    }

    /// Follow Do not disturb from now on (once, through [`super::watch`]; the subscription lasts as
    /// long as the process).
    pub fn watch() {
        use std::sync::atomic::{AtomicBool, Ordering};
        static WATCHING: AtomicBool = AtomicBool::new(false);
        if WATCHING.swap(true, Ordering::SeqCst) {
            return;
        }
        // From the current stamp, so only changes call back.
        let mut stamp = 0u32;
        let mut value = 0u32;
        let mut size = 4u32;
        let _ = unsafe { NtQueryWnfStateData(&STATE, core::ptr::null(), core::ptr::null(), &mut stamp, (&mut value as *mut u32).cast(), &mut size) };
        let mut subscription = core::ptr::null_mut();
        let status = unsafe { RtlSubscribeWnfStateChangeNotification(&mut subscription, STATE, stamp, changed, core::ptr::null(), core::ptr::null(), 0, 0) };
        if status < 0 {
            eprintln!("calendar: could not follow Do not disturb (0x{status:08x}); it is read when the panel or the clock next look");
        }
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
