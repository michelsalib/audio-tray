//! Making the injected strip react to the pointer.
//!
//! The strip is XAML we handed to a `ContentPresenter`, so it is ordinary tree
//! content and ordinary routed events reach it. What is *not* ordinary is where
//! the handlers live: they are Rust objects inside `explorer.exe`, invoked by the
//! shell's own UI thread.
//!
//! Two consequences shape everything here:
//!
//! - A panic must never escape `Invoke` — unwinding through the COM boundary
//!   would abort Explorer.
//! - The handlers are attached from `OnVisualTreeChange` when our own elements
//!   are announced back to us. Injected content is reported like any other, so
//!   the segments are found by `x:Name` in the recorded tree rather than by
//!   walking what `XamlReader` returned.

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
use std::mem::ManuallyDrop;
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

/// Runs `work`, swallowing any panic. Nothing may unwind into Explorer.
fn guard(what: &str, work: impl FnOnce()) -> HRESULT {
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).is_err() {
        logf!("{what} handler panicked — event swallowed");
    }
    S_OK
}

/// Sets the hover plate's opacity on enter/exit. One instance per event, each
/// carrying the opacity it applies, so there is no state to track.
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
            // Not deduped: setting the same opacity twice is idempotent, so the
            // doubled delivery is harmless here.
            decorate::set_opacity(&diagnostics, self.plate, self.opacity);
        })
    }
}

/// Suppresses the second delivery of a single event.
///
/// Measured: one click reaches a handler **twice** — same `sender`, same event
/// args object, same thread, one registration. Whatever the taskbar's input
/// hosting is doing, acting on both would cycle the device two steps per click.
///
/// Keyed on the identity of the event args rather than on a timer, because that
/// is exactly what distinguishes the two cases: a redelivery carries the *same*
/// args object, while a genuine second click carries a new one. The time bound
/// only guards against COM recycling that address for a later event.
fn already_seen(args: *mut c_void) -> bool {
    use std::time::{Duration, Instant};
    const RECYCLE_WINDOW: Duration = Duration::from_millis(500);

    static LAST: Mutex<Option<(usize, Instant)>> = Mutex::new(None);
    let mut last = match LAST.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
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

/// Wheel or two-finger scroll over a segment — changes that endpoint's volume.
///
/// This is the *touchpad's* way in, and the reason it exists at all. A precision touchpad's
/// two-finger scroll never reaches a global mouse hook — Windows routes it straight to the
/// window under the pointer — so audio-tray's own hook, which has handled the wheel since
/// before the strip existed, cannot see it. It does arrive here, on the element the finger is
/// over, which also means the segment comes for free rather than being worked out from
/// coordinates.
///
/// The two paths cannot both act on one notch: audio-tray's hook *swallows* the wheel event
/// it acts on, so XAML never delivers that one here.
///
/// Deliberately **not** put through [`already_seen`], unlike the tap handlers. That test keys
/// on the identity of the args object, and it is only sound where events are far apart: a
/// touchpad emits tens of these a second, and if XAML ever pools the args (which the
/// dedup's own `RECYCLE_WINDOW` exists because COM does) it would silently eat half a
/// gesture. A doubled delivery here costs a slightly faster scroll; a dropped one costs a
/// gesture that stutters. audio-tray sums the deltas either way.
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

/// The wheel delta on a `PointerWheelChanged` args object, in `WHEEL_DELTA` units — and, when
/// there is one, the event marked handled so the shell does not get a second go at the same
/// scroll.
///
/// `None` for anything that is not a vertical scroll to act on: a *horizontal* two-finger
/// swipe raises this same event and must not touch the volume, and a zero delta says nothing.
/// Those are left unhandled, so whatever the shell wants to do with them it still can.
///
/// # Safety
/// `args` is the borrowed pointer XAML passed to the handler — it must not be released here,
/// hence the [`ManuallyDrop`]. The two objects fetched below *are* ours and are dropped.
unsafe fn claim_wheel(args: *mut c_void) -> Option<i32> {
    if args.is_null() {
        return None;
    }
    let borrowed = ManuallyDrop::new(core::mem::transmute::<*mut c_void, IInspectable>(args));
    let event = borrowed.cast::<IPointerRoutedEventArgs>().ok()?;

    // `relativeTo` is null: the position is of no interest, only the properties hanging off
    // the point, and null asks for coordinates relative to the app rather than an element.
    let mut raw_point: *mut c_void = core::ptr::null_mut();
    if event.GetCurrentPoint(core::ptr::null_mut(), &mut raw_point) != S_OK || raw_point.is_null() {
        return None;
    }
    let point = core::mem::transmute::<*mut c_void, IPointerPoint>(raw_point);
    let mut raw_properties: *mut c_void = core::ptr::null_mut();
    if point.get_Properties(&mut raw_properties) != S_OK || raw_properties.is_null() {
        return None;
    }
    let properties =
        core::mem::transmute::<*mut c_void, IPointerPointProperties>(raw_properties);

    let mut horizontal = 0u8;
    if properties.get_IsHorizontalMouseWheel(&mut horizontal) == S_OK && horizontal != 0 {
        return None;
    }
    let mut delta = 0i32;
    if properties.get_MouseWheelDelta(&mut delta) != S_OK || delta == 0 {
        return None;
    }
    // Unchecked, and unlogged: a property set on the args cannot meaningfully fail, and
    // anything said here would be said once per event of a gesture.
    let _ = event.put_Handled(1);
    Some(delta)
}

/// The first scroll on each segment, and then only under `debug=1`.
///
/// A line per event is not an option — a touchpad gesture is tens of them — but the first one
/// is exactly what a bug report needs: it says the route works at all, and what the device
/// reports per step (±120 for a wheel notch, a fraction of that for a touchpad).
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
    let mut raw: *mut c_void = core::ptr::null_mut();
    if diagnostics.GetIInspectableFromHandle(handle, &mut raw) != S_OK || raw.is_null() {
        return None;
    }
    let object = core::mem::transmute::<*mut c_void, IInspectable>(raw);
    object.cast::<IUIElement>().ok()
}

/// Wires hover, left click, right click and scroll onto one segment.
///
/// The delegates are handed to XAML, which takes its own reference — dropping
/// our side afterwards is correct and is why no tokens are kept. Nothing here is
/// ever detached: the TAP lives as long as the Explorer process it is pinned in.
///
/// # Safety
/// XAML UI thread (i.e. the visual-tree callback thread) only.
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
    // The lit opacity depends on what the plate is made of — accent on the pill,
    // white without one — so it comes from the same place the markup does.
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
