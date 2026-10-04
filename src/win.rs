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

/// Calls `visit` for every top-level window, in any process, until it returns `false`.
///
/// `EnumWindows` rather than `FindWindow`, which does not find our hidden windows across processes.
pub(crate) fn enum_windows(mut visit: impl FnMut(windows::Win32::Foundation::HWND) -> bool) {
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::EnumWindows;
    use windows_core::BOOL;

    type Visit<'a> = &'a mut dyn FnMut(HWND) -> bool;
    unsafe extern "system" fn thunk(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let visit = unsafe { &mut *(lparam.0 as *mut Visit) };
        // A panic must not unwind into user32; treat it as "stop".
        BOOL::from(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| visit(hwnd))).unwrap_or(false))
    }
    let mut visit: Visit = &mut visit;
    let _ = unsafe { EnumWindows(Some(thunk), LPARAM(&mut visit as *mut Visit as isize)) };
}

/// A window's class name, empty for a dead handle.
pub(crate) fn class_name(hwnd: windows::Win32::Foundation::HWND) -> String {
    use windows::Win32::UI::WindowsAndMessaging::GetClassNameW;
    let mut buf = [0u16; 256];
    let len = unsafe { GetClassNameW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..len.max(0) as usize])
}

/// Every top-level window with exactly this class, in any process.
pub(crate) fn windows_by_class(class: &str) -> Vec<windows::Win32::Foundation::HWND> {
    let mut found = Vec::new();
    enum_windows(|hwnd| {
        if class_name(hwnd) == class {
            found.push(hwnd);
        }
        true
    });
    found
}

/// Register `class` (re-registering is a harmless no-op) and create a `WS_POPUP` window of it.
///
/// One helper for the app's three windows: the tray's hidden message window, the flyout and the
/// readout. The arrow cursor is set on the class because a captured window gets no `WM_SETCURSOR`.
pub(crate) fn create_popup(
    class: PCWSTR,
    title: PCWSTR,
    proc: windows::Win32::UI::WindowsAndMessaging::WNDPROC,
    ex_style: windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE,
    (x, y, width, height): (i32, i32, i32, i32),
) -> windows::core::Result<windows::Win32::Foundation::HWND> {
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, LoadCursorW, RegisterClassW, IDC_ARROW, WNDCLASSW, WS_POPUP,
    };

    let instance = unsafe { GetModuleHandleW(None) }?;
    let wc = WNDCLASSW {
        lpfnWndProc: proc,
        hInstance: instance.into(),
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default(),
        lpszClassName: class,
        ..Default::default()
    };
    unsafe { RegisterClassW(&wc) };
    unsafe { CreateWindowExW(ex_style, class, title, WS_POPUP, x, y, width, height, None, None, Some(instance.into()), None) }
}
