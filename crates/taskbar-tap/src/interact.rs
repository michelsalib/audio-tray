//! Pointer handlers for the injected strip and the music tile's transport buttons: Rust delegates
//! invoked on Explorer's UI thread, so a panic must never escape `Invoke`. Our segments are found
//! by `x:Name` when the tree announces them back, and wired by the sweep.

use crate::decorate;
use crate::log::logf;
use crate::winrt::{
    IPointerEventHandler, IPointerEventHandler_Impl, IPointerPoint, IPointerPointProperties,
    IPointerRoutedEventArgs, IRightTappedEventHandler, IRightTappedEventHandler_Impl,
    ITappedEventHandler, ITappedEventHandler_Impl, IUIElement,
};
use crate::xamlom::{IXamlDiagnostics, InstanceHandle};
use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use windows::Win32::Foundation::S_OK;
use windows_core::{implement, IInspectable, Interface, HRESULT};

/// Which half of the strip an event came from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Segment {
    Output,
    Input,
}

impl Segment {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            decorate::SEGMENT_OUT => Some(Self::Output),
            decorate::SEGMENT_IN => Some(Self::Input),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Output => "output",
            Self::Input => "input",
        }
    }
}

/// Runs a handler's `work` under [`crate::guard`] (a panic is logged and the event swallowed: nothing
/// may unwind into Explorer), answering XAML `S_OK` either way.
fn guard(what: &str, work: impl FnOnce()) -> HRESULT {
    crate::guard(what, work);
    S_OK
}

/// Sets the hover plate's opacity on enter/exit (one instance per event, each with its opacity).
#[implement(IPointerEventHandler)]
struct Hover {
    plate: InstanceHandle,
    opacity: f64,
}

impl IPointerEventHandler_Impl for Hover_Impl {
    unsafe fn Invoke(&self, _sender: *mut c_void, _args: *mut c_void) -> HRESULT {
        guard("hover", || {
            let Some(diagnostics) = crate::diagnostics() else {
                return;
            };
            // Not deduped: idempotent.
            crate::xaml::set_opacity(&diagnostics, self.plate, self.opacity);
        })
    }
}

/// Suppresses the second delivery of a single event (one click reaches a handler twice with the
/// same args object). Keyed on the args' identity; the time bound guards against address reuse.
fn already_seen(args: *mut c_void) -> bool {
    use std::time::{Duration, Instant};
    const RECYCLE_WINDOW: Duration = Duration::from_millis(500);

    static LAST: Mutex<Option<(usize, Instant)>> = Mutex::new(None);
    let mut last = crate::lock(&LAST);
    let now = Instant::now();
    let duplicate = last
        .map(|(seen, at)| seen == args as usize && now.duration_since(at) < RECYCLE_WINDOW)
        .unwrap_or(false);
    *last = Some((args as usize, now));
    duplicate
}

/// Left click on a segment — cycles that endpoint.
#[implement(ITappedEventHandler)]
struct Tap(Segment);

impl ITappedEventHandler_Impl for Tap_Impl {
    unsafe fn Invoke(&self, _sender: *mut c_void, args: *mut c_void) -> HRESULT {
        guard("tap", || {
            if already_seen(args) {
                return;
            }
            logf!("tap on {} segment", self.0.label());
            crate::ipc::send(crate::ipc::Action::Cycle(self.0));
        })
    }
}

/// Right click on either segment — opens the full panel.
#[implement(IRightTappedEventHandler)]
struct RightTap(Segment);

impl IRightTappedEventHandler_Impl for RightTap_Impl {
    unsafe fn Invoke(&self, _sender: *mut c_void, args: *mut c_void) -> HRESULT {
        guard("right-tap", || {
            if already_seen(args) {
                return;
            }
            logf!("right-tap on {} segment", self.0.label());
            crate::ipc::send(crate::ipc::Action::OpenPanel);
        })
    }
}

/// Wheel or two-finger scroll over a segment: changes that endpoint's volume. The touchpad's way
/// in (its scroll never reaches audio-tray's global hook, which swallows the wheel events it acts on).
///
/// Deliberately **not** deduped with [`already_seen`]: at touchpad rates pooled args would eat
/// half a gesture, while a doubled delivery only scrolls slightly faster.
#[implement(IPointerEventHandler)]
struct Wheel(Segment);

impl IPointerEventHandler_Impl for Wheel_Impl {
    unsafe fn Invoke(&self, _sender: *mut c_void, args: *mut c_void) -> HRESULT {
        guard("wheel", || {
            let Some(delta) = claim_wheel(args) else {
                return;
            };
            log_wheel(self.0, delta);
            crate::ipc::send_scroll(self.0, delta);
        })
    }
}

/// The vertical wheel delta (`WHEEL_DELTA` units) of a `PointerWheelChanged`, marking it handled.
/// `None`, left unhandled, for horizontal swipes and zero deltas.
///
/// # Safety
/// `args` is borrowed from XAML: never release it (hence `from_raw_borrowed`).
unsafe fn claim_wheel(args: *mut c_void) -> Option<i32> {
    let event = IInspectable::from_raw_borrowed(&args)?.cast::<IPointerRoutedEventArgs>().ok()?;

    // `relativeTo` null: only the point's properties matter.
    let mut raw_point: *mut c_void = core::ptr::null_mut();
    if event.GetCurrentPoint(core::ptr::null_mut(), &mut raw_point) != S_OK || raw_point.is_null() {
        return None;
    }
    // Owned: dropping them releases the out-parameters' references.
    let point = IPointerPoint::from_raw(raw_point);
    let mut raw_properties: *mut c_void = core::ptr::null_mut();
    if point.get_Properties(&mut raw_properties) != S_OK || raw_properties.is_null() {
        return None;
    }
    let properties = IPointerPointProperties::from_raw(raw_properties);

    let mut horizontal = 0u8;
    if properties.get_IsHorizontalMouseWheel(&mut horizontal) == S_OK && horizontal != 0 {
        return None;
    }
    let mut delta = 0i32;
    if properties.get_MouseWheelDelta(&mut delta) != S_OK || delta == 0 {
        return None;
    }
    let _ = event.put_Handled(1);
    Some(delta)
}

/// Logs the first scroll on each segment, and every one under `debug=1`.
fn log_wheel(segment: Segment, delta: i32) {
    static LOGGED_OUTPUT: AtomicBool = AtomicBool::new(false);
    static LOGGED_INPUT: AtomicBool = AtomicBool::new(false);
    let logged = match segment {
        Segment::Output => &LOGGED_OUTPUT,
        Segment::Input => &LOGGED_INPUT,
    };
    if !logged.swap(true, Ordering::SeqCst) || crate::log::verbose() {
        logf!("scroll on {} segment: delta {delta}", segment.label());
    }
}

unsafe fn ui_element(
    diagnostics: &IXamlDiagnostics,
    handle: InstanceHandle,
) -> Option<IUIElement> {
    crate::xaml::object_from_handle(diagnostics, handle)?
        .cast::<IUIElement>()
        .ok()
}

/// Wires hover, left click, right click and scroll onto one segment. XAML holds its own
/// references to the delegates; nothing is ever detached, so no tokens are kept.
///
/// # Safety
/// XAML UI thread (the tray's) only.
pub unsafe fn attach(
    diagnostics: &IXamlDiagnostics,
    segment: Segment,
    element: InstanceHandle,
    plate: InstanceHandle,
) -> bool {
    let Some(target) = ui_element(diagnostics, element) else {
        logf!("segment 0x{element:x} is not a UIElement — not wiring it up");
        return false;
    };

    let mut token = 0i64;
    // The lit opacity depends on the accent, as in the markup.
    let accent = crate::strip_state().and_then(|state| state.accent);
    let enter: IPointerEventHandler = Hover {
        plate,
        opacity: decorate::hover_opacity(accent),
    }
    .into();
    let entered = target.add_PointerEntered(enter.as_raw(), &mut token);

    let exit: IPointerEventHandler = Hover { plate, opacity: 0.0 }.into();
    let exited = target.add_PointerExited(exit.as_raw(), &mut token);

    let tapped: ITappedEventHandler = Tap(segment).into();
    let tap = target.add_Tapped(tapped.as_raw(), &mut token);

    let right: IRightTappedEventHandler = RightTap(segment).into();
    let right_tap = target.add_RightTapped(right.as_raw(), &mut token);

    // Shares `PointerEventHandler` with hover — same delegate type, different event.
    let wheel: IPointerEventHandler = Wheel(segment).into();
    let wheeled = target.add_PointerWheelChanged(wheel.as_raw(), &mut token);

    let ok =
        entered == S_OK && exited == S_OK && tap == S_OK && right_tap == S_OK && wheeled == S_OK;
    if ok {
        logf!(
            "{} segment wired: element 0x{element:x}, hover plate 0x{plate:x}",
            segment.label()
        );
    } else {
        logf!(
            "{} segment wiring failed: entered=0x{:08x} exited=0x{:08x} tapped=0x{:08x} right=0x{:08x} wheel=0x{:08x}",
            segment.label(),
            entered.0,
            exited.0,
            tap.0,
            right_tap.0,
            wheeled.0
        );
    }
    ok
}

/// A click on one of the music tile's transport glyphs. **Must** mark the tap handled, or the
/// shell's own button also activates the player.
#[implement(ITappedEventHandler)]
struct MusicTap(crate::music::tick::Segment);

impl ITappedEventHandler_Impl for MusicTap_Impl {
    unsafe fn Invoke(&self, _sender: *mut c_void, args: *mut c_void) -> HRESULT {
        guard("music-tap", || {
            let suppressed = suppress_tap(args);
            if already_seen(args) {
                return;
            }
            logf!(
                "music: tap on {} (Handled set: {suppressed})",
                self.0.label()
            );
            crate::ipc::send_code(self.0.code());
        })
    }
}

/// `PointerPressed` on a transport glyph, marked handled so the press does not activate the app.
/// Never do this on the tile body: the press starts the shell's drag-to-reorder.
#[implement(IPointerEventHandler)]
struct MusicPress;

impl IPointerEventHandler_Impl for MusicPress_Impl {
    unsafe fn Invoke(&self, _sender: *mut c_void, args: *mut c_void) -> HRESULT {
        guard("music-press", || {
            // Not deduped: every delivery must be suppressed.
            suppress_pointer(args);
        })
    }
}

/// Mark a completed tap handled, so it does not continue to the shell's own handler.
///
/// # Safety
/// `args` must be the live args pointer XAML passed to `Invoke`.
unsafe fn suppress_tap(args: *mut c_void) -> bool {
    // Borrowed: XAML owns the args, never released here.
    IInspectable::from_raw_borrowed(&args)
        .and_then(|inspectable| inspectable.cast::<crate::winrt::ITappedRoutedEventArgs>().ok())
        .is_some_and(|event| event.put_Handled(1) == S_OK)
}

/// The same, for a pointer press.
///
/// # Safety
/// `args` must be the live args pointer XAML passed to `Invoke`.
unsafe fn suppress_pointer(args: *mut c_void) -> bool {
    // Borrowed: XAML owns the args, never released here.
    IInspectable::from_raw_borrowed(&args)
        .and_then(|inspectable| inspectable.cast::<IPointerRoutedEventArgs>().ok())
        .is_some_and(|event| event.put_Handled(1) == S_OK)
}

/// Wire one of the music tile's transport glyphs (press suppression and tap; no hover or wheel).
///
/// # Safety
/// XAML UI thread (the tray's) only.
pub unsafe fn attach_music(
    diagnostics: &IXamlDiagnostics,
    segment: crate::music::tick::Segment,
    element: InstanceHandle,
) -> bool {
    let Some(target) = ui_element(diagnostics, element) else {
        logf!("music: 0x{element:x} is not a UIElement — not wiring it up");
        return false;
    };
    // A token each, so either could be detached one day.
    let mut press_token = 0i64;
    let mut tap_token = 0i64;
    let pressed: IPointerEventHandler = MusicPress.into();
    let press = target.add_PointerPressed(pressed.as_raw(), &mut press_token);
    let tapped: ITappedEventHandler = MusicTap(segment).into();
    let tap = target.add_Tapped(tapped.as_raw(), &mut tap_token);
    press == S_OK && tap == S_OK
}

/// A press on the taskbar clock, marked handled while the calendar is ours. That alone does not
/// keep the shell's flyout shut (`NotificationCenterButton` opens it regardless: the app's mouse
/// hook takes mouse clicks before Explorer sees them); it is kept for the log, and for touch and
/// pen, which that hook never sees.
#[implement(IPointerEventHandler)]
struct ClockPress;

impl IPointerEventHandler_Impl for ClockPress_Impl {
    unsafe fn Invoke(&self, _sender: *mut c_void, args: *mut c_void) -> HRESULT {
        guard("clock-press", || {
            // Not deduped: every delivery must be suppressed.
            let enabled = crate::clock::enabled();
            let suppressed = enabled && suppress_pointer(args);
            if crate::log::verbose() {
                logf!("clock: press (ours {enabled}, handled {suppressed}; {})", crate::clock::why());
            }
        })
    }
}

/// A completed click on the clock: open audio-tray's calendar.
#[implement(ITappedEventHandler)]
struct ClockTap;

impl ITappedEventHandler_Impl for ClockTap_Impl {
    unsafe fn Invoke(&self, _sender: *mut c_void, args: *mut c_void) -> HRESULT {
        guard("clock-tap", || {
            if !crate::clock::calendar() {
                logf!("clock: tap left to the shell ({}; the calendar the app's or not)", crate::clock::why());
                return;
            }
            suppress_tap(args);
            if already_seen(args) {
                return;
            }
            logf!("clock: tap — opening the calendar");
            crate::ipc::send_calendar();
        })
    }
}

/// Wire the clock's button: press, tap, and the pointer entering and leaving it (the app's mouse
/// hook takes clicks only while the pointer is over the clock). Never detached, so the tokens go.
///
/// # Safety
/// XAML UI thread only.
pub unsafe fn attach_clock(diagnostics: &IXamlDiagnostics, element: InstanceHandle) -> bool {
    let Some(target) = ui_element(diagnostics, element) else {
        return false;
    };
    let mut token = 0i64;
    let pressed: IPointerEventHandler = ClockPress.into();
    let press = target.add_PointerPressed(pressed.as_raw(), &mut token);
    let tapped: ITappedEventHandler = ClockTap.into();
    let tap = target.add_Tapped(tapped.as_raw(), &mut token);
    let entered: IPointerEventHandler = ClockHover { section: element, over: true }.into();
    let enter = target.add_PointerEntered(entered.as_raw(), &mut token);
    let exited: IPointerEventHandler = ClockHover { section: element, over: false }.into();
    let exit = target.add_PointerExited(exited.as_raw(), &mut token);
    press == S_OK && tap == S_OK && enter == S_OK && exit == S_OK
}

/// The pointer entering (`true`) or leaving the clock, told to the app: its mouse hook takes the
/// clock's clicks before Explorer sees them (marking the press handled here does not stop
/// `NotificationCenterButton`, which opens its flyout regardless).
#[implement(IPointerEventHandler)]
struct ClockHover {
    section: InstanceHandle,
    over: bool,
}

impl IPointerEventHandler_Impl for ClockHover_Impl {
    unsafe fn Invoke(&self, _sender: *mut c_void, _args: *mut c_void) -> HRESULT {
        guard("clock-hover", || {
            // Leaving is always told, so the app never keeps a stale "over the clock".
            if self.over && !crate::clock::enabled() {
                return;
            }
            crate::ipc::send_clock_hover(self.over);
            // Its strips (beside and between the parts) light the nearest part, as the shell's one
            // plate lit over the whole button; the bell's is told to the app from there.
            crate::clock::section_hover(self.section, self.over);
            crate::clock::watch_closely();
        })
    }
}

/// Wire an icon inside the clock's button (the clock's, or the bell's): the pointer entering and
/// leaving it lights its own plate, and the bell's is told to the app, whose hook sends its clicks
/// to audio-tray's notifications (or leaves them to the shell when those are off). Never detached.
///
/// # Safety
/// XAML UI thread only.
pub unsafe fn attach_part(diagnostics: &IXamlDiagnostics, element: InstanceHandle, bell: bool) -> bool {
    let Some(target) = ui_element(diagnostics, element) else {
        return false;
    };
    let mut token = 0i64;
    let entered: IPointerEventHandler = PartHover { icon: element, bell, over: true }.into();
    let enter = target.add_PointerEntered(entered.as_raw(), &mut token);
    let exited: IPointerEventHandler = PartHover { icon: element, bell, over: false }.into();
    let exit = target.add_PointerExited(exited.as_raw(), &mut token);
    enter == S_OK && exit == S_OK
}

/// Wire Windows' battery icon (inside the Quick Settings button): the pointer entering and leaving
/// it is told to the app, whose hook sends its clicks to audio-tray's batteries, and lights its own
/// plate. Never detached: the handler checks [`crate::battery::enabled`] on every event.
///
/// # Safety
/// XAML UI thread only.
pub unsafe fn attach_battery(diagnostics: &IXamlDiagnostics, element: InstanceHandle) -> bool {
    let Some(target) = ui_element(diagnostics, element) else {
        return false;
    };
    let mut token = 0i64;
    let entered: IPointerEventHandler = BatteryHover { over: true }.into();
    let enter = target.add_PointerEntered(entered.as_raw(), &mut token);
    let exited: IPointerEventHandler = BatteryHover { over: false }.into();
    let exit = target.add_PointerExited(exited.as_raw(), &mut token);
    enter == S_OK && exit == S_OK
}

/// The pointer entering or leaving the battery icon.
#[implement(IPointerEventHandler)]
struct BatteryHover {
    over: bool,
}

impl IPointerEventHandler_Impl for BatteryHover_Impl {
    unsafe fn Invoke(&self, _sender: *mut c_void, _args: *mut c_void) -> HRESULT {
        guard("battery-hover", || {
            // Leaving is always told and always unlit, so nothing stays stale.
            if self.over && !crate::battery::enabled() {
                return;
            }
            crate::ipc::send_battery_hover(self.over);
            crate::battery::hover(self.over);
        })
    }
}

/// The pointer entering or leaving the clock's or the bell's icon.
#[implement(IPointerEventHandler)]
struct PartHover {
    icon: InstanceHandle,
    bell: bool,
    over: bool,
}

impl IPointerEventHandler_Impl for PartHover_Impl {
    unsafe fn Invoke(&self, _sender: *mut c_void, _args: *mut c_void) -> HRESULT {
        guard("part-hover", || {
            // Leaving is always told and always unlit, so nothing stays stale.
            if self.over && !crate::clock::enabled() {
                return;
            }
            // Where it is on the screen is measured by the app (UI Automation, in its own process):
            // nothing more runs in Explorer on a hover. Measuring here through the pointer's
            // position crashed Explorer (an `IInspectable` handed where XAML wants a `UIElement`).
            crate::clock::part_hover(self.icon, self.bell, self.over);
        })
    }
}
