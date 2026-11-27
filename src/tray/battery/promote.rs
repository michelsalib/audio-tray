//! Audio Tray's own battery icon taken out of the taskbar's overflow, once.

use std::time::Duration;

/// A new icon lands in the taskbar's overflow; the battery icon is asked for, so it is shown beside
/// the others, unless the user has chosen already (Windows' `IsPromoted`, set by the Settings app's
/// switch). Windows writes the icon's entry a moment after it is added: looked for on a thread, once
/// per run (the icon comes and goes with the devices; its entry, once decided, stays).
pub(super) fn once() {
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if STARTED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    std::thread::spawn(|| {
        for _ in 0..8 {
            std::thread::sleep(Duration::from_millis(750));
            if promote_entry() {
                return;
            }
        }
    });
}

/// Where Windows keeps a key per notification icon.
const SETTINGS: &str = r"Control Panel\NotifyIconSettings";

/// The battery icon's entry under `HKCU\Control Panel\NotifyIconSettings` marked promoted (if
/// undecided). `true` once its entry is found.
fn promote_entry() -> bool {
    use windows::core::HSTRING;
    use windows::Win32::System::Registry::{
        RegCloseKey, RegGetValueW, RegOpenKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_DWORD, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
    };
    let Ok(exe) = std::env::current_exe() else { return true };
    let exe = exe.to_string_lossy().to_lowercase();
    let text = |key: HKEY, name: &str| -> String {
        let mut buf = [0u16; 520];
        let mut size = (buf.len() * 2) as u32;
        let read = unsafe { RegGetValueW(key, None, &HSTRING::from(name), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as *mut _), Some(&mut size)) };
        if read.is_err() {
            return String::new();
        }
        String::from_utf16_lossy(&buf[..(size as usize / 2).saturating_sub(1)])
    };
    for name in crate::win::subkeys(HKEY_CURRENT_USER, SETTINGS) {
        let mut entry = HKEY::default();
        let path = HSTRING::from(format!(r"{SETTINGS}\{name}"));
        if unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, &path, Some(0), KEY_READ | KEY_WRITE, &mut entry) }.is_err() {
            continue;
        }
        let found = text(entry, "ExecutablePath").to_lowercase() == exe && text(entry, "InitialTooltip") == super::REGISTERED_TOOLTIP;
        if found {
            let mut value = 0u32;
            let mut size = 4u32;
            let decided = unsafe { RegGetValueW(entry, None, &HSTRING::from("IsPromoted"), RRF_RT_REG_DWORD, None, Some(&mut value as *mut u32 as *mut _), Some(&mut size)) }.is_ok();
            if !decided {
                let one = 1u32.to_le_bytes();
                let set = unsafe { RegSetValueExW(entry, &HSTRING::from("IsPromoted"), Some(0), REG_DWORD, Some(&one)) };
                println!("batteries: the battery icon shown on the taskbar ({})", if set.is_ok() { "promoted" } else { "Windows refused" });
            }
        }
        let _ = unsafe { RegCloseKey(entry) };
        if found {
            return true;
        }
    }
    false
}
