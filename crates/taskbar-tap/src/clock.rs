//! The taskbar clock, when audio-tray's calendar replaces the shell's (`clock=1` in the init data):
//! a press on the clock is marked handled so `NotificationCenterButton` never opens its flyout,
//! and the completed tap is reported as [`tap_proto::ACTION_OPEN_CALENDAR`]. The bell beside it
//! is left alone, so Windows' notifications stay one click away.
//!
//! The clock is `SystemTray.DateTimeIconContent` inside a `SystemTray.IconView`; the handlers go on
//! the view (the clock's whole hit area). Attaching handlers writes nothing to the tree, so this
//! runs behind the short wiring gate, like the music tile's transport buttons. Handlers are never
//! detached: they check [`enabled`] on every event, so turning the calendar off, a revert, or the
//! app quitting gives the clock back to the shell at once.

use core::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use crate::log::logf;
use crate::{tree, xamlom};

/// The clock's content, the same on the main and the secondary taskbars.
const CONTENT: &str = "SystemTray.DateTimeIconContent";
/// The section it lives in, for the diagnostic dump when it is not found.
const SECTION: &str = "NotificationCenterButton";

static ENABLED: AtomicBool = AtomicBool::new(false);
/// Views that already have the handlers.
static WIRED: Mutex<Vec<u64>> = Mutex::new(Vec::new());
/// The clock's section was found without the clock in it (and reported).
static MISSING: AtomicBool = AtomicBool::new(false);

pub fn set_enabled(on: bool) {
    if ENABLED.swap(on, Ordering::SeqCst) != on {
        logf!("clock: {}", if on { "clicks open audio-tray's calendar" } else { "left to the shell" });
    }
}

/// Whether a click on the clock is ours: asked for, the TAP live, and the app there to take it.
pub fn enabled() -> bool {
    ENABLED.load(Ordering::SeqCst) && crate::live() && crate::ipc::receiver_present()
}

/// Nothing left to wire: off, or every clock found so far has its handlers.
pub fn settled() -> bool {
    if !ENABLED.load(Ordering::SeqCst) {
        return true;
    }
    let clocks = tree::find_by_type(CONTENT);
    if clocks.is_empty() {
        // Not on this build (reported once): nothing to wait for.
        return MISSING.load(Ordering::SeqCst);
    }
    let wired = crate::lock(&WIRED);
    clocks.iter().all(|c| wired.contains(&target_of(*c)))
}

/// The element the handlers go on: the clock's `IconView`, else the content itself.
fn target_of(content: xamlom::InstanceHandle) -> xamlom::InstanceHandle {
    crate::ancestor_of_type(content, crate::ICON_VIEW, 8).unwrap_or(content)
}

/// Attach the handlers to every clock not yet wired.
///
/// # Safety
/// XAML UI thread only, behind the wiring gate.
pub unsafe fn wire(diagnostics: &xamlom::IXamlDiagnostics) {
    if !ENABLED.load(Ordering::SeqCst) {
        return;
    }
    let clocks = tree::find_by_type(CONTENT);
    {
        // Views gone from the tree (a taskbar rebuilt) are forgotten: their handles can come back.
        let mut wired = crate::lock(&WIRED);
        wired.retain(|h| tree::type_of(*h).is_some());
    }
    if clocks.is_empty() {
        report_missing();
        return;
    }
    for content in clocks {
        let target = target_of(content);
        if crate::lock(&WIRED).contains(&target) {
            continue;
        }
        // Recorded either way: a failure is logged once, not retried every sweep.
        crate::lock(&WIRED).push(target);
        make_hit_testable(diagnostics, content);
        let wired = crate::interact::attach_clock(diagnostics, target);
        logf!(
            "clock: {} 0x{target:x} ({})",
            if wired { "wired" } else { "could not wire" },
            tree::type_of(target).unwrap_or_default()
        );
    }
}

/// The button around the clock.
const BUTTON: &str = "SystemTray.OmniButton";

/// The shell's template leaves the clock's elements out of hit-testing, so a press lands on the
/// button itself and never reaches handlers on the clock. Every element from the clock's text up
/// to the button is made hit-testable, so the press reaches [`crate::interact::attach_clock`]'s
/// handler first. Left on after a revert: with no handler acting, the press bubbles on to the
/// button as before. Each element's original value is logged.
///
/// # Safety
/// XAML UI thread only.
unsafe fn make_hit_testable(diagnostics: &xamlom::IXamlDiagnostics, content: xamlom::InstanceHandle) {
    use crate::winrt::IUIElement;
    let mut chain = Vec::new();
    // Up from the content to the button (exclusive).
    let mut handle = content;
    for _ in 0..16 {
        if tree::type_of(handle).as_deref() == Some(BUTTON) {
            break;
        }
        chain.push(handle);
        let Some(parent) = tree::parent_of(handle) else { break };
        handle = parent;
    }
    // And everything inside the content (the time and date text).
    let mut queue = tree::children_of(content);
    while let Some(child) = queue.pop() {
        if chain.len() > 64 {
            break;
        }
        chain.push(child);
        queue.extend(tree::children_of(child));
    }
    for handle in chain {
        let Some(element) = crate::decorate::element::<IUIElement>(diagnostics, handle) else { continue };
        let mut was = 0u8;
        let _ = element.get_IsHitTestVisible(&mut was);
        let set = if was == 0 { element.put_IsHitTestVisible(1).0 } else { 0 };
        let name = tree::name_of(handle).filter(|n| !n.is_empty()).map(|n| format!("#{n}")).unwrap_or_default();
        logf!(
            "clock:   {}{name} hit-test {}{}",
            tree::type_of(handle).unwrap_or_default(),
            was != 0,
            if was == 0 { format!(" -> on (0x{set:08x})") } else { String::new() }
        );
    }
}

/// Once per shell: what the clock's section holds, when the clock's class is not in it (another
/// Windows build), so the next fix knows what to look for.
fn report_missing() {
    let Some(section) = tree::find_by_name(SECTION).into_iter().next() else { return };
    if MISSING.swap(true, Ordering::SeqCst) {
        return;
    }
    logf!("clock: no {CONTENT} recorded; {SECTION} holds:");
    let mut queue = vec![(section, 0usize)];
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

/// What [`enabled`] read, for the log.
pub fn why() -> String {
    format!(
        "asked {}, live {}, app there {}",
        ENABLED.load(Ordering::SeqCst),
        crate::live(),
        crate::ipc::receiver_present()
    )
}
