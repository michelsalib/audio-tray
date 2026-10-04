//! Wheel over the taskbar → [`WM_TASKBAR_SCROLL`] to the tray's message window.
//!
//! A `WH_MOUSE_LL` hook on a thread of its own, with a bare message loop: Windows silently
//! unhooks a low-level hook whose thread misses `LowLevelHooksTimeout`, and the tray thread does
//! slow COM work. The callback only classifies the point and posts.

use std::sync::Mutex;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetAncestor, GetMessageW, PostThreadMessageW, SetWindowsHookExW,
    UnhookWindowsHookEx, WindowFromPoint, GA_ROOT, MSG, MSLLHOOKSTRUCT, WH_MOUSE_LL, WM_MOUSEWHEEL,
    WM_QUIT,
};

use crate::audio::Flow;
use crate::taskbar::WM_TASKBAR_SCROLL;

/// The hook thread; stopping it (on drop) unhooks.
pub(super) struct WheelHook {
    thread: u32,
    join: Option<std::thread::JoinHandle<()>>,
}

impl WheelHook {
    /// Install the hook on a fresh thread. `None` if it could not be installed.
    pub(super) fn spawn() -> Option<Self> {
        let (ready, started) = std::sync::mpsc::channel();
        let join = std::thread::Builder::new()
            .name("wheel-hook".into())
            .spawn(move || {
                // Only the physical wheel reaches a low-level hook; precision-touchpad scroll
                // arrives from the TAP's own `PointerWheelChanged` handler instead.
                let hook = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), None, 0) };
                let _ = ready.send(hook.is_ok().then(|| unsafe { GetCurrentThreadId() }));
                let Ok(hook) = hook else { return };
                let mut msg = MSG::default();
                while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {}
                let _ = unsafe { UnhookWindowsHookEx(hook) };
            })
            .ok()?;
        match started.recv().ok().flatten() {
            Some(thread) => Some(Self { thread, join: Some(join) }),
            None => {
                let _ = join.join();
                None
            }
        }
    }
}

impl Drop for WheelHook {
    fn drop(&mut self) {
        let _ = unsafe { PostThreadMessageW(self.thread, WM_QUIT, WPARAM(0), LPARAM(0)) };
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// The tray icon's slot on the taskbar, as the tray last read it. Shared with the hook thread,
/// which splits it into the two buttons.
static ICON_RECT: Mutex<Option<RECT>> = Mutex::new(None);

/// Record the icon's slot; returns whether it moved.
pub(super) fn set_icon_rect(rect: Option<RECT>) -> bool {
    let mut held = ICON_RECT.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let moved = *held != rect;
    *held = rect;
    moved
}

fn icon_rect() -> Option<RECT> {
    *ICON_RECT.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 && wparam.0 as u32 == WM_MOUSEWHEEL {
        let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        let delta = (info.mouseData >> 16) as i16;
        if delta != 0 && unsafe { point_over_tray(info.pt) } {
            let flow = crate::taskbar::flow_code(flow_at(info.pt));
            // Swallowed only when the tray took it: so the shell does not also scroll, and so
            // XAML never sees it — which keeps the TAP's wheel handler from acting twice.
            if super::post(WM_TASKBAR_SCROLL, flow, delta as isize) {
                return LRESULT(1);
            }
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Which endpoint a wheel at this screen point belongs to: the strip's two buttons split the
/// icon slot at its midpoint (32 epx halves, see the TAP's `decorate`); anywhere else, and with
/// no strip, the output.
fn flow_at(pt: POINT) -> Flow {
    if !crate::taskbar::strip_is_up() {
        return Flow::Output;
    }
    match icon_rect() {
        Some(slot) if pt.x >= slot.left && pt.x < slot.right && pt.y >= slot.top && pt.y < slot.bottom => {
            if pt.x < (slot.left + slot.right) / 2 {
                Flow::Output
            } else {
                Flow::Input
            }
        }
        _ => Flow::Output,
    }
}

/// Is the screen point over the taskbar / notification area (incl. the Win11 tray overflow)?
unsafe fn point_over_tray(pt: POINT) -> bool {
    let hwnd = unsafe { WindowFromPoint(pt) };
    if hwnd.is_invalid() {
        return false;
    }
    let root: HWND = unsafe { GetAncestor(hwnd, GA_ROOT) };
    matches!(
        crate::win::class_name(root).as_str(),
        "Shell_TrayWnd"
            | "Shell_SecondaryTrayWnd"
            | "NotifyIconOverflowWindow"
            | "TopLevelWindowForOverflowXamlIsland"
            | "Xaml_WindowedPopupClass"
    )
}
