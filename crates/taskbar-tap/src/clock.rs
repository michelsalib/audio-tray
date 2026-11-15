//! The taskbar clock: where audio-tray's calendar opens from, and what it reads.
//!
//! **Clicks** (`clock=1` in the init data): the shell's `NotificationCenterButton` opens its own
//! flyout whatever its XAML handlers do with a press, so the TAP only says when the pointer is over
//! the button (`WM_TASKBAR_CLOCK_HOVER`) and the app's mouse hook takes the click. The handlers go on
//! the whole button (the clock's view alone leaves its edges to the shell), behind the short wiring
//! gate (attaching writes nothing), and are never detached: they check [`enabled`] on every event.
//!
//! **Text** (`tap_proto::CLOCK_MAGIC`): once the app styles the clock, the TAP writes the shell's
//! two text blocks. Nothing can be added beside them (`Panel.Children` refuses writes, and the
//! shell resets the clock's slot when its content is replaced), and the shell rewrites them every
//! minute through compiled bindings: the TAP watches every frame through the 1.5 s after each minute and
//! puts its text back within one, and holds the clock's width at the widest it has drawn so the taskbar does
//! not shift meanwhile. The unread count goes before the time, behind a colour emoji dot picked to
//! match the accent (a text block has one colour; emoji keep their own).

use core::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use crate::log::logf;
use crate::{tree, xamlom};

/// The clock's content, the same on the main and the secondary taskbars.
const CONTENT: &str = "SystemTray.DateTimeIconContent";
/// The button around it (the bell's too, when shown), by `x:Name`.
const SECTION: &str = "NotificationCenterButton";
const TIME: &str = "TimeInnerTextBlock";
const DATE: &str = "DateInnerTextBlock";
const STACK: &str = "DateTimeStackPanel";

static ENABLED: AtomicBool = AtomicBool::new(false);
/// Elements that already have the handlers.
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

/// What [`enabled`] read, for the log.
pub fn why() -> String {
    format!("asked {}, live {}, app there {}", ENABLED.load(Ordering::SeqCst), crate::live(), crate::ipc::receiver_present())
}

/// What the handlers go on: the button around each clock.
fn targets() -> Vec<u64> {
    tree::find_by_name(SECTION)
}

/// Nothing left to wire: off, or every clock's button has its handlers.
pub fn settled() -> bool {
    if !ENABLED.load(Ordering::SeqCst) {
        return true;
    }
    let targets = targets();
    if targets.is_empty() || tree::find_by_type(CONTENT).is_empty() {
        // Not on this build (reported once): nothing to wait for.
        return MISSING.load(Ordering::SeqCst);
    }
    let wired = crate::lock(&WIRED);
    targets.iter().all(|t| wired.contains(t))
}

/// Attach the handlers to every clock's button not yet wired.
///
/// # Safety
/// XAML UI thread only, behind the wiring gate.
pub unsafe fn wire(diagnostics: &xamlom::IXamlDiagnostics) {
    if !ENABLED.load(Ordering::SeqCst) {
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
    /// After the time (the next meeting, the focus time left).
    top: String,
    /// Windows' unread notifications (0: none).
    badge: u32,
}

/// The app's latest word; `None` when it never styled the clock or has gone.
static TEXT: Mutex<Option<Text>> = Mutex::new(None);
/// The clock's width held (DIPs): the widest drawn since its text last changed shape; negative
/// when it is to be measured afresh.
static HELD_WIDTH: Mutex<f64> = Mutex::new(0.0);

/// Control-window timer that updates the text, at each minute (each second with seconds shown).
pub const TEXT_TIMER: usize = 3;

/// The app's new text settings (init-data encoded).
pub fn set_text(data: &str) {
    let flag = |key: &str| tap_proto::init_value(data, key).as_deref() == Some("1");
    let value = |key: &str| tap_proto::init_value(data, key).unwrap_or_default();
    let text = Text {
        time: value("time"),
        date: value("date"),
        one_line: flag("oneline"),
        top: value("top"),
        badge: value("badge").parse().unwrap_or(0),
    };
    let mut held = crate::lock(&TEXT);
    if held.as_ref() != Some(&text) {
        logf!("clock: text now {text:?}");
        // A new shape: the width is measured afresh.
        *crate::lock(&HELD_WIDTH) = -1.0;
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
    *crate::lock(&HELD_WIDTH) = 0.0;
    for stack in tree::find_by_name(STACK) {
        set_min_width(diagnostics, stack, 0.0);
    }
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
    if !text.top.is_empty() {
        time = format!("{time} · {}", text.top);
    }
    if text.badge > 0 {
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
    let text = crate::lock(&TEXT).clone()?;
    let Some(diagnostics) = crate::diagnostics() else { return Some(1000) };
    let st = now();
    let dot = accent_dot(crate::strip_state().and_then(|s| s.accent));
    let (time, date) = lines(&text, &st, dot);
    for block in tree::find_by_name(TIME) {
        if crate::decorate::text_of(&diagnostics, block).as_deref() != Some(time.as_str()) {
            crate::decorate::set_text(&diagnostics, block, &time);
        }
    }
    for block in tree::find_by_name(DATE) {
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
    // Hold the width at the widest drawn, so the shell's own (shorter) text, shown for a moment
    // after each minute, does not move the taskbar.
    for stack in tree::find_by_name(STACK) {
        let width = crate::decorate::actual_width(&diagnostics, stack).unwrap_or(0.0);
        let mut held = crate::lock(&HELD_WIDTH);
        if *held < 0.0 {
            // A new shape: start from what it measures now.
            *held = 0.0;
            set_min_width(&diagnostics, stack, 0.0);
        } else if width > *held + 0.5 {
            *held = width;
            set_min_width(&diagnostics, stack, width);
        }
    }
    // A burst of looks asked for (the pointer over the clock): one frame at a time until spent.
    if BURST.load(Ordering::SeqCst) > 0 {
        BURST.fetch_sub(1, Ordering::SeqCst);
        return Some(16);
    }
    Some(next_look(st.wSecond as u32 * 1000 + st.wMilliseconds as u32, text.time.contains('s')))
}

/// Frame-by-frame looks still due, after the pointer entered or left the clock.
static BURST: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// The pointer entered or left the clock: the shell may redraw its text then, so look every frame
/// for about a second.
pub fn watch_closely() {
    if crate::lock(&TEXT).is_some() {
        BURST.store(60, Ordering::SeqCst);
        crate::lifecycle::arm_clock_timer(16);
    }
}

/// When to look next (ms), `into` ms into the minute. The shell rewrites its text about half a
/// second after each minute (measured: 0.5 to 0.7 s): through the first 1.5 s the TAP looks every
/// frame, so its text is back within one. The shell also rewrites it at other moments, so outside
/// that window it looks every 2 s (every second with seconds shown).
fn next_look(into: u32, seconds: bool) -> u32 {
    const WATCH: u32 = 1500;
    const FRAME: u32 = 16;
    const LOOK: u32 = 2000;
    if into < WATCH {
        return FRAME;
    }
    if seconds {
        return 1000 - into % 1000 + 15;
    }
    // Never past the next minute's watch.
    LOOK.min(60_000 - into.min(59_999) + 5)
}

#[cfg(test)]
mod tests {
    use super::accent_dot;

    #[test]
    fn the_clock_is_watched_every_frame_just_after_each_minute() {
        use super::next_look;
        assert_eq!(next_look(0, false), 16);
        assert_eq!(next_look(1499, false), 16);
        assert_eq!(next_look(1500, false), 2000);
        assert_eq!(next_look(30_000, false), 2000);
        assert_eq!(next_look(59_000, false), 1005);
        assert_eq!(next_look(59_990, false), 15);
        assert_eq!(next_look(700, true), 16);
        assert_eq!(next_look(5_200, true), 815);
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
