//! Wheel over the taskbar → [`WM_TASKBAR_SCROLL`] to the tray's message window; a left click on the
//! taskbar clock → the calendar, on the bell beside it → the notifications, and on Windows' battery
//! icon → the batteries, while they replace the shell's (the TAP says when the pointer is over
//! each, [`set_clock_hover`], [`set_battery_hover`]). The click is swallowed whole, down and up:
//! Explorer's button opens its own flyout whatever its XAML handlers do with the press.
//!
//! A `WH_MOUSE_LL` hook (and, while a shortcut is on, a `WH_KEYBOARD_LL` one) on a thread of its
//! own, with a bare message loop: Windows silently unhooks a low-level hook whose thread misses
//! `LowLevelHooksTimeout`, and the tray thread does slow COM work. The callbacks only classify the
//! point and post. Where the parts are is measured with UI Automation on another thread
//! ([`measure_parts`]).

use std::sync::atomic::Ordering::{Relaxed, SeqCst};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64};
use std::sync::Mutex;

use windows::Win32::Foundation::{LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetAncestor, GetCursorPos, GetMessageW, PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx, WindowFromPoint, GA_ROOT, HHOOK, MSG,
    MSLLHOOKSTRUCT, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_APP, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEWHEEL, WM_QUIT, WM_RBUTTONDOWN,
};

use crate::audio::Flow;
use crate::taskbar::{WM_TASKBAR_ACTION, WM_TASKBAR_SCROLL};

use super::messages::{pack_point, WM_BELL_MIDDLE, WM_ICON_RECT_CHECK, WM_STRIP_MIDDLE};
use super::post;

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
            .name("input-hooks".into())
            .spawn(move || {
                // A precision touchpad's scroll reaches it too, as fine wheel steps (measured on
                // 26300); where the hook passes on, the TAP's `PointerWheelChanged` handler takes it.
                let hook = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), None, 0) };
                let _ = ready.send(hook.is_ok().then(|| unsafe { GetCurrentThreadId() }));
                let Ok(hook) = hook else { return };
                // Win+Ctrl+V, Win+Alt+D and Win+N, only while a shortcut is on: a keyboard hook
                // routes every keystroke in the session through this thread.
                let mut keys = None;
                let want = |on: bool, keys: &mut Option<HHOOK>| {
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
                want(crate::calendar::SHORTCUT.load(SeqCst) || AUDIO_SHORTCUT.load(SeqCst), &mut keys);
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
                HOOK_THREAD.store(thread, SeqCst);
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
const WM_WANT_KEYS: u32 = WM_APP + 1;
/// The hook thread, for [`want_keys`] (0: none).
static HOOK_THREAD: AtomicU32 = AtomicU32::new(0);

/// Any shortcut on, or none: the keyboard hook follows (call after `set_shortcut` and
/// [`AUDIO_SHORTCUT`]).
pub(super) fn want_keys(on: bool) {
    let thread = HOOK_THREAD.load(SeqCst);
    if thread != 0 {
        let _ = unsafe { PostThreadMessageW(thread, WM_WANT_KEYS, WPARAM(on as usize), LPARAM(0)) };
    }
}

impl Drop for Hooks {
    fn drop(&mut self) {
        HOOK_THREAD.store(0, SeqCst);
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
    let message = wparam.0 as u32;
    if code >= 0 && matches!(message, WM_LBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONDOWN) {
        let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        if message != WM_RBUTTONDOWN && (clock_click(message == WM_LBUTTONDOWN, info.pt) || battery_click(message == WM_LBUTTONDOWN, info.pt)) {
            return LRESULT(1);
        }
        if message != WM_LBUTTONUP && outside_calendar(info.pt) {
            // Passed on: the click still does what it does there. Logged by the tray thread, not
            // here: a hook that blocks (on a console write) past Windows' timeout is unhooked.
            post(crate::calendar::WM_CALENDAR, crate::calendar::CLOSE, pack_point(info.pt.x, info.pt.y));
        }
    }
    if code >= 0 && matches!(message, WM_MBUTTONDOWN | WM_MBUTTONUP) {
        let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        if middle_click(message == WM_MBUTTONDOWN, info.pt) {
            return LRESULT(1);
        }
    }
    if code >= 0 && message == WM_MOUSEWHEEL {
        let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        let delta = (info.mouseData >> 16) as i16;
        match if delta != 0 { wheel_target(info.pt) } else { Target::Elsewhere } {
            Target::Ours => {
                let flow = crate::taskbar::flow_code(flow_at(info.pt));
                // Swallowed only when the tray took it: so the shell does not also scroll, and so
                // XAML never sees it — which keeps the TAP's wheel handler from acting twice.
                // A touchpad's scroll reaches this hook too, as wheel input: tagged by its steps.
                let source = if crate::win::touchpad_delta(delta.into()) { 0 } else { crate::taskbar::SCROLL_FROM_WHEEL };
                if post(WM_TASKBAR_SCROLL, flow | source, delta as isize) {
                    return LRESULT(1);
                }
            }
            // Maybe over the buttons after all: the slot is only re-read now and then, and the
            // icons move whenever another app's do. Have the tray look again for the next notch.
            Target::TaskbarOutsideSlot => {
                if !RECT_CHECK_PENDING.swap(true, Relaxed) && !post(WM_ICON_RECT_CHECK, 0, 0) {
                    RECT_CHECK_PENDING.store(false, Relaxed);
                }
            }
            Target::Elsewhere => {}
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Whether `pt` is inside `r` (its right and bottom edges outside, as Windows' rects are).
fn rect_contains(r: &RECT, pt: POINT) -> bool {
    pt.x >= r.left && pt.x < r.right && pt.y >= r.top && pt.y < r.bottom
}

/// The class of the top-level window under `pt`; empty where there is none.
fn root_class(pt: POINT) -> String {
    let hwnd = unsafe { WindowFromPoint(pt) };
    if hwnd.is_invalid() {
        return String::new();
    }
    crate::win::class_name(unsafe { GetAncestor(hwnd, GA_ROOT) })
}

/// The taskbars: the main one and those on other screens.
fn is_taskbar(class: &str) -> bool {
    matches!(class, "Shell_TrayWnd" | "Shell_SecondaryTrayWnd")
}

/// The taskbars and the notification area's overflow.
fn in_tray_area(class: &str) -> bool {
    is_taskbar(class) || matches!(class, "NotifyIconOverflowWindow" | "TopLevelWindowForOverflowXamlIsland")
}

/// Whether `pt` is on a taskbar (so a stale word from the TAP cannot take clicks elsewhere).
fn over_taskbar(pt: POINT) -> bool {
    is_taskbar(&root_class(pt))
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
static MIDDLE_PRESSED: AtomicBool = AtomicBool::new(false);

/// Whether a middle button event is the strip's to take: a press on our icon slot (in a taskbar,
/// not under another app's window), posted to the tray as [`WM_STRIP_MIDDLE`] with the half it
/// landed on, or the release of one. Swallowed, so the shell does nothing with it.
fn middle_click(down: bool, pt: POINT) -> bool {
    if !down {
        return MIDDLE_PRESSED.swap(false, Relaxed);
    }
    let class = root_class(pt);
    // The bell's (Do not disturb, or clear all, as `[calendar] bell_middle_click` says).
    let posted = if on_clock_part(pt) == Some(true) && BELL_MIDDLE_TAKEN.load(Relaxed) && is_taskbar(&class) {
        post(WM_BELL_MIDDLE, 0, 0)
    } else if in_slot(pt) && wheel_belongs_to_tray(&class, crate::taskbar::buttons_up()) {
        post(WM_STRIP_MIDDLE, crate::taskbar::flow_code(flow_at(pt)), 0)
    } else {
        if in_tray_area(&class) {
            // The slot recorded may be wrong: Windows' word on it (`Shell_NotifyIconGetRect`) still
            // gives the plain icon's place, the overflow chevron's even, once the strip has widened
            // and moved beside the wifi. Measured with UI Automation instead, and the click carried
            // out if it was on our buttons after all. This press goes on to the shell, which does
            // nothing with a middle click there.
            ask(Measure::MiddleClick(pt));
        }
        false
    };
    if posted {
        MIDDLE_PRESSED.store(true, Relaxed);
    }
    posted
}

/// Whether a middle click on the bell is Audio Tray's (its notifications are on, and the click is
/// set to do something).
static BELL_MIDDLE_TAKEN: AtomicBool = AtomicBool::new(false);

pub(super) fn set_bell_middle_taken(on: bool) {
    BELL_MIDDLE_TAKEN.store(on, Relaxed);
}

/// One of the shell's taskbar parts whose left click Audio Tray can take (the clock, the bell,
/// Windows' battery icon): whether it does, where the pointer is as the TAP says, where the part
/// was measured, and a press taken (so its release goes too). Shared with the hook thread.
struct HoverPart {
    /// Its clicks are Audio Tray's.
    taken: AtomicBool,
    /// The pointer is over it, as the TAP last said.
    hover: AtomicBool,
    /// Where the pointer was when the TAP said it entered ([`pack`]ed).
    at: AtomicU64,
    /// Where it is on the screen, as measured when the pointer last entered it
    /// (`tap_proto::pack_rect`; 0: not known).
    rect: AtomicI64,
    /// A press on it was swallowed: its release goes too.
    pressed: AtomicBool,
    /// It is about this wide: a press further than this from where the pointer entered it is
    /// elsewhere (a leave the TAP never said).
    reach: i32,
}

impl HoverPart {
    const fn new(reach: i32) -> Self {
        Self { taken: AtomicBool::new(false), hover: AtomicBool::new(false), at: AtomicU64::new(0), rect: AtomicI64::new(0), pressed: AtomicBool::new(false), reach }
    }

    fn taken(&self) -> bool {
        self.taken.load(Relaxed)
    }

    fn hovered(&self) -> bool {
        self.hover.load(Relaxed)
    }

    /// The TAP says the pointer entered it: where the pointer is now is where it entered.
    fn enter(&self) {
        let mut pt = POINT::default();
        let _ = unsafe { GetCursorPos(&mut pt) };
        self.at.store(pack(pt), Relaxed);
        self.hover.store(true, Relaxed);
    }

    fn leave(&self) {
        self.hover.store(false, Relaxed);
    }

    /// Whether `pt` is within reach of where the pointer entered it.
    fn within_reach(&self, pt: POINT) -> bool {
        let at = unpack(self.at.load(Relaxed));
        (pt.x - at.x).abs() <= self.reach && (pt.y - at.y).abs() <= self.reach
    }

    fn measured(&self) -> bool {
        self.rect.load(Relaxed) != 0
    }

    /// Whether `pt` is inside where it was measured (`false` when it was not).
    fn contains(&self, pt: POINT) -> bool {
        let packed = self.rect.load(Relaxed);
        if packed == 0 {
            return false;
        }
        let (left, top, right, bottom) = tap_proto::unpack_rect(packed as isize);
        rect_contains(&RECT { left, top, right, bottom }, pt)
    }

    /// Post `action` for a press on it; `true` (the press is swallowed) when it went, and its
    /// release then goes too.
    fn press(&self, action: usize) -> bool {
        if !post(WM_TASKBAR_ACTION, action, 0) {
            return false;
        }
        self.pressed.store(true, Relaxed);
        true
    }

    /// Whether a release is that of a press it took.
    fn release(&self) -> bool {
        self.pressed.swap(false, Relaxed)
    }
}

/// The clock's button: the clock opens the calendar. Its `at`, `pressed` and `reach` are the whole
/// button's, the bell's included.
static CLOCK: HoverPart = HoverPart::new(160);
/// The bell inside the clock's button: it opens the notifications (its `taken` and `hover` alone
/// are its own).
static BELL: HoverPart = HoverPart::new(0);
/// Windows' battery icon: it opens the batteries.
static BATTERY: HoverPart = HoverPart::new(80);

/// The TAP's word on where the pointer is on the clock's button (`tap_proto::HOVER_*`; it only says
/// "over" while the calendar replaces the shell's).
pub(super) fn set_clock_hover(code: usize) {
    match code {
        tap_proto::HOVER_OVER | tap_proto::HOVER_BELL => {
            // Where it was over the button: a click far from there is not on it, whatever a stale
            // word (the TAP unloaded before saying "left") claims. The bell is inside the button:
            // over it is over the button.
            CLOCK.enter();
            if code == tap_proto::HOVER_BELL {
                BELL.hover.store(true, Relaxed);
            }
        }
        tap_proto::HOVER_BELL_LEFT => BELL.leave(),
        _ => {
            CLOCK.leave();
            BELL.leave();
        }
    }
}

/// The TAP's word: the pointer entered (`true`) or left Windows' battery icon.
pub(super) fn set_battery_hover(over: bool) {
    if over {
        BATTERY.enter();
    } else {
        BATTERY.leave();
    }
}

/// The TAP that measured the clock is gone (a revert, Explorer restarted): so are its rects.
pub(super) fn forget_clock() {
    set_clock_hover(tap_proto::HOVER_LEFT);
    set_battery_hover(false);
    set_part_rect(tap_proto::PART_FORGET, 0);
}

/// The calendar on or off, and the bell's notifications, each on its own: off, the clock's (or the
/// bell's) clicks are the shell's whatever the TAP says.
pub(super) fn set_clock_taken(calendar: bool, bell: bool) {
    CLOCK.taken.store(calendar, Relaxed);
    BELL.taken.store(bell, Relaxed);
    if !calendar && !bell {
        CLOCK.leave();
        BELL.leave();
        set_part_rect(tap_proto::PART_FORGET, 0);
    }
}

/// The batteries take the battery icon's clicks, or leave them to the shell.
pub(super) fn set_battery_taken(on: bool) {
    BATTERY.taken.store(on, Relaxed);
    if !on {
        BATTERY.leave();
        BATTERY.rect.store(0, Relaxed);
    }
}

/// Where the clock, the bell or the battery is (`tap_proto::PART_*`, packed by
/// `tap_proto::pack_rect`), or that they moved.
pub(super) fn set_part_rect(part: usize, packed: isize) {
    match part {
        tap_proto::PART_CLOCK => CLOCK.rect.store(packed as i64, Relaxed),
        tap_proto::PART_BELL => BELL.rect.store(packed as i64, Relaxed),
        tap_proto::PART_BATTERY => BATTERY.rect.store(packed as i64, Relaxed),
        _ => {
            for part in [&CLOCK, &BELL, &BATTERY] {
                part.rect.store(0, Relaxed);
            }
        }
    }
}

fn pack(pt: POINT) -> u64 {
    ((pt.x as u32 as u64) << 32) | pt.y as u32 as u64
}

fn unpack(v: u64) -> POINT {
    POINT { x: (v >> 32) as u32 as i32, y: v as u32 as i32 }
}

/// A measurement is running.
static MEASURING: AtomicBool = AtomicBool::new(false);

/// What the measuring thread is asked.
enum Measure {
    /// Where the clock, the bell or the battery under the pointer is.
    Part,
    /// Whether a middle click at this point was on our buttons (the slot recorded may be out of
    /// date): measured, and the click carried out when it was.
    MiddleClick(POINT),
}

/// The measuring thread's mailbox: one long-lived thread, its UI Automation made once (in our
/// process: nothing runs in Explorer for it, and a slow answer holds up no one).
fn ask(request: Measure) {
    static WORKER: std::sync::OnceLock<Option<Mutex<std::sync::mpsc::Sender<Measure>>>> = std::sync::OnceLock::new();
    let worker = WORKER.get_or_init(|| {
        let (send, receive) = std::sync::mpsc::channel::<Measure>();
        std::thread::Builder::new().name("taskbar-rects".into()).spawn(move || measure_worker(receive)).ok().map(|_| Mutex::new(send))
    });
    let part = matches!(request, Measure::Part);
    let sent = worker.as_ref().is_some_and(|w| w.lock().unwrap_or_else(|e| e.into_inner()).send(request).is_ok());
    if part && !sent {
        MEASURING.store(false, SeqCst);
    }
}

/// The pointer has just entered the clock, the bell or the battery: measure where that is on the
/// screen, with UI Automation. The element under the pointer, or its nearest ancestor, is the
/// clock's (`SystemTray.OmniButtonLeft`), or the battery's or the bell's (`SystemTray.AccentButton`,
/// as the TAP said the pointer is over the one or the other).
pub(super) fn measure_parts() {
    if MEASURING.swap(true, SeqCst) {
        return;
    }
    ask(Measure::Part);
}

/// [`ask`]'s thread: a measurement per request.
fn measure_worker(receive: std::sync::mpsc::Receiver<Measure>) {
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
    use windows::Win32::UI::Accessibility::{CUIAutomation, IUIAutomation, IUIAutomationElement};
    let automation: Option<IUIAutomation> = unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok().ok().and_then(|_| CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok())
    };
    // The element at `pt` and up to five of its ancestors, the first `pick` takes.
    let find = |pt: POINT, pick: &dyn Fn(&IUIAutomationElement, &str) -> bool| -> Option<IUIAutomationElement> {
        let automation = automation.as_ref()?;
        let walker = unsafe { automation.ControlViewWalker() }.ok()?;
        let mut element = unsafe { automation.ElementFromPoint(pt) }.ok()?;
        for _ in 0..6 {
            let class = unsafe { element.CurrentClassName() }.map(|c| c.to_string()).unwrap_or_default();
            if pick(&element, &class) {
                return Some(element);
            }
            element = unsafe { walker.GetParentElement(&element) }.ok()?;
        }
        None
    };
    let rect_of = |element: &IUIAutomationElement| unsafe { element.CurrentBoundingRectangle() }.ok();
    while let Ok(request) = receive.recv() {
        match request {
            Measure::Part => {
                let mut pt = POINT::default();
                let _ = unsafe { GetCursorPos(&mut pt) };
                let part_of = |class: &str| match class {
                    "SystemTray.OmniButtonLeft" => Some(tap_proto::PART_CLOCK),
                    // The battery is an `AccentButton` too, in the Quick Settings button.
                    "SystemTray.AccentButton" if BATTERY.hovered() => Some(tap_proto::PART_BATTERY),
                    "SystemTray.AccentButton" if BELL.hovered() => Some(tap_proto::PART_BELL),
                    _ => None,
                };
                let found = find(pt, &|_, class| part_of(class).is_some());
                let measured = found.as_ref().and_then(|e| {
                    let class = unsafe { e.CurrentClassName() }.map(|c| c.to_string()).unwrap_or_default();
                    Some((part_of(&class)?, rect_of(e)?))
                });
                if let Some((part, r)) = measured {
                    set_part_rect(part, tap_proto::pack_rect(r.left, r.top, r.right, r.bottom));
                }
                MEASURING.store(false, SeqCst);
            }
            Measure::MiddleClick(pt) => {
                // Our buttons: the notification icon whose name ends with the tray's marker.
                let ours = |e: &IUIAutomationElement, class: &str| {
                    class == "SystemTray.NormalButton" && unsafe { e.CurrentName() }.is_ok_and(|n| n.to_string().contains(super::TRAY_MARKER))
                };
                let Some(r) = find(pt, &ours).and_then(|e| rect_of(&e)) else { continue };
                set_icon_rect(Some(r));
                if in_slot(pt) {
                    let _ = post(WM_STRIP_MIDDLE, crate::taskbar::flow_code(flow_at(pt)), 0);
                }
            }
        }
    }
}

/// The bell's middle on the screen, when it has been measured: where a banner going to the list
/// flies.
pub fn bell_point() -> Option<POINT> {
    let packed = BELL.rect.load(Relaxed);
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
    if CLOCK.hovered() {
        CLOCK.within_reach(pt).then(|| BELL.hovered())
    } else if BELL.contains(pt) {
        Some(true)
    } else if CLOCK.contains(pt) {
        Some(false)
    } else {
        None
    }
}

/// Whether a left button event is a click on the clock (or its bell) to take: a press while over
/// it (and over a taskbar, so a stale word from the TAP cannot take clicks elsewhere), or the
/// release of one. The clock opens the calendar, the bell the notifications.
///
/// Over it as the TAP last said, or, with no word since the pointer left, inside where it was
/// measured: Windows' right-click menu takes the pointer (the TAP hears it leave) and gives no
/// enter back when it closes, so the next click on the clock went to the shell's own flyout.
fn clock_click(down: bool, pt: POINT) -> bool {
    if !down {
        return CLOCK.release();
    }
    if !CLOCK.taken() && !BELL.taken() {
        return false;
    }
    let Some(bell) = on_clock_part(pt) else { return false };
    // A part that is not ours is the shell's: Windows' notification centre, or its calendar.
    if !(if bell { &BELL } else { &CLOCK }).taken() || !over_taskbar(pt) {
        return false;
    }
    CLOCK.press(if bell { tap_proto::ACTION_OPEN_NOTIFICATIONS } else { tap_proto::ACTION_OPEN_CALENDAR })
}

/// Whether `pt` is on Windows' battery icon: inside where it was measured; while the TAP says the
/// pointer is on it and it is not measured yet, within reach of where the pointer entered. Measured,
/// a press beside it is never taken, whatever stale word the TAP left (a leave it never heard).
fn on_battery(pt: POINT) -> bool {
    if BATTERY.measured() {
        return BATTERY.contains(pt);
    }
    BATTERY.hovered() && BATTERY.within_reach(pt)
}

/// Whether a left button event is a click on the battery icon to take: a press on it (and on a
/// taskbar), or the release of one. It opens the batteries.
fn battery_click(down: bool, pt: POINT) -> bool {
    if !down {
        return BATTERY.release();
    }
    if !BATTERY.taken() || !on_battery(pt) || !over_taskbar(pt) {
        return false;
    }
    BATTERY.press(tap_proto::ACTION_OPEN_BATTERIES)
}

/// Whether the calendar is open and `pt` is outside it: the click that closes it, as a menu's would.
/// The calendar closes on losing the foreground too, but Windows may never have given it that.
fn outside_calendar(pt: POINT) -> bool {
    let open = *crate::calendar::OPEN_CARD.lock().unwrap_or_else(|e| e.into_inner());
    // Audio Tray's own battery icon: its click turns the card into the batteries (as the clock's
    // and the bell's do), it does not close it first.
    let own = OWN_BATTERY_ICON.lock().unwrap_or_else(|e| e.into_inner()).is_some_and(|r| rect_contains(&r, pt));
    !own && open.is_some_and(|r| !rect_contains(&r, pt))
}

/// Where Audio Tray's own battery icon is on the screen, while it shows.
static OWN_BATTERY_ICON: Mutex<Option<RECT>> = Mutex::new(None);

/// Audio Tray's own battery icon is at `rect` (`None`: not shown).
pub(super) fn set_own_battery_icon(rect: Option<RECT>) {
    *OWN_BATTERY_ICON.lock().unwrap_or_else(|e| e.into_inner()) = rect;
}

/// A [`WM_ICON_RECT_CHECK`] is on its way: one is enough however fast the wheel turns.
static RECT_CHECK_PENDING: AtomicBool = AtomicBool::new(false);

/// The tray has re-read the slot.
pub(super) fn rect_checked() {
    RECT_CHECK_PENDING.store(false, Relaxed);
}

/// Where a wheel at a screen point lands, for the hook.
enum Target {
    /// Over our audio icons.
    Ours,
    /// Over the taskbar or its overflow, but not where the icon slot was last seen.
    TaskbarOutsideSlot,
    Elsewhere,
}

fn wheel_target(pt: POINT) -> Target {
    let class = root_class(pt);
    if class.is_empty() {
        return Target::Elsewhere;
    }
    if in_slot(pt) && wheel_belongs_to_tray(&class, crate::taskbar::buttons_up()) {
        Target::Ours
    } else if in_tray_area(&class) {
        Target::TaskbarOutsideSlot
    } else {
        Target::Elsewhere
    }
}

/// Whether a wheel over our icon slot, in a window of `class`, is ours: the slot must be showing,
/// not covered by another app. A `Xaml_WindowedPopupClass` is any XAML popup in any app; it only
/// counts while our strip is up (its hover tooltip is one).
fn wheel_belongs_to_tray(class: &str, strip_up: bool) -> bool {
    in_tray_area(class) || (class == "Xaml_WindowedPopupClass" && strip_up)
}

fn in_slot(pt: POINT) -> bool {
    icon_rect().is_some_and(|slot| rect_contains(&slot, pt))
}

/// The V, D or N of a shortcut taken: its release goes too.
static SHORTCUT_HELD: AtomicBool = AtomicBool::new(false);

/// Win+Ctrl+V (Windows' shortcut for its sound output) opens the audio panel, while on.
pub static AUDIO_SHORTCUT: AtomicBool = AtomicBool::new(false);

/// Windows' own shortcuts for what each of our taskbar buttons replaces open ours: Win+Ctrl+V the
/// audio panel, Win+Alt+D the calendar, Win+N the notifications (each while it is on).
unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VIRTUAL_KEY, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT};
    use windows::Win32::UI::WindowsAndMessaging::{KBDLLHOOKSTRUCT, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP};
    if code >= 0 {
        let key = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let calendar = crate::calendar::SHORTCUT.load(Relaxed);
        // The action and the modifiers it takes (Ctrl, Alt), exactly: other combinations are other
        // apps' shortcuts.
        let shortcut = match key.vkCode {
            k if k == b'V' as u32 && AUDIO_SHORTCUT.load(Relaxed) => Some((tap_proto::ACTION_OPEN_PANEL, true, false)),
            k if k == b'D' as u32 && calendar && CLOCK.taken() => Some((tap_proto::ACTION_OPEN_CALENDAR, false, true)),
            k if k == b'N' as u32 && calendar && BELL.taken() => Some((tap_proto::ACTION_OPEN_NOTIFICATIONS, false, false)),
            _ => None,
        };
        if let Some((action, ctrl, alt)) = shortcut {
            let down = matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
            let up = matches!(wparam.0 as u32, WM_KEYUP | WM_SYSKEYUP);
            let held = |vk: VIRTUAL_KEY| unsafe { GetAsyncKeyState(vk.0 as i32) } as u16 & 0x8000 != 0;
            let wanted = held(VK_CONTROL) == ctrl && held(VK_MENU) == alt && !held(VK_SHIFT);
            if down && (held(VK_LWIN) || held(VK_RWIN)) && wanted {
                if !SHORTCUT_HELD.swap(true, Relaxed) {
                    post(WM_TASKBAR_ACTION, action, 0);
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
    use super::{pack, rect_contains, unpack, wheel_belongs_to_tray};
    use windows::Win32::Foundation::{POINT, RECT};

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
        assert!(wheel_belongs_to_tray("NotifyIconOverflowWindow", false), "the overflow");
        assert!(wheel_belongs_to_tray("Xaml_WindowedPopupClass", true));
        assert!(!wheel_belongs_to_tray("Xaml_WindowedPopupClass", false), "no strip up");
        assert!(!wheel_belongs_to_tray("Chrome_WidgetWin_1", true), "another app over the slot");
    }

    #[test]
    fn a_rect_holds_its_left_and_top_edges_only() {
        let r = RECT { left: 10, top: 20, right: 30, bottom: 40 };
        assert!(rect_contains(&r, POINT { x: 10, y: 20 }));
        assert!(!rect_contains(&r, POINT { x: 30, y: 25 }));
        assert!(!rect_contains(&r, POINT { x: 15, y: 40 }));
    }
}
