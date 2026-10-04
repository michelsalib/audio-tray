//! Reports gestures on the strip to audio-tray, which decides what they mean.
//!
//! Posted (never sent) to audio-tray's hidden receiver window, so a busy or wedged app can never
//! block Explorer's UI thread.

use core::ffi::c_void;
use core::sync::atomic::{AtomicIsize, Ordering};

use crate::interact::Segment;
use crate::log::logf;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows_core::BOOL;
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, PostMessageW,
};

use tap_proto::RECEIVER_CLASS as RECEIVER_CLASS_NAME;

use tap_proto::WM_TASKBAR_ACTION;

use tap_proto::WM_TASKBAR_SCROLL;

/// What the user did on the strip.
#[derive(Clone, Copy)]
pub enum Action {
    /// Cycle to the next device for this endpoint.
    Cycle(Segment),
    /// Open the full panel (right click anywhere on the strip).
    OpenPanel,
}

impl Action {
    /// Wire code (explicit, shared via `tap_proto`, never derived from enum order).
    fn code(self) -> usize {
        match self {
            Self::Cycle(Segment::Output) => tap_proto::ACTION_CYCLE_OUTPUT,
            Self::Cycle(Segment::Input) => tap_proto::ACTION_CYCLE_INPUT,
            Self::OpenPanel => tap_proto::ACTION_OPEN_PANEL,
        }
    }
}

/// Finds audio-tray's receiver window by walking top-level windows (`FindWindow` does not find
/// it from here; see FINDINGS.md, "Talking to audio-tray").
fn find_receiver() -> Option<HWND> {
    unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // A panic must not unwind into user32 (and through it, Explorer): treat it as "stop".
        std::panic::catch_unwind(|| {
            if unsafe { class_of(hwnd) } == RECEIVER_CLASS_NAME {
                unsafe { *(lparam.0 as *mut HWND) = hwnd };
                return BOOL(0); // found — stop enumerating
            }
            BOOL(1)
        })
        .unwrap_or(BOOL(0))
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

/// The receiver window, cached so scroll bursts do not each walk every top-level window. May go
/// stale: re-checked before use and dropped when a post fails.
static RECEIVER: AtomicIsize = AtomicIsize::new(0);

/// audio-tray's receiver window: the cached one if its class still matches (handles get
/// recycled), else a fresh scan.
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

/// Adopt the receiver window named in the init data (`hwnd=`), so a handover takes effect at once
/// instead of waiting for the old window to die. Ignored unless it really is a receiver.
pub fn set_receiver(hwnd: isize) {
    if unsafe { class_of(HWND(hwnd as *mut c_void)) } == RECEIVER_CLASS_NAME {
        RECEIVER.store(hwnd, Ordering::Relaxed);
    }
}

/// Posts one message to audio-tray if it is running; otherwise only logs (the strip can outlive
/// the app briefly, until `lifecycle::watch_owner` reverts it).
fn post(message: u32, wparam: usize, lparam: isize) {
    let Some(hwnd) = receiver() else {
        logf!("no audio-tray receiver window — dropping the gesture");
        return;
    };
    let posted = unsafe { PostMessageW(Some(hwnd), message, WPARAM(wparam), LPARAM(lparam)) };
    if let Err(err) = posted {
        RECEIVER.store(0, Ordering::Relaxed);
        logf!("PostMessage to audio-tray failed: {err}");
    }
}

/// Reports a click: which segment, and which button — nothing about what it should mean.
pub fn send(action: Action) {
    post(WM_TASKBAR_ACTION, action.code(), 0);
}

/// Reports a scroll over one segment: `delta` in signed `WHEEL_DELTA` units, as the pointer
/// reported it; audio-tray scales and coalesces.
pub fn send_scroll(segment: Segment, delta: i32) {
    let flow = match segment {
        Segment::Output => tap_proto::FLOW_OUTPUT,
        Segment::Input => tap_proto::FLOW_INPUT,
    };
    post(WM_TASKBAR_SCROLL, flow, delta as isize);
}

/// Post a raw action code, for the music tile (its codes are unrelated to [`Action`]'s, so they
/// are kept out of that enum).
pub fn send_code(code: usize) {
    post(WM_TASKBAR_ACTION, code, 0);
}
