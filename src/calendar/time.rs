//! Dates without a date library: days since 1970-01-01 for a calendar day, seconds since the epoch
//! for an instant, and Windows' own time-zone rules (DST included, for any date) to turn one into
//! the other on this PC's clock.

/// Seconds in a day.
pub const DAY: i64 = 86_400;

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's `days_from_civil`).
pub fn days_from_civil(y: i32, m: u32, d: u32) -> i32 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u32;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe as i32 - 719_468
}

/// The date of a day number: `(year, month 1..=12, day 1..=31)`.
pub fn civil_from_days(z: i32) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i32 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// The weekday of a day number, Monday 0 to Sunday 6 (1970-01-01 was a Thursday).
pub fn weekday(day: i32) -> u32 {
    (day + 3).rem_euclid(7) as u32
}

pub fn days_in_month(y: i32, m: u32) -> u32 {
    let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
    (days_from_civil(ny, nm, 1) - days_from_civil(y, m, 1)) as u32
}

/// The month `delta` months from `(y, m)`.
pub fn add_months(y: i32, m: u32, delta: i32) -> (i32, u32) {
    let index = y * 12 + m as i32 - 1 + delta;
    (index.div_euclid(12), index.rem_euclid(12) as u32 + 1)
}

/// The day number of a wall-clock moment (seconds since the epoch, read as a local time).
pub fn day_of(wall: i64) -> i32 {
    wall.div_euclid(DAY) as i32
}

/// Seconds into the day of a wall-clock moment.
pub fn second_of_day(wall: i64) -> i64 {
    wall.rem_euclid(DAY)
}

/// How instants map to this PC's wall clock. A trait so tests can use a fixed offset.
pub trait Zone {
    /// The local wall-clock time of an instant (both as seconds since the epoch).
    fn to_local(&self, utc: i64) -> i64;
    /// The instant a local wall-clock time names (the earlier one in a DST overlap).
    fn to_utc(&self, wall: i64) -> i64;
}

/// Windows' current time zone, with its full DST rules.
pub struct Local;

impl Zone for Local {
    fn to_local(&self, utc: i64) -> i64 {
        use windows::Win32::System::Time::SystemTimeToTzSpecificLocalTimeEx;
        let st = to_systemtime(utc);
        let mut out = Default::default();
        match unsafe { SystemTimeToTzSpecificLocalTimeEx(None, &st, &mut out) } {
            Ok(()) => from_systemtime(&out),
            Err(_) => utc,
        }
    }

    fn to_utc(&self, wall: i64) -> i64 {
        use windows::Win32::System::Time::TzSpecificLocalTimeToSystemTimeEx;
        let st = to_systemtime(wall);
        let mut out = Default::default();
        match unsafe { TzSpecificLocalTimeToSystemTimeEx(None, &st, &mut out) } {
            Ok(()) => from_systemtime(&out),
            Err(_) => wall,
        }
    }
}

fn to_systemtime(secs: i64) -> windows::Win32::Foundation::SYSTEMTIME {
    let day = day_of(secs);
    let (y, m, d) = civil_from_days(day);
    let s = second_of_day(secs);
    windows::Win32::Foundation::SYSTEMTIME {
        wYear: y.clamp(1601, 30827) as u16,
        wMonth: m as u16,
        wDayOfWeek: ((weekday(day) + 1) % 7) as u16,
        wDay: d as u16,
        wHour: (s / 3600) as u16,
        wMinute: (s / 60 % 60) as u16,
        wSecond: (s % 60) as u16,
        wMilliseconds: 0,
    }
}

fn from_systemtime(st: &windows::Win32::Foundation::SYSTEMTIME) -> i64 {
    days_from_civil(st.wYear as i32, st.wMonth as u32, st.wDay as u32) as i64 * DAY
        + st.wHour as i64 * 3600
        + st.wMinute as i64 * 60
        + st.wSecond as i64
}

/// Now, in seconds since the epoch.
pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// Seconds from 1601 (Windows' epoch) to 1970.
const WINDOWS_EPOCH: i64 = 11_644_473_600;

/// Seconds since the epoch as Windows time (100 ns ticks since 1601).
pub fn to_filetime(secs: i64) -> i64 {
    secs.saturating_add(WINDOWS_EPOCH).saturating_mul(10_000_000)
}

/// Windows time (100 ns ticks since 1601) as seconds since the epoch.
pub fn from_filetime(ticks: i64) -> i64 {
    ticks / 10_000_000 - WINDOWS_EPOCH
}

/// Today's day number on the local clock.
pub fn today() -> i32 {
    day_of(Local.to_local(now()))
}

/// Seconds since the epoch of `YYYY-MM-DD[THH:MM[:SS[.fff]]][Z|±HH:MM]` (RFC 3339 and the shapes
/// Graph and Google send). Without an offset the time is returned as written (the caller knows
/// which clock it is on). A bare date is its midnight.
pub fn parse_iso(text: &str) -> Option<(i64, bool)> {
    let text = text.trim();
    let date = text.get(..10)?;
    let mut parts = date.split('-');
    let y: i32 = parts.next()?.parse().ok()?;
    let m: u32 = parts.next()?.parse().ok()?;
    let d: u32 = parts.next()?.parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let mut secs = days_from_civil(y, m, d) as i64 * DAY;
    let rest = &text[10..];
    if rest.is_empty() {
        return Some((secs, false));
    }
    let rest = rest.strip_prefix(['T', 't', ' '])?;
    // The clock part runs up to an offset: `Z`, `+`, or a `-` after the time.
    let end = rest.find(['Z', 'z', '+', '-']).unwrap_or(rest.len());
    let (clock, zone) = rest.split_at(end);
    let clock = clock.split('.').next()?;
    let mut fields = clock.split(':');
    let h: i64 = fields.next()?.parse().ok()?;
    let min: i64 = fields.next().unwrap_or("0").parse().ok()?;
    let s: i64 = fields.next().unwrap_or("0").parse().ok()?;
    secs += h * 3600 + min * 60 + s;
    match zone.chars().next() {
        None => Some((secs, false)),
        Some('Z' | 'z') => Some((secs, true)),
        Some(sign) => {
            let digits: String = zone[1..].chars().filter(char::is_ascii_digit).collect();
            let oh: i64 = digits.get(..2)?.parse().ok()?;
            let om: i64 = digits.get(2..4).unwrap_or("0").parse().ok()?;
            let offset = (oh * 3600 + om * 60) * if sign == '-' { -1 } else { 1 };
            Some((secs - offset, true))
        }
    }
}

#[cfg(test)]
pub(crate) struct Fixed(pub i64);

#[cfg(test)]
impl Zone for Fixed {
    fn to_local(&self, utc: i64) -> i64 {
        utc + self.0
    }
    fn to_utc(&self, wall: i64) -> i64 {
        wall - self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_round_trip_across_leap_years_and_the_epoch() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        for day in [-1000, -1, 0, 59, 11_016, 20_770, 25_000] {
            let (y, m, d) = civil_from_days(day);
            assert_eq!(days_from_civil(y, m, d), day);
        }
        assert_eq!(civil_from_days(days_from_civil(2024, 2, 29)), (2024, 2, 29));
    }

    #[test]
    fn weekdays_and_month_lengths() {
        // 13 November 2026 is a Friday.
        assert_eq!(weekday(days_from_civil(2026, 11, 13)), 4);
        assert_eq!(days_in_month(2026, 2), 28);
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(2026, 12), 31);
        assert_eq!(add_months(2026, 11, 2), (2027, 1));
        assert_eq!(add_months(2026, 1, -1), (2025, 12));
    }

    #[test]
    fn iso_times_with_and_without_offsets() {
        let base = days_from_civil(2026, 11, 13) as i64 * DAY;
        assert_eq!(parse_iso("2026-11-13"), Some((base, false)));
        assert_eq!(parse_iso("2026-11-13T10:30:00Z"), Some((base + 10 * 3600 + 1800, true)));
        assert_eq!(parse_iso("2026-11-13T10:30:00+01:00"), Some((base + 9 * 3600 + 1800, true)));
        assert_eq!(parse_iso("2026-11-13T10:30:00-05:00"), Some((base + 15 * 3600 + 1800, true)));
        assert_eq!(parse_iso("2026-11-13T09:00:00.0000000"), Some((base + 9 * 3600, false)));
        assert_eq!(parse_iso("nonsense"), None);
    }
}
