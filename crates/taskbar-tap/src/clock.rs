//! The taskbar clock: where audio-tray's calendar opens from, and what it reads.
//!
//! **Clicks** (`clock=1` in the init data): the shell's `NotificationCenterButton` opens its own
//! flyout whatever its XAML handlers do with a press, so the TAP only says when the pointer is over
//! the button (`WM_TASKBAR_CLOCK_HOVER`) and the app's mouse hook takes the click. The handlers go on
//! the whole button (the clock's view alone leaves its edges to the shell), behind the short wiring
//! gate (attaching writes nothing), and are never detached: they check [`enabled`] on every event.
//! The bell beside the clock (when Windows shows it) is inside the same button: its own enter and
//! leave are told too, and the app's hook sends its clicks to audio-tray's notifications.
//!
//! **Text** (`tap_proto::CLOCK_MAGIC`): once the app styles the clock, the TAP writes the shell's
//! two text blocks. Nothing can be added beside them (`Panel.Children` refuses writes, and the
//! shell resets the clock's slot when its content is replaced), and the shell rewrites them every
//! minute through compiled bindings: each text block calls back when its text changes ([`watch`]),
//! and the TAP puts its text back at once (where a block could not be watched, it looks every frame
//! through the 1.5 s after each minute instead). It holds the clock's width at the widest it has
//! drawn so the taskbar does not shift meanwhile. The unread count is a badge the app draws (an
//! accent pill with the count, a PNG at the taskbar's scale), painted on the bell's corner
//! ([`show_bell_badge`]), or without a bell as the clock stack's background left of the two lines
//! ([`show_badge`]); without a picture, an emoji dot near the accent and the count go before the
//! time.

use core::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use crate::log::logf;
use crate::winrt::{IDependencyPropertyChangedCallback, IDependencyPropertyChangedCallback_Impl};
use windows_core::implement;
use crate::{tree, xamlom};

/// The clock's content, the same on the main and the secondary taskbars.
const CONTENT: &str = "SystemTray.DateTimeIconContent";
/// The button around it (the bell's too, when shown), by `x:Name`.
const SECTION: &str = "NotificationCenterButton";
const TIME: &str = "TimeInnerTextBlock";
const DATE: &str = "DateInnerTextBlock";
const STACK: &str = "DateTimeStackPanel";

/// Text blocks that call back when their text changes ([`watch`]): never detached (the TAP stays
/// loaded), the callback checks there is text to put back. By handle and element: a handle can be
/// reused by a rebuilt taskbar's new block, which needs a callback of its own.
static WATCHED: Mutex<Vec<(u64, usize)>> = Mutex::new(Vec::new());
/// A text change has called back (said once, as proof it works).
static CALLED: AtomicBool = AtomicBool::new(false);
/// A block would not be watched (said once).
static REFUSED: AtomicBool = AtomicBool::new(false);

/// A clock's text changed (the shell's rewrite, or the TAP's own): look at once. The TAP's own
/// write comes from inside [`tick`], whose next look then replaces this one.
#[implement(IDependencyPropertyChangedCallback)]
struct TextChanged(());

impl IDependencyPropertyChangedCallback_Impl for TextChanged_Impl {
    unsafe fn Invoke(&self, _sender: *mut core::ffi::c_void, _dp: *mut core::ffi::c_void) -> windows_core::HRESULT {
        if std::panic::catch_unwind(|| {
            if !CALLED.swap(true, Ordering::SeqCst) {
                logf!("clock: text changes call back");
            }
            if crate::lock(&TEXT).is_some() {
                crate::lifecycle::arm_clock_timer(1);
            }
        })
        .is_err()
        {
            logf!("clock: text callback panicked");
        }
        windows_core::HRESULT(0)
    }
}

/// Have `block`'s text changes call back ([`TextChanged`]); `false` if XAML would not.
///
/// # Safety
/// XAML UI thread only.
unsafe fn watch(diagnostics: &xamlom::IXamlDiagnostics, block: u64) -> bool {
    use windows_core::Interface;
    // A handle whose element is gone (its removal never announced) has nothing to watch: it
    // must not hold the clock to looking every frame.
    let Some(object) = crate::decorate::element::<crate::winrt::IDependencyObject2>(diagnostics, block) else { return true };
    let identity = object.as_raw() as usize;
    if crate::lock(&WATCHED).contains(&(block, identity)) {
        return true;
    }
    let Some(statics) = crate::decorate::factory::<crate::winrt::ITextBlockStatics>("Windows.UI.Xaml.Controls.TextBlock") else { return false };
    let mut dp: *mut core::ffi::c_void = core::ptr::null_mut();
    if statics.get_TextProperty(&mut dp).is_err() || dp.is_null() {
        return false;
    }
    // Ours to release.
    let dp = windows_core::IUnknown::from_raw(dp);
    let callback: IDependencyPropertyChangedCallback = TextChanged(()).into();
    let mut token = 0i64;
    let ok = object.RegisterPropertyChangedCallback(dp.as_raw(), callback.as_raw(), &mut token).is_ok();
    if ok {
        let mut watched = crate::lock(&WATCHED);
        // Blocks gone (a taskbar rebuilt) are forgotten.
        let alive = tree::find_by_name(TIME).into_iter().chain(tree::find_by_name(DATE)).collect::<Vec<_>>();
        watched.retain(|(b, _)| alive.contains(b) && *b != block);
        watched.push((block, identity));
    } else if !REFUSED.swap(true, Ordering::SeqCst) {
        logf!("clock: a text block would not report its changes; looking every frame after each minute");
    }
    ok
}

static ENABLED: AtomicBool = AtomicBool::new(false);
/// Elements that already have the handlers.
static WIRED: Mutex<Vec<u64>> = Mutex::new(Vec::new());
/// The clock's section was found without the clock in it (and reported).
static MISSING: AtomicBool = AtomicBool::new(false);

/// Turned off with the button split: [`wire`] puts it back whole at the next sweep (this runs off
/// the XAML thread).
static UNSPLIT_PENDING: AtomicBool = AtomicBool::new(false);

pub fn set_enabled(on: bool) {
    if ENABLED.swap(on, Ordering::SeqCst) != on {
        logf!("clock: {}", if on { "clicks open audio-tray's calendar" } else { "left to the shell" });
        UNSPLIT_PENDING.store(!on && !crate::lock(&SPLIT).is_empty(), Ordering::SeqCst);
    }
}

/// Whether a click on the clock is ours: asked for, the TAP live, and the app there to take it.
pub fn enabled() -> bool {
    ENABLED.load(Ordering::SeqCst) && crate::live() && crate::ipc::receiver_present()
}

/// What [`enabled`] read, for the log.
pub fn why() -> String {
    format!("asked {}, live {}, app there {}", ENABLED.load(Ordering::SeqCst), crate::live(), crate::ipc::receiver_present())
}

/// What the handlers go on: the button around each clock.
fn targets() -> Vec<u64> {
    tree::find_by_name(SECTION)
}

/// The bell beside each clock (Windows' "Show notification bell icon"), inside the clock's button.
/// None while Windows hides it.
fn bells() -> Vec<u64> {
    targets().into_iter().filter_map(bell_of).collect()
}

/// The bell in the clock's button `section`: of its two icon views, the one that does not hold the
/// clock (measured on 26300: it holds a `SystemTray.TextIconContent`, the bell's glyph).
fn bell_of(section: u64) -> Option<u64> {
    descendants(section).into_iter().find(|h| tree::type_of(*h).as_deref() == Some(ICON) && !holds_clock(*h))
}

/// The clock's own icon view in `section`.
fn clock_of(section: u64) -> Option<u64> {
    descendants(section).into_iter().find(|h| tree::type_of(*h).as_deref() == Some(ICON) && holds_clock(*h))
}

fn holds_clock(icon: u64) -> bool {
    descendants(icon).iter().any(|d| tree::type_of(*d).as_deref() == Some(CONTENT))
}

/// The first element named `name` under `root`, nearest first.
fn named(root: u64, name: &str) -> Option<u64> {
    descendants(root).into_iter().find(|h| tree::name_of(*h).as_deref() == Some(name))
}

/// An icon of the tray (the clock's, the bell's).
const ICON: &str = "SystemTray.IconView";

/// Every recorded element under `root`, nearest first (bounded: the button holds a few dozen).
fn descendants(root: u64) -> Vec<u64> {
    // Kept until the tree grows: each clock tick asks for the same few (the bell's, a section's)
    // several times, and a walk locks the tree and sorts at every node. A handle gone since is
    // harmless (reading through it finds nothing); a part rebuilt adds elements, so a new walk.
    /// The tree's version, then each root walked and what was under it.
    type Found = (u64, Vec<(u64, Vec<u64>)>);
    static FOUND: Mutex<Found> = Mutex::new((u64::MAX, Vec::new()));
    let version = tree::version();
    let mut found = crate::lock(&FOUND);
    if found.0 != version {
        *found = (version, Vec::new());
    }
    if let Some((_, kept)) = found.1.iter().find(|(r, _)| *r == root) {
        return kept.clone();
    }
    let walked = walk(root);
    found.1.push((root, walked.clone()));
    walked
}

/// Every element under `root`, breadth first (400 at most).
fn walk(root: u64) -> Vec<u64> {
    let mut found = Vec::new();
    let mut queue = std::collections::VecDeque::from(tree::children_of(root));
    while let Some(handle) = queue.pop_front() {
        if found.len() >= 400 {
            break;
        }
        found.push(handle);
        queue.extend(tree::children_of(handle));
    }
    found
}

/// Nothing left to wire: off, or every clock's button (and bell) has its handlers.
pub fn settled() -> bool {
    if !ENABLED.load(Ordering::SeqCst) {
        return !UNSPLIT_PENDING.load(Ordering::SeqCst);
    }
    let targets = targets();
    if targets.is_empty() || tree::find_by_type(CONTENT).is_empty() {
        // Not on this build (reported once): nothing to wait for.
        return MISSING.load(Ordering::SeqCst);
    }
    let parts: Vec<u64> = targets.iter().filter(|s| bell_of(**s).is_some()).filter_map(|s| clock_of(*s)).chain(bells()).collect();
    // A button that could not be split (reported once) counts as done: retrying it every sweep
    // would only sweep forever.
    let failed = crate::lock(&SPLIT_FAILED).clone();
    let split = targets.iter().filter(|s| bell_of(**s).is_some()).all(|s| failed.contains(s) || crate::lock(&SPLIT).iter().any(|p| p.section == *s));
    let wired = crate::lock(&WIRED);
    split && targets.iter().chain(parts.iter()).all(|t| wired.contains(t))
}

/// Attach the handlers to every clock's button not yet wired.
///
/// # Safety
/// XAML UI thread only, behind the wiring gate.
pub unsafe fn wire(diagnostics: &xamlom::IXamlDiagnostics) {
    if !ENABLED.load(Ordering::SeqCst) {
        // Turned off while live: the button whole again and the bell the shell's.
        if UNSPLIT_PENDING.swap(false, Ordering::SeqCst) {
            restore(diagnostics);
            unsplit(diagnostics);
        }
        return;
    }
    // Elements gone from the tree (a taskbar rebuilt) are forgotten: their handles can come back.
    crate::lock(&WIRED).retain(|h| tree::type_of(*h).is_some());
    if tree::find_by_type(CONTENT).is_empty() {
        report_missing();
        return;
    }
    for target in targets() {
        if crate::lock(&WIRED).contains(&target) {
            continue;
        }
        // Recorded either way: a failure is logged once, not retried every sweep.
        crate::lock(&WIRED).push(target);
        let wired = crate::interact::attach_clock(diagnostics, target);
        logf!("clock: {} 0x{target:x} ({})", if wired { "wired" } else { "could not wire" }, tree::type_of(target).unwrap_or_default());
    }
    for section in targets() {
        let Some(bell) = bell_of(section) else { continue };
        // The clock and the bell: each lights on its own (see [`split`]), the bell's clicks go to
        // the notifications.
        for (icon, is_bell) in [(clock_of(section), false), (Some(bell), true)] {
            let Some(icon) = icon else { continue };
            if crate::lock(&WIRED).contains(&icon) {
                continue;
            }
            crate::lock(&WIRED).push(icon);
            let wired = crate::interact::attach_part(diagnostics, icon, is_bell);
            logf!("clock: {} {} 0x{icon:x}", if is_bell { "bell" } else { "clock icon" }, if wired { "wired" } else { "could not be wired" });
        }
        if !DESCRIBED.swap(true, Ordering::SeqCst) {
            describe(bell, "the bell");
        }
        split(diagnostics, section);
    }
}

// ---- Two buttons: the clock and the bell lit apart ----
//
// The shell draws one plate (`Border#BackgroundBorder`) behind the whole button, clock and bell
// together. While the bell is the notifications' button, that plate is hidden and each icon's grid
// becomes a plate of its own (a grid's background and corners are property values): inset as the
// shell's, its corners, a gap between them. Lit under the pointer, and while its card is open.

/// The shell's plate behind the whole button.
const PLATE: &str = "BackgroundBorder";
/// Each icon's outer grid: its plate.
const ICON_GRID: &str = "ContainerGrid";
/// The bell's inner grid: its badge.
const INNER_GRID: &str = "ContentGrid";
/// The room between the two plates, in DIPs.
const PLATE_GAP: f64 = 2.0;

/// A button split in two, and what was there before (put back by [`restore`]).
struct Split {
    section: u64,
    plate: u64,
    /// Each icon's grid, with its margin and corners before.
    grids: Vec<(u64, crate::winrt::Thickness, crate::winrt::CornerRadius)>,
    clock_grid: u64,
    bell_grid: u64,
    /// The bell's grid's minimum width before it was split, and its width then.
    bell_min: f64,
    bell_width: f64,
}

/// What the bell is widened by while it wears a badge, in DIPs: the badge sits beside the glyph,
/// at its upper right (a grid's background is drawn under its children, so it cannot go over it).
const BELL_ROOM: f64 = 10.0;
/// The bell's glyph, moved left by the room while the badge shows.
const BELL_GLYPH: &str = "SystemTray.TextIconContent";

static SPLIT: Mutex<Vec<Split>> = Mutex::new(Vec::new());
/// Sections whose button could not be split (missing parts): left whole, not retried.
static SPLIT_FAILED: Mutex<Vec<u64>> = Mutex::new(Vec::new());

/// `section` left whole: what was missing, logged once.
fn split_failed(section: u64, missing: &str) {
    let mut failed = crate::lock(&SPLIT_FAILED);
    if !failed.contains(&section) {
        logf!("clock: the button cannot be split ({missing} missing); left whole");
        failed.push(section);
    }
}

/// The icon grid lit as under the pointer.
static HOVERED: Mutex<Option<u64>> = Mutex::new(None);
/// The part (icon, and whether it is the bell) the pointer is on, or was last on while still in
/// the button: the strips beside and between the parts belong to the nearest one.
static PART: Mutex<Option<(u64, bool)>> = Mutex::new(None);
/// The pointer is in the clock's button (anywhere in it).
static SECTION_OVER: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
/// Which card audio-tray has open (`tap_proto::OPEN_*`).
static OPEN: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(tap_proto::OPEN_NONE);
thread_local! {
    /// Tooltips hidden while a panel is open (the XAML thread's objects).
    static TOOLTIPS: std::cell::RefCell<Vec<Hidden>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// A tooltip hidden while a panel is open: its owner, the tooltip, and whether it was faded out
/// (else taken off the owner).
struct Hidden {
    owner: u64,
    tip: windows_core::IInspectable,
    faded: bool,
}

/// Split `section`'s button in two (once).
///
/// # Safety
/// XAML UI thread only.
unsafe fn split(diagnostics: &xamlom::IXamlDiagnostics, section: u64) {
    use crate::winrt::{CornerRadius, IBorder, IFrameworkElement, IGrid2, Thickness};
    use windows_core::Interface;
    if crate::lock(&SPLIT).iter().any(|s| s.section == section) || crate::lock(&SPLIT_FAILED).contains(&section) {
        return;
    }
    let (Some(plate), Some(clock), Some(bell)) = (named(section, PLATE), clock_of(section), bell_of(section)) else {
        return split_failed(section, "the plate, the clock or the bell");
    };
    let (Some(clock_grid), Some(bell_grid)) = (named(clock, ICON_GRID), named(bell, ICON_GRID)) else {
        return split_failed(section, "an icon grid");
    };
    // The shell's plate: its inset and its corners, for ours.
    let mut margin = Thickness::default();
    let mut radius = CornerRadius::default();
    if let Some(border) = crate::decorate::element::<IFrameworkElement>(diagnostics, plate) {
        let _ = border.get_Margin(&mut margin);
    }
    if let Some(border) = crate::decorate::element::<IBorder>(diagnostics, plate) {
        let _ = border.get_CornerRadius(&mut radius as *mut CornerRadius as *mut core::ffi::c_void);
    }
    if radius.top_left <= 0.0 {
        radius = CornerRadius { top_left: 4.0, top_right: 4.0, bottom_right: 4.0, bottom_left: 4.0 };
    }
    let sizes = |h: u64| crate::decorate::actual_size(diagnostics, h).unwrap_or_default();
    logf!(
        "clock: splitting the button: plate {:?} margin {margin:?} radius {}, clock grid {:?}, bell grid {:?}",
        sizes(plate),
        radius.top_left,
        sizes(clock_grid),
        sizes(bell_grid)
    );
    // The plate's edge, fixed from the start: on a grid a border takes room from what it holds, so
    // one that came and went with the pointer moved the icons (and the taskbar) on every hover.
    // Taken out of the margin, so the button keeps its size.
    let mut edge = Thickness::default();
    if let Some(border) = crate::decorate::element::<IBorder>(diagnostics, plate) {
        let _ = border.get_BorderThickness(&mut edge as *mut Thickness as *mut core::ffi::c_void);
    }
    let edge = edge.top.max(edge.left).max(1.0);
    // Both grids found (and their looks saved) before either is written: half a split could not
    // be put back.
    let mut found = Vec::new();
    for (grid, left, right) in [(clock_grid, margin.left, PLATE_GAP / 2.0), (bell_grid, PLATE_GAP / 2.0, margin.right)] {
        let (Some(framework), Some(rounded)) = (crate::decorate::element::<IFrameworkElement>(diagnostics, grid), crate::decorate::element::<IGrid2>(diagnostics, grid)) else {
            return split_failed(section, "an icon grid's interfaces");
        };
        let mut was_margin = Thickness::default();
        let mut was_radius = CornerRadius::default();
        let _ = framework.get_Margin(&mut was_margin);
        let _ = rounded.get_CornerRadius(&mut was_radius);
        found.push((grid, left, right, framework, rounded, was_margin, was_radius));
    }
    let mut grids = Vec::new();
    for (grid, left, right, framework, rounded, was_margin, was_radius) in found {
        let _ = framework.put_Margin(Thickness { left: left - edge, top: margin.top - edge, right: right - edge, bottom: margin.bottom - edge });
        let _ = rounded.put_BorderThickness(Thickness { left: edge, top: edge, right: edge, bottom: edge });
        let _ = rounded.put_CornerRadius(radius);
        grids.push((grid, was_margin, was_radius));
    }
    // The bell's width as the shell lays it out, widened while it wears a badge ([`show_bell_badge`]).
    let mut bell_min = 0.0;
    if let Some(framework) = crate::decorate::element::<IFrameworkElement>(diagnostics, bell_grid) {
        let _ = framework.get_MinWidth(&mut bell_min);
    }
    let bell_width = crate::decorate::actual_width(diagnostics, bell_grid).unwrap_or(0.0).max(bell_min);
    // Filled and faded as the shell's plate is.
    if let Some(shell) = crate::decorate::element::<crate::winrt::IBorder2>(diagnostics, plate) {
        let mut sizing = 0i32;
        let _ = shell.get_BackgroundSizing(&mut sizing);
        let mut transition: *mut core::ffi::c_void = core::ptr::null_mut();
        let _ = shell.get_BackgroundTransition(&mut transition);
        // Ours to release.
        let transition = (!transition.is_null()).then(|| windows_core::IUnknown::from_raw(transition));
        for grid in [clock_grid, bell_grid] {
            if let Some(sized) = crate::decorate::element::<crate::winrt::IGrid4>(diagnostics, grid) {
                let _ = sized.put_BackgroundSizing(sizing);
            }
            // A transition of its own each (one instance cannot serve two elements).
            let own = transition.as_ref().and_then(|_| crate::decorate::load_xaml(r#"<BrushTransition xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation" Duration="0:0:0.083"/>"#));
            if let (Some(fading), Some(own)) = (crate::decorate::element::<crate::winrt::IPanel2>(diagnostics, grid), own) {
                let _ = fading.put_BackgroundTransition(windows_core::Interface::as_raw(&own));
            }
        }
        logf!("clock: the shell's plate: sizing {sizing}, fades {}", transition.is_some());
    }
    watch_plate(diagnostics, plate);
    crate::decorate::set_opacity(diagnostics, plate, 0.0);
    crate::lock(&SPLIT).push(Split { section, plate, grids, clock_grid, bell_grid, bell_min, bell_width });
    light(diagnostics);
}

/// The plate's change callback was seen (said once).
static PLATE_CALLED: AtomicBool = AtomicBool::new(false);

/// The shell's plate changed its look (the pointer came or went): the icons' plates follow.
#[implement(IDependencyPropertyChangedCallback)]
struct PlateChanged(());

impl IDependencyPropertyChangedCallback_Impl for PlateChanged_Impl {
    unsafe fn Invoke(&self, _sender: *mut core::ffi::c_void, _dp: *mut core::ffi::c_void) -> windows_core::HRESULT {
        if std::panic::catch_unwind(|| {
            if !PLATE_CALLED.swap(true, Ordering::SeqCst) {
                logf!("clock: the shell's plate reports its changes");
            }
            if let Some(diagnostics) = crate::diagnostics() {
                light(&diagnostics);
            }
        })
        .is_err()
        {
            logf!("clock: plate callback panicked");
        }
        windows_core::HRESULT(0)
    }
}

/// Have the shell's plate call back when its background or border changes. Never detached (the
/// callback only lights what is split).
///
/// # Safety
/// XAML UI thread only.
unsafe fn watch_plate(diagnostics: &xamlom::IXamlDiagnostics, plate: u64) {
    use windows_core::Interface;
    /// Plates already watched: a re-split after a stand-down (callbacks never detached) must not
    /// pile another pair on.
    static WATCHED: Mutex<Vec<u64>> = Mutex::new(Vec::new());
    {
        let mut watched = crate::lock(&WATCHED);
        watched.retain(|h| tree::type_of(*h).is_some());
        if watched.contains(&plate) {
            return;
        }
        watched.push(plate);
    }
    let Some(object) = crate::decorate::element::<crate::winrt::IDependencyObject2>(diagnostics, plate) else { return };
    let Some(statics) = crate::decorate::factory::<crate::winrt::IBorderStatics>(crate::winrt::BORDER) else { return };
    let mut ok = true;
    for get in [crate::winrt::IBorderStatics::get_BackgroundProperty, crate::winrt::IBorderStatics::get_BorderBrushProperty] {
        let mut dp: *mut core::ffi::c_void = core::ptr::null_mut();
        if get(&statics, &mut dp).is_err() || dp.is_null() {
            ok = false;
            continue;
        }
        let dp = windows_core::IUnknown::from_raw(dp);
        let callback: IDependencyPropertyChangedCallback = PlateChanged(()).into();
        let mut token = 0i64;
        ok &= object.RegisterPropertyChangedCallback(dp.as_raw(), callback.as_raw(), &mut token).is_ok();
    }
    if !ok {
        logf!("clock: the shell's plate would not report its changes; the icons' plates use their own wash");
    }
}

/// Put the button back whole, its tooltips with it (a revert).
///
/// # Safety
/// XAML UI thread only.
pub unsafe fn unsplit(diagnostics: &xamlom::IXamlDiagnostics) {
    OPEN.store(tap_proto::OPEN_NONE, Ordering::SeqCst);
    quiet_tooltips(diagnostics, false);
    use crate::winrt::{IFrameworkElement, IGrid2, IPanel};
    crate::lock(&SPLIT_FAILED).clear();
    for split in std::mem::take(&mut *crate::lock(&SPLIT)) {
        crate::decorate::set_opacity(diagnostics, split.plate, 1.0);
        if let Some(framework) = crate::decorate::element::<IFrameworkElement>(diagnostics, split.bell_grid) {
            let _ = framework.put_MinWidth(split.bell_min);
        }
        let glyph = bell_of(split.section).and_then(|bell| descendants(bell).into_iter().find(|h| tree::type_of(*h).as_deref() == Some(BELL_GLYPH)));
        if let Some(framework) = glyph.and_then(|g| crate::decorate::element::<IFrameworkElement>(diagnostics, g)) {
            let _ = framework.put_Margin(crate::winrt::Thickness::default());
        }
        for (grid, margin, radius) in split.grids {
            if let Some(framework) = crate::decorate::element::<IFrameworkElement>(diagnostics, grid) {
                let _ = framework.put_Margin(margin);
            }
            if let Some(rounded) = crate::decorate::element::<IGrid2>(diagnostics, grid) {
                let _ = rounded.put_CornerRadius(radius);
                let _ = rounded.put_BorderThickness(crate::winrt::Thickness::default());
                let _ = rounded.put_BorderBrush(core::ptr::null_mut());
            }
            if let Some(panel) = crate::decorate::element::<IPanel>(diagnostics, grid) {
                let _ = panel.put_Background(core::ptr::null_mut());
            }
        }
    }
    *crate::lock(&HOVERED) = None;
    *crate::lock(&PART) = None;
}

/// The bell's ring: the glyph's sideways offsets (DIPs), one per [`RING_MS`], a swing dying down.
const RING: [f64; 9] = [2.5, -2.5, 2.0, -2.0, 1.4, -1.4, 0.8, -0.6, 0.0];
pub const RING_MS: u32 = 45;
pub const RING_TIMER: usize = 4;
/// The ring's next step (`RING.len()`: none running).
static RING_STEP: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(RING.len());

/// A notification went to the list: the bell rings ([`ring_step`] from the control window's
/// timer). Only the glyph moves, within its own room: nothing else on the taskbar does.
pub fn ring() {
    RING_STEP.store(0, Ordering::SeqCst);
}

/// One step of the ring; `false` once it has died down.
///
/// # Safety
/// XAML UI thread only.
pub unsafe fn ring_step() -> bool {
    let step = RING_STEP.load(Ordering::SeqCst);
    let Some(&offset) = RING.get(step) else { return false };
    RING_STEP.store(step + 1, Ordering::SeqCst);
    let Some(diagnostics) = crate::diagnostics() else { return false };
    for bell in bells() {
        let Some(glyph) = descendants(bell).into_iter().find(|h| tree::type_of(*h).as_deref() == Some(BELL_GLYPH)) else { continue };
        // The badge's room stays as [`show_bell_badge`] set it: the swing is on top of it.
        let room = named(bell, INNER_GRID).filter(|g| crate::lock(&BADGED).iter().any(|(h, k)| h == g && !k.is_empty())).map_or(0.0, |_| BELL_ROOM);
        if let Some(framework) = crate::decorate::element::<crate::winrt::IFrameworkElement>(&diagnostics, glyph) {
            let _ = framework.put_Margin(crate::winrt::Thickness { left: offset, right: room - offset, ..Default::default() });
        }
    }
    step + 1 < RING.len()
}

/// The pointer entered or left an icon (the clock's or the bell's): its plate follows.
pub fn part_hover(icon: u64, bell: bool, over: bool) {
    let mut part = crate::lock(&PART);
    if over {
        // Another part was lit (the pointer crossed the strip between them): it is no longer.
        if let Some((_, was_bell)) = part.replace((icon, bell)) {
            if was_bell && !bell {
                crate::ipc::send_bell_hover(false);
            }
        }
        if bell {
            crate::ipc::send_bell_hover(true);
        }
    } else if !SECTION_OVER.load(Ordering::SeqCst) && part.is_some_and(|(i, _)| i == icon) {
        *part = None;
        if bell {
            crate::ipc::send_bell_hover(false);
        }
    }
    // Left for the strip beside it, still in the button: it stays lit, as the shell's one plate
    // did over the whole button. It goes when the pointer leaves the button ([`section_hover`]).
    drop(part);
    relight();
}

/// The pointer entering or leaving the clock's button as a whole (both parts and the strips
/// around and between them). Entered on a strip, not on a part (from Show desktop, from above):
/// the bell lights, the nearest part there is on the strip at its right.
pub fn section_hover(section: u64, over: bool) {
    SECTION_OVER.store(over, Ordering::SeqCst);
    let mut part = crate::lock(&PART);
    if over {
        if part.is_none() {
            if let Some(bell) = bell_of(section) {
                *part = Some((bell, true));
                crate::ipc::send_bell_hover(true);
            }
        }
    } else if let Some((_, bell)) = part.take() {
        if bell {
            crate::ipc::send_bell_hover(false);
        }
    }
    drop(part);
    relight();
}

/// The part to light, from [`PART`], lit.
fn relight() {
    let grid = crate::lock(&PART).and_then(|(icon, _)| named(icon, ICON_GRID));
    *crate::lock(&HOVERED) = grid;
    if let Some(diagnostics) = crate::diagnostics() {
        unsafe { light(&diagnostics) };
    }
}

/// audio-tray opened a panel, swapped it, or closed it (`tap_proto::OPEN_*`): the calendar's or the
/// notifications' button stays lit, and no tooltip of ours (strip, clock, bell) shows over the
/// panel meanwhile.
pub fn set_panel(open: usize) {
    OPEN.store(open, Ordering::SeqCst);
    let Some(diagnostics) = crate::diagnostics() else { return };
    unsafe {
        light(&diagnostics);
        quiet_tooltips(&diagnostics, open != tap_proto::OPEN_NONE);
    }
}

/// The shell's plate's look while the pointer was over it: its brush, border brush and border
/// width. Kept for the open card's button once the pointer has gone (the shell's plate is at rest
/// then). XAML thread's, as the brushes are.
#[derive(Clone)]
struct Look {
    background: windows_core::IUnknown,
    border: Option<windows_core::IUnknown>,
    thickness: crate::winrt::Thickness,
}

thread_local! {
    static LIT_LOOK: std::cell::RefCell<Option<Look>> = const { std::cell::RefCell::new(None) };
}

/// The shell's plate's look now (`None`: no background, at rest).
///
/// # Safety
/// XAML UI thread only.
unsafe fn plate_look(diagnostics: &xamlom::IXamlDiagnostics, plate: u64) -> Option<Look> {
    use crate::winrt::{IBorder, Thickness};
    use windows_core::Interface;
    let border = crate::decorate::element::<IBorder>(diagnostics, plate)?;
    let mut raw: *mut core::ffi::c_void = core::ptr::null_mut();
    if border.get_Background(&mut raw).is_err() || raw.is_null() {
        return None;
    }
    let background = windows_core::IUnknown::from_raw(raw);
    let mut raw: *mut core::ffi::c_void = core::ptr::null_mut();
    let _ = border.get_BorderBrush(&mut raw);
    let border_brush = (!raw.is_null()).then(|| windows_core::IUnknown::from_raw(raw));
    let mut thickness = Thickness::default();
    let _ = border.get_BorderThickness(&mut thickness as *mut Thickness as *mut core::ffi::c_void);
    Some(Look { background, border: border_brush, thickness })
}

/// Each split button's plates, lit as they should be now: the icon under the pointer wears exactly
/// what the shell's own plate wears at that moment (so it matches every other taskbar button, in
/// any theme), and the open card's icon the look last seen under the pointer.
///
/// # Safety
/// XAML UI thread only.
unsafe fn light(diagnostics: &xamlom::IXamlDiagnostics) {
    use crate::winrt::{IGrid2, IPanel};
    use windows_core::Interface;
    let hovered = *crate::lock(&HOVERED);
    let open = OPEN.load(Ordering::SeqCst);
    for split in crate::lock(&SPLIT).iter() {
        let now = plate_look(diagnostics, split.plate);
        if hovered.is_some() {
            if let Some(look) = &now {
                LIT_LOOK.with(|l| *l.borrow_mut() = Some(look.clone()));
            }
        }
        let remembered = LIT_LOOK.with(|l| l.borrow().clone());
        for (grid, part) in [(split.clock_grid, tap_proto::OPEN_CALENDAR), (split.bell_grid, tap_proto::OPEN_NOTIFICATIONS)] {
            let look = if hovered == Some(grid) {
                now.clone()
            } else if open == part {
                remembered.clone().or_else(|| fallback_look())
            } else {
                None
            };
            let (Some(panel), Some(edged)) = (crate::decorate::element::<IPanel>(diagnostics, grid), crate::decorate::element::<IGrid2>(diagnostics, grid)) else { continue };
            // Brushes only: the edge's width is fixed ([`split`]), so nothing moves.
            match look {
                Some(look) => {
                    let _ = panel.put_Background(look.background.as_raw());
                    let edge = look.border.as_ref().filter(|_| look.thickness.top > 0.0 || look.thickness.left > 0.0);
                    let _ = edged.put_BorderBrush(edge.map_or(core::ptr::null_mut(), |b| b.as_raw()));
                }
                None => {
                    let _ = panel.put_Background(core::ptr::null_mut());
                    let _ = edged.put_BorderBrush(core::ptr::null_mut());
                }
            }
        }
    }
}

/// A card opened before the pointer was ever over the clock (Win+N): a wash near the shell's.
///
/// # Safety
/// XAML UI thread only.
unsafe fn fallback_look() -> Option<Look> {
    use windows_core::Interface;
    let color = if light_taskbar() { "#0A000000" } else { "#12FFFFFF" };
    let markup = format!(r#"<SolidColorBrush xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation" Color="{color}"/>"#);
    let brush = crate::decorate::load_xaml(&markup)?;
    Some(Look { background: brush.cast().ok()?, border: None, thickness: crate::winrt::Thickness::default() })
}

/// Whether the taskbar is light (Windows' "default app mode" is the apps'; the taskbar follows the
/// system's).
fn light_taskbar() -> bool {
    use windows::core::w;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
    let mut value = 0u32;
    let mut size = 4u32;
    let read = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
            w!("SystemUsesLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut core::ffi::c_void),
            Some(&mut size),
        )
    };
    read.is_ok() && value != 0
}

/// Hide the tooltips of the clock's button, its icons and what lies between, and of our strip's
/// icon (`quiet`), or show again those hidden: no tooltip of ours shows over an open panel.
///
/// A tooltip that is an element is faded out (opacity 0) and stays attached: taking it off its
/// owner while the pointer was on it (the click that opened the audio panel) left that owner's
/// tooltip broken after it was put back. Only one that is not an element (a bare value) is taken
/// off and put back.
///
/// # Safety
/// XAML UI thread only.
unsafe fn quiet_tooltips(diagnostics: &xamlom::IXamlDiagnostics, quiet: bool) {
    use crate::winrt::{IDependencyObject, IToolTipServiceStatics, IUIElement, TOOLTIP_SERVICE};
    use windows_core::Interface;
    let Some(service) = crate::decorate::factory::<IToolTipServiceStatics>(TOOLTIP_SERVICE) else { return };
    if !quiet {
        for hidden in TOOLTIPS.with(|t| std::mem::take(&mut *t.borrow_mut())) {
            match hidden.tip.cast::<IUIElement>() {
                Ok(faded) if hidden.faded => {
                    let _ = faded.put_Opacity(1.0);
                }
                _ => {
                    if let Some(object) = crate::decorate::element::<IDependencyObject>(diagnostics, hidden.owner) {
                        let _ = service.SetToolTip(object.as_raw(), hidden.tip.as_raw());
                    }
                }
            }
        }
        return;
    }
    let mut hidden = TOOLTIPS.with(|t| std::mem::take(&mut *t.borrow_mut()));
    // The strip's notify icon (its tooltip is the shell's: the device and "Audio Tray").
    let strip = crate::decorated_pair().map(|(icon, _)| icon);
    for root in targets().into_iter().chain(strip) {
        for owner in std::iter::once(root).chain(descendants(root)) {
            if hidden.iter().any(|h| h.owner == owner) {
                continue;
            }
            let Some(object) = crate::decorate::element::<IDependencyObject>(diagnostics, owner) else { continue };
            let mut raw: *mut core::ffi::c_void = core::ptr::null_mut();
            if service.GetToolTip(object.as_raw(), &mut raw).is_err() || raw.is_null() {
                continue;
            }
            // Ours to release, or to give back.
            let tip = windows_core::IInspectable::from_raw(raw);
            let faded = tip.cast::<IUIElement>().is_ok_and(|element| element.put_Opacity(0.0).is_ok());
            if faded || service.SetToolTip(object.as_raw(), core::ptr::null_mut()).is_ok() {
                hidden.push(Hidden { owner, tip, faded });
            }
        }
    }
    TOOLTIPS.with(|t| *t.borrow_mut() = hidden);
}

/// The bell's inside was logged (once per shell, for placing the badge on another build).
static DESCRIBED: AtomicBool = AtomicBool::new(false);

/// Once per shell: what the clock's section holds, when the clock's class is not in it (another
/// Windows build), so the next fix knows what to look for.
fn report_missing() {
    let Some(section) = tree::find_by_name(SECTION).into_iter().next() else { return };
    if MISSING.swap(true, Ordering::SeqCst) {
        return;
    }
    describe(section, &format!("no {CONTENT} recorded; {SECTION}"));
}

/// Log the recorded tree under `root`, as `what` holds it.
fn describe(root: u64, what: &str) {
    logf!("clock: {what} holds:");
    let mut queue = vec![(root, 0usize)];
    let mut listed = 0;
    while let Some((handle, depth)) = queue.pop() {
        if listed >= 80 {
            break;
        }
        listed += 1;
        let name = tree::name_of(handle).filter(|n| !n.is_empty()).map(|n| format!("#{n}")).unwrap_or_default();
        logf!("clock:   {}{}{name}", "  ".repeat(depth), tree::type_of(handle).unwrap_or_default());
        queue.extend(tree::children_of(handle).into_iter().map(|c| (c, depth + 1)));
    }
}

// ---- The clock's text, when the app styles it ----

/// What the clock says, from the app. Patterns are `GetTimeFormatEx` / `GetDateFormatEx` ones;
/// empty means the locale's short time or date.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Text {
    time: String,
    /// `-` hides the date.
    date: String,
    /// The date after the time, on one line.
    one_line: bool,
    /// How many notifications wait in audio-tray's list (0: none).
    badge: u32,
    /// The badge drawn by the app (a PNG), its width in DIPs and its pixels per DIP; empty: the
    /// emoji dot and the count in the text instead.
    badge_image: String,
    badge_width: f64,
    badge_scale: f64,
    /// Windows keeps notifications quiet: the bell is drawn crossed.
    quiet: bool,
    /// Anything waits in audio-tray's list: the bell filled (`None`: the shell's fill).
    waiting: Option<bool>,
}

/// The app's latest word; `None` when it never styled the clock or has gone.
static TEXT: Mutex<Option<Text>> = Mutex::new(None);
/// The app handed the clock back: the next tick undoes the TAP's changes, then the timer stops.
static RELEASING: AtomicBool = AtomicBool::new(false);
/// Each clock's width held (stack handle, DIPs): the widest it has drawn since the text last
/// changed shape. Empty: measured afresh. Per clock, as each taskbar's clock has its own width.
static HELD: Mutex<Vec<(u64, f64)>> = Mutex::new(Vec::new());
/// Date blocks the TAP hid (a hidden date, one line), shown again when it lets go.
static HIDDEN: Mutex<Vec<u64>> = Mutex::new(Vec::new());

/// Control-window timer that updates the text, at each minute (each second with seconds shown).
pub const TEXT_TIMER: usize = 3;

/// The app's new text settings (init-data encoded).
pub fn set_text(data: &str) {
    let flag = |key: &str| tap_proto::init_value(data, key).as_deref() == Some("1");
    if flag("release") {
        // The app has nothing to show: the clock goes back to the shell at the next tick.
        if crate::lock(&TEXT).take().is_some() {
            logf!("clock: handed back to the shell");
            RELEASING.store(true, Ordering::SeqCst);
            crate::lifecycle::arm_clock_timer(1);
        }
        return;
    }
    let value = |key: &str| tap_proto::init_value(data, key).unwrap_or_default();
    let text = Text {
        time: value("time"),
        date: value("date"),
        one_line: flag("oneline"),
        badge: value("badge").parse().unwrap_or(0),
        badge_image: value("badge_image"),
        badge_width: value("badge_width").parse().unwrap_or(0.0),
        badge_scale: value("badge_scale").parse().unwrap_or(1.0),
        quiet: flag("quiet"),
        waiting: match value("waiting").as_str() {
            "1" => Some(true),
            "0" => Some(false),
            _ => None,
        },
    };
    let mut held = crate::lock(&TEXT);
    if held.as_ref() != Some(&text) {
        logf!("clock: text now {text:?}");
        // A new shape: the width is measured afresh.
        crate::lock(&HELD).clear();
        *held = Some(text);
    }
    drop(held);
    crate::lifecycle::arm_clock_timer(1);
}

pub fn keep_text() -> Option<Text> {
    crate::lock(&TEXT).clone()
}

/// The app's text, kept across a revert that re-applies everything for the same app.
pub fn put_back_text(text: Option<Text>) {
    *crate::lock(&TEXT) = text;
    crate::lifecycle::arm_clock_timer(1);
}

/// The app is gone: the shell writes its own text again at the next minute.
pub fn release_text() {
    *crate::lock(&TEXT) = None;
}

/// Let go of the clock's width (a revert).
///
/// # Safety
/// XAML UI thread only.
pub unsafe fn restore(diagnostics: &xamlom::IXamlDiagnostics) {
    crate::lock(&HELD).clear();
    for stack in tree::find_by_name(STACK) {
        set_min_width(diagnostics, stack, 0.0);
        show_badge(diagnostics, stack, None);
    }
    // What is left wears it on a bell, which gives its room back, and its own glyph.
    for bell in bells() {
        show_bell_badge(diagnostics, bell, None);
        uncross_bell(diagnostics, bell);
        // Its fill as the shell has it again.
        fill_bell(diagnostics, bell, None);
    }
    crate::lock(&BADGED).clear();
    // The shell's bindings rewrite the date's text, never its visibility.
    for block in std::mem::take(&mut *crate::lock(&HIDDEN)) {
        crate::decorate::set_visible(diagnostics, block, true);
    }
    // The shell's own text at once (its locale's time and date), not the TAP's badge or dot until
    // the shell next writes.
    let st = now();
    for block in tree::find_by_name(TIME) {
        crate::decorate::set_text(diagnostics, block, &format_time(&st, ""));
    }
    for block in tree::find_by_name(DATE) {
        crate::decorate::set_text(diagnostics, block, &format_date(&st, ""));
    }
}

/// The room between the badge and the text, in DIPs.
const BADGE_GAP: f64 = 6.0;
/// Clocks wearing a badge, and which image.
static BADGED: Mutex<Vec<(u64, String)>> = Mutex::new(Vec::new());

/// Draw the app's badge (`image`, `width` DIPs wide, at `scale` pixels per DIP) left of the clock's
/// two lines, or take it off. It is the stack's background (a brush is a property value, which the
/// shell takes where it refuses new children), with padding making room for it. The image is shown
/// at its pixel size and scaled back by `1 / scale` about the stack's middle, so it lands pixel for
/// pixel and centred on both lines.
///
/// # Safety
/// XAML UI thread only.
unsafe fn show_badge(diagnostics: &xamlom::IXamlDiagnostics, stack: u64, image: Option<(&str, f64, f64)>) {
    use crate::winrt::{IPanel, IStackPanel2, Thickness};
    let Some(panel) = crate::decorate::element::<IPanel>(diagnostics, stack) else { return };
    let Some(padded) = crate::decorate::element::<IStackPanel2>(diagnostics, stack) else { return };
    let mut padding = Thickness::default();
    let _ = padded.get_Padding(&mut padding);
    let left = image.map_or(0.0, |(_, width, _)| width + BADGE_GAP);
    let key = image.map(|(path, ..)| path.to_string()).unwrap_or_default();
    let mut badged = crate::lock(&BADGED);
    let current = badged.iter().find(|(h, _)| *h == stack).map(|(_, k)| k.clone()).unwrap_or_default();
    if current == key && (padding.left - left).abs() < 0.01 {
        return;
    }
    match image {
        Some((path, _, scale)) => {
            let shrink = 1.0 / scale.max(0.5);
            let markup = format!(
                r#"<ImageBrush xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation" ImageSource="{}" Stretch="None" AlignmentX="Left" AlignmentY="Center"><ImageBrush.RelativeTransform><ScaleTransform CenterY="0.5" ScaleX="{shrink}" ScaleY="{shrink}"/></ImageBrush.RelativeTransform></ImageBrush>"#,
                file_uri(path)
            );
            let Some(brush) = crate::decorate::load_xaml(&markup) else { return };
            let hr = panel.put_Background(windows_core::Interface::as_raw(&brush));
            if hr.is_err() {
                logf!("clock: badge background refused: 0x{:08x}", hr.0);
                return;
            }
        }
        None => {
            let _ = panel.put_Background(core::ptr::null_mut());
        }
    }
    let _ = padded.put_Padding(Thickness { left, ..padding });
    badged.retain(|(h, _)| *h != stack);
    if !key.is_empty() {
        badged.push((stack, key));
    }
    // The width changes with the badge: measured afresh.
    crate::lock(&HELD).retain(|(h, _)| *h != stack);
}

/// How far below the bell's middle the badge's middle sits, in DIPs (negative: above): level with
/// the glyph's top.
const BELL_BADGE_DROP: f64 = -5.0;

/// The badge beside the bell's glyph, at its upper right (`image` as in [`show_badge`]), or taken
/// off. It is the background of the bell's inner grid, the bell widened by [`BELL_ROOM`] and its
/// glyph moved left meanwhile; the image is shown at its pixel size and scaled back by `1 / scale`.
///
/// # Safety
/// XAML UI thread only.
unsafe fn show_bell_badge(diagnostics: &xamlom::IXamlDiagnostics, bell: u64, image: Option<(&str, f64, f64)>) {
    use crate::winrt::IPanel;
    // The inner grid: the outer one is the bell's plate ([`split`]).
    let Some(grid) = named(bell, INNER_GRID) else { return };
    // Room for it: the bell wider (once split, whose width it knows), the glyph to the left. Equal
    // values are no change to XAML, so writing them at each tick costs nothing.
    let room = if image.is_some() { BELL_ROOM } else { 0.0 };
    let outer = named(bell, ICON_GRID);
    let base = crate::lock(&SPLIT).iter().find(|s| Some(s.bell_grid) == outer).map(|s| (s.bell_width, s.bell_min));
    if let (Some(outer), Some((width, min))) = (outer, base) {
        if let Some(framework) = crate::decorate::element::<crate::winrt::IFrameworkElement>(diagnostics, outer) {
            let wanted = if room > 0.0 { width + room } else { min };
            let mut was = 0.0;
            let _ = framework.get_MinWidth(&mut was);
            if (was - wanted).abs() > 0.5 {
                let _ = framework.put_MinWidth(wanted);
                // The clock and the bell moved: the rects the app keeps are out of date.
                crate::ipc::forget_part_rects();
            }
        }
        let glyph = descendants(bell).into_iter().find(|h| tree::type_of(*h).as_deref() == Some(BELL_GLYPH));
        if let Some(framework) = glyph.and_then(|g| crate::decorate::element::<crate::winrt::IFrameworkElement>(diagnostics, g)) {
            // Written only when it differs (each tick asks; a write is work for XAML).
            let wanted = crate::winrt::Thickness { right: room, ..Default::default() };
            let mut now = crate::winrt::Thickness::default();
            let _ = framework.get_Margin(&mut now);
            if now != wanted && RING_STEP.load(Ordering::SeqCst) >= RING.len() {
                let _ = framework.put_Margin(wanted);
            }
        }
    }
    let Some(panel) = crate::decorate::element::<IPanel>(diagnostics, grid) else { return };
    let key = image.map(|(path, ..)| path.to_string()).unwrap_or_default();
    let mut badged = crate::lock(&BADGED);
    let current = badged.iter().find(|(h, _)| *h == grid).map(|(_, k)| k.clone()).unwrap_or_default();
    if current == key {
        return;
    }
    match image {
        Some((path, _, scale)) => {
            let shrink = 1.0 / scale.max(0.5);
            // At the glyph's upper right: from the grid's right edge and middle, moved by
            // BELL_BADGE_DROP (relative to its height; negative is up).
            let height = crate::decorate::actual_size(diagnostics, grid).map_or(0.0, |(_, h)| h);
            let drop = if height > 0.0 { BELL_BADGE_DROP / height } else { 0.0 };
            let markup = format!(
                r#"<ImageBrush xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation" ImageSource="{}" Stretch="None" AlignmentX="Right" AlignmentY="Center"><ImageBrush.RelativeTransform><TransformGroup><ScaleTransform CenterX="1" CenterY="0.5" ScaleX="{shrink}" ScaleY="{shrink}"/><TranslateTransform Y="{drop}"/></TransformGroup></ImageBrush.RelativeTransform></ImageBrush>"#,
                file_uri(path)
            );
            let Some(brush) = crate::decorate::load_xaml(&markup) else { return };
            let hr = panel.put_Background(windows_core::Interface::as_raw(&brush));
            if hr.is_err() {
                logf!("clock: the bell's badge was refused: 0x{:08x}", hr.0);
                return;
            }
        }
        None => {
            let _ = panel.put_Background(core::ptr::null_mut());
        }
    }
    badged.retain(|(h, _)| *h != grid);
    if !key.is_empty() {
        badged.push((grid, key));
    }
}

/// The crossed bell (Segoe's `RingerSilent`), drawn while Windows keeps notifications quiet.
const CROSSED_BELL: &str = "\u{E7ED}";
/// The bell's glyph layers (`InnerTextBlock`s under its base, accent and underlay).
const GLYPH_LAYER: &str = "InnerTextBlock";
/// The bell's glyph layer (the shell's filled bell when something is new).
const BASE_LAYER: &str = "Base";

/// The shell's bell with something new (its `Base` layer, filled, in the accent; the underlay and
/// the accent overlay stay empty).
const SHELL_BELL: &str = "\u{F2A5}";
/// The shell's ordinary bell, nothing new (an outline); also the TAP's with nothing waiting in
/// Audio Tray's list. Any glyph other than these two (and the cross) is its quiet bell ("zz"):
/// Windows keeps notifications quiet by a rule of its own (a full-screen app, a game, a
/// presentation) that Audio Tray is not told of.
const SHELL_PLAIN_BELL: &str = "\u{F2A3}";

/// Each bell layer whose glyph was crossed, and the shell's glyph to put back.
static UNCROSSED: Mutex<Vec<(u64, String)>> = Mutex::new(Vec::new());
/// The bells whose shell drew its own quiet bell, until it draws its ordinary one again.
static SHELL_QUIET: Mutex<Vec<u64>> = Mutex::new(Vec::new());

/// The glyph's code points, for the log.
fn code_points(glyph: &str) -> String {
    glyph.chars().map(|c| format!("U+{:04X}", c as u32)).collect::<Vec<_>>().join(" ")
}

/// Draw `bell` crossed while notifications are quiet, or with the shell's own glyph again. Quiet
/// is what Audio Tray says (`quiet`), or what the shell draws itself: its "zz" bell is crossed
/// too, so the bell always says quiet the same way. Every layer takes the same glyph, so the
/// accent fill (anything new) shows on the crossed bell too. Written only when it differs, and
/// again whenever the shell draws over it.
///
/// # Safety
/// XAML UI thread only.
unsafe fn cross_bell(diagnostics: &xamlom::IXamlDiagnostics, bell: u64, quiet: bool) {
    let layers: Vec<u64> = descendants(bell).into_iter().filter(|h| tree::name_of(*h).as_deref() == Some(GLYPH_LAYER)).collect();
    // Every rewrite by the shell calls back (the shared text callback looks again, [`tick`]): its
    // quiet bell is crossed at once, and the cross goes when it draws its ordinary bell again.
    for &layer in &layers {
        watch_glyph(diagnostics, layer);
    }
    let texts: Vec<(u64, String)> = layers.iter().filter_map(|&l| Some((l, crate::decorate::text_of(diagnostics, l)?))).collect();
    // What the shell itself says, from what it drew last (the cross is ours, an empty layer
    // says nothing).
    {
        let mut shell_quiet = crate::lock(&SHELL_QUIET);
        let drawn = texts.iter().map(|(_, t)| t.as_str()).find(|t| !t.is_empty() && *t != CROSSED_BELL);
        let was = shell_quiet.contains(&bell);
        // The shell's bell, or Audio Tray's outline in its place ([`fill_bell`]): ordinary.
        let ordinary = |g: &str| g == SHELL_BELL || g == SHELL_PLAIN_BELL;
        match drawn {
            Some(glyph) if ordinary(glyph) && was => {
                shell_quiet.retain(|b| *b != bell);
                logf!("clock: the shell's own quiet is over");
            }
            Some(glyph) if !ordinary(glyph) && !was => {
                shell_quiet.push(bell);
                logf!("clock: the shell went quiet on its own ({}); the bell is crossed", code_points(glyph));
            }
            _ => {}
        }
    }
    let cross = quiet || crate::lock(&SHELL_QUIET).contains(&bell);
    let mut uncrossed = crate::lock(&UNCROSSED);
    for (layer, now) in texts {
        if cross {
            if now == CROSSED_BELL {
                continue;
            }
            // What goes back is the shell's ordinary bell, never its quiet one.
            if !uncrossed.iter().any(|(h, _)| *h == layer) {
                let back = if now.is_empty() || now == SHELL_BELL { now } else { SHELL_BELL.to_string() };
                uncrossed.push((layer, back));
            }
            crate::decorate::set_text(diagnostics, layer, CROSSED_BELL);
        } else if let Some(at) = uncrossed.iter().position(|(h, _)| *h == layer) {
            let (_, glyph) = uncrossed.remove(at);
            // Drawn over by the shell already: its glyph stays.
            if now == CROSSED_BELL {
                crate::decorate::set_text(diagnostics, layer, &glyph);
            }
        }
    }
}

/// The bell's "something new" look (the shell's filled glyph, in the accent): the shell's says what
/// *it* has not shown, which it counts as seen only when its own centre opens, never now that the
/// bell opens Audio Tray's. So it follows Audio Tray's list instead (`Some(waiting)`): the shell's
/// filled bell while anything waits there, its plain one otherwise. `None`: the bell is not Audio
/// Tray's, so the glyph the shell drew is given back (saved before the first write) and left alone.
/// What the bell's layers hold is logged at each change.
///
/// # Safety
/// XAML UI thread only.
unsafe fn fill_bell(diagnostics: &xamlom::IXamlDiagnostics, bell: u64, waiting: Option<bool>) {
    /// The layers' texts as last logged.
    static LAYERS_SEEN: Mutex<String> = Mutex::new(String::new());
    /// Each glyph block written here, and the shell's glyph in it before.
    static SHELL_GLYPHS: Mutex<Vec<(u64, String)>> = Mutex::new(Vec::new());
    let Some(base) = named(bell, BASE_LAYER) else { return };
    let Some(glyph) = descendants(base).into_iter().find(|h| tree::name_of(*h).as_deref() == Some(GLYPH_LAYER)) else { return };
    let now = crate::decorate::text_of(diagnostics, glyph).unwrap_or_default();
    let Some(filled) = waiting else {
        // Not Audio Tray's: what the shell drew, back (unless it has drawn anew since).
        let mut saved = crate::lock(&SHELL_GLYPHS);
        if let Some(at) = saved.iter().position(|(g, _)| *g == glyph) {
            let (_, shell) = saved.remove(at);
            if now == SHELL_BELL || now == SHELL_PLAIN_BELL {
                crate::decorate::set_text(diagnostics, glyph, &shell);
            }
        }
        return;
    };
    {
        let mut seen = crate::lock(&LAYERS_SEEN);
        let layers = format!("{} (Audio Tray has {})", code_points(&now), if filled { "some waiting" } else { "none waiting" });
        if *seen != layers {
            logf!("clock: the bell's glyph now: {layers}");
            *seen = layers;
        }
    }
    // The cross, when quiet, takes over ([`cross_bell`] writes every layer then, and leaves this
    // glyph alone). Looked at each tick (the cross ending puts back what it found), written only
    // when it differs.
    if now == CROSSED_BELL {
        return;
    }
    let wanted = if filled { SHELL_BELL } else { SHELL_PLAIN_BELL };
    if now != wanted {
        let mut saved = crate::lock(&SHELL_GLYPHS);
        if !saved.iter().any(|(g, _)| *g == glyph) {
            saved.push((glyph, now.clone()));
        }
        crate::decorate::set_text(diagnostics, glyph, wanted);
    }
}

/// `bell` with the shell's own glyph again, whatever the quiet (the TAP lets go).
///
/// # Safety
/// XAML UI thread only.
unsafe fn uncross_bell(diagnostics: &xamlom::IXamlDiagnostics, bell: u64) {
    crate::lock(&SHELL_QUIET).retain(|b| *b != bell);
    let layers = descendants(bell).into_iter().filter(|h| tree::name_of(*h).as_deref() == Some(GLYPH_LAYER));
    let mut uncrossed = crate::lock(&UNCROSSED);
    for layer in layers {
        if let Some(at) = uncrossed.iter().position(|(h, _)| *h == layer) {
            let (_, glyph) = uncrossed.remove(at);
            if crate::decorate::text_of(diagnostics, layer).as_deref() == Some(CROSSED_BELL) {
                crate::decorate::set_text(diagnostics, layer, &glyph);
            }
        }
    }
}

/// Bell glyph layers that call back when the shell rewrites them (by handle and element; never
/// detached: the callback only asks for a look, which crosses the bell only while it is quiet).
static GLYPHS_WATCHED: Mutex<Vec<(u64, usize)>> = Mutex::new(Vec::new());

/// Have bell layer `block` call back on a change of its text (once per element).
///
/// # Safety
/// XAML UI thread only.
unsafe fn watch_glyph(diagnostics: &xamlom::IXamlDiagnostics, block: u64) {
    use windows_core::Interface;
    let Some(object) = crate::decorate::element::<crate::winrt::IDependencyObject2>(diagnostics, block) else { return };
    let identity = object.as_raw() as usize;
    let mut watched = crate::lock(&GLYPHS_WATCHED);
    if watched.contains(&(block, identity)) {
        return;
    }
    let Some(statics) = crate::decorate::factory::<crate::winrt::ITextBlockStatics>("Windows.UI.Xaml.Controls.TextBlock") else { return };
    let mut dp: *mut core::ffi::c_void = core::ptr::null_mut();
    if statics.get_TextProperty(&mut dp).is_err() || dp.is_null() {
        return;
    }
    let dp = windows_core::IUnknown::from_raw(dp);
    let callback: IDependencyPropertyChangedCallback = TextChanged(()).into();
    let mut token = 0i64;
    if object.RegisterPropertyChangedCallback(dp.as_raw(), callback.as_raw(), &mut token).is_ok() {
        // Gone from the tree (a taskbar rebuilt): forgotten.
        watched.retain(|(h, _)| tree::type_of(*h).is_some() && *h != block);
        watched.push((block, identity));
    }
}

/// `C:\a b\c.png` as `file:///C:/a%20b/c.png`, quoted for an XAML attribute.
fn file_uri(path: &str) -> String {
    let mut uri = String::from("file:///");
    for c in path.chars() {
        match c {
            '\\' => uri.push('/'),
            ' ' => uri.push_str("%20"),
            '%' => uri.push_str("%25"),
            '#' => uri.push_str("%23"),
            '&' => uri.push_str("&amp;"),
            '"' => uri.push_str("&quot;"),
            '<' => uri.push_str("&lt;"),
            c => uri.push(c),
        }
    }
    uri
}

unsafe fn set_min_width(diagnostics: &xamlom::IXamlDiagnostics, handle: u64, width: f64) {
    if let Some(element) = crate::decorate::element::<crate::winrt::IFrameworkElement>(diagnostics, handle) {
        let _ = element.put_MinWidth(width);
    }
}

fn now() -> windows::Win32::Foundation::SYSTEMTIME {
    unsafe { windows::Win32::System::SystemInformation::GetLocalTime() }
}

fn format_time(st: &windows::Win32::Foundation::SYSTEMTIME, pattern: &str) -> String {
    use windows::core::PCWSTR;
    use windows::Win32::Globalization::{GetTimeFormatEx, TIME_FORMAT_FLAGS, TIME_NOSECONDS};
    let wide = crate::wide(pattern);
    let (flags, format) = if pattern.is_empty() { (TIME_NOSECONDS, PCWSTR::null()) } else { (TIME_FORMAT_FLAGS(0), PCWSTR(wide.as_ptr())) };
    let mut buf = [0u16; 64];
    let n = unsafe { GetTimeFormatEx(PCWSTR::null(), flags, Some(st), format, Some(&mut buf)) };
    String::from_utf16_lossy(&buf[..(n.max(1) - 1) as usize])
}

fn format_date(st: &windows::Win32::Foundation::SYSTEMTIME, pattern: &str) -> String {
    use windows::core::PCWSTR;
    use windows::Win32::Globalization::{GetDateFormatEx, DATE_SHORTDATE, ENUM_DATE_FORMATS_FLAGS};
    let wide = crate::wide(pattern);
    let (flags, format) = if pattern.is_empty() { (DATE_SHORTDATE, PCWSTR::null()) } else { (ENUM_DATE_FORMATS_FLAGS(0), PCWSTR(wide.as_ptr())) };
    let mut buf = [0u16; 96];
    let n = unsafe { GetDateFormatEx(PCWSTR::null(), flags, Some(st), format, Some(&mut buf), PCWSTR::null()) };
    String::from_utf16_lossy(&buf[..(n.max(1) - 1) as usize])
}

/// The colour emoji circle nearest the accent: 🔴 🟠 🟡 🟢 🔵 🟣, or ⚪ for a grey accent.
pub fn accent_dot(accent: Option<[u8; 3]>) -> &'static str {
    let Some([r, g, b]) = accent else { return "🔵" };
    let (r, g, b) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    if max - min < 0.12 {
        return "⚪";
    }
    let d = max - min;
    let hue = if max == r {
        60.0 * ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    match hue {
        h if !(15.0..345.0).contains(&h) => "🔴",
        h if h < 45.0 => "🟠",
        h if h < 70.0 => "🟡",
        h if h < 170.0 => "🟢",
        h if h < 260.0 => "🔵",
        _ => "🟣",
    }
}

/// The two lines for `text` at `st`: the time line, and the date line (`None`: hidden).
fn lines(text: &Text, st: &windows::Win32::Foundation::SYSTEMTIME, dot: &str) -> (String, Option<String>) {
    let mut time = format_time(st, &text.time);
    if text.badge > 0 && text.badge_image.is_empty() {
        time = format!("{dot} {}   {time}", text.badge);
    }
    let date = if text.date == "-" { String::new() } else { format_date(st, &text.date) };
    if text.one_line {
        let line = if date.is_empty() { time } else { format!("{time}   {date}") };
        return (line, None);
    }
    (time, (!date.is_empty()).then_some(date))
}

/// Write our text where the shell's differs, hold the width, and say when to look next (ms);
/// `None` stops the timer.
///
/// # Safety
/// Tray thread only (the control window's timer).
pub unsafe fn tick() -> Option<u32> {
    // No quiet gate: setting text on elements that exist is no structural change (like the hover
    // plates' opacity), and while music plays the tree is seldom quiet: waiting for it left the
    // shell's text up for long stretches.
    let Some(text) = crate::lock(&TEXT).clone() else {
        if RELEASING.load(Ordering::SeqCst) {
            // Undone when XAML can be reached; tried again until then.
            let Some(diagnostics) = crate::diagnostics() else { return Some(1000) };
            RELEASING.store(false, Ordering::SeqCst);
            restore(&diagnostics);
        }
        return None;
    };
    let Some(diagnostics) = crate::diagnostics() else { return Some(1000) };
    let st = now();
    let dot = accent_dot(crate::strip_state().and_then(|s| s.accent));
    let (time, date) = lines(&text, &st, dot);
    // Every block calls back on a change: no looking out for the shell's rewrites.
    let mut watched = true;
    for block in tree::find_by_name(TIME) {
        watched &= watch(&diagnostics, block);
        if crate::decorate::text_of(&diagnostics, block).as_deref() != Some(time.as_str()) {
            crate::decorate::set_text(&diagnostics, block, &time);
        }
    }
    for block in tree::find_by_name(DATE) {
        watched &= watch(&diagnostics, block);
        // Visibility is written only when it changes: a write each frame costs Explorer for nothing.
        let hidden = crate::lock(&HIDDEN).contains(&block);
        match &date {
            Some(date) => {
                if hidden {
                    crate::decorate::set_visible(&diagnostics, block, true);
                    crate::lock(&HIDDEN).retain(|b| *b != block);
                }
                if crate::decorate::text_of(&diagnostics, block).as_deref() != Some(date.as_str()) {
                    crate::decorate::set_text(&diagnostics, block, date);
                }
            }
            None if !hidden => {
                crate::decorate::set_visible(&diagnostics, block, false);
                crate::lock(&HIDDEN).push(block);
            }
            None => {}
        }
    }
    // A picture is shown whatever the count: the focus timer's pie comes with none.
    let badge = (!text.badge_image.is_empty()).then_some((text.badge_image.as_str(), text.badge_width, text.badge_scale));
    // On the bell's corner when Windows shows the bell, else beside the clock's lines.
    let bells = bells();
    for &bell in &bells {
        show_bell_badge(&diagnostics, bell, badge);
        cross_bell(&diagnostics, bell, text.quiet);
        fill_bell(&diagnostics, bell, text.waiting);
    }
    for stack in tree::find_by_name(STACK) {
        show_badge(&diagnostics, stack, if bells.is_empty() { badge } else { None });
        // Hold each clock's width at the widest it drew, so the shell's own (shorter) text, shown
        // for a moment after each minute, does not move the taskbar.
        let width = crate::decorate::actual_width(&diagnostics, stack).unwrap_or(0.0);
        let mut held = crate::lock(&HELD);
        match held.iter_mut().find(|(h, _)| *h == stack) {
            None => {
                // A new shape: start from what it measures now.
                held.push((stack, 0.0));
                set_min_width(&diagnostics, stack, 0.0);
            }
            Some((_, w)) if width > *w + 0.5 => {
                *w = width;
                set_min_width(&diagnostics, stack, width);
            }
            Some(_) => {}
        }
    }
    // A burst of looks asked for (the pointer over the clock): one frame at a time until spent.
    // Watched blocks say when the shell rewrites them: no burst needed.
    if watched {
        BURST.store(0, Ordering::SeqCst);
    } else if BURST.load(Ordering::SeqCst) > 0 {
        BURST.fetch_sub(1, Ordering::SeqCst);
        return Some(16);
    }
    Some(next_look(st.wSecond as u32 * 1000 + st.wMilliseconds as u32, text.time.contains('s'), watched))
}

/// Frame-by-frame looks still due, after the pointer entered or left the clock.
static BURST: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// The pointer entered or left the clock: the shell may redraw its text then, so look every frame
/// for about a second.
pub fn watch_closely() {
    // Not re-armed while one runs: sweeping across the clock would keep Explorer at 60 Hz.
    if crate::lock(&TEXT).is_some() && BURST.load(Ordering::SeqCst) == 0 {
        BURST.store(BURST_FRAMES, Ordering::SeqCst);
        crate::lifecycle::arm_clock_timer(16);
    }
}

/// About half a second of frames: the shell redraws its text within that of a hover.
const BURST_FRAMES: u32 = 30;

/// When to look next (ms), `into` ms into the minute. With every block `watched`, the shell's
/// rewrites call back, so only the TAP's own text is due: at the next second (seconds shown) or
/// minute. Otherwise: the shell rewrites its text about half a second after each minute (measured:
/// 0.5 to 0.7 s), so through the first 1.5 s the TAP looks every frame and its text is back within
/// one; the shell also rewrites it at other moments, so outside that window it looks every 2 s.
fn next_look(into: u32, seconds: bool, watched: bool) -> u32 {
    const WATCH: u32 = 1500;
    const FRAME: u32 = 16;
    const LOOK: u32 = 2000;
    let next_minute = 60_000 - into.min(59_999) + 5;
    if seconds {
        return if !watched && into < WATCH { FRAME } else { 1000 - into % 1000 + 15 };
    }
    if watched {
        return next_minute;
    }
    if into < WATCH {
        return FRAME;
    }
    // Never past the next minute's watch.
    LOOK.min(next_minute)
}

#[cfg(test)]
mod tests {
    use super::accent_dot;

    #[test]
    fn the_clock_is_watched_every_frame_just_after_each_minute() {
        use super::next_look;
        assert_eq!(next_look(0, false, false), 16);
        assert_eq!(next_look(1499, false, false), 16);
        assert_eq!(next_look(1500, false, false), 2000);
        assert_eq!(next_look(30_000, false, false), 2000);
        assert_eq!(next_look(59_000, false, false), 1005);
        assert_eq!(next_look(59_990, false, false), 15);
        assert_eq!(next_look(700, true, false), 16);
        assert_eq!(next_look(5_200, true, false), 815);
    }

    #[test]
    fn a_watched_clock_is_looked_at_only_when_its_own_text_is_due() {
        use super::next_look;
        assert_eq!(next_look(0, false, true), 60_005);
        assert_eq!(next_look(30_000, false, true), 30_005);
        assert_eq!(next_look(59_990, false, true), 15);
        assert_eq!(next_look(700, true, true), 315);
    }

    #[test]
    fn badge_paths_become_file_uris_fit_for_an_attribute() {
        assert_eq!(super::file_uri(r"C:\Users\a b\x&y.png"), "file:///C:/Users/a%20b/x&amp;y.png");
        assert_eq!(super::file_uri(r#"C:\b%#\"q<.png"#), "file:///C:/b%25%23/&quot;q&lt;.png", "escaped for the address and the attribute");
    }

    #[test]
    fn the_clock_lines_follow_the_text_asked_for() {
        use super::{lines, Text};
        let st = windows::Win32::Foundation::SYSTEMTIME { wYear: 2026, wMonth: 11, wDay: 18, wHour: 9, wMinute: 5, ..Default::default() };
        let plain = Text { time: "HH:mm".into(), date: "dd/MM".into(), ..Default::default() };
        assert_eq!(lines(&plain, &st, "x"), ("09:05".into(), Some("18/11".into())));
        let one = Text { one_line: true, ..plain.clone() };
        assert_eq!(lines(&one, &st, "x"), ("09:05   18/11".into(), None));
        let hidden = Text { date: "-".into(), ..plain.clone() };
        assert_eq!(lines(&hidden, &st, "x"), ("09:05".into(), None));
        // A count without its badge picture: the dot and the count go first.
        let badged = Text { badge: 3, ..plain.clone() };
        assert_eq!(lines(&badged, &st, "x").0, "x 3   09:05");
        let pictured = Text { badge: 3, badge_image: r"C:\b.png".into(), ..plain };
        assert_eq!(lines(&pictured, &st, "x").0, "09:05");
    }

    #[test]
    fn the_dot_follows_the_accent_hue() {
        assert_eq!(accent_dot(Some([0xD8, 0x8D, 0xE1])), "🟣");
        assert_eq!(accent_dot(Some([0x00, 0x78, 0xD4])), "🔵");
        assert_eq!(accent_dot(Some([0xE8, 0x11, 0x23])), "🔴");
        assert_eq!(accent_dot(Some([0x10, 0x7C, 0x10])), "🟢");
        assert_eq!(accent_dot(Some([0x80, 0x80, 0x84])), "⚪");
    }
}
