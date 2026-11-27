//! The clock's text, when the app styles it: written over the shell's at each of its rewrites, the
//! clock's width held, and the badge beside the two lines when there is no bell to wear it.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;

use super::bell;
use crate::log::logf;
use crate::{tree, xaml, xamlom};
use tap_proto::key;

const TIME: &str = "TimeInnerTextBlock";
const DATE: &str = "DateInnerTextBlock";
const STACK: &str = "DateTimeStackPanel";

/// The text callback ([`text_changed`]), as the log names it.
pub(super) const TEXT_CALLBACK: &str = "clock: the text callback";

/// Text blocks that call back when their text changes ([`watch`]): never detached (the TAP stays
/// loaded), the callback checks there is text to put back. By handle and element: a handle can be
/// reused by a rebuilt taskbar's new block, which needs a callback of its own.
static WATCHED: Mutex<Vec<(u64, usize)>> = Mutex::new(Vec::new());
/// A block would not be watched (said once).
static REFUSED: AtomicBool = AtomicBool::new(false);

/// A clock's text changed (the shell's rewrite, or the TAP's own; or a bell layer's glyph): look at
/// once. The TAP's own write comes from inside [`tick`], whose next look then replaces this one.
pub(super) fn text_changed() {
    if crate::lock(&TEXT).is_some() {
        crate::lifecycle::arm_clock_timer(1);
    }
}

/// Have `block`'s text changes call back ([`text_changed`]); `false` if XAML would not.
///
/// # Safety
/// XAML UI thread only.
unsafe fn watch(diagnostics: &xamlom::IXamlDiagnostics, block: u64) -> bool {
    use windows_core::Interface;
    // A handle whose element is gone (its removal never announced) has nothing to watch: it
    // must not hold the clock to looking every frame.
    let Some(object) = xaml::element::<crate::winrt::IDependencyObject2>(diagnostics, block) else { return true };
    let identity = object.as_raw() as usize;
    if crate::lock(&WATCHED).contains(&(block, identity)) {
        return true;
    }
    let Some(dp) = xaml::text_property() else { return false };
    let ok = crate::part::on_change(diagnostics, block, &dp, TEXT_CALLBACK, text_changed);
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

/// The app's new text settings (init-data encoded).
pub fn set_text(data: &str) {
    let flag = |key: &str| tap_proto::init_value(data, key).as_deref() == Some("1");
    if flag(key::RELEASE) {
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
        time: value(key::TIME),
        date: value(key::DATE),
        one_line: flag(key::ONE_LINE),
        badge: value(key::BADGE).parse().unwrap_or(0),
        badge_image: value(key::BADGE_IMAGE),
        badge_width: value(key::BADGE_WIDTH).parse().unwrap_or(0.0),
        badge_scale: value(key::BADGE_SCALE).parse().unwrap_or(1.0),
        quiet: flag(key::QUIET),
        waiting: match value(key::WAITING).as_str() {
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
    for bell in super::bells() {
        bell::show_bell_badge(diagnostics, bell, None);
        bell::uncross_bell(diagnostics, bell);
        // Its fill as the shell has it again.
        bell::fill_bell(diagnostics, bell, None);
    }
    // The shell's bindings rewrite the date's text, never its visibility.
    for block in std::mem::take(&mut *crate::lock(&HIDDEN)) {
        xaml::set_visible(diagnostics, block, true);
    }
    // The shell's own text at once (its locale's time and date), not the TAP's badge or dot until
    // the shell next writes.
    let st = now();
    for block in tree::find_by_name(TIME) {
        xaml::set_text(diagnostics, block, &format_time(&st, ""));
    }
    for block in tree::find_by_name(DATE) {
        xaml::set_text(diagnostics, block, &format_date(&st, ""));
    }
}

/// The room between the badge and the text, in DIPs.
const BADGE_GAP: f64 = 6.0;

/// Draw the app's badge (`image`, `width` DIPs wide, at `scale` pixels per DIP) left of the clock's
/// two lines, or take it off. It is the stack's background (a brush is a property value, which the
/// shell takes where it refuses new children), with padding making room for it. The image is shown
/// at its pixel size and scaled back by `1 / scale` about the stack's middle, so it lands pixel for
/// pixel and centred on both lines.
///
/// # Safety
/// XAML UI thread only.
unsafe fn show_badge(diagnostics: &xamlom::IXamlDiagnostics, stack: u64, image: Option<(&str, f64, f64)>) {
    use crate::part::Brushed;
    use crate::winrt::{IStackPanel2, Thickness};
    let Some(padded) = xaml::element::<IStackPanel2>(diagnostics, stack) else { return };
    let mut padding = Thickness::default();
    let _ = padded.get_Padding(&mut padding);
    let left = image.map_or(0.0, |(_, width, _)| width + BADGE_GAP);
    let key = image.map_or("", |(path, ..)| path);
    let brushed = crate::part::set_grid_brush(diagnostics, stack, "clock", key, || {
        let (path, _, scale) = image?;
        crate::part::image_brush(path, scale, ("Left", "Center"), (0.0, 0.5), 0.0)
    });
    if brushed == Brushed::Refused || (brushed == Brushed::Already && (padding.left - left).abs() < 0.01) {
        return;
    }
    let _ = padded.put_Padding(Thickness { left, ..padding });
    // The width changes with the badge: measured afresh.
    crate::lock(&HELD).retain(|(h, _)| *h != stack);
}

unsafe fn set_min_width(diagnostics: &xamlom::IXamlDiagnostics, handle: u64, width: f64) {
    if let Some(element) = xaml::element::<crate::winrt::IFrameworkElement>(diagnostics, handle) {
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
fn accent_dot(accent: Option<[u8; 3]>) -> &'static str {
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
        if xaml::text_of(&diagnostics, block).as_deref() != Some(time.as_str()) {
            xaml::set_text(&diagnostics, block, &time);
        }
    }
    for block in tree::find_by_name(DATE) {
        watched &= watch(&diagnostics, block);
        // Visibility is written only when it changes: a write each frame costs Explorer for nothing.
        let hidden = crate::lock(&HIDDEN).contains(&block);
        match &date {
            Some(date) => {
                if hidden {
                    xaml::set_visible(&diagnostics, block, true);
                    crate::lock(&HIDDEN).retain(|b| *b != block);
                }
                if xaml::text_of(&diagnostics, block).as_deref() != Some(date.as_str()) {
                    xaml::set_text(&diagnostics, block, date);
                }
            }
            None if !hidden => {
                xaml::set_visible(&diagnostics, block, false);
                crate::lock(&HIDDEN).push(block);
            }
            None => {}
        }
    }
    // A picture is shown whatever the count: the focus timer's pie comes with none.
    let badge = (!text.badge_image.is_empty()).then_some((text.badge_image.as_str(), text.badge_width, text.badge_scale));
    // On the bell's corner when Windows shows the bell, else beside the clock's lines.
    let bells = super::bells();
    for &bell in &bells {
        bell::show_bell_badge(&diagnostics, bell, badge);
        bell::cross_bell(&diagnostics, bell, text.quiet);
        bell::fill_bell(&diagnostics, bell, text.waiting);
    }
    for stack in tree::find_by_name(STACK) {
        show_badge(&diagnostics, stack, if bells.is_empty() { badge } else { None });
        // Hold each clock's width at the widest it drew, so the shell's own (shorter) text, shown
        // for a moment after each minute, does not move the taskbar.
        let width = xaml::actual_width(&diagnostics, stack).unwrap_or(0.0);
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
static BURST: AtomicU32 = AtomicU32::new(0);

/// About half a second of frames: the shell redraws its text within that of a hover.
const BURST_FRAMES: u32 = 30;

/// The pointer entered or left the clock: the shell may redraw its text then, so look every frame
/// for about half a second ([`BURST_FRAMES`]).
pub fn watch_closely() {
    // Not re-armed while one runs: sweeping across the clock would keep Explorer at 60 Hz.
    if crate::lock(&TEXT).is_some() && BURST.load(Ordering::SeqCst) == 0 {
        BURST.store(BURST_FRAMES, Ordering::SeqCst);
        crate::lifecycle::arm_clock_timer(16);
    }
}

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
