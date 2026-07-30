//! Telling audio-tray what the user clicked.
//!
//! The TAP runs inside `explorer.exe`; the thing that owns the audio devices is
//! a separate process. The strip therefore does no work of its own — it reports
//! the gesture and audio-tray decides what it means.
//!
//! Transport is a posted window message to a hidden window that audio-tray
//! registers. `PostMessage` is used rather than `SendMessage` so a busy or wedged
//! audio-tray can never block Explorer's UI thread, which is where these handlers
//! run.

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

/// Posts `action` to audio-tray, if it is running.
///
/// Deliberately silent about a missing window beyond one log line: there is a
/// window between audio-tray exiting and the TAP noticing (see
/// `lifecycle::watch_owner`) in which the strip is still on screen with nobody
/// to answer it, and a user who has quit the app should not get errors from
/// their taskbar.
/// Finds audio-tray's receiver window by walking top-level windows.
///
/// `FindWindow` is the obvious tool and does not work here: with the receiver
/// created as a message-only window it cannot see it at all, and even as a
/// hidden top-level window `FindWindow`/`FindWindowEx` returned nothing while
/// `EnumWindows` listed it. Enumerating and comparing the class name is what
/// actually finds it, and it only runs on a click.
fn find_receiver() -> Option<HWND> {
    unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let mut class = [0u16; 64];
        let len = unsafe { GetClassNameW(hwnd, &mut class) };
        if len > 0 {
            let name = String::from_utf16_lossy(&class[..len as usize]);
            if name == RECEIVER_CLASS_NAME {
                unsafe { *(lparam.0 as *mut HWND) = hwnd };
                return BOOL(0); // found — stop enumerating
            }
        }
        BOOL(1)
    }

    let mut found = HWND(core::ptr::null_mut());
    let _ = unsafe { EnumWindows(Some(visit), LPARAM(&mut found as *mut HWND as isize)) };
    (!found.0.is_null()).then_some(found)
}

pub fn send(action: Action) {
    let Some(hwnd) = find_receiver() else {
        logf!("no audio-tray receiver window — dropping action");
        return;
    };
    let posted = unsafe {
        PostMessageW(
            Some(hwnd),
            WM_TASKBAR_ACTION,
            WPARAM(action.code()),
            LPARAM(0),
        )
    };
    if let Err(err) = posted {
        logf!("PostMessage to audio-tray failed: {err}");
    }
}
