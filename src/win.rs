//! The two-line Win32 helpers that every other module needs a copy of otherwise:
//! wide strings, a registry DWORD, and the shell's small-icon size.

use windows::core::PCWSTR;

/// A Rust string as the NUL-terminated UTF-16 buffer the `W` APIs take.
///
/// The buffer has to outlive the call, so callers bind it before taking a
/// `PCWSTR` to it — a pointer into a temporary would dangle.
pub(crate) fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// A `REG_DWORD` under `HKEY_CURRENT_USER`, or `None` if it is absent or another
/// type. Both of Windows' theme signals we follow are stored this way.
pub(crate) fn hkcu_dword(subkey: PCWSTR, name: PCWSTR) -> Option<u32> {
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};

    let mut value = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            subkey,
            name,
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut core::ffi::c_void),
            Some(&mut size),
        )
    };
    (status.0 == 0).then_some(value)
}

/// The DPI-scaled small-icon size Windows wants for tray-sized glyphs — 24 px on a
/// 144-DPI display, not 16, because the process is per-monitor DPI aware.
pub(crate) fn small_icon_size() -> u32 {
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSMICON};

    let px = unsafe { GetSystemMetrics(SM_CXSMICON) };
    if px <= 0 {
        16
    } else {
        px as u32
    }
}
