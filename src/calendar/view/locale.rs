//! Dates and times in the user's own language and format, as Windows writes them.

use windows::core::PCWSTR;
use windows::Win32::Foundation::SYSTEMTIME;
use windows::Win32::Globalization::{
    GetDateFormatEx, GetLocaleInfoEx, GetTimeFormatEx, DATE_LONGDATE, DATE_YEARMONTH, ENUM_DATE_FORMATS_FLAGS,
    LOCALE_IFIRSTDAYOFWEEK, LOCALE_RETURN_NUMBER, LOCALE_SSHORTESTDAYNAME1, TIME_NOSECONDS,
};

use crate::calendar::time;

fn system_time(day: i32, secs: i64) -> SYSTEMTIME {
    let (y, m, d) = time::civil_from_days(day);
    SYSTEMTIME {
        wYear: y as u16,
        wMonth: m as u16,
        wDayOfWeek: ((time::weekday(day) + 1) % 7) as u16,
        wDay: d as u16,
        wHour: (secs / 3600) as u16,
        wMinute: (secs / 60 % 60) as u16,
        wSecond: 0,
        wMilliseconds: 0,
    }
}

fn date(day: i32, flags: ENUM_DATE_FORMATS_FLAGS) -> String {
    let st = system_time(day, 0);
    let mut buf = [0u16; 128];
    let n = unsafe { GetDateFormatEx(PCWSTR::null(), flags, Some(&st), PCWSTR::null(), Some(&mut buf), PCWSTR::null()) };
    String::from_utf16_lossy(&buf[..(n.max(1) - 1) as usize])
}

/// The day's long date without its year: "vendredi 13 novembre", "Friday, November 13".
pub fn day_title(day: i32) -> String {
    let long = date(day, DATE_LONGDATE);
    let year = time::civil_from_days(day).0.to_string();
    without_year(&long, &year)
}

fn without_year(long: &str, year: &str) -> String {
    let text = long.replacen(year, "", 1);
    // What held the year in place: "13 novembre ", "November 13, ", "2026年11月13日" keeps its own.
    let text = text.trim_matches(|c: char| c == ',' || c == ' ' || c == '.' && !long.ends_with('.'));
    text.replace(" ,", ",").replace("  ", " ")
}

/// "novembre 2026".
pub fn month_title(y: i32, m: u32) -> String {
    date(time::days_from_civil(y, m, 1), DATE_YEARMONTH)
}

/// A time of day ("09:00", "9:00 AM"), from seconds into the day.
pub fn clock(secs: i64) -> String {
    let st = system_time(0, secs);
    let mut buf = [0u16; 64];
    let n = unsafe { GetTimeFormatEx(PCWSTR::null(), TIME_NOSECONDS, Some(&st), PCWSTR::null(), Some(&mut buf)) };
    String::from_utf16_lossy(&buf[..(n.max(1) - 1) as usize])
}

/// The week's first day, Monday 0 to Sunday 6.
pub fn first_weekday() -> u32 {
    let mut buf = [0u16; 2];
    let n = unsafe { GetLocaleInfoEx(PCWSTR::null(), LOCALE_IFIRSTDAYOFWEEK | LOCALE_RETURN_NUMBER, Some(&mut buf)) };
    if n == 2 { (buf[0] as u32).min(6) } else { 0 }
}

/// The days' shortest names ("lu", "Mo"), Monday first.
pub fn weekday_names() -> [String; 7] {
    std::array::from_fn(|i| {
        let mut buf = [0u16; 16];
        let n = unsafe { GetLocaleInfoEx(PCWSTR::null(), LOCALE_SSHORTESTDAYNAME1 + i as u32, Some(&mut buf)) };
        String::from_utf16_lossy(&buf[..(n.max(1) - 1) as usize])
    })
}

/// The day in a pattern of `GetDateFormatEx`'s ("dddd", "d MMMM").
pub fn format(day: i32, pattern: &str) -> String {
    let st = system_time(day, 0);
    let wide: Vec<u16> = pattern.encode_utf16().chain(std::iter::once(0)).collect();
    let mut buf = [0u16; 128];
    let flags = windows::Win32::Globalization::ENUM_DATE_FORMATS_FLAGS(0);
    let n = unsafe { GetDateFormatEx(PCWSTR::null(), flags, Some(&st), PCWSTR(wide.as_ptr()), Some(&mut buf), PCWSTR::null()) };
    String::from_utf16_lossy(&buf[..(n.max(1) - 1) as usize])
}

#[cfg(test)]
mod tests {
    use super::without_year;

    #[test]
    fn the_year_comes_out_of_long_dates_in_any_order() {
        assert_eq!(without_year("vendredi 13 novembre 2026", "2026"), "vendredi 13 novembre");
        assert_eq!(without_year("Friday, November 13, 2026", "2026"), "Friday, November 13");
        assert_eq!(without_year("Freitag, 13. November 2026", "2026"), "Freitag, 13. November");
    }
}
