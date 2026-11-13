//! Wheel over the taskbar → [`WM_TASKBAR_SCROLL`] to the tray's message window; a left click on the
//! taskbar clock → the calendar, while it replaces the shell's (the TAP says when the pointer is
//! over the clock, [`set_clock_hover`]). The click is swallowed whole, down and up: Explorer's
//! clock button opens its own flyout whatever its XAML handlers do with the press.
//!
//! A `WH_MOUSE_LL` hook on a thread of its own, with a bare message loop: Windows silently
//! unhooks a low-level hook whose thread misses `LowLevelHooksTimeout`, and the tray thread does
//! slow COM work. The callback only classifies the point and posts.

use std::sync::Mutex;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetAncestor, GetMessageW, PostThreadMessageW, SetWindowsHookExW,
    UnhookWindowsHookEx, WindowFromPoint, GA_ROOT, MSG, MSLLHOOKSTRUCT, WH_MOUSE_LL, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MOUSEWHEEL, WM_QUIT, WM_RBUTTONDOWN, WH_KEYBOARD_LL,
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
                // A precision touchpad's scroll reaches it too, as fine wheel steps (measured on
                // 26300); where the hook passes on, the TAP's `PointerWheelChanged` handler takes it.
                let hook = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), None, 0) };
                let _ = ready.send(hook.is_ok().then(|| unsafe { GetCurrentThreadId() }));
                let Ok(hook) = hook else { return };
                // Win+Alt+D, for the calendar; it passes everything else straight on.
                let keys = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), None, 0) }.ok();
                let mut msg = MSG::default();
                while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {}
                if let Some(keys) = keys {
                    let _ = unsafe { UnhookWindowsHookEx(keys) };
                }
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
    if code >= 0 && matches!(wparam.0 as u32, WM_LBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONDOWN) {
        let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        let message = wparam.0 as u32;
        if message != WM_RBUTTONDOWN && clock_click(message == WM_LBUTTONDOWN, info.pt) {
            return LRESULT(1);
        }
        if message != WM_LBUTTONUP && outside_calendar(info.pt) {
            // Passed on: the click still does what it does there.
            super::post(crate::calendar::WM_CALENDAR, crate::calendar::CLOSE, 0);
        }
    }
    if code >= 0 && wparam.0 as u32 == WM_MOUSEWHEEL {
        let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        let delta = (info.mouseData >> 16) as i16;
        match if delta != 0 { unsafe { wheel_target(info.pt) } } else { Target::Elsewhere } {
            Target::Ours => {
                let flow = crate::taskbar::flow_code(flow_at(info.pt));
                // Swallowed only when the tray took it: so the shell does not also scroll, and so
                // XAML never sees it — which keeps the TAP's wheel handler from acting twice.
                // A touchpad's scroll reaches this hook too, as wheel input: tagged by its steps.
                let source = if crate::win::touchpad_delta(delta.into()) { 0 } else { crate::taskbar::SCROLL_FROM_WHEEL };
                if super::post(WM_TASKBAR_SCROLL, flow | source, delta as isize) {
                    return LRESULT(1);
                }
            }
            // Maybe over the buttons after all: the slot is only re-read now and then, and the
            // icons move whenever another app's do. Have the tray look again for the next notch.
            Target::TaskbarOutsideSlot => {
                if !RECT_CHECK_PENDING.swap(true, std::sync::atomic::Ordering::Relaxed)
                    && !super::post(super::WM_ICON_RECT_CHECK, 0, 0)
                {
                    RECT_CHECK_PENDING.store(false, std::sync::atomic::Ordering::Relaxed);
                }
            }
            Target::Elsewhere => {}
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Which endpoint a wheel at this screen point belongs to: the strip's two buttons split the
/// icon slot at its midpoint (32 epx halves, see the TAP's `decorate`); with no strip, the output.
fn flow_at(pt: POINT) -> Flow {
    if !crate::taskbar::buttons_up() {
        return Flow::Output;
    }
    match icon_rect() {
        Some(slot) if pt.x >= (slot.left + slot.right) / 2 => Flow::Input,
        _ => Flow::Output,
    }
}

/// The pointer is over the taskbar clock, as the TAP last said.
static CLOCK_HOVER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// A press on the clock was swallowed: its release goes too.
static CLOCK_PRESSED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The TAP's word on whether the pointer is over the clock (it only says "over" while the calendar
/// replaces the shell's).
pub(super) fn set_clock_hover(over: bool) {
    CLOCK_HOVER.store(over, std::sync::atomic::Ordering::Relaxed);
}

/// Whether a left button event is a click on the clock to take: a press while over it (and over a
/// taskbar, so a stale word from the TAP cannot take clicks elsewhere), or the release of one.
fn clock_click(down: bool, pt: POINT) -> bool {
    use std::sync::atomic::Ordering::Relaxed;
    if !down {
        return CLOCK_PRESSED.swap(false, Relaxed);
    }
    if !CLOCK_HOVER.load(Relaxed) {
        return false;
    }
    let hwnd = unsafe { WindowFromPoint(pt) };
    let root: HWND = unsafe { GetAncestor(hwnd, GA_ROOT) };
    if !matches!(crate::win::class_name(root).as_str(), "Shell_TrayWnd" | "Shell_SecondaryTrayWnd") {
        return false;
    }
    if !super::post(crate::taskbar::WM_TASKBAR_ACTION, tap_proto::ACTION_OPEN_CALENDAR, 0) {
        return false;
    }
    CLOCK_PRESSED.store(true, Relaxed);
    true
}

/// Whether the calendar is open and `pt` is outside it: the click that closes it, as a menu's would.
/// The calendar closes on losing the foreground too, but Windows may never have given it that.
fn outside_calendar(pt: POINT) -> bool {
    let open = *crate::calendar::OPEN_RECT.lock().unwrap_or_else(|e| e.into_inner());
    open.is_some_and(|r| pt.x < r.left || pt.x >= r.right || pt.y < r.top || pt.y >= r.bottom)
}

/// A [`super::WM_ICON_RECT_CHECK`] is on its way: one is enough however fast the wheel turns.
static RECT_CHECK_PENDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The tray has re-read the slot.
pub(super) fn rect_checked() {
    RECT_CHECK_PENDING.store(false, std::sync::atomic::Ordering::Relaxed);
}

/// Where a wheel at a screen point lands, for the hook.
enum Target {
    /// Over our audio icons.
    Ours,
    /// Over the taskbar or its overflow, but not where the icon slot was last seen.
    TaskbarOutsideSlot,
    Elsewhere,
}

unsafe fn wheel_target(pt: POINT) -> Target {
    let hwnd = unsafe { WindowFromPoint(pt) };
    if hwnd.is_invalid() {
        return Target::Elsewhere;
    }
    let root: HWND = unsafe { GetAncestor(hwnd, GA_ROOT) };
    let class = crate::win::class_name(root);
    if in_slot(pt) && wheel_belongs_to_tray(&class, crate::taskbar::buttons_up()) {
        Target::Ours
    } else if is_taskbar(&class) {
        Target::TaskbarOutsideSlot
    } else {
        Target::Elsewhere
    }
}

/// The taskbars and the notification area's overflow.
fn is_taskbar(class: &str) -> bool {
    matches!(class, "Shell_TrayWnd" | "Shell_SecondaryTrayWnd" | "NotifyIconOverflowWindow" | "TopLevelWindowForOverflowXamlIsland")
}

/// Whether a wheel over our icon slot, in a window of `class`, is ours: the slot must be showing,
/// not covered by another app. A `Xaml_WindowedPopupClass` is any XAML popup in any app; it only
/// counts while our strip is up (its hover tooltip is one).
fn wheel_belongs_to_tray(class: &str, strip_up: bool) -> bool {
    is_taskbar(class) || (class == "Xaml_WindowedPopupClass" && strip_up)
}

fn in_slot(pt: POINT) -> bool {
    icon_rect().is_some_and(|slot| pt.x >= slot.left && pt.x < slot.right && pt.y >= slot.top && pt.y < slot.bottom)
}

/// The D of a Win+Alt+D taken for the calendar: its release goes too.
static SHORTCUT_HELD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Win+Alt+D (Windows' shortcut for the clock's calendar) opens ours, while it replaces Windows'.
unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    use std::sync::atomic::Ordering::Relaxed;
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LWIN, VK_MENU, VK_RWIN};
    use windows::Win32::UI::WindowsAndMessaging::{KBDLLHOOKSTRUCT, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP};
    if code >= 0 && crate::calendar::SHORTCUT.load(Relaxed) {
        let key = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        if key.vkCode == b'D' as u32 {
            let down = matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
            let up = matches!(wparam.0 as u32, WM_KEYUP | WM_SYSKEYUP);
            let held = |vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY| unsafe { GetAsyncKeyState(vk.0 as i32) } as u16 & 0x8000 != 0;
            if down && (held(VK_LWIN) || held(VK_RWIN)) && held(VK_MENU) {
                if !SHORTCUT_HELD.swap(true, Relaxed) {
                    super::post(crate::taskbar::WM_TASKBAR_ACTION, tap_proto::ACTION_OPEN_CALENDAR, 0);
                    // A key between the Windows key's press and release, so letting go of it does
                    // not open Start.
                    mask_windows_key();
                }
                return LRESULT(1);
            }
            if up && SHORTCUT_HELD.swap(false, Relaxed) {
                return LRESULT(1);
            }
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Press and release an unassigned key (0xE8), which the shell counts as "another key".
fn mask_windows_key() {
    use windows::Win32::UI::Input::KeyboardAndMouse::{SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VIRTUAL_KEY};
    let key = |up: bool| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(0xE8), dwFlags: if up { KEYEVENTF_KEYUP } else { Default::default() }, ..Default::default() } },
    };
    unsafe { SendInput(&[key(false), key(true)], std::mem::size_of::<INPUT>() as i32) };
}

#[cfg(test)]
mod tests {
    use super::wheel_belongs_to_tray;

    #[test]
    fn the_slot_counts_only_in_the_taskbar_or_our_popup() {
        assert!(wheel_belongs_to_tray("Shell_TrayWnd", true));
        assert!(wheel_belongs_to_tray("Shell_TrayWnd", false), "plain icon");
        assert!(wheel_belongs_to_tray("Xaml_WindowedPopupClass", true));
        assert!(!wheel_belongs_to_tray("Xaml_WindowedPopupClass", false), "no strip up");
        assert!(!wheel_belongs_to_tray("Chrome_WidgetWin_1", true), "another app over the slot");
    }
}
