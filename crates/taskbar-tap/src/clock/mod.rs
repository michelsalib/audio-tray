//! The taskbar clock: where audio-tray's calendar opens from, and what it reads.
//!
//! **Clicks** (`clock=1` in the init data): the shell's `NotificationCenterButton` opens its own
//! flyout whatever its XAML handlers do with a press, so the TAP only says when the pointer is over
//! the button (`WM_TASKBAR_CLOCK_HOVER`) and the app's mouse hook takes the click. The handlers go on
//! the whole button (the clock's view alone leaves its edges to the shell), behind the short wiring
//! gate (attaching writes nothing), and are never detached: they check [`enabled`] on every event.
//! The bell beside the clock (when Windows shows it) is inside the same button: its own enter and
//! leave are told too, and the app's hook sends its clicks to audio-tray's notifications. The
//! button is split in two plates, the clock's and the bell's ([`plate`]).
//!
//! **Text** (`tap_proto::CLOCK_MAGIC`, [`text`]): once the app styles the clock, the TAP writes the
//! shell's two text blocks. Nothing can be added beside them (`Panel.Children` refuses writes, and
//! the shell resets the clock's slot when its content is replaced), and the shell rewrites them
//! every minute through compiled bindings: each text block calls back when its text changes, and
//! the TAP puts its text back at once (where a block could not be watched, it looks every frame
//! through the 1.5 s after each minute instead). It holds the clock's width at the widest it has
//! drawn so the taskbar does not shift meanwhile. The unread count is a badge the app draws (an
//! accent pill with the count, a PNG at the taskbar's scale), painted on the bell's corner
//! ([`bell`]), or without a bell as the clock stack's background left of the two lines; without a
//! picture, an emoji dot near the accent and the count go before the time.

mod bell;
mod plate;
mod text;

use core::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use crate::log::logf;
use crate::part::{describe, ICON};
use crate::tree::descendants;
use crate::{tree, xamlom};

pub use bell::{ring, ring_step, RING_MS};
pub use plate::{open_panel, part_hover, quiet_tooltips, section_hover, set_panel, unsplit};
pub use text::{keep_text, put_back_text, release_text, restore, set_text, tick, watch_closely};

/// The clock's content, the same on the main and the secondary taskbars.
const CONTENT: &str = "SystemTray.DateTimeIconContent";
/// The button around it (the bell's too, when shown), by `x:Name`.
const SECTION: &str = "NotificationCenterButton";

static ENABLED: AtomicBool = AtomicBool::new(false);
/// Elements that already have the handlers.
static WIRED: Mutex<Vec<u64>> = Mutex::new(Vec::new());
/// The clock's section was found without the clock in it (and reported).
static MISSING: AtomicBool = AtomicBool::new(false);
/// The bell's inside was logged (once per shell, for placing the badge on another build).
static DESCRIBED: AtomicBool = AtomicBool::new(false);

/// Turned off with the button split: [`wire`] puts it back whole at the next sweep (this runs off
/// the XAML thread).
static UNSPLIT_PENDING: AtomicBool = AtomicBool::new(false);

pub fn set_enabled(on: bool) {
    if ENABLED.swap(on, Ordering::SeqCst) != on {
        logf!("clock: {}", if on { "clicks open audio-tray's calendar" } else { "left to the shell" });
        UNSPLIT_PENDING.store(!on && plate::any_split(), Ordering::SeqCst);
    }
}

/// Whether a click on the clock is ours: asked for, the TAP live, and the app there to take it.
pub fn enabled() -> bool {
    ENABLED.load(Ordering::SeqCst) && crate::live() && crate::ipc::receiver_present()
}

/// The clock itself opens the app's calendar; else only its bell is the app's (the button is split
/// all the same, to light and measure the bell), and a tap on the clock stays the shell's.
static CALENDAR: AtomicBool = AtomicBool::new(false);

pub fn set_calendar(on: bool) {
    CALENDAR.store(on, Ordering::SeqCst);
}

/// Whether a tap on the clock's button opens the app's calendar.
pub fn calendar() -> bool {
    CALENDAR.load(Ordering::SeqCst) && enabled()
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
    let split = targets.iter().filter(|s| bell_of(**s).is_some()).all(|s| plate::split_or_left_whole(*s));
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
        // The clock and the bell: each lights on its own (see [`plate`]), the bell's clicks go to
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
            describe(bell, "clock: the bell");
        }
        plate::split(diagnostics, section);
    }
}

/// Once per shell: what the clock's section holds, when the clock's class is not in it (another
/// Windows build), so the next fix knows what to look for.
fn report_missing() {
    let Some(section) = tree::find_by_name(SECTION).into_iter().next() else { return };
    if MISSING.swap(true, Ordering::SeqCst) {
        return;
    }
    describe(section, &format!("clock: no {CONTENT} recorded; {SECTION}"));
}
