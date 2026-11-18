//! iCalendar (RFC 5545) feeds, as Google's "secret address" and Outlook's published calendars
//! serve them: their events, with recurring ones expanded over the days asked for.
//!
//! Times with a `TZID` are read on this PC's clock: a feed's own `VTIMEZONE` rules are not
//! interpreted, which is right for every calendar kept in the user's own time zone.

use std::collections::HashSet;

use super::model::{Details, When};
use super::time::{self, days_from_civil, Zone, DAY};

/// One event (or occurrence) in a feed.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub when: When,
    pub title: String,
    pub location: String,
    pub link: Option<String>,
    /// A Teams, Meet or Zoom address found in its description or location.
    pub join: Option<String>,
    pub details: Details,
    /// Its `UID`: the provider's own id (`…@google.com` for Google's feeds).
    pub uid: String,
}

/// A date-time as the feed writes it.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Stamp {
    /// A whole day.
    Date(i32),
    /// A wall-clock time (floating, or in a named zone taken to be ours).
    Wall(i64),
    Utc(i64),
}

impl Stamp {
    /// Its local wall-clock time.
    fn wall(self, zone: &dyn Zone) -> i64 {
        match self {
            Stamp::Date(d) => d as i64 * DAY,
            Stamp::Wall(w) => w,
            Stamp::Utc(u) => zone.to_local(u),
        }
    }
}

#[derive(Default)]
struct Raw {
    uid: String,
    title: String,
    location: String,
    url: Option<String>,
    start: Option<Stamp>,
    end: Option<Stamp>,
    duration: Option<i64>,
    rule: Option<String>,
    exdates: Vec<Stamp>,
    recurrence_id: Option<Stamp>,
    cancelled: bool,
    description: String,
    organizer: String,
    attendees: Vec<String>,
}

/// The events of `text` that touch the local days `[from, to)`.
pub fn parse(text: &str, (from, to): (i32, i32), zone: &dyn Zone) -> Vec<Item> {
    let raws = read_events(text);
    // Moved or changed occurrences of a series replace the one they name.
    let overridden: HashSet<(String, i64)> = raws
        .iter()
        .filter_map(|r| r.recurrence_id.map(|id| (r.uid.clone(), id.wall(zone))))
        .collect();
    let mut out = Vec::new();
    for raw in &raws {
        if raw.cancelled {
            continue;
        }
        let Some(start) = raw.start else { continue };
        let all_day = matches!(start, Stamp::Date(_));
        let start_wall = start.wall(zone);
        // The length: whole days for a dated event, real seconds for a timed one.
        let (days_long, secs_long) = match (start, raw.end) {
            (Stamp::Date(a), Some(Stamp::Date(b))) => ((b - a).max(1), 0),
            (Stamp::Date(_), _) => ((raw.duration.unwrap_or(DAY) / DAY).max(1) as i32, 0),
            (_, Some(end)) => (0, (utc_of(end, zone) - utc_of(start, zone)).max(0)),
            (_, None) => (0, raw.duration.unwrap_or(0).max(0)),
        };
        let starts: Vec<i64> = match raw.rule.as_deref().and_then(Rule::parse).filter(|_| raw.recurrence_id.is_none()) {
            Some(rule) => {
                let exdates: Vec<i64> = raw.exdates.iter().map(|s| s.wall(zone)).collect();
                let ex_days: Vec<i32> = raw.exdates.iter().filter_map(|s| if let Stamp::Date(d) = s { Some(*d) } else { None }).collect();
                // An occurrence ending after `from` may start this many days before it.
                let reach = if all_day { days_long } else { (secs_long / DAY) as i32 + 1 };
                rule.expand(start_wall, (from - reach, to), zone)
                    .into_iter()
                    .filter(|w| !exdates.contains(w) && !ex_days.contains(&time::day_of(*w)))
                    .filter(|w| !overridden.contains(&(raw.uid.clone(), *w)))
                    .collect()
            }
            None => vec![start_wall],
        };
        for wall in starts {
            let when = if all_day {
                let first = time::day_of(wall);
                When::AllDay { first, end: first + days_long }
            } else {
                // Each occurrence keeps the series' wall-clock time across DST changes.
                let start = match start {
                    Stamp::Utc(_) | Stamp::Wall(_) if raw.rule.is_some() => zone.to_utc(wall),
                    _ => utc_of(start, zone),
                };
                When::At { start, end: start + secs_long }
            };
            let (first, last) = match when {
                When::AllDay { first, end } => (first, end - 1),
                When::At { start, end } => (time::day_of(zone.to_local(start)), time::day_of(zone.to_local(end.max(start + 1) - 1))),
            };
            if first < to && last >= from {
                let details = Details { people: raw.attendees.clone(), organizer: raw.organizer.clone(), response: String::new(), notes: raw.description.clone() };
                let join = find_join(&raw.description).or_else(|| find_join(&raw.location));
                out.push(Item { when, title: raw.title.clone(), location: raw.location.clone(), link: raw.url.clone(), join, details, uid: raw.uid.clone() });
            }
        }
    }
    out
}

fn utc_of(stamp: Stamp, zone: &dyn Zone) -> i64 {
    match stamp {
        Stamp::Date(d) => zone.to_utc(d as i64 * DAY),
        Stamp::Wall(w) => zone.to_utc(w),
        Stamp::Utc(u) => u,
    }
}

/// Every `VEVENT`, its properties read.
fn read_events(text: &str) -> Vec<Raw> {
    let mut events = Vec::new();
    let mut current: Option<Raw> = None;
    let mut depth = 0usize; // components nested inside the event (alarms)
    for line in unfold(text) {
        let Some((name, params, value)) = split_line(&line) else { continue };
        match (name.as_str(), value) {
            ("BEGIN", "VEVENT") => {
                current = Some(Raw::default());
                depth = 0;
            }
            ("END", "VEVENT") => {
                if let Some(raw) = current.take() {
                    events.push(raw);
                }
            }
            ("BEGIN", _) if current.is_some() => depth += 1,
            ("END", _) if current.is_some() => depth = depth.saturating_sub(1),
            _ => {
                let Some(raw) = current.as_mut().filter(|_| depth == 0) else { continue };
                match name.as_str() {
                    "UID" => raw.uid = value.to_string(),
                    "SUMMARY" => raw.title = unescape(value),
                    "LOCATION" => raw.location = unescape(value),
                    "URL" => raw.url = Some(value.to_string()),
                    "DTSTART" => raw.start = stamp(value, &params),
                    "DTEND" => raw.end = stamp(value, &params),
                    "DURATION" => raw.duration = duration(value),
                    "RRULE" => raw.rule = Some(value.to_string()),
                    "EXDATE" => raw.exdates.extend(value.split(',').filter_map(|v| stamp(v, &params))),
                    "RECURRENCE-ID" => raw.recurrence_id = stamp(value, &params),
                    "STATUS" => raw.cancelled = value.eq_ignore_ascii_case("CANCELLED"),
                    "DESCRIPTION" => raw.description = unescape_lines(value),
                    "ORGANIZER" => raw.organizer = person(value, &params),
                    "ATTENDEE" => raw.attendees.push(person(value, &params)),
                    _ => {}
                }
            }
        }
    }
    events
}

/// Content lines, with folded continuations joined back.
fn unfold(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        match line.strip_prefix([' ', '\t']) {
            Some(rest) if !lines.is_empty() => lines.last_mut().expect("not empty").push_str(rest),
            _ => lines.push(line.to_string()),
        }
    }
    lines
}

/// `NAME;PARAM=V;…:value`, the name upper-cased; a `:` inside a quoted parameter is not the split.
/// A content line's parameters, names upper-cased.
type Params = Vec<(String, String)>;

fn split_line(line: &str) -> Option<(String, Params, &str)> {
    let mut quoted = false;
    let colon = line.char_indices().find(|&(_, c)| {
        if c == '"' {
            quoted = !quoted;
        }
        c == ':' && !quoted
    })?;
    let (head, value) = (&line[..colon.0], &line[colon.0 + 1..]);
    let mut parts = head.split(';');
    let name = parts.next()?.trim().to_ascii_uppercase();
    let params = parts
        .filter_map(|p| p.split_once('='))
        .map(|(k, v)| (k.trim().to_ascii_uppercase(), v.trim_matches('"').to_string()))
        .collect();
    Some((name, params, value))
}

fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n' | 'N') => out.push(' '),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out.trim().to_string()
}

/// A text value with its line breaks kept.
fn unescape_lines(text: &str) -> String {
    unescape(&text.replace("\\n", "\n").replace("\\N", "\n"))
}

/// An organiser or attendee: its `CN`, else its address.
fn person(value: &str, params: &[(String, String)]) -> String {
    params
        .iter()
        .find(|(k, _)| k == "CN")
        .map(|(_, v)| v.clone())
        .unwrap_or_else(|| value.trim_start_matches("mailto:").trim_start_matches("MAILTO:").to_string())
}

/// The first meeting address (Teams, Meet, Zoom, Webex) in `text`.
pub fn find_join(text: &str) -> Option<String> {
    const HOSTS: [&str; 5] = ["https://teams.microsoft.com/l/meetup-join", "https://teams.live.com/meet", "https://meet.google.com/", ".zoom.us/j/", ".webex.com/"];
    HOSTS.iter().filter_map(|host| text.find(host)).min().map(|at| {
        let start = text[..at].rfind("https://").filter(|_| !text[at..].starts_with("https://")).unwrap_or(at);
        let end = text[start..].find(|c: char| c.is_whitespace() || matches!(c, '>' | '"' | '<' | ')')).map_or(text.len(), |n| start + n);
        text[start..end].to_string()
    })
}

/// `20261113`, `20261113T090000` or `20261113T090000Z`.
fn stamp(value: &str, params: &[(String, String)]) -> Option<Stamp> {
    let value = value.trim();
    let digits = |s: &str| s.parse::<i64>().ok();
    let (y, m, d) = (digits(value.get(0..4)?)?, digits(value.get(4..6)?)?, digits(value.get(6..8)?)?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let day = days_from_civil(y as i32, m as u32, d as u32);
    let is_date = params.iter().any(|(k, v)| k == "VALUE" && v.eq_ignore_ascii_case("DATE")) || value.len() == 8;
    if is_date {
        return Some(Stamp::Date(day));
    }
    let clock = value.get(9..15)?;
    let part = |r: std::ops::Range<usize>| clock.get(r).and_then(digits);
    let secs = day as i64 * DAY + part(0..2)? * 3600 + part(2..4)? * 60 + part(4..6)?;
    let utc_zone = params.iter().any(|(k, v)| k == "TZID" && matches!(v.as_str(), "UTC" | "Etc/UTC" | "GMT" | "Etc/GMT" | "Z"));
    Some(if value.ends_with('Z') || utc_zone { Stamp::Utc(secs) } else { Stamp::Wall(secs) })
}

/// An RFC 5545 duration (`P1D`, `PT1H30M`, `P2W`) in seconds.
fn duration(value: &str) -> Option<i64> {
    let (sign, rest) = match value.trim().strip_prefix('-') {
        Some(rest) => (-1, rest),
        None => (1, value.trim().trim_start_matches('+')),
    };
    let rest = rest.strip_prefix('P')?;
    let mut total = 0i64;
    let mut number = String::new();
    for c in rest.chars() {
        match c {
            '0'..='9' => number.push(c),
            'T' => {}
            unit => {
                let n: i64 = number.parse().ok()?;
                number.clear();
                let unit = match unit {
                    'W' => 7 * DAY,
                    'D' => DAY,
                    'H' => 3600,
                    'M' => 60,
                    'S' => 1,
                    _ => return None,
                };
                // A feed's nonsense (`P99999999999999W`) is no duration, not an overflow.
                total = total.checked_add(n.checked_mul(unit)?)?;
            }
        }
    }
    Some(sign * total)
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Freq {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

/// The parts of an `RRULE` this reads.
#[derive(Debug)]
struct Rule {
    freq: Freq,
    interval: i32,
    count: Option<u32>,
    until: Option<Stamp>,
    /// `(ordinal, weekday)`: ordinal 0 is every such weekday, else the nth (from the end if negative).
    by_day: Vec<(i32, u32)>,
    by_month_day: Vec<i32>,
    by_month: Vec<u32>,
    by_set_pos: Vec<i32>,
    week_start: u32,
}

/// How many periods a series is followed for at most, from its start.
const MAX_PERIODS: i32 = 50_000;

impl Rule {
    fn parse(text: &str) -> Option<Rule> {
        let mut rule = Rule {
            freq: Freq::Daily,
            interval: 1,
            count: None,
            until: None,
            by_day: Vec::new(),
            by_month_day: Vec::new(),
            by_month: Vec::new(),
            by_set_pos: Vec::new(),
            week_start: 0,
        };
        let mut freq = None;
        for part in text.split(';') {
            let Some((key, value)) = part.split_once('=') else { continue };
            let list = || value.split(',').filter_map(|v| v.trim().parse::<i32>().ok());
            match key.trim().to_ascii_uppercase().as_str() {
                "FREQ" => {
                    freq = Some(match value {
                        "DAILY" => Freq::Daily,
                        "WEEKLY" => Freq::Weekly,
                        "MONTHLY" => Freq::Monthly,
                        "YEARLY" => Freq::Yearly,
                        // Hourly and finer are not offered by calendar apps.
                        _ => return None,
                    })
                }
                // Capped: a series every thousand periods is as rare as never, and the sums stay small.
                "INTERVAL" => rule.interval = value.parse::<i32>().ok()?.clamp(1, 1000),
                "COUNT" => rule.count = value.parse().ok(),
                "UNTIL" => rule.until = stamp(value, &[]),
                "BYDAY" => rule.by_day = value.split(',').filter_map(by_day).collect(),
                "BYMONTHDAY" => rule.by_month_day = list().collect(),
                "BYMONTH" => rule.by_month = list().map(|m| m as u32).collect(),
                "BYSETPOS" => rule.by_set_pos = list().collect(),
                "WKST" => rule.week_start = weekday_code(value)?,
                _ => {}
            }
        }
        rule.freq = freq?;
        Some(rule)
    }

    /// Wall-clock starts of the occurrences of a series starting at `start` whose day falls in
    /// `[from, to)`, counting from the series' start.
    fn expand(&self, start: i64, (from, to): (i32, i32), zone: &dyn Zone) -> Vec<i64> {
        let first_day = time::day_of(start);
        let clock = time::second_of_day(start);
        let until = self.until.map(|u| u.wall(zone));
        let (sy, sm, sd) = time::civil_from_days(first_day);
        let mut out = Vec::new();
        let mut counted = 0u32;
        for k in 0..MAX_PERIODS {
            let n = k * self.interval;
            let mut days = match self.freq {
                Freq::Daily => {
                    let day = first_day + n;
                    if self.weekday_ok(day) && self.month_day_ok(day) { vec![day] } else { vec![] }
                }
                Freq::Weekly => {
                    let week = first_day - ((time::weekday(first_day) + 7 - self.week_start) % 7) as i32 + 7 * n;
                    let weekdays: Vec<u32> = if self.by_day.is_empty() { vec![time::weekday(first_day)] } else { self.by_day.iter().map(|&(_, w)| w).collect() };
                    weekdays.into_iter().map(|w| week + ((w + 7 - self.week_start) % 7) as i32).collect()
                }
                Freq::Monthly => {
                    let (y, m) = time::add_months(sy, sm, n);
                    self.month_days(y, m, sd)
                }
                Freq::Yearly => {
                    let y = sy + n;
                    let months = if self.by_month.is_empty() { vec![sm] } else { self.by_month.clone() };
                    months.into_iter().flat_map(|m| self.month_days(y, m, sd)).collect()
                }
            };
            days.retain(|&d| self.by_month.is_empty() || self.by_month.contains(&time::civil_from_days(d).1));
            days.sort_unstable();
            days.dedup();
            let days = self.set_positions(days);
            let period_start = days.first().copied();
            for day in days {
                if day < first_day {
                    continue;
                }
                let wall = day as i64 * DAY + clock;
                if until.is_some_and(|u| wall > u) || self.count.is_some_and(|c| counted >= c) {
                    return out;
                }
                counted += 1;
                if day >= from && day < to {
                    out.push(wall);
                }
            }
            // Past the window with nothing left to count: done.
            if self.past(period_start, to, first_day, n) {
                break;
            }
        }
        out
    }

    /// Whether every later period starts on or after `to`.
    fn past(&self, period_start: Option<i32>, to: i32, first_day: i32, n: i32) -> bool {
        match period_start {
            Some(day) => day >= to,
            // An empty period (no matching day this month): judge by how far the periods have run.
            None => match self.freq {
                Freq::Daily => first_day + n >= to,
                Freq::Weekly => first_day + 7 * n >= to,
                Freq::Monthly => first_day + 28 * n >= to,
                Freq::Yearly => first_day + 365 * n >= to,
            },
        }
    }

    fn weekday_ok(&self, day: i32) -> bool {
        self.by_day.is_empty() || self.by_day.iter().any(|&(_, w)| w == time::weekday(day))
    }

    fn month_day_ok(&self, day: i32) -> bool {
        if self.by_month_day.is_empty() {
            return true;
        }
        let (y, m, d) = time::civil_from_days(day);
        let len = time::days_in_month(y, m) as i32;
        self.by_month_day.iter().any(|&n| if n > 0 { n == d as i32 } else { len + n + 1 == d as i32 })
    }

    /// The days of month `(y, m)` the rule picks; the series' own day of month by default.
    fn month_days(&self, y: i32, m: u32, default: u32) -> Vec<i32> {
        let len = time::days_in_month(y, m) as i32;
        let first = days_from_civil(y, m, 1);
        if !self.by_month_day.is_empty() {
            return self
                .by_month_day
                .iter()
                .map(|&n| if n > 0 { n } else { len + n + 1 })
                .filter(|&d| (1..=len).contains(&d))
                .map(|d| first + d - 1)
                .filter(|&day| self.weekday_ok(day))
                .collect();
        }
        if !self.by_day.is_empty() {
            let mut out = Vec::new();
            for &(ordinal, weekday) in &self.by_day {
                let matching: Vec<i32> = (0..len).map(|i| first + i).filter(|&d| time::weekday(d) == weekday).collect();
                match ordinal {
                    0 => out.extend(matching),
                    n if n > 0 => out.extend(matching.get(n as usize - 1)),
                    n => out.extend(matching.len().checked_sub(n.unsigned_abs() as usize).and_then(|i| matching.get(i))),
                }
            }
            return out;
        }
        if default as i32 <= len { vec![first + default as i32 - 1] } else { vec![] }
    }

    fn set_positions(&self, days: Vec<i32>) -> Vec<i32> {
        if self.by_set_pos.is_empty() {
            return days;
        }
        let len = days.len() as i32;
        let mut picked: Vec<i32> = self
            .by_set_pos
            .iter()
            .filter_map(|&p| {
                let i = if p > 0 { p - 1 } else { len + p };
                (0..len).contains(&i).then(|| days[i as usize])
            })
            .collect();
        picked.sort_unstable();
        picked.dedup();
        picked
    }
}

/// `MO`, `2TU`, `-1FR`.
fn by_day(text: &str) -> Option<(i32, u32)> {
    let text = text.trim();
    let split = text.len().checked_sub(2)?;
    let (ordinal, code) = (text.get(..split)?, text.get(split..)?);
    let ordinal = if ordinal.is_empty() { 0 } else { ordinal.trim_start_matches('+').parse().ok()? };
    Some((ordinal, weekday_code(code)?))
}

fn weekday_code(code: &str) -> Option<u32> {
    ["MO", "TU", "WE", "TH", "FR", "SA", "SU"].iter().position(|c| c.eq_ignore_ascii_case(code.trim())).map(|i| i as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calendar::time::Fixed;

    fn feed(events: &str) -> String {
        format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n{events}END:VCALENDAR\r\n")
    }

    fn day(y: i32, m: u32, d: u32) -> i32 {
        days_from_civil(y, m, d)
    }

    fn starts(items: &[Item], zone: &dyn Zone) -> Vec<(i32, i64)> {
        let mut out: Vec<(i32, i64)> = items
            .iter()
            .map(|i| match i.when {
                When::At { start, .. } => {
                    let wall = zone.to_local(start);
                    (time::day_of(wall), time::second_of_day(wall) / 60)
                }
                When::AllDay { first, .. } => (first, -1),
            })
            .collect();
        out.sort();
        out
    }

    #[test]
    fn single_events_in_utc_local_and_whole_days() {
        let text = feed(
            "BEGIN:VEVENT\r\nUID:a\r\nSUMMARY:Stand-up\\, daily\r\nDTSTART:20261113T090000Z\r\nDTEND:20261113T093000Z\r\nLOCATION:Room 1\r\nEND:VEVENT\r\n\
             BEGIN:VEVENT\r\nUID:b\r\nSUMMARY:Lunch\r\nDTSTART;TZID=Romance Standard Time:20261113T120000\r\nDURATION:PT1H\r\nEND:VEVENT\r\n\
             BEGIN:VEVENT\r\nUID:c\r\nSUMMARY:Holiday\r\nDTSTART;VALUE=DATE:20261111\r\nDTEND;VALUE=DATE:20261112\r\nEND:VEVENT\r\n",
        );
        let zone = Fixed(3600);
        let items = parse(&text, (day(2026, 11, 9), day(2026, 11, 16)), &zone);
        assert_eq!(items.len(), 3);
        let standup = items.iter().find(|i| i.title == "Stand-up, daily").unwrap();
        assert_eq!(standup.location, "Room 1");
        assert_eq!(starts(&items, &zone), vec![(day(2026, 11, 11), -1), (day(2026, 11, 13), 600), (day(2026, 11, 13), 720)]);
    }

    #[test]
    fn folded_lines_and_alarms_do_not_confuse_the_event() {
        let text = feed(
            "BEGIN:VEVENT\r\nUID:a\r\nSUMMARY:A very long\r\n  title\r\nDTSTART:20261113T090000Z\r\nBEGIN:VALARM\r\nSUMMARY:Alarm\r\nEND:VALARM\r\nEND:VEVENT\r\n",
        );
        let items = parse(&text, (day(2026, 11, 1), day(2026, 12, 1)), &Fixed(0));
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "A very long title");
    }

    #[test]
    fn a_weekly_series_skips_its_exceptions_and_takes_its_moved_occurrence() {
        let text = feed(
            "BEGIN:VEVENT\r\nUID:w\r\nSUMMARY:Sync\r\nDTSTART:20261102T100000\r\nDTEND:20261102T110000\r\nRRULE:FREQ=WEEKLY;BYDAY=MO,WE\r\nEXDATE:20261104T100000\r\nEND:VEVENT\r\n\
             BEGIN:VEVENT\r\nUID:w\r\nSUMMARY:Sync (moved)\r\nRECURRENCE-ID:20261109T100000\r\nDTSTART:20261109T150000\r\nDTEND:20261109T160000\r\nEND:VEVENT\r\n",
        );
        let zone = Fixed(0);
        let items = parse(&text, (day(2026, 11, 2), day(2026, 11, 16)), &zone);
        assert_eq!(
            starts(&items, &zone),
            vec![(day(2026, 11, 2), 600), (day(2026, 11, 9), 900), (day(2026, 11, 11), 600)]
        );
        assert!(items.iter().any(|i| i.title == "Sync (moved)"));
    }

    #[test]
    fn counts_and_until_end_a_series_counted_from_its_start() {
        let text = feed(
            "BEGIN:VEVENT\r\nUID:d\r\nSUMMARY:Daily\r\nDTSTART;VALUE=DATE:20261101\r\nRRULE:FREQ=DAILY;COUNT=5\r\nEND:VEVENT\r\n\
             BEGIN:VEVENT\r\nUID:u\r\nSUMMARY:Until\r\nDTSTART:20261101T080000Z\r\nRRULE:FREQ=DAILY;INTERVAL=2;UNTIL=20261107T080000Z\r\nEND:VEVENT\r\n",
        );
        let zone = Fixed(0);
        let items = parse(&text, (day(2026, 11, 3), day(2026, 11, 30)), &zone);
        let daily: Vec<_> = items.iter().filter(|i| i.title == "Daily").collect();
        assert_eq!(daily.len(), 3, "the 3rd, 4th and 5th of five");
        let until: Vec<_> = items.iter().filter(|i| i.title == "Until").collect();
        assert_eq!(until.len(), 3, "the 3rd, 5th and 7th");
    }

    #[test]
    fn monthly_by_weekday_ordinal_and_set_position() {
        let text = feed(
            "BEGIN:VEVENT\r\nUID:m\r\nSUMMARY:Last Friday\r\nDTSTART:20260130T170000\r\nRRULE:FREQ=MONTHLY;BYDAY=-1FR\r\nEND:VEVENT\r\n\
             BEGIN:VEVENT\r\nUID:p\r\nSUMMARY:Last workday\r\nDTSTART:20260130T090000\r\nRRULE:FREQ=MONTHLY;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=-1\r\nEND:VEVENT\r\n\
             BEGIN:VEVENT\r\nUID:y\r\nSUMMARY:Birthday\r\nDTSTART;VALUE=DATE:19900229\r\nRRULE:FREQ=YEARLY\r\nEND:VEVENT\r\n",
        );
        let zone = Fixed(0);
        let items = parse(&text, (day(2026, 11, 1), day(2026, 12, 1)), &zone);
        let on = |title: &str| starts(&items.iter().filter(|i| i.title == title).cloned().collect::<Vec<_>>(), &zone);
        assert_eq!(on("Last Friday"), vec![(day(2026, 11, 27), 1020)]);
        assert_eq!(on("Last workday"), vec![(day(2026, 11, 30), 540)]);
        assert!(on("Birthday").is_empty());
    }

    #[test]
    fn a_series_keeps_its_wall_clock_time_across_a_dst_change() {
        // A zone that is UTC+2 before day X and UTC+1 from it.
        struct Dst(i32);
        impl Zone for Dst {
            fn to_local(&self, utc: i64) -> i64 {
                utc + if utc < self.0 as i64 * DAY { 7200 } else { 3600 }
            }
            fn to_utc(&self, wall: i64) -> i64 {
                wall - if wall < self.0 as i64 * DAY { 7200 } else { 3600 }
            }
        }
        let zone = Dst(day(2026, 10, 25));
        let text = feed("BEGIN:VEVENT\r\nUID:s\r\nSUMMARY:Gym\r\nDTSTART:20261021T070000Z\r\nRRULE:FREQ=WEEKLY\r\nEND:VEVENT\r\n");
        let items = parse(&text, (day(2026, 10, 19), day(2026, 11, 2)), &zone);
        assert_eq!(starts(&items, &zone), vec![(day(2026, 10, 21), 540), (day(2026, 10, 28), 540)]);
    }

    #[test]
    fn descriptions_people_and_meeting_links_are_read() {
        let text = feed(concat!(
            "BEGIN:VEVENT\r\nUID:j\r\nSUMMARY:Sync\r\nDTSTART:20261113T090000Z\r\n",
            "ORGANIZER;CN=Ana:mailto:ana@x.com\r\nATTENDEE;CN=Jo;PARTSTAT=ACCEPTED:mailto:jo@x.com\r\nATTENDEE:mailto:bo@x.com\r\n",
            "DESCRIPTION:Join here\\nhttps://teams.microsoft.com/l/meetup-join/abc%40thread> \r\n more\r\nEND:VEVENT\r\n",
        ));
        let items = parse(&text, (day(2026, 11, 1), day(2026, 12, 1)), &Fixed(0));
        let item = &items[0];
        assert_eq!(item.details.organizer, "Ana");
        assert_eq!(item.details.people, vec!["Jo".to_string(), "bo@x.com".to_string()]);
        assert_eq!(item.join.as_deref(), Some("https://teams.microsoft.com/l/meetup-join/abc%40thread"));
        assert!(item.details.notes.starts_with("Join here\n"));
        assert_eq!(find_join("call on https://us02web.zoom.us/j/123?pwd=x now").as_deref(), Some("https://us02web.zoom.us/j/123?pwd=x"));
    }

    #[test]
    fn durations_parse() {
        assert_eq!(duration("PT1H30M"), Some(5400));
        assert_eq!(duration("P1D"), Some(DAY));
        assert_eq!(duration("P2W"), Some(14 * DAY));
        assert_eq!(duration("-PT15M"), Some(-900));
    }

    #[test]
    fn a_hostile_feed_is_read_as_far_as_it_makes_sense_and_never_panics() {
        assert_eq!(stamp("20261113Té90000", &[]), None, "a split character in the time");
        assert_eq!(stamp("20261113T0900", &[]), None, "a truncated time");
        assert_eq!(by_day("é1"), None);
        assert_eq!(by_day("1é"), None);
        assert_eq!(duration("P99999999999999W"), None);
        assert_eq!(duration("PT9223372036854775807H"), None);
        let text = feed(
            "BEGIN:VEVENT\r\nUID:a\r\nSUMMARY:Huge\r\nDTSTART:20261113T090000Z\r\nDURATION:PT1H\r\nRRULE:FREQ=YEARLY;INTERVAL=2147483647\r\nEND:VEVENT\r\n\
             BEGIN:VEVENT\r\nUID:b\r\nSUMMARY:Never\r\nDTSTART:20261113T090000Z\r\nDURATION:PT1H\r\nRRULE:FREQ=MONTHLY;BYMONTHDAY=40;INTERVAL=999999\r\nEND:VEVENT\r\n\
             BEGIN:VEVENT\r\nUID:c\r\nSUMMARY:Cut\r\nDTSTART:2026\r\nEND:VEVENT\r\n\
             BEGIN:VEVENT\r\nUID:d\r\nSUMMARY:Odd days\r\nDTSTART:20261113T090000Z\r\nDURATION:PT1H\r\nRRULE:FREQ=WEEKLY;BYDAY=é1,MO\r\nEND:VEVENT\r\n",
        );
        let zone = Fixed(0);
        let items = parse(&text, (day(2026, 11, 9), day(2026, 11, 23)), &zone);
        assert!(items.iter().any(|i| i.title == "Huge"), "its first occurrence stays");
        assert!(items.iter().any(|i| i.title == "Odd days"), "the readable weekday stays");
    }
}
