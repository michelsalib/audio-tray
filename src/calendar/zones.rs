//! World clocks: the two "additional clocks" set in Windows' own Date and time settings
//! (`HKCU\Control Panel\TimeDate\AdditionalClocks`), read rather than kept a second time.

use windows::core::{HSTRING, PCWSTR};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RRF_RT_REG_SZ};
use windows::Win32::System::Time::{EnumDynamicTimeZoneInformation, SystemTimeToTzSpecificLocalTimeEx, DYNAMIC_TIME_ZONE_INFORMATION};

use super::time::{self, Local, Zone};

/// One additional clock.
#[derive(Clone, Debug, PartialEq)]
pub struct WorldClock {
    pub name: String,
    /// Its time now, as wall-clock seconds since the epoch, and how far it is from this PC's.
    pub wall: i64,
    pub offset: i64,
}

fn reg_string(key: &str, name: &str) -> Option<String> {
    let (key, name) = (HSTRING::from(key), HSTRING::from(name));
    let mut buf = [0u16; 260];
    let mut size = (buf.len() * 2) as u32;
    let found = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, PCWSTR(key.as_ptr()), PCWSTR(name.as_ptr()), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as *mut _), Some(&mut size))
    };
    found.is_ok().then(|| String::from_utf16_lossy(&buf[..(size as usize / 2).saturating_sub(1)]))
}

fn reg_dword(key: &str, name: &str) -> Option<u32> {
    let (key, name) = (HSTRING::from(key), HSTRING::from(name));
    let mut value = 0u32;
    let mut size = 4u32;
    let found = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, PCWSTR(key.as_ptr()), PCWSTR(name.as_ptr()), RRF_RT_REG_DWORD, None, Some(&mut value as *mut u32 as *mut _), Some(&mut size))
    };
    found.is_ok().then_some(value)
}

/// The time zone Windows names `key` (`Pacific Standard Time`).
fn zone(key: &str) -> Option<DYNAMIC_TIME_ZONE_INFORMATION> {
    for i in 0..512 {
        let mut info = DYNAMIC_TIME_ZONE_INFORMATION::default();
        if unsafe { EnumDynamicTimeZoneInformation(i, &mut info) } != 0 {
            return None;
        }
        let name = String::from_utf16_lossy(&info.TimeZoneKeyName);
        if name.trim_end_matches('\0') == key {
            return Some(info);
        }
    }
    None
}

/// The enabled additional clocks, with their time now.
pub fn clocks() -> Vec<WorldClock> {
    let now = time::now();
    let here = Local.to_local(now);
    (1..=2)
        .filter_map(|i| {
            let key = format!(r"Control Panel\TimeDate\AdditionalClocks\{i}");
            if reg_dword(&key, "Enable")? == 0 {
                return None;
            }
            let info = zone(&reg_string(&key, "TzRegKeyName")?)?;
            let name = reg_string(&key, "DisplayName").filter(|n| !n.is_empty()).unwrap_or_else(|| String::from_utf16_lossy(&info.TimeZoneKeyName).trim_end_matches('\0').to_string());
            let utc = to_systemtime(now);
            let mut local = Default::default();
            unsafe { SystemTimeToTzSpecificLocalTimeEx(Some(&info), &utc, &mut local) }.ok()?;
            let wall = time::days_from_civil(local.wYear as i32, local.wMonth as u32, local.wDay as u32) as i64 * time::DAY
                + local.wHour as i64 * 3600
                + local.wMinute as i64 * 60
                + local.wSecond as i64;
            Some(WorldClock { name, wall, offset: wall - here })
        })
        .collect()
}

fn to_systemtime(secs: i64) -> windows::Win32::Foundation::SYSTEMTIME {
    let day = time::day_of(secs);
    let (y, m, d) = time::civil_from_days(day);
    let s = time::second_of_day(secs);
    windows::Win32::Foundation::SYSTEMTIME {
        wYear: y as u16,
        wMonth: m as u16,
        wDayOfWeek: ((time::weekday(day) + 1) % 7) as u16,
        wDay: d as u16,
        wHour: (s / 3600) as u16,
        wMinute: (s / 60 % 60) as u16,
        wSecond: (s % 60) as u16,
        wMilliseconds: 0,
    }
}

/// Windows' own page for them: Date and time's Additional clocks tab.
pub fn edit() {
    crate::win::shell_open("control.exe", Some("timedate.cpl,,1"));
}

/// "+6 h", "−5 h 30", "same time".
pub fn offset_words(offset: i64) -> String {
    let minutes = (offset / 60).abs();
    if minutes == 0 {
        return "same time".into();
    }
    let sign = if offset < 0 { "−" } else { "+" };
    match minutes % 60 {
        0 => format!("{sign}{} h", minutes / 60),
        m => format!("{sign}{} h {m:02}", minutes / 60),
    }
}

#[cfg(test)]
mod tests {
    use super::offset_words;

    #[test]
    fn offsets_read_in_hours_and_minutes() {
        assert_eq!(offset_words(-6 * 3600), "−6 h");
        assert_eq!(offset_words(5 * 3600 + 1800), "+5 h 30");
        assert_eq!(offset_words(0), "same time");
    }
}
