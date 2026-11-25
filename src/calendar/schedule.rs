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
    /// Audio Tray keeps a meeting's or a call's quiet.
    quiet_held: bool,
    /// The user ended that quiet (the moon, Windows' Quick Settings): left off until this meeting
    /// ends (its end), or this call (`None`: while the call lasts).
    quiet_off: Option<Option<i64>>,
}

/// Whether the user's "off" still stands for quiet wanted until `until` (in a call or not).
fn quiet_overridden(off: Option<Option<i64>>, until: i64, in_call: bool) -> bool {
    match off {
        Some(None) => in_call,
        Some(Some(end)) => until <= end,
        None => false,
    }
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

/// Whether an event starting at `start` is to be reminded of now, `ahead` seconds before (and up
/// to a minute after its start, for a tick that came late).
fn remind_now(start: i64, ahead: i64, now: i64) -> bool {
    start - ahead <= now && now < start + 60
}

/// A reminder's line: "in 5 min · Room 1", "now".
fn reminder_line(start: i64, now: i64, location: &str) -> String {
    let minutes = ((start - now) + 59) / 60;
    let mut line = if minutes > 0 { format!("in {minutes} min") } else { "now".to_string() };
    if !location.is_empty() {
        line.push_str(&format!(" · {location}"));
    }
    line
}

/// The end of the meeting under way, if quiet is kept in meetings.
fn meeting_quiet(events: &[Event], now: i64, in_meetings: bool) -> Option<i64> {
    in_meetings.then(|| events.iter().filter_map(span).filter(|(start, end)| *start <= now && now < *end).map(|(_, end)| end).max()).flatten()
}

/// Until when things are kept quiet now: the end of the meeting under way (with `in_meetings`),
/// else a while from now during a call (with `in_call`: its end is not known, so it moves on).
fn quiet_until(events: &[Event], now: i64, in_meetings: bool, in_call: bool) -> Option<i64> {
    match (meeting_quiet(events, now, in_meetings), in_call) {
        (Some(end), _) => Some(end),
        (None, true) => Some(now + MEETING_MAX),
        (None, false) => None,
    }
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
    // In the background every quarter hour (the panel opening refreshes sooner).
    let due = super::fetched_at().is_none_or(|at| now - at >= REFRESH_EVERY);
    if calendar.enabled && due && super::has_sources(&setup) {
        let (y, m, _) = time::civil_from_days(time::today());
        super::fetch(&setup, super::days_around(y, m), false);
    }
    let events = if calendar.enabled { visible(config) } else { Vec::new() };
    let mut next = now + REFRESH_EVERY;

    STATE.with(|cell| {
        let mut s = cell.borrow_mut();

        // The focus timer, ended by time, or stopped from outside (the moon, Windows' own toggle):
        // only one that ran its course says so.
        if let Some((end, minutes, held)) = s.focus {
            if now >= end {
                s.focus = None;
                focus::stop(Why::Timer);
                crate::hub::emit(crate::hub::Event::Notice(crate::hub::Notice::FocusDone { minutes }));
            } else if held && !focus::active() {
                s.focus = None;
                focus::stop(Why::Timer);
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
                if remind_now(start, ahead, now) && !s.reminded.contains(&key) {
                    s.reminded.insert(key);
                    let line = reminder_line(start, now, &event.location);
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
        let in_call = calendar.quiet_in_calls && s.in_call;
        let quiet_until = quiet_until(&events, now, calendar.quiet_in_meetings, in_call);
        match quiet_until {
            Some(until) if s.focus.is_none() => {
                let ours = focus::still_ours();
                if s.quiet_held && ours.is_none() {
                    // Ended by the user: theirs until this meeting or call is over.
                    s.quiet_held = false;
                    s.quiet_off = Some(meeting_quiet(&events, now, calendar.quiet_in_meetings));
                    println!("calendar: quiet switched off by hand, left off until this {} ends", if s.quiet_off == Some(None) { "call" } else { "meeting" });
                }
                // A call's end moves with every tick: extended only when it gained a while.
                if !quiet_overridden(s.quiet_off, until, in_call) && ours.is_none_or(|(why, end)| why == Why::Quiet && end + QUIET_SLACK < until) {
                    s.quiet_held = focus::start(Why::Quiet, until) || s.quiet_held;
                }
            }
            Some(_) => {}
            None => {
                focus::stop(Why::Quiet);
                s.quiet_held = false;
                s.quiet_off = None;
            }
        }
        for (start, end) in events.iter().filter_map(span) {
            for at in [start, end] {
                if at > now {
                    next = next.min(at);
                }
            }
        }

        // The clock says the time, nothing more: what waits and the focus timer are the bell's.
        let bell = super::merges_notices(config);
        let count = if bell { super::notifications::count() } else { 0 };
        // How many wait, as the notification card's header says. Only with the notification list
        // on: a badge pointing at nothing helps no one.
        let unread = if config.clock.unread { count } else { 0 };
        // With none waiting, a running focus timer shows as a pie on the bell, filling as it goes.
        let focus_done = s.focus.filter(|_| bell && unread == 0).map(|(end, minutes, _)| {
            let length = (minutes as i64 * 60).max(60);
            (1.0 - (end - now) as f32 / length as f32).clamp(0.0, 1.0)
        });
        if focus_done.is_some() {
            // The pie moves with the minute.
            next = next.min(now - now % 60 + 60);
        }
        let (time_pattern, date_pattern) = patterns(&config.clock);
        let badge_image = match focus_done {
            Some(done) => super::view::focus_pie(done),
            None if unread > 0 => super::view::badge(unread as u32),
            None => None,
        };
        // The bell crossed while Windows keeps notifications quiet, when it is the notifications' button.
        let quiet = bell && focus::active();
        // The bell filled while anything waits in Audio Tray's list, whatever its count badge: the
        // shell's fill says what *it* has not shown, never seen now that its centre stays shut.
        let waiting = bell.then_some(count > 0);
        let text = ClockText { time: time_pattern, date: date_pattern, one_line: config.clock.one_line, badge: unread as u32, badge_image, quiet, waiting };
        // Nothing to say: the shell's clock is the shell's (handed back if the TAP had it). While
        // the bell is Audio Tray's, the TAP keeps it (its fill follows Audio Tray's list).
        let wanted = config.clock.styled() || bell || text.badge > 0 || text.badge_image.is_some() || text.quiet;
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
                println!("calendar: the clock's bell now has {} waiting", text.badge);
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

/// Start the focus timer for `minutes`, with Windows' Do not disturb when it allows. `false`:
/// the timer runs, but Windows keeps notifications on.
pub fn start_focus(minutes: u32) -> bool {
    let end = time::now() + minutes as i64 * 60;
    let held = focus::start(Why::Timer, end);
    STATE.with(|cell| {
        let mut s = cell.borrow_mut();
        s.focus = Some((end, minutes, held));
        // The timer holds what quiet there was: its end is not the user ending the quiet.
        s.quiet_held = false;
    });
    held
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

    fn meeting(start: i64, end: i64) -> Event {
        Event {
            when: When::At { start, end },
            title: "Stand-up".into(),
            location: String::new(),
            link: None,
            join: None,
            source: super::super::Source::Google,
            color: [0; 3],
            calendar: String::new(),
            details: Default::default(),
        }
    }

    #[test]
    fn quiet_lasts_the_meeting_under_way_or_a_while_into_a_call() {
        let now = 1_000_000;
        let events = vec![meeting(now - 600, now + 1200), meeting(now + 3600, now + 7200), meeting(now - 3600, now + 5 * 3600)];
        assert_eq!(quiet_until(&events, now, true, false), Some(now + 1200), "the all-morning block is no meeting");
        assert_eq!(quiet_until(&events, now, false, false), None);
        assert_eq!(quiet_until(&[], now, true, true), Some(now + MEETING_MAX));
        assert_eq!(quiet_until(&events, now, true, true), Some(now + 1200), "the meeting's end is known");
    }

    #[test]
    fn quiet_switched_off_by_hand_stays_off_for_that_meeting_or_call() {
        let end = 1_000_000;
        // Off during a meeting: until it ends, not past it.
        assert!(quiet_overridden(Some(Some(end)), end, false));
        assert!(!quiet_overridden(Some(Some(end)), end + 1800, false), "the next meeting is quiet again");
        // Off during a call: while it lasts.
        assert!(quiet_overridden(Some(None), end + 3600, true));
        assert!(!quiet_overridden(Some(None), end, false));
        assert!(!quiet_overridden(None, end, true));
    }

    #[test]
    fn reminders_come_ahead_and_up_to_a_minute_late() {
        let start = 1_000_000;
        assert!(!remind_now(start, 300, start - 301));
        assert!(remind_now(start, 300, start - 300));
        assert!(remind_now(start, 300, start + 59));
        assert!(!remind_now(start, 300, start + 60));
        assert_eq!(reminder_line(start, start - 241, "Room 1"), "in 5 min · Room 1");
        assert_eq!(reminder_line(start, start + 10, ""), "now");
    }

    #[test]
    fn seconds_go_after_the_minutes_in_any_short_time_format() {
        assert_eq!(with_seconds("HH:mm"), "HH:mm:ss");
        assert_eq!(with_seconds("h:mm tt"), "h:mm:ss tt");
        assert_eq!(with_seconds("H.mm"), "H.mm:ss");
    }
}
