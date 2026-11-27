//! Events and notifications in words, for the panel: when the next event is, an event's times and
//! length, its details, when a notification came.

use super::*;

/// Whether to offer Join now: from shortly before the meeting to its end (days ahead, a list of
/// Join buttons was noise, and one away from the wrong meeting); its details always offer it.
pub(super) fn join_now(event: &Event, now: i64) -> bool {
    const BEFORE: i64 = 10 * 60;
    event.join.is_some() && matches!(event.when, When::At { start, end } if now >= start - BEFORE && now < end)
}

/// Each grid row's ISO week number (from its Monday, or its first day if none falls in it).
pub(super) fn week_numbers(days: &[i32]) -> Vec<u32> {
    days.chunks(7)
        .filter_map(|row| {
            let monday = row.iter().copied().find(|d| time::weekday(*d) == 0).or(row.first().copied())?;
            // The ISO week is the one of its Thursday.
            let thursday = monday - time::weekday(monday) as i32 + 3;
            let (year, _, _) = time::civil_from_days(thursday);
            Some(((thursday - time::days_from_civil(year, 1, 1)) / 7 + 1) as u32)
        })
        .collect()
}

/// "Next · in 6 h 35", "Now · until 21:03", "Next · Monday".
pub(super) fn soon(event: &Event, now: i64) -> String {
    let t = words::words();
    match event.when {
        When::At { start, end } if start <= now => (t.now_until)(&locale::clock(time::second_of_day(Local.to_local(end)))),
        When::At { start, .. } => {
            let minutes = (start - now + 59) / 60;
            let day = time::day_of(Local.to_local(start));
            if day == time::today() || minutes < 12 * 60 {
                (t.next_in)(minutes)
            } else {
                (t.next_on)(&capitalised(&locale::format(day, "dddd")))
            }
        }
        When::AllDay { first, .. } if first <= time::today() => t.today_all_day.into(),
        When::AllDay { first, .. } => (t.next_on)(&capitalised(&locale::format(first, "dddd"))),
    }
}

/// "15:59 – 21:03", "All day".
pub(super) fn span_words(event: &Event) -> String {
    match event.when {
        When::AllDay { first, end } if end - first > 1 => format!("{} – {}", locale::format(first, "d MMM"), locale::format(end - 1, "d MMM")),
        When::AllDay { .. } => words::words().all_day.into(),
        When::At { start, end } => {
            let (s, e) = (Local.to_local(start), Local.to_local(end));
            let first = locale::clock(time::second_of_day(s));
            format!("{first} – {}", locale::clock(time::second_of_day(e)))
        }
    }
}

/// A row's day and time in the week's list: "Mon 09:15", "Tue".
pub(super) fn week_label(event: &Event, today: i32) -> String {
    let (first, _) = event.days(&Local);
    let day = if first <= today { words::words().today.to_string() } else { capitalised(&locale::format(first, "ddd")).trim_end_matches('.').to_string() };
    match event.when {
        When::At { start, .. } => format!("{day} {}", locale::clock(time::second_of_day(Local.to_local(start)))),
        When::AllDay { .. } => day,
    }
}

/// A row's time on its day: "09:15", "All day".
pub(super) fn day_label(event: &Event, day: i32) -> String {
    match event.when {
        When::AllDay { .. } => words::words().all_day.into(),
        When::At { start, .. } => {
            let s = Local.to_local(start);
            if time::day_of(s) == day { locale::clock(time::second_of_day(s)) } else { "…".into() }
        }
    }
}

/// "1 h 30", "45 min"; nothing for an all-day event.
pub(super) fn duration_words(event: &Event) -> String {
    match event.when {
        When::At { start, end } if end > start => (words::words().duration)((end - start) / 60),
        _ => String::new(),
    }
}

pub(super) fn join_parts(parts: &[String]) -> String {
    parts.iter().filter(|p| !p.is_empty()).cloned().collect::<Vec<_>>().join(" · ")
}

/// An event's details, in words.
pub(super) fn detail_view(event: &Event, setup: &Setup, now: i64, day: i32) -> scene::Detail {
    let d = &event.details;
    let initials = d
        .people
        .iter()
        .take(3)
        .map(|name| name.split([' ', '.', '@']).filter(|w| !w.is_empty()).take(2).filter_map(|w| w.chars().next()).flat_map(char::to_uppercase).collect())
        .collect();
    let mut people: Vec<String> = d.people.iter().take(2).map(|p| p.split(' ').next().unwrap_or(p).to_string()).collect();
    if d.people.len() > 2 {
        people.push(format!("+{}", d.people.len() - 2));
    }
    let mut people = people.join(", ");
    if !d.response.is_empty() {
        people = join_parts(&[people, (words::words().you)(&d.response.to_lowercase())]);
    }
    let source = crate::calendar::status(setup).calendars.iter().find(|c| c.key == event.calendar).map_or_else(
        || match event.source {
            Source::Google => "Google".to_string(),
            Source::Microsoft => "Outlook".to_string(),
            Source::Link(i) => setup.links.get(i).map_or_else(|| words::words().calendar.into(), |l| l.name.clone()),
        },
        |c| c.name.clone(),
    );
    let open = event.link.as_ref().map(|link| match event.source {
        Source::Google => words::words().open_google.to_string(),
        Source::Microsoft => words::words().open_outlook.to_string(),
        Source::Link(_) if link.contains("calendar.google.com") => words::words().open_google.to_string(),
        Source::Link(_) if link.contains("outlook.") => words::words().open_outlook.to_string(),
        Source::Link(_) => words::words().open.to_string(),
    });
    let (first, last) = event.days(&Local);
    scene::Detail {
        back: capitalised(&locale::day_title(day.clamp(first, last.max(first)))),
        title: event.title.clone(),
        when: join_parts(&[span_words(event), duration_words(event)]),
        soon: soon(event, now),
        color: event.color,
        place: event.location.clone(),
        initials,
        people,
        notes: d.notes.clone(),
        source,
        open,
        joinable: event.join.is_some(),
        directions: !event.location.is_empty() && !online(&event.location),
    }
}

/// Whether a location is an online meeting's (an address, Teams, Zoom, Meet, Webex), not a place.
pub(super) fn online(location: &str) -> bool {
    let l = location.to_ascii_lowercase();
    ["http://", "https://", "www.", "teams", "zoom", "meet.google", "google meet", "webex", "skype"].iter().any(|w| l.contains(w))
}

/// When a notification arrived, in a few words.
pub(super) fn arrived(at: i64) -> String {
    let now = time::now();
    let minutes = (now - at).max(0) / 60;
    let wall = Local.to_local(at);
    match minutes {
        0 => words::words().now.into(),
        1..=59 => (words::words().minutes)(minutes),
        _ if time::day_of(wall) == time::day_of(Local.to_local(now)) => locale::clock(time::second_of_day(wall)),
        _ => capitalised(&locale::format(time::day_of(wall), "ddd d")),
    }
}

pub(super) fn capitalised(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |c| c.to_uppercase().chain(chars).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn week_numbers_are_iso_from_each_row_s_monday() {
        let monday = time::days_from_civil(2025, 12, 29);
        assert_eq!(week_numbers(&(monday..monday + 14).collect::<Vec<_>>()), vec![1, 2], "the week of 1 January 2026 is its first");
        let row = time::days_from_civil(2026, 11, 16);
        assert_eq!(week_numbers(&(row..row + 7).collect::<Vec<_>>()), vec![47]);
        // A week starting on Sunday counts from the Monday in it.
        assert_eq!(week_numbers(&(row - 1..row + 6).collect::<Vec<_>>()), vec![47]);
    }

    fn meeting(start: i64, end: i64, join: Option<&str>) -> Event {
        Event {
            when: When::At { start, end },
            title: "Sync".into(),
            location: String::new(),
            link: None,
            join: join.map(str::to_string),
            source: Source::Google,
            color: [0; 3],
            calendar: String::new(),
            details: Default::default(),
        }
    }

    #[test]
    fn join_is_offered_from_ten_minutes_before_to_the_end() {
        let start = 1_000_000;
        let e = meeting(start, start + 1800, Some("https://meet.google.com/x"));
        assert!(!join_now(&e, start - 601));
        assert!(join_now(&e, start - 600));
        assert!(join_now(&e, start + 1799));
        assert!(!join_now(&e, start + 1800));
        assert!(!join_now(&meeting(start, start + 1800, None), start));
    }

    #[test]
    fn online_meetings_get_no_directions() {
        assert!(online("Microsoft Teams Meeting"));
        assert!(online("https://zoom.us/j/1"));
        assert!(!online("Interference, Toulouse"));
        assert!(!online("Room 4.12"));
    }
}
