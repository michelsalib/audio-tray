//! This PC's own battery, as Windows reports it: its level, power source and energy saver
//! ([`system`]), whether there is one at all ([`presence`]), and Windows telling a window when any
//! of it changes ([`watch_power`]).

use windows::core::GUID;

/// This PC's own battery, as Windows reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SystemBattery {
    /// 0..=100.
    pub level: u8,
    /// On mains power.
    pub plugged: bool,
    pub charging: bool,
    /// Windows' energy saver is on.
    pub saver: bool,
    /// Seconds left on the battery, when Windows has worked it out (unplugged only).
    pub left: Option<u32>,
}

/// `SYSTEM_POWER_STATUS`, declared here: the `windows` feature that has it would rebuild the whole
/// crate for one call.
#[repr(C)]
#[derive(Default)]
struct PowerStatus {
    ac_line_status: u8,
    battery_flag: u8,
    battery_life_percent: u8,
    system_status_flag: u8,
    battery_life_time: u32,
    battery_full_life_time: u32,
}

#[link(name = "kernel32")]
extern "system" {
    fn GetSystemPowerStatus(status: *mut PowerStatus) -> i32;
}

#[link(name = "user32")]
extern "system" {
    fn RegisterPowerSettingNotification(recipient: *mut core::ffi::c_void, setting: *const GUID, flags: u32) -> *mut core::ffi::c_void;
}

/// `SYSTEM_POWER_STATUS.BatteryFlag`: charging; no system battery; not known.
const FLAG_CHARGING: u8 = 8;
const FLAG_NO_BATTERY: u8 = 128;
const FLAG_UNKNOWN: u8 = 255;
/// `ACLineStatus`: on mains power.
const AC_ONLINE: u8 = 1;
/// `SystemStatusFlag`: the battery (energy) saver is on.
const SAVER_ON: u8 = 1;
/// `BatteryLifeTime` when Windows has not worked it out.
const TIME_UNKNOWN: u32 = u32::MAX;

/// Whether this PC has a battery of its own, as Windows says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presence {
    Battery,
    /// A desktop: Windows shows no battery icon.
    NoBattery,
    /// Windows does not know yet (early after sign-in): neither.
    Unknown,
}

fn read_status() -> Option<PowerStatus> {
    let mut status = PowerStatus::default();
    (unsafe { GetSystemPowerStatus(&mut status) } != 0).then_some(status)
}

/// A dev run where a laptop pretends to be a desktop (Audio Tray's own battery icon, Windows' left
/// alone).
fn pretends_desktop() -> bool {
    cfg!(feature = "dev") && std::env::var_os("AUDIO_TRAY_NO_PC_BATTERY").is_some()
}

/// This PC's battery; `None` without one (a desktop) or when Windows does not know yet.
pub fn system() -> Option<SystemBattery> {
    if pretends_desktop() {
        return None;
    }
    from_status(&read_status()?)
}

/// Whether this PC has a battery, as last read ([`refresh_presence`]); read once if never.
pub fn presence() -> Presence {
    match PRESENCE.load(std::sync::atomic::Ordering::Relaxed) {
        0 => {
            refresh_presence();
            presence_from(PRESENCE.load(std::sync::atomic::Ordering::Relaxed))
        }
        code => presence_from(code),
    }
}

/// [`presence`] as last read: 0 never read, then 1 + the variant's place.
static PRESENCE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

fn presence_from(code: u8) -> Presence {
    match code {
        1 => Presence::Battery,
        2 => Presence::NoBattery,
        _ => Presence::Unknown,
    }
}

/// Read again whether this PC has a battery (at start, and when Windows reports a power change).
/// `true` when the answer changed.
pub fn refresh_presence() -> bool {
    let now = if pretends_desktop() { Presence::NoBattery } else { read_status().map_or(Presence::Unknown, |s| presence_of(&s)) };
    let code = match now {
        Presence::Battery => 1,
        Presence::NoBattery => 2,
        Presence::Unknown => 3,
    };
    let was = PRESENCE.swap(code, std::sync::atomic::Ordering::Relaxed);
    was != 0 && was != code
}

fn presence_of(status: &PowerStatus) -> Presence {
    if status.battery_flag == FLAG_UNKNOWN {
        Presence::Unknown
    } else if status.battery_flag & FLAG_NO_BATTERY != 0 {
        Presence::NoBattery
    } else {
        Presence::Battery
    }
}

fn from_status(status: &PowerStatus) -> Option<SystemBattery> {
    if presence_of(status) != Presence::Battery || status.battery_life_percent > 100 {
        return None;
    }
    let plugged = status.ac_line_status == AC_ONLINE;
    Some(SystemBattery {
        level: status.battery_life_percent,
        plugged,
        charging: status.battery_flag & FLAG_CHARGING != 0,
        saver: status.system_status_flag & SAVER_ON != 0,
        left: (!plugged && status.battery_life_time != TIME_UNKNOWN).then_some(status.battery_life_time),
    })
}

/// `WM_POWERBROADCAST`'s `wParam` when a setting registered with [`watch_power`] changed.
pub const PBT_POWERSETTINGCHANGE: usize = 0x8013;
pub const WM_POWERBROADCAST: u32 = 0x0218;

/// Have Windows tell `hwnd` (`WM_POWERBROADCAST`, [`PBT_POWERSETTINGCHANGE`]) when this PC's charge,
/// its power source or the energy saver changes: no polling. Registered once for the window's life.
pub fn watch_power(hwnd: windows::Win32::Foundation::HWND) {
    const SETTINGS: [GUID; 4] = [
        // GUID_BATTERY_PERCENTAGE_REMAINING
        GUID::from_u128(0xA7AD_8041_B45A_4CAE_87A3_EECB_B468_A9E1),
        // GUID_ACDC_POWER_SOURCE
        GUID::from_u128(0x5D3E_9A59_E9D5_4B00_A6BD_FF34_FF51_6548),
        // GUID_POWER_SAVING_STATUS (battery saver)
        GUID::from_u128(0xE009_58C0_C213_4ACE_AC77_FECC_ED2E_EEA5),
        // GUID_ENERGY_SAVER_STATUS (Windows 11's energy saver)
        GUID::from_u128(0x550E_8400_E29B_41D4_A716_4466_5544_0000),
    ];
    for setting in &SETTINGS {
        // DEVICE_NOTIFY_WINDOW_HANDLE. Older builds refuse the energy saver's: no matter.
        let handle = unsafe { RegisterPowerSettingNotification(hwnd.0, setting, 0) };
        if handle.is_null() {
            eprintln!("batteries: Windows will not report power setting {setting:?}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(ac: u8, flag: u8, percent: u8, saver: u8, life: u32) -> PowerStatus {
        PowerStatus { ac_line_status: ac, battery_flag: flag, battery_life_percent: percent, system_status_flag: saver, battery_life_time: life, battery_full_life_time: u32::MAX }
    }

    /// A desktop (no battery) and a battery Windows does not know yet are not a battery; a laptop's
    /// reads as Windows says, the time left only on battery.
    #[test]
    fn the_pc_battery_reads_as_windows_reports_it() {
        assert_eq!(from_status(&status(1, 128, 255, 0, u32::MAX)), None);
        assert_eq!(from_status(&status(255, 255, 255, 0, u32::MAX)), None);
        let charging = from_status(&status(1, 8, 43, 0, u32::MAX)).unwrap();
        assert_eq!(charging, SystemBattery { level: 43, plugged: true, charging: true, saver: false, left: None });
        let away = from_status(&status(0, 2, 18, 1, 5400)).unwrap();
        assert_eq!(away, SystemBattery { level: 18, plugged: false, charging: false, saver: true, left: Some(5400) });
        assert_eq!(from_status(&status(0, 0, 60, 0, u32::MAX)).unwrap().left, None);
    }

    /// "Not known yet" is neither a laptop nor a desktop: only an explicit "no battery" is one.
    #[test]
    fn only_an_explicit_no_battery_is_a_desktop() {
        assert_eq!(presence_of(&status(1, 128, 255, 0, u32::MAX)), Presence::NoBattery);
        assert_eq!(presence_of(&status(255, 255, 255, 0, u32::MAX)), Presence::Unknown);
        assert_eq!(presence_of(&status(1, 8, 43, 0, u32::MAX)), Presence::Battery);
    }
}
