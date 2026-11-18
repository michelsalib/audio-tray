//! What the calendar does on its own, between clicks: the pill's meeting reminders, the clock's
//! text and badge, the focus timer, and Do not disturb during meetings and calls.
//!
//! One tray timer drives it, set by [`tick`] for the next moment anything changes (a countdown's
//! next minute, a reminder, an event starting or ending, the focus timer's end), never more often
//! than needed: with nothing coming up it sleeps until the next background fetch.

use std::cell::RefCell;
use std::collections::HashSet;
use std::time::Duration;

use super::focus::{self, Why};
use super::model::{Event, When};
use super::time;
use crate::config::Config;
use crate::taskbar::ClockText;

/// How often events are fetched in the background, for reminders and the clock.
const REFRESH_EVERY: i64 = 15 * 60;
/// Events longer than this are not meetings (an all-morning block, a trip).
const MEETING_MAX: i64 = 4 * 3600;
/// How much later a quiet time must end before it is pushed back.
const QUIET_SLACK: i64 = 10 * 60;

#[derive(Default)]
struct State {
    /// Events already reminded of, by title and start.
    reminded: HashSet<(String, i64)>,
    /// The focus timer: when it ends, how long it was, and whether Windows holds a session for it.
    focus: Option<(i64, u32, bool)>,
    /// What the TAP was last told, so an unchanged clock costs nothing.
    sent: Option<ClockText>,
    /// In a call, as the hub last said.
    in_call: bool,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

/// The events that count: shown calendars only.
fn visible(config: &Config) -> Vec<Event> {
    let hidden = &config.calendar.hidden;
    super::events().0.into_iter().filter(|e| !hidden.contains(&e.calendar)).collect()
}

/// A timed event's start and end, if it is a meeting-sized one.
fn span(event: &Event) -> Option<(i64, i64)> {
    match event.when {
        When::At { start, end } if end - start <= MEETING_MAX => Some((start, end)),
        _ => None,
    }
}

/// The text after the time: the focus time left, if a focus timer runs. Meetings stay off the
/// clock: the time is the time.
fn focus_left(now: i64, end: i64) -> String {
    format!("Focus {} min", ((end - now) + 59) / 60)
}

/// The clock's time and date patterns, from the `[clock]` settings and the locale's own formats.
pub fn patterns(clock: &crate::config::Clock) -> (String, String) {
    let time = if !clock.time_pattern.is_empty() {
        clock.time_pattern.clone()
    } else if clock.seconds {
        let short = locale_string(windows::Win32::Globalization::LOCALE_SSHORTTIME).unwrap_or_else(|| "HH:mm".into());
        with_seconds(&short)
    } else {
        String::new()
    };
    let date = if clock.hide_date {
        "-".to_string()
    } else if !clock.date_pattern.is_empty() {
        clock.date_pattern.clone()
    } else if clock.weekday {
        let short = locale_string(windows::Win32::Globalization::LOCALE_SSHORTDATE).unwrap_or_else(|| "dd/MM/yyyy".into());
        format!("ddd {short}")
    } else {
        String::new()
    };
    (time, date)
}

/// A short time pattern with the seconds after the minutes: `HH:mm` → `HH:mm:ss`.
fn with_seconds(short: &str) -> String {
    match short.find("mm") {
        Some(at) => format!("{}mm:ss{}", &short[..at], &short[at + 2..]),
        None => format!("{short}:ss"),
    }
}

fn locale_string(what: u32) -> Option<String> {
    use windows::core::PCWSTR;
    use windows::Win32::Globalization::GetLocaleInfoEx;
    let mut buf = [0u16; 80];
    let n = unsafe { GetLocaleInfoEx(PCWSTR::null(), what, Some(&mut buf)) };
    (n > 1).then(|| String::from_utf16_lossy(&buf[..n as usize - 1]))
}

/// Do the work due now, and say when to look again.
pub fn tick(config: &Config) -> Duration {
    let now = time::now();
    let calendar = &config.calendar;
    let setup = calendar.setup();
    if calendar.enabled && super::has_sources(&setup) {
        let (y, m, _) = time::civil_from_days(time::today());
        super::fetch(&setup, super::days_around(y, m), false);
    }
    let events = if calendar.enabled { visible(config) } else { Vec::new() };
    let mut next = now + REFRESH_EVERY;

    STATE.with(|cell| {
        let mut s = cell.borrow_mut();

        // The focus timer, ended by time or by Windows (the user stopped the session there).
        if let Some((end, minutes, held)) = s.focus {
            if now >= end || (held && !focus::active()) {
                s.focus = None;
                focus::stop(Why::Timer);
                crate::hub::emit(crate::hub::Event::Notice(crate::hub::Notice::FocusDone { minutes }));
            } else {
                next = next.min(end);
            }
        }

        // Reminders.
        let ahead = calendar.remind_minutes as i64 * 60;
        if ahead > 0 {
            for event in &events {
                let Some((start, _)) = span(event) else { continue };
                let key = (event.title.clone(), start);
                if start - ahead <= now && now < start + 60 && !s.reminded.contains(&key) {
                    s.reminded.insert(key);
                    let minutes = ((start - now) + 59) / 60;
                    let mut line = if minutes > 0 { format!("in {minutes} min") } else { "now".to_string() };
                    if !event.location.is_empty() {
                        line.push_str(&format!(" · {}", event.location));
                    }
                    let url = event.join.clone().or_else(|| event.link.clone());
                    let lasts = (start - now).max(0) as u64 + 120;
                    crate::hub::emit(crate::hub::Event::Notice(crate::hub::Notice::Meeting { title: event.title.clone(), line, url, lasts }));
                } else if start - ahead > now {
                    next = next.min(start - ahead);
                }
            }
            s.reminded.retain(|(_, start)| *start + 3600 > now);
        }

        // Quiet: during a meeting, or a call.
        let meeting = events.iter().filter_map(span).filter(|(start, end)| *start <= now && now < *end).map(|(_, end)| end).max();
        let quiet_until = match (calendar.quiet_in_meetings.then_some(meeting).flatten(), calendar.quiet_in_calls && s.in_call) {
            (Some(end), _) => Some(end),
            (None, true) => Some(now + MEETING_MAX),
            (None, false) => None,
        };
        match quiet_until {
            Some(until) if s.focus.is_none() => {
                // A call's end moves with every tick: extended only when it gained a while.
                if focus::ours().is_none_or(|(why, end)| why == Why::Quiet && end + QUIET_SLACK < until) {
                    focus::start(Why::Quiet, until);
                }
            }
            Some(_) => {}
            None => focus::stop(Why::Quiet),
        }
        for (start, end) in events.iter().filter_map(span) {
            for at in [start, end] {
                if at > now {
                    next = next.min(at);
                }
            }
        }

        // The clock.
        let top = s.focus.map_or_else(String::new, |(end, _, _)| focus_left(now, end));
        let unread = if config.calendar.enabled && config.clock.unread { super::notifications::count() } else { 0 };
        if !top.is_empty() {
            // The countdown moves with the minute.
            next = next.min(now - now % 60 + 60);
        }
        let (time_pattern, date_pattern) = patterns(&config.clock);
        let badge_image = if unread > 0 { super::view::badge(unread as u32) } else { None };
        let text = ClockText { time: time_pattern, date: date_pattern, one_line: config.clock.one_line, top, badge: unread as u32, badge_image };
        // Nothing to say: the shell's clock is the shell's (handed back if the TAP had it).
        let wanted = config.clock.styled() || !text.top.is_empty() || text.badge > 0;
        if !wanted {
            if s.sent.is_some() {
                if crate::taskbar::release_clock() {
                    println!("calendar: the clock is the shell's again");
                    s.sent = None;
                } else {
                    next = next.min(now + 5);
                }
            }
        } else if s.sent.as_ref() != Some(&text) {
            if crate::taskbar::send_clock(&text) {
                println!("calendar: the clock now reads {:?}, {} unread", text.top, text.badge);
                s.sent = Some(text);
            } else {
                // The TAP is busy or reloading: soon again, not at the next quarter hour.
                eprintln!("calendar: the TAP did not take the clock's text");
                next = next.min(now + 5);
            }
        }
    });
    Duration::from_secs((next - now).clamp(1, REFRESH_EVERY) as u64)
}

/// The TAP was reloaded (an Explorer restart, a fresh injection): it must be told again.
pub fn forget_clock() {
    STATE.with(|cell| cell.borrow_mut().sent = None);
}

/// The hub's calls changed: in one or not.
pub fn set_in_call(in_call: bool) -> bool {
    STATE.with(|cell| std::mem::replace(&mut cell.borrow_mut().in_call, in_call) != in_call)
}

/// Start the focus timer for `minutes`, with Windows' Do not disturb when it allows.
pub fn start_focus(minutes: u32) {
    let end = time::now() + minutes as i64 * 60;
    let held = focus::start(Why::Timer, end);
    STATE.with(|cell| cell.borrow_mut().focus = Some((end, minutes, held)));
}

pub fn stop_focus() {
    focus::stop(Why::Timer);
    STATE.with(|cell| cell.borrow_mut().focus = None);
}

/// The focus timer's end, if it runs.
pub fn focus_end() -> Option<i64> {
    STATE.with(|cell| cell.borrow().focus.map(|(end, ..)| end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_focus_time_left_rounds_up_to_the_minute() {
        let now = 1_000_000;
        assert_eq!(focus_left(now, now + 18 * 60), "Focus 18 min");
        assert_eq!(focus_left(now, now + 17 * 60 + 1), "Focus 18 min");
    }

    #[test]
    fn seconds_go_after_the_minutes_in_any_short_time_format() {
        assert_eq!(with_seconds("HH:mm"), "HH:mm:ss");
        assert_eq!(with_seconds("h:mm tt"), "h:mm:ss tt");
        assert_eq!(with_seconds("H.mm"), "H.mm:ss");
    }
}
