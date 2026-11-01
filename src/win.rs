//! Small shared Win32 helpers: wide strings, registry DWORDs, icon size, window enumeration,
//! popup creation and per-monitor scale.

use windows::core::PCWSTR;

/// A Rust string as a NUL-terminated UTF-16 buffer. Bind it to a local before taking a
/// `PCWSTR`: a pointer into a temporary dangles.
pub(crate) fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// A `REG_DWORD` under `HKEY_CURRENT_USER`, or `None` if it is absent or another type.
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

/// The DPI-scaled small-icon size for tray-sized glyphs (24 px at 144 DPI).
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

/// Register `class` (re-registering is a no-op) and create a `WS_POPUP` window of it. The class
/// carries the arrow cursor because a captured window gets no `WM_SETCURSOR`.
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

/// The display scale (DPI / 96, at least 1) and work area of the monitor nearest `point`.
pub(crate) fn monitor_at(point: windows::Win32::Foundation::POINT) -> (f32, windows::Win32::Foundation::RECT) {
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST};
    use windows::Win32::UI::HiDpi::{GetDpiForMonitor, GetDpiForSystem, MDT_EFFECTIVE_DPI};

    let monitor = unsafe { MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST) };
    let (mut dpi_x, mut dpi_y) = (0u32, 0u32);
    let dpi = match unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) } {
        Ok(()) if dpi_x > 0 => dpi_x,
        _ => unsafe { GetDpiForSystem() },
    };
    let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    let work = if unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() { info.rcWork } else { Default::default() };
    ((dpi as f32 / 96.0).max(1.0), work)
}

/// A window's title, empty when it has none.
pub(crate) fn window_title(hwnd: windows::Win32::Foundation::HWND) -> String {
    use windows::Win32::UI::WindowsAndMessaging::GetWindowTextW;
    let mut buf = [0u16; 512];
    let len = unsafe { GetWindowTextW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..len.max(0) as usize])
}

/// Open `file` (a path, a URL, an exe on the App Paths list) with optional `args`, as Explorer
/// would. `false` when the shell refused it.
pub(crate) fn shell_open(file: &str, args: Option<&str>) -> bool {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let (verb, file) = (wide("open"), wide(file));
    let args = args.map(wide);
    let args = args.as_ref().map_or(PCWSTR::null(), |a| PCWSTR(a.as_ptr()));
    let result = unsafe { ShellExecuteW(None, PCWSTR(verb.as_ptr()), PCWSTR(file.as_ptr()), args, PCWSTR::null(), SW_SHOWNORMAL) };
    // An `HINSTANCE` of 32 or less is an error code.
    result.0 as usize > 32
}

/// The clipboard's text, if it holds any.
pub(crate) fn clipboard_text() -> Option<String> {
    use windows::Win32::Foundation::HGLOBAL;
    use windows::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard};
    use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};
    use windows::Win32::System::Ole::CF_UNICODETEXT;

    unsafe {
        OpenClipboard(None).ok()?;
        let text = GetClipboardData(CF_UNICODETEXT.0 as u32).ok().and_then(|handle| {
            let global = HGLOBAL(handle.0);
            let ptr = GlobalLock(global) as *const u16;
            if ptr.is_null() {
                return None;
            }
            let len = (0..).take_while(|&i| *ptr.add(i) != 0).count();
            let text = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
            let _ = GlobalUnlock(global);
            Some(text)
        });
        let _ = CloseClipboard();
        text
    }
}

/// Put `text` on the clipboard.
pub(crate) fn set_clipboard_text(text: &str) -> bool {
    use windows::Win32::Foundation::{GlobalFree, HANDLE};
    use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    use windows::Win32::System::Ole::CF_UNICODETEXT;

    let text = wide(text);
    unsafe {
        let Ok(global) = GlobalAlloc(GMEM_MOVEABLE, text.len() * 2) else { return false };
        let ptr = GlobalLock(global) as *mut u16;
        if ptr.is_null() {
            let _ = GlobalFree(Some(global));
            return false;
        }
        std::ptr::copy_nonoverlapping(text.as_ptr(), ptr, text.len());
        let _ = GlobalUnlock(global);
        if OpenClipboard(None).is_err() {
            let _ = GlobalFree(Some(global));
            return false;
        }
        let _ = EmptyClipboard();
        // The clipboard owns the memory once this succeeds.
        let placed = SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(global.0))).is_ok();
        if !placed {
            let _ = GlobalFree(Some(global));
        }
        let _ = CloseClipboard();
        placed
    }
}
