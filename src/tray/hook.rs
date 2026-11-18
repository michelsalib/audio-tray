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

/// The hooks' thread: the mouse's (the wheel over the strip, the clock's click, a click outside
/// the calendar) and, while the shortcut is on, the keyboard's. Stopping it (on drop) unhooks both.
pub(super) struct Hooks {
    thread: u32,
    join: Option<std::thread::JoinHandle<()>>,
}

impl Hooks {
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
                // Win+Alt+D and Win+N, for the calendar, only while its shortcut is on: a keyboard
                // hook routes every keystroke in the session through this thread.
                let mut keys = None;
                let want = |on: bool, keys: &mut Option<windows::Win32::UI::WindowsAndMessaging::HHOOK>| {
                    match (on, keys.is_some()) {
                        (true, false) => *keys = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), None, 0) }.ok(),
                        (false, true) => {
                            if let Some(k) = keys.take() {
                                let _ = unsafe { UnhookWindowsHookEx(k) };
                            }
                        }
                        _ => {}
                    }
                };
                want(crate::calendar::SHORTCUT.load(std::sync::atomic::Ordering::SeqCst), &mut keys);
                let mut msg = MSG::default();
                while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {
                    if msg.message == WM_WANT_KEYS {
                        want(msg.wParam.0 != 0, &mut keys);
                    }
                }
                want(false, &mut keys);
                let _ = unsafe { UnhookWindowsHookEx(hook) };
            })
            .ok()?;
        match started.recv().ok().flatten() {
            Some(thread) => {
                HOOK_THREAD.store(thread, std::sync::atomic::Ordering::SeqCst);
                Some(Self { thread, join: Some(join) })
            }
            None => {
                let _ = join.join();
                None
            }
        }
    }
}

/// Hook thread message: install (`wParam` 1) or remove (0) the keyboard hook.
const WM_WANT_KEYS: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 1;
/// The hook thread, for [`want_keys`] (0: none).
static HOOK_THREAD: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// The calendar's shortcut on or off: the keyboard hook follows (call after `set_shortcut`).
pub(super) fn want_keys(on: bool) {
    let thread = HOOK_THREAD.load(std::sync::atomic::Ordering::SeqCst);
    if thread != 0 {
        let _ = unsafe { PostThreadMessageW(thread, WM_WANT_KEYS, WPARAM(on as usize), LPARAM(0)) };
    }
}

impl Drop for Hooks {
    fn drop(&mut self) {
        HOOK_THREAD.store(0, std::sync::atomic::Ordering::SeqCst);
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
            // Passed on: the click still does what it does there. Logged by the tray thread, not
            // here: a hook that blocks (on a console write) past Windows' timeout is unhooked.
            super::post(crate::calendar::WM_CALENDAR, crate::calendar::CLOSE, ((info.pt.y as u16 as isize) << 16) | info.pt.x as u16 as isize);
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
    use std::sync::atomic::Ordering::Relaxed;
    if over {
        // Where it was over the clock: a click far from there is not on the clock, whatever a
        // stale word (the TAP unloaded before saying "left") claims.
        let mut pt = POINT::default();
        let _ = unsafe { windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut pt) };
        CLOCK_AT.store(pack(pt), Relaxed);
    }
    CLOCK_HOVER.store(over, Relaxed);
}

/// Whether clicks on the clock open the calendar (the calendar is on).
static CLOCK_TAKEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Where the pointer was when the TAP said it was over the clock (x, y packed).
static CLOCK_AT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// The clock's button is about this wide: a press further than this from where the pointer
/// entered it is elsewhere.
const CLOCK_REACH: i32 = 160;

fn pack(pt: POINT) -> u64 {
    ((pt.x as u32 as u64) << 32) | pt.y as u32 as u64
}

fn unpack(v: u64) -> POINT {
    POINT { x: (v >> 32) as u32 as i32, y: v as u32 as i32 }
}

/// The calendar on or off: off, the clock's clicks are the shell's whatever the TAP says.
pub(super) fn set_clock_taken(on: bool) {
    use std::sync::atomic::Ordering::Relaxed;
    CLOCK_TAKEN.store(on, Relaxed);
    if !on {
        CLOCK_HOVER.store(false, Relaxed);
    }
}

/// Whether a left button event is a click on the clock to take: a press while over it (and over a
/// taskbar, so a stale word from the TAP cannot take clicks elsewhere), or the release of one.
fn clock_click(down: bool, pt: POINT) -> bool {
    use std::sync::atomic::Ordering::Relaxed;
    if !down {
        return CLOCK_PRESSED.swap(false, Relaxed);
    }
    if !CLOCK_TAKEN.load(Relaxed) || !CLOCK_HOVER.load(Relaxed) {
        return false;
    }
    let at = unpack(CLOCK_AT.load(Relaxed));
    if (pt.x - at.x).abs() > CLOCK_REACH || (pt.y - at.y).abs() > CLOCK_REACH {
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

/// The D or N of a shortcut taken for the calendar: its release goes too.
static SHORTCUT_HELD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Win+Alt+D and Win+N (Windows' shortcuts for the clock's calendar and the notification centre)
/// open ours, while it replaces Windows'.
unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    use std::sync::atomic::Ordering::Relaxed;
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT};
    use windows::Win32::UI::WindowsAndMessaging::{KBDLLHOOKSTRUCT, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP};
    if code >= 0 && crate::calendar::SHORTCUT.load(Relaxed) {
        let key = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        // Win+Alt+D (the clock's calendar) and Win+N (the notification centre).
        if key.vkCode == b'D' as u32 || key.vkCode == b'N' as u32 {
            let down = matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
            let up = matches!(wparam.0 as u32, WM_KEYUP | WM_SYSKEYUP);
            let held = |vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY| unsafe { GetAsyncKeyState(vk.0 as i32) } as u16 & 0x8000 != 0;
            // Exactly those: Win+Ctrl+N or Win+Shift+N are other apps' shortcuts.
            let others = held(VK_CONTROL) || held(VK_SHIFT);
            let wanted = !others && if key.vkCode == b'D' as u32 { held(VK_MENU) } else { !held(VK_MENU) };
            if down && (held(VK_LWIN) || held(VK_RWIN)) && wanted {
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
    use super::{pack, unpack, wheel_belongs_to_tray};
    use windows::Win32::Foundation::POINT;

    #[test]
    fn points_survive_packing_on_any_monitor() {
        for (x, y) in [(0, 0), (1540, 1166), (-1920, -40), (i32::MAX, i32::MIN)] {
            let back = unpack(pack(POINT { x, y }));
            assert_eq!((back.x, back.y), (x, y));
        }
    }

    #[test]
    fn the_slot_counts_only_in_the_taskbar_or_our_popup() {
        assert!(wheel_belongs_to_tray("Shell_TrayWnd", true));
        assert!(wheel_belongs_to_tray("Shell_TrayWnd", false), "plain icon");
        assert!(wheel_belongs_to_tray("Xaml_WindowedPopupClass", true));
        assert!(!wheel_belongs_to_tray("Xaml_WindowedPopupClass", false), "no strip up");
        assert!(!wheel_belongs_to_tray("Chrome_WidgetWin_1", true), "another app over the slot");
    }
}
