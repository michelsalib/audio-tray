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

// ---- The clock's text, when the app styles it ----

/// What the clock's two lines say, from the app (`tap_proto::CLOCK_MAGIC`). Patterns are
/// `GetTimeFormatEx` / `GetDateFormatEx` ones; empty means the locale's short time or date.
#[derive(Clone, Debug, Default, PartialEq)]
struct Text {
    time: String,
    /// `-` hides the date.
    date: String,
    /// The date after the time, on one line.
    one_line: bool,
    /// Appended to the time line (the next meeting, the focus time left).
    top: String,
    /// Appended to the date line (the unread count).
    bottom: String,
}

/// `None` until the app first styles the clock: the shell's text is then never touched. Once set,
/// the TAP keeps the clock ticking itself for the life of the shell (going back to the locale's
/// format when the app leaves), since the shell may stop updating text it no longer owns.
static TEXT: Mutex<Option<Text>> = Mutex::new(None);

/// Control-window timer that updates the text, at each minute (each second with seconds shown).
pub const TEXT_TIMER: usize = 3;

/// The app's new text settings (init-data encoded).
pub fn set_text(data: &str) {
    let flag = |key: &str| tap_proto::init_value(data, key).as_deref() == Some("1");
    let value = |key: &str| tap_proto::init_value(data, key).unwrap_or_default();
    let text = Text { time: value("time"), date: value("date"), one_line: flag("oneline"), top: value("top"), bottom: value("bottom") };
    let mut held = crate::lock(&TEXT);
    if held.as_ref() != Some(&text) {
        logf!("clock: text now {text:?}");
        *held = Some(text);
    }
    drop(held);
    crate::lifecycle::arm_clock_timer(1);
}

/// The app is gone: back to the locale's own format, still kept ticking.
pub fn release_text() {
    let mut held = crate::lock(&TEXT);
    if held.is_some() {
        *held = Some(Text::default());
        drop(held);
        crate::lifecycle::arm_clock_timer(1);
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

/// The two lines for `text` at `st`: the time line, and the date line (`None`: hidden).
fn lines(text: &Text, st: &windows::Win32::Foundation::SYSTEMTIME) -> (String, Option<String>) {
    let join = |a: String, b: &str| if b.is_empty() { a } else if a.is_empty() { b.to_string() } else { format!("{a} · {b}") };
    let time = join(format_time(st, &text.time), &text.top);
    let date = if text.date == "-" { String::new() } else { format_date(st, &text.date) };
    let date = join(date, &text.bottom);
    if text.one_line {
        let line = if date.is_empty() { time } else { format!("{time}   {date}") };
        return (line, None);
    }
    (time, (!date.is_empty()).then_some(date))
}

/// Update every clock's text, and say when to next (ms).
///
/// # Safety
/// Tray thread only (the control window's timer).
pub unsafe fn tick() -> Option<u32> {
    let text = crate::lock(&TEXT).clone()?;
    if !crate::tree::quiet_for(std::time::Duration::from_millis(32)) {
        return Some(50);
    }
    let Some(diagnostics) = crate::diagnostics() else { return Some(1000) };
    let st = now();
    let (time, date) = lines(&text, &st);
    for block in tree::find_by_name("TimeInnerTextBlock") {
        if crate::decorate::text_of(&diagnostics, block).as_deref() != Some(time.as_str()) {
            crate::decorate::set_text(&diagnostics, block, &time);
        }
    }
    for block in tree::find_by_name("DateInnerTextBlock") {
        match &date {
            Some(date) => {
                crate::decorate::set_visible(&diagnostics, block, true);
                if crate::decorate::text_of(&diagnostics, block).as_deref() != Some(date.as_str()) {
                    crate::decorate::set_text(&diagnostics, block, date);
                }
            }
            None => {
                crate::decorate::set_visible(&diagnostics, block, false);
            }
        }
    }
    // Just past the next second or minute, so the text changes with the clock. The shell writes
    // its own text at the minute too, sometimes after us: a few looks after each minute put ours
    // back within moments.
    let ms = st.wMilliseconds as u32;
    if text.time.contains('s') {
        return Some(1000 - ms + 15);
    }
    const FOLLOW_UPS: [u32; 3] = [3000, 1000, 250];
    let left = FOLLOW.load(Ordering::SeqCst);
    if left > 0 {
        FOLLOW.store(left - 1, Ordering::SeqCst);
        return Some(FOLLOW_UPS[(left - 1) as usize]);
    }
    FOLLOW.store(FOLLOW_UPS.len() as u32, Ordering::SeqCst);
    Some((60 - st.wSecond as u32) * 1000 - ms + 30)
}

/// Looks still due after the last minute's change.
static FOLLOW: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
