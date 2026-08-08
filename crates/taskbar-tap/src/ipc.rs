//! Telling audio-tray what the user did on the strip.
//!
//! The TAP runs inside `explorer.exe`; the thing that owns the audio devices is
//! a separate process. The strip therefore does no work of its own — it reports
//! the gesture and audio-tray decides what it means.
//!
//! Transport is a posted window message to a hidden window that audio-tray
//! registers. `PostMessage` is used rather than `SendMessage` so a busy or wedged
//! audio-tray can never block Explorer's UI thread, which is where these handlers
//! run.

use core::ffi::c_void;
use core::sync::atomic::{AtomicIsize, Ordering};

use crate::interact::Segment;
use crate::log::logf;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows_core::BOOL;
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, PostMessageW, WM_APP,
};

/// Window class of audio-tray's receiver. Must match `RECEIVER_CLASS` on the
/// audio-tray side.
const RECEIVER_CLASS_NAME: &str = "AudioTrayTaskbarIpc";

/// The message audio-tray listens for. `wParam` carries the [`Action`] code.
const WM_TASKBAR_ACTION: u32 = WM_APP + 20;

/// A scroll over one of the segments: `wParam` is the direction (0 = output, 1 = input) and
/// `lParam` the signed wheel delta, in `WHEEL_DELTA` units.
///
/// Its own message rather than another [`Action`] code, because it carries a delta and
/// because audio-tray coalesces these — a touchpad gesture is tens of them, and draining
/// them from its queue must not swallow queued clicks. Must match `WM_TASKBAR_SCROLL` on the
/// audio-tray side.
const WM_TASKBAR_SCROLL: u32 = WM_APP + 24;

/// What the user did on the strip.
#[derive(Clone, Copy)]
pub enum Action {
    /// Cycle to the next device for this endpoint.
    Cycle(Segment),
    /// Open the full panel (right click anywhere on the strip).
    OpenPanel,
}

impl Action {
    /// Wire code. Kept explicit rather than derived from enum order so the TAP
    /// and audio-tray can be rebuilt independently without silently disagreeing.
    fn code(self) -> usize {
        match self {
            Self::Cycle(Segment::Output) => 1,
            Self::Cycle(Segment::Input) => 2,
            Self::OpenPanel => 3,
        }
    }
}

/// Finds audio-tray's receiver window by walking top-level windows.
///
/// `FindWindow` is the obvious tool and does not work here: with the receiver
/// created as a message-only window it cannot see it at all, and even as a
/// hidden top-level window `FindWindow`/`FindWindowEx` returned nothing while
/// `EnumWindows` listed it. Enumerating and comparing the class name is what
/// actually finds it.
fn find_receiver() -> Option<HWND> {
    unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
        if unsafe { class_of(hwnd) } == RECEIVER_CLASS_NAME {
            unsafe { *(lparam.0 as *mut HWND) = hwnd };
            return BOOL(0); // found — stop enumerating
        }
        BOOL(1)
    }

    let mut found = HWND(core::ptr::null_mut());
    let _ = unsafe { EnumWindows(Some(visit), LPARAM(&mut found as *mut HWND as isize)) };
    (!found.0.is_null()).then_some(found)
}

/// # Safety
/// `hwnd` may be any value; a dead handle simply reads as an empty class.
unsafe fn class_of(hwnd: HWND) -> String {
    let mut class = [0u16; 64];
    let len = GetClassNameW(hwnd, &mut class);
    if len <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&class[..len as usize])
}

/// The receiver window, remembered between events.
///
/// A full `EnumWindows` per event is fine for a click and not for a scroll: a touchpad
/// gesture posts tens of them, all from Explorer's UI thread, and walking every top-level
/// window in the session that often is real work in the wrong place. The handle can go stale
/// (audio-tray quitting, or restarting itself after an update), so it is checked before use
/// and dropped whenever a post fails.
static RECEIVER: AtomicIsize = AtomicIsize::new(0);

/// audio-tray's receiver window: the remembered one if it is still ours, else a fresh scan.
///
/// One `GetClassNameW` is what makes the cache safe to trust — a bare "is it non-null" test
/// would eventually post our messages into whatever window inherited a recycled handle.
fn receiver() -> Option<HWND> {
    let cached = RECEIVER.load(Ordering::Relaxed);
    if cached != 0 {
        let hwnd = HWND(cached as *mut c_void);
        if unsafe { class_of(hwnd) } == RECEIVER_CLASS_NAME {
            return Some(hwnd);
        }
    }
    let found = find_receiver()?;
    RECEIVER.store(found.0 as isize, Ordering::Relaxed);
    Some(found)
}

/// Posts one message to audio-tray, if it is running.
///
/// Deliberately silent about a missing window beyond one log line: there is a
/// window between audio-tray exiting and the TAP noticing (see
/// `lifecycle::watch_owner`) in which the strip is still on screen with nobody
/// to answer it, and a user who has quit the app should not get errors from
/// their taskbar.
fn post(message: u32, wparam: usize, lparam: isize) {
    let Some(hwnd) = receiver() else {
        logf!("no audio-tray receiver window — dropping the gesture");
        return;
    };
    let posted = unsafe { PostMessageW(Some(hwnd), message, WPARAM(wparam), LPARAM(lparam)) };
    if let Err(err) = posted {
        // Whatever we had is no use — forget it so the next gesture looks again.
        RECEIVER.store(0, Ordering::Relaxed);
        logf!("PostMessage to audio-tray failed: {err}");
    }
}

/// Reports a click: which segment, and which button — nothing about what it should mean.
pub fn send(action: Action) {
    post(WM_TASKBAR_ACTION, action.code(), 0);
}

/// Reports a scroll over one segment: `delta` in `WHEEL_DELTA` units, signed, exactly as the
/// pointer reported it. What it *means* — how much volume that is, and whether to coalesce it
/// with the ones behind it — is audio-tray's to decide, like every other gesture here.
pub fn send_scroll(segment: Segment, delta: i32) {
    let flow = match segment {
        Segment::Output => 0,
        Segment::Input => 1,
    };
    post(WM_TASKBAR_SCROLL, flow, delta as isize);
}

/// Post a raw action code.
///
/// For the music tile, whose codes come from `music::tick::Segment` rather than from [`Action`]. Kept
/// as a separate entry point rather than folding those into `Action`: the two halves of this TAP send
/// to the same window but mean unrelated things, and one enum spanning both would invite a `match`
/// that silently treats a media click as an audio one.
pub fn send_code(code: usize) {
    post(WM_TASKBAR_ACTION, code, 0);
}
