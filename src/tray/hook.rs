//! Wheel over the taskbar → [`WM_TASKBAR_SCROLL`] to the tray's message window; a left click on the
//! taskbar clock → the calendar, and on the bell beside it → the notifications, while they replace
//! the shell's (the TAP says when the pointer is over either, [`set_clock_hover`]). The click is
//! swallowed whole, down and up: Explorer's button opens its own flyout whatever its XAML handlers
//! do with the press.
//!
//! A `WH_MOUSE_LL` hook on a thread of its own, with a bare message loop: Windows silently
//! unhooks a low-level hook whose thread misses `LowLevelHooksTimeout`, and the tray thread does
//! slow COM work. The callback only classifies the point and posts.

use std::sync::Mutex;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetAncestor, GetMessageW, PostThreadMessageW, SetWindowsHookExW,
    UnhookWindowsHookEx, WindowFromPoint, GA_ROOT, MSG, MSLLHOOKSTRUCT, WH_MOUSE_LL, WM_LBUTTONDOWN, WM_MBUTTONDOWN, WM_MBUTTONUP,
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
                // Win+Ctrl+V, Win+Alt+D and Win+N, only while a shortcut is on: a keyboard hook
                // routes every keystroke in the session through this thread.
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
                want(crate::calendar::SHORTCUT.load(std::sync::atomic::Ordering::SeqCst) || AUDIO_SHORTCUT.load(std::sync::atomic::Ordering::SeqCst), &mut keys);
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

/// Any shortcut on, or none: the keyboard hook follows (call after `set_shortcut` and
/// [`AUDIO_SHORTCUT`]).
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
    if code >= 0 && matches!(wparam.0 as u32, WM_MBUTTONDOWN | WM_MBUTTONUP) {
        let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        if middle_click(wparam.0 as u32 == WM_MBUTTONDOWN, info.pt) {
            return LRESULT(1);
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

/// A middle press on the strip was taken: its release goes too.
static MIDDLE_PRESSED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether a middle button event is the strip's to take: a press on our icon slot (in a taskbar,
/// not under another app's window), posted to the tray as [`super::WM_STRIP_MIDDLE`] with the
/// half it landed on, or the release of one. Swallowed, so the shell does nothing with it.
fn middle_click(down: bool, pt: POINT) -> bool {
    use std::sync::atomic::Ordering::Relaxed;
    if !down {
        return MIDDLE_PRESSED.swap(false, Relaxed);
    }
    let hwnd = unsafe { WindowFromPoint(pt) };
    let root: HWND = unsafe { GetAncestor(hwnd, GA_ROOT) };
    let class = crate::win::class_name(root);
    // The bell's (Do not disturb, or clear all, as `[calendar] bell_middle_click` says).
    let posted = if on_clock_part(pt) == Some(true) && BELL_MIDDLE_TAKEN.load(Relaxed) && matches!(class.as_str(), "Shell_TrayWnd" | "Shell_SecondaryTrayWnd") {
        super::post(super::WM_BELL_MIDDLE, 0, 0)
    } else if in_slot(pt) && wheel_belongs_to_tray(&class, crate::taskbar::buttons_up()) {
        super::post(super::WM_STRIP_MIDDLE, crate::taskbar::flow_code(flow_at(pt)), 0)
    } else {
        false
    };
    if posted {
        MIDDLE_PRESSED.store(true, Relaxed);
    }
    posted
}

/// Whether a middle click on the bell is Audio Tray's (its notifications are on, and the click is
/// set to do something).
static BELL_MIDDLE_TAKEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(super) fn set_bell_middle_taken(on: bool) {
    BELL_MIDDLE_TAKEN.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// The pointer is over the taskbar clock's button, as the TAP last said.
static CLOCK_HOVER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// ... and over the bell in it.
static BELL_HOVER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// A press on the clock was swallowed: its release goes too.
static CLOCK_PRESSED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The TAP's word on where the pointer is on the clock's button (`tap_proto::HOVER_*`; it only says
/// "over" while the calendar replaces the shell's).
pub(super) fn set_clock_hover(code: usize) {
    use std::sync::atomic::Ordering::Relaxed;
    match code {
        tap_proto::HOVER_OVER | tap_proto::HOVER_BELL => {
            // Where it was over the button: a click far from there is not on it, whatever a stale
            // word (the TAP unloaded before saying "left") claims.
            let mut pt = POINT::default();
            let _ = unsafe { windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut pt) };
            CLOCK_AT.store(pack(pt), Relaxed);
            // The bell is inside the button: over it is over the button.
            CLOCK_HOVER.store(true, Relaxed);
            if code == tap_proto::HOVER_BELL {
                BELL_HOVER.store(true, Relaxed);
            }
        }
        tap_proto::HOVER_BELL_LEFT => BELL_HOVER.store(false, Relaxed),
        _ => {
            CLOCK_HOVER.store(false, Relaxed);
            BELL_HOVER.store(false, Relaxed);
        }
    }
}

/// The TAP that measured the clock is gone (a revert, Explorer restarted): so are its rects.
pub(super) fn forget_clock() {
    set_clock_hover(tap_proto::HOVER_LEFT);
    set_part_rect(tap_proto::PART_FORGET, 0);
}

/// Whether clicks on the clock open the calendar (the calendar is on).
static CLOCK_TAKEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Whether clicks on the bell open audio-tray's notifications (they are on too); else the bell's
/// clicks are the shell's.
static BELL_TAKEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
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

/// The calendar on or off, and its notifications: off, the clock's (or the bell's) clicks are the
/// shell's whatever the TAP says.
pub(super) fn set_clock_taken(calendar: bool, notifications: bool) {
    use std::sync::atomic::Ordering::Relaxed;
    CLOCK_TAKEN.store(calendar, Relaxed);
    BELL_TAKEN.store(calendar && notifications, Relaxed);
    if !calendar {
        CLOCK_HOVER.store(false, Relaxed);
        BELL_HOVER.store(false, Relaxed);
        set_part_rect(tap_proto::PART_FORGET, 0);
    }
}

/// Where the clock and the bell are on the screen, as the TAP measured them when the pointer last
/// entered each (`tap_proto::pack_rect`; 0: not known).
static CLOCK_RECT: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);
static BELL_RECT: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

/// The TAP's word on where the clock or the bell is (`tap_proto::PART_*`), or that they moved.
pub(super) fn set_part_rect(part: usize, packed: isize) {
    use std::sync::atomic::Ordering::Relaxed;
    match part {
        tap_proto::PART_CLOCK => CLOCK_RECT.store(packed as i64, Relaxed),
        tap_proto::PART_BELL => BELL_RECT.store(packed as i64, Relaxed),
        _ => {
            CLOCK_RECT.store(0, Relaxed);
            BELL_RECT.store(0, Relaxed);
        }
    }
}

/// A measurement is running.
static MEASURING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The pointer has just entered the clock or the bell: measure where that is on the screen, with UI
/// Automation from a thread of our own (in our process: nothing runs in Explorer for it, and a slow
/// answer holds up no one). The element under the pointer, or its nearest ancestor, is the clock's
/// (`SystemTray.OmniButtonLeft`) or the bell's (`SystemTray.AccentButton`, the one in the clock's
/// button the TAP said the pointer is over). One long-lived thread, its UI Automation made once.
pub(super) fn measure_parts() {
    use std::sync::atomic::Ordering::SeqCst;
    static WORKER: std::sync::OnceLock<Option<std::sync::mpsc::Sender<()>>> = std::sync::OnceLock::new();
    if MEASURING.swap(true, SeqCst) {
        return;
    }
    let worker = WORKER.get_or_init(|| {
        let (send, receive) = std::sync::mpsc::channel::<()>();
        std::thread::Builder::new().name("clock-rects".into()).spawn(move || measure_worker(receive)).ok().map(|_| send)
    });
    if !worker.as_ref().is_some_and(|w| w.send(()).is_ok()) {
        MEASURING.store(false, SeqCst);
    }
}

/// [`measure_parts`]' thread: a measurement per word.
fn measure_worker(receive: std::sync::mpsc::Receiver<()>) {
    use std::sync::atomic::Ordering::SeqCst;
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
    use windows::Win32::UI::Accessibility::{CUIAutomation, IUIAutomation};
    let automation: Option<IUIAutomation> = unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok().ok().and_then(|_| CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok())
    };
    while receive.recv().is_ok() {
        let measured = (|| -> Option<(usize, RECT)> {
            let automation = automation.as_ref()?;
            let mut pt = POINT::default();
            unsafe { windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut pt) }.ok()?;
            let walker = unsafe { automation.ControlViewWalker() }.ok()?;
            let mut element = unsafe { automation.ElementFromPoint(pt) }.ok()?;
            for _ in 0..6 {
                let class = unsafe { element.CurrentClassName() }.map(|c| c.to_string()).unwrap_or_default();
                let part = match class.as_str() {
                    "SystemTray.OmniButtonLeft" => Some(tap_proto::PART_CLOCK),
                    "SystemTray.AccentButton" if BELL_HOVER.load(std::sync::atomic::Ordering::Relaxed) => Some(tap_proto::PART_BELL),
                    _ => None,
                };
                if let Some(part) = part {
                    return Some((part, unsafe { element.CurrentBoundingRectangle() }.ok()?));
                }
                element = unsafe { walker.GetParentElement(&element) }.ok()?;
            }
            None
        })();
        if let Some((part, r)) = measured {
            set_part_rect(part, tap_proto::pack_rect(r.left, r.top, r.right, r.bottom));
        }
        MEASURING.store(false, SeqCst);
    }
}

/// The bell's middle on the screen, when it has been measured: where a banner going to the list
/// flies.
pub fn bell_point() -> Option<POINT> {
    let packed = BELL_RECT.load(std::sync::atomic::Ordering::Relaxed);
    if packed == 0 {
        return None;
    }
    let (left, top, right, bottom) = tap_proto::unpack_rect(packed as isize);
    Some(POINT { x: (left + right) / 2, y: (top + bottom) / 2 })
}

/// Whether `pt` is on the clock's button: `Some(true)` on its bell, `Some(false)` on the clock.
/// Over it as the TAP last said (and within reach of where the pointer entered, so a stale word
/// cannot take presses elsewhere), else inside where each was measured.
fn on_clock_part(pt: POINT) -> Option<bool> {
    use std::sync::atomic::Ordering::Relaxed;
    if CLOCK_HOVER.load(Relaxed) {
        let at = unpack(CLOCK_AT.load(Relaxed));
        if (pt.x - at.x).abs() > CLOCK_REACH || (pt.y - at.y).abs() > CLOCK_REACH {
            return None;
        }
        Some(BELL_HOVER.load(Relaxed))
    } else if in_rect(&BELL_RECT, pt) {
        Some(true)
    } else if in_rect(&CLOCK_RECT, pt) {
        Some(false)
    } else {
        None
    }
}

/// Whether `pt` is in a rect kept by [`set_part_rect`].
fn in_rect(rect: &std::sync::atomic::AtomicI64, pt: POINT) -> bool {
    let packed = rect.load(std::sync::atomic::Ordering::Relaxed);
    if packed == 0 {
        return false;
    }
    let (left, top, right, bottom) = tap_proto::unpack_rect(packed as isize);
    pt.x >= left && pt.x < right && pt.y >= top && pt.y < bottom
}

/// Whether a left button event is a click on the clock (or its bell) to take: a press while over
/// it (and over a taskbar, so a stale word from the TAP cannot take clicks elsewhere), or the
/// release of one. The clock opens the calendar, the bell the notifications.
///
/// Over it as the TAP last said, or, with no word since the pointer left, inside where it was
/// measured: Windows' right-click menu takes the pointer (the TAP hears it leave) and gives no
/// enter back when it closes, so the next click on the clock went to the shell's own flyout.
fn clock_click(down: bool, pt: POINT) -> bool {
    use std::sync::atomic::Ordering::Relaxed;
    if !down {
        return CLOCK_PRESSED.swap(false, Relaxed);
    }
    if !CLOCK_TAKEN.load(Relaxed) {
        return false;
    }
    let Some(bell) = on_clock_part(pt) else { return false };
    if bell && !BELL_TAKEN.load(Relaxed) {
        // Windows' own notification centre.
        return false;
    }
    let hwnd = unsafe { WindowFromPoint(pt) };
    let root: HWND = unsafe { GetAncestor(hwnd, GA_ROOT) };
    if !matches!(crate::win::class_name(root).as_str(), "Shell_TrayWnd" | "Shell_SecondaryTrayWnd") {
        return false;
    }
    let action = if bell { tap_proto::ACTION_OPEN_NOTIFICATIONS } else { tap_proto::ACTION_OPEN_CALENDAR };
    if !super::post(crate::taskbar::WM_TASKBAR_ACTION, action, 0) {
        return false;
    }
    CLOCK_PRESSED.store(true, Relaxed);
    true
}

/// Whether the calendar is open and `pt` is outside it: the click that closes it, as a menu's would.
/// The calendar closes on losing the foreground too, but Windows may never have given it that.
fn outside_calendar(pt: POINT) -> bool {
    let open = *crate::calendar::OPEN_CARD.lock().unwrap_or_else(|e| e.into_inner());
    open.is_some_and(|r| !(pt.x >= r.left && pt.x < r.right && pt.y >= r.top && pt.y < r.bottom))
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

/// The V, D or N of a shortcut taken: its release goes too.
static SHORTCUT_HELD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Win+Ctrl+V (Windows' shortcut for its sound output) opens the audio panel, while on.
pub static AUDIO_SHORTCUT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Windows' own shortcuts for what each of our taskbar buttons replaces open ours: Win+Ctrl+V the
/// audio panel, Win+Alt+D the calendar, Win+N the notifications (each while it is on).
unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    use std::sync::atomic::Ordering::Relaxed;
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT};
    use windows::Win32::UI::WindowsAndMessaging::{KBDLLHOOKSTRUCT, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP};
    if code >= 0 {
        let key = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let calendar = crate::calendar::SHORTCUT.load(Relaxed);
        // The action and the modifiers it takes (Ctrl, Alt), exactly: other combinations are other
        // apps' shortcuts.
        let shortcut = match key.vkCode {
            k if k == b'V' as u32 && AUDIO_SHORTCUT.load(Relaxed) => Some((tap_proto::ACTION_OPEN_PANEL, true, false)),
            k if k == b'D' as u32 && calendar => Some((tap_proto::ACTION_OPEN_CALENDAR, false, true)),
            k if k == b'N' as u32 && calendar && BELL_TAKEN.load(Relaxed) => Some((tap_proto::ACTION_OPEN_NOTIFICATIONS, false, false)),
            _ => None,
        };
        if let Some((action, ctrl, alt)) = shortcut {
            let down = matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
            let up = matches!(wparam.0 as u32, WM_KEYUP | WM_SYSKEYUP);
            let held = |vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY| unsafe { GetAsyncKeyState(vk.0 as i32) } as u16 & 0x8000 != 0;
            let wanted = held(VK_CONTROL) == ctrl && held(VK_MENU) == alt && !held(VK_SHIFT);
            if down && (held(VK_LWIN) || held(VK_RWIN)) && wanted {
                if !SHORTCUT_HELD.swap(true, Relaxed) {
                    super::post(crate::taskbar::WM_TASKBAR_ACTION, action, 0);
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
