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
/// `Err` when another program holds the clipboard open (clipboard managers do, briefly).
pub(crate) fn clipboard_text() -> Result<Option<String>, ()> {
    use windows::Win32::Foundation::HGLOBAL;
    use windows::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard};
    use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
    use windows::Win32::System::Ole::CF_UNICODETEXT;

    unsafe {
        OpenClipboard(crate::taskbar::receiver()).map_err(|_| ())?;
        let text = GetClipboardData(CF_UNICODETEXT.0 as u32).ok().and_then(|handle| {
            let global = HGLOBAL(handle.0);
            let ptr = GlobalLock(global) as *const u16;
            if ptr.is_null() {
                return None;
            }
            // Up to the terminator, never past the block (a producer may have left none).
            let units = GlobalSize(global) / 2;
            let len = (0..units).take_while(|&i| *ptr.add(i) != 0).count();
            let text = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
            let _ = GlobalUnlock(global);
            Some(text)
        });
        let _ = CloseClipboard();
        Ok(text)
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
        // An owner window: opened with none, the clipboard has no owner and setting data fails.
        if OpenClipboard(crate::taskbar::receiver()).is_err() {
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

/// Whether a wheel `delta` came from a touchpad. Windows hands a precision touchpad's scroll on as
/// wheel input from a hardware mouse (the input source cannot tell them apart), but in small
/// steps, where a wheel turns in whole notches of 120. A gesture is judged as a whole: a step of
/// a whole notch within half a second of a fine one is the touchpad's too, so a fast swipe never
/// flips direction half-way. (A free-spinning high-resolution wheel also steps finely, and counts
/// as a touchpad: the price of a guess no API answers for the taskbar's scrolls.)
pub(crate) fn touchpad_delta(delta: i32) -> bool {
    static SOURCE: std::sync::Mutex<ScrollSource> = std::sync::Mutex::new(ScrollSource { last_fine: None });
    SOURCE.lock().unwrap_or_else(|p| p.into_inner()).touchpad(delta, millis())
}

/// Milliseconds on a monotonic clock, from the first call.
fn millis() -> u64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64
}

/// [`touchpad_delta`]'s memory: when the last fine step came.
struct ScrollSource {
    last_fine: Option<u64>,
}

impl ScrollSource {
    /// A gesture's whole-notch steps this close to a fine one are the touchpad's too.
    const GESTURE_MS: u64 = 500;

    fn touchpad(&mut self, delta: i32, now: u64) -> bool {
        use windows::Win32::UI::WindowsAndMessaging::WHEEL_DELTA;
        if delta % WHEEL_DELTA as i32 != 0 {
            self.last_fine = Some(now);
            return true;
        }
        self.last_fine.is_some_and(|last| now.saturating_sub(last) < Self::GESTURE_MS)
    }
}

/// A scroll `delta` as a volume change: up is louder. A touchpad set to scroll the "natural" way
/// (content follows the fingers, Windows' default) reports pushing two fingers up as scrolling
/// down; a volume is a slider, not content, so the fingers going up still raise it.
pub(crate) fn volume_delta(delta: i32, touchpad: bool) -> i32 {
    if touchpad && natural_scrolling() {
        -delta
    } else {
        delta
    }
}

/// Whether the touchpad scrolls the "natural" way. Read from the registry at most every few
/// seconds (a touchpad sends a hundred steps a second), so a change in Settings soon applies.
fn natural_scrolling() -> bool {
    use std::sync::atomic::{AtomicU64, Ordering};
    use windows::core::w;

    const FRESH_MS: u64 = 5000;
    // The reading in the low bit, when it was taken (+1, so 0 means never) above it.
    static CACHED: AtomicU64 = AtomicU64::new(0);
    let now = millis() + 1;
    let cached = CACHED.load(Ordering::Relaxed);
    if cached != 0 && now - (cached >> 1) < FRESH_MS {
        return cached & 1 == 1;
    }
    // 0 is "down motion scrolls up"; the other choice is stored as 0xFFFFFFFF.
    let natural = hkcu_dword(w!(r"Software\Microsoft\Windows\CurrentVersion\PrecisionTouchPad"), w!("ScrollDirection")).unwrap_or(0) == 0;
    CACHED.store((now << 1) | u64::from(natural), Ordering::Relaxed);
    natural
}

#[cfg(test)]
mod scroll_tests {
    use super::ScrollSource;

    #[test]
    fn fine_steps_are_a_touchpad_and_keep_its_gesture_together() {
        let mut source = ScrollSource { last_fine: None };
        assert!(!source.touchpad(120, 0), "a whole notch alone is a wheel");
        assert!(source.touchpad(-27, 1_000), "a fine step is a touchpad");
        assert!(source.touchpad(-120, 1_200), "a whole notch mid-gesture stays the touchpad's");
        assert!(!source.touchpad(120, 2_000), "a notch well after is a wheel again");
    }
}
/// The full path of process `pid`'s program, if it can be queried (not for protected processes).
pub(crate) fn process_exe(pid: u32) -> Option<String> {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};

    if pid == 0 {
        return None;
    }
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(process);
        ok.ok()?;
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// The full path of the program owning a window.
pub(crate) fn window_exe(hwnd: windows::Win32::Foundation::HWND) -> Option<String> {
    use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    process_exe(pid)
}

/// The lowercase file name of the program owning a window (`msedge.exe`, …).
pub(crate) fn window_process(hwnd: windows::Win32::Foundation::HWND) -> Option<String> {
    let path = window_exe(hwnd)?;
    Some(path.rsplit('\\').next().unwrap_or(&path).to_ascii_lowercase())
}