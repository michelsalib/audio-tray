//! What a calendar holds, whichever account or link it came from.

use super::time::{self, Zone, DAY};

/// When an event happens.
#[derive(Clone, Debug, PartialEq)]
pub enum When {
    /// A timed event: instants, in seconds since the epoch.
    At { start: i64, end: i64 },
    /// Whole days on the local calendar: `first` up to `end`, exclusive.
    AllDay { first: i32, end: i32 },
}

/// Where an event came from: a signed-in account, or calendar link `i`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    Google,
    Microsoft,
    Link(usize),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub when: When,
    pub title: String,
    pub location: String,
    /// The event in its calendar's web page.
    pub link: Option<String>,
    /// The meeting to join (Meet, Teams), when it has one.
    pub join: Option<String>,
    pub source: Source,
    /// Its calendar's colour, as the calendar names it (Settings can override it by key).
    pub color: [u8; 3],
    /// Its calendar ([`Calendar::key`]).
    pub calendar: String,
    pub details: Details,
}

/// What the event's card shows beyond the agenda row.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Details {
    /// Who is invited, by name (or address).
    pub people: Vec<String>,
    pub organizer: String,
    /// Your reply: accepted, tentative, needs action…
    pub response: String,
    /// The description, as plain text.
    pub notes: String,
}

/// A calendar events come from: one of a Google account's, the Microsoft account's, or a link.
#[derive(Clone, Debug, PartialEq)]
pub struct Calendar {
    /// Stable across fetches: `google:<id>`, `outlook`, `link:<hash of its address>`.
    pub key: String,
    pub name: String,
    pub color: [u8; 3],
    pub source: Source,
}

impl Event {
    /// The local days it covers, `[first, last]`.
    pub fn days(&self, zone: &dyn Zone) -> (i32, i32) {
        match self.when {
            When::AllDay { first, end } => (first, (end - 1).max(first)),
            When::At { start, end } => {
                let first = time::day_of(zone.to_local(start));
                // An event ending at midnight does not spill into the next day.
                let last = time::day_of(zone.to_local(end.max(start + 1) - 1));
                (first, last.max(first))
            }
        }
    }

    /// All-day events first, then by start, then by title.
    pub fn order(&self) -> (u8, i64) {
        match self.when {
            When::AllDay { first, .. } => (0, first as i64 * DAY),
            When::At { start, .. } => (1, start),
        }
    }
}

/// The events on `day`, in the order the agenda lists them, each event's days given in `spans`
/// (from [`Event::days`], worked out once: a time-zone sum each).
pub fn on_day<'a>(events: &'a [Event], spans: &[(i32, i32)], day: i32) -> Vec<&'a Event> {
    let mut list: Vec<&Event> = events.iter().zip(spans).filter(|(_, (first, last))| *first <= day && day <= *last).map(|(e, _)| e).collect();
    list.sort_by(|a, b| a.order().cmp(&b.order()).then_with(|| a.title.cmp(&b.title)));
    list
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calendar::time::{days_from_civil, Fixed};

    fn timed(day: i32, from_h: i64, to_h: i64) -> Event {
        let base = day as i64 * DAY;
        Event {
            when: When::At { start: base + from_h * 3600, end: base + to_h * 3600 },
            title: format!("{from_h}h"),
            location: String::new(),
            link: None,
            join: None,
            source: Source::Google,
            color: [1, 2, 3],
            calendar: "google:me".into(),
            details: Details::default(),
        }
    }

    #[test]
    fn a_timed_event_falls_on_its_local_days_and_midnight_ends_do_not_spill() {
        let day = days_from_civil(2026, 11, 13);
        let zone = Fixed(3600);
        // 23:30 UTC is 00:30 the next day at UTC+1.
        let late = Event { when: When::At { start: day as i64 * DAY + 23 * 3600 + 1800, end: day as i64 * DAY + 24 * 3600 + 1800 }, ..timed(day, 0, 1) };
        assert_eq!(late.days(&zone), (day + 1, day + 1));
        let to_midnight = timed(day, 22, 23); // 23:00 to 00:00 local
        assert_eq!(to_midnight.days(&zone), (day, day));
    }

    #[test]
    fn all_day_events_come_first_and_span_their_days() {
        let day = days_from_civil(2026, 11, 13);
        let zone = Fixed(0);
        let trip = Event { when: When::AllDay { first: day, end: day + 3 }, title: "Trip".into(), ..timed(day, 0, 1) };
        let events = vec![timed(day, 9, 10), trip.clone(), timed(day, 8, 9)];
        let spans: Vec<(i32, i32)> = events.iter().map(|e| e.days(&zone)).collect();
        let list = on_day(&events, &spans, day);
        assert_eq!(list[0].title, "Trip");
        assert_eq!(list[1].title, "8h");
        assert_eq!(trip.days(&zone), (day, day + 2));
    }
}
