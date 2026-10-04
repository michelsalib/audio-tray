//! The player behind the strip: which window it is, how to bring it forward, and its progress bar.
//!
//! Clicks on the tile body are left to the shell (it is the app's own taskbar button).

use anyhow::{bail, Context, Result};
use windows::Win32::Foundation::HWND;
use windows_core::PCWSTR;

use super::session;

/// Where the player's AUMID is remembered between runs: the session (and its id) only exists while
/// playing, and the URL fallback opens a browser tab instead of the PWA.
fn remembered_path() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")?;
    Some(std::path::PathBuf::from(base).join("audio-tray").join("player-aumid.txt"))
}

/// Remember a packaged app id (`…!App`) so the PWA can be launched from cold; others are ignored.
pub fn remember_player(app_id: &str) {
    if !app_id.contains('!') {
        return;
    }
    let mut known = REMEMBERED.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if known.as_deref() == Some(app_id) {
        return;
    }
    let Some(path) = remembered_path() else { return };
    *known = Some(app_id.to_string());
    if std::fs::read_to_string(&path).is_ok_and(|on_disk| on_disk.trim() == app_id) {
        return;
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, app_id);
}

/// What [`remember_player`] last wrote or confirmed, so a poll costs no file read.
static REMEMBERED: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// The last packaged player identity we saw.
pub fn remembered_player() -> Option<String> {
    let path = remembered_path()?;
    let id = std::fs::read_to_string(path).ok()?.trim().to_string();
    (!id.is_empty()).then_some(id)
}

/// What [`activate_player`] actually did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Activation {
    /// A window was already open; this is which route brought it forward, or that none did.
    Raised(Raise),
    /// The packaged app was activated; the shell reported this process id for it.
    Started(u32),
    /// No packaged identity is known, so the site was opened in the browser instead.
    OpenedUrl,
}

pub fn activate_player(app_id: Option<&str>) -> Result<Activation> {
    // Raise an existing window first: activation is not idempotent, each call opens a new window.
    if let Some(hwnd) = player_window() {
        return Ok(Activation::Raised(raise(hwnd)));
    }

    // Live packaged id, else the remembered one; the URL is the last resort (opens a browser tab).
    let identity = app_id
        .filter(|id| id.contains('!'))
        .map(str::to_string)
        .or_else(remembered_player);
    match identity {
        Some(aumid) => activate_packaged(&aumid).map(Activation::Started),
        None => launch("https://music.youtube.com").map(|()| Activation::OpenedUrl),
    }
}

/// Activate a packaged app by AUMID, the way the Start menu does. Unlike `ShellExecuteW` on
/// `shell:AppsFolder`, this reports failure and returns the started pid.
fn activate_packaged(aumid: &str) -> Result<u32> {
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_LOCAL_SERVER, COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::{
        ApplicationActivationManager, IApplicationActivationManager, AO_NONE,
    };

    unsafe {
        // Already-initialised (either apartment) is fine; this call works in both.
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let manager: IApplicationActivationManager =
            CoCreateInstance(&ApplicationActivationManager, None, CLSCTX_LOCAL_SERVER)
                .context("CoCreateInstance(ApplicationActivationManager)")?;
        let aumid_w = crate::win::wide(aumid);
        manager
            .ActivateApplication(PCWSTR(aumid_w.as_ptr()), PCWSTR::null(), AO_NONE)
            .with_context(|| format!("ActivateApplication({aumid})"))
    }
}

/// Open a URL in the default browser (no packaged identity known). `ShellExecuteW` rather than
/// spawning explorer.exe, which reports success even when it ignores the argument.
fn launch(target: &str) -> Result<()> {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let target_w = crate::win::wide(target);
    let verb = crate::win::wide("open");
    let result = unsafe {
        ShellExecuteW(
            None,
            PCWSTR(verb.as_ptr()),
            PCWSTR(target_w.as_ptr()),
            None,
            None,
            SW_SHOWNORMAL,
        )
    };
    // An `HINSTANCE` of 32 or less is an error code.
    if result.0 as usize <= 32 {
        bail!("ShellExecute refused {target} (code {})", result.0 as usize);
    }
    Ok(())
}

/// The shell's AppUserModelID for a window (what the taskbar groups by); `None` if it publishes
/// none, the normal case for a plain Win32 app.
pub fn window_app_id(hwnd: HWND) -> Option<String> {
    use windows::Win32::Storage::EnhancedStorage::PKEY_AppUserModel_ID;
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
    use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow};

    unsafe {
        let store: IPropertyStore = SHGetPropertyStoreForWindow(hwnd).ok()?;
        let value = store.GetValue(&PKEY_AppUserModel_ID).ok()?;
        // No id set answers VT_EMPTY rather than an error, so emptiness means "no identity".
        let text = PropVariantToStringAlloc(&value).ok()?;
        let id = text.to_string().ok();
        CoTaskMemFree(Some(text.0 as *const core::ffi::c_void));
        id.filter(|id| !id.trim().is_empty())
    }
}

/// The lowercase image name of the process owning a window (`msedge.exe`, …).
pub fn window_process(hwnd: HWND) -> Option<String> {
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
    use windows_core::PWSTR;

    unsafe {
        let mut pid = 0u32;
        let _ = GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buffer = [0u16; 260];
        let mut len = buffer.len() as u32;
        let path = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut len,
        )
        .ok()
        .map(|()| String::from_utf16_lossy(&buffer[..len as usize]));
        let _ = windows::Win32::Foundation::CloseHandle(process);
        path.map(|path| {
            path.rsplit('\\')
                .next()
                .unwrap_or(&path)
                .to_ascii_lowercase()
        })
    }
}

/// One window surveyed by [`player_windows`]. `hwnd` is an `isize` so the list can cross to
/// [`crate::music::player_verdicts_from_mta`].
#[cfg(feature = "dev")]
pub struct WindowReport {
    pub hwnd: isize,
    pub player: bool,
    pub line: String,
}

/// Every top-level window with `youtube` in its title, described for `--music-windows` (cloak state,
/// app id, process, verdict). `all` lists every visible titled window instead.
#[cfg(feature = "dev")]
pub fn player_windows(all: bool) -> Vec<WindowReport> {
    use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongW, GetWindowRect, GetWindowThreadProcessId, IsIconic, IsWindowVisible, GWL_EXSTYLE,
    };

    let mut found = Vec::new();
    crate::win::enum_windows(|hwnd| {
        let title = crate::win::window_title(hwnd);
        let visible = unsafe { IsWindowVisible(hwnd) }.as_bool();
        if title.is_empty() || (!all && !title.to_lowercase().contains("youtube")) || (all && !visible) {
            return true;
        }
        let mut pid = 0u32;
        let _ = unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        let mut cloaked = 0u32;
        let _ = unsafe {
            DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut core::ffi::c_void, size_of::<u32>() as u32)
        };
        let mut rect = windows::Win32::Foundation::RECT::default();
        let _ = unsafe { GetWindowRect(hwnd, &mut rect) };
        let class = crate::win::class_name(hwnd);
        let ex_style = unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) };
        let app_id = window_app_id(hwnd);
        let process = window_process(hwnd);
        let player = session::window_is_player(app_id.as_deref(), process.as_deref());
        let app_id = app_id.unwrap_or_else(|| "<none>".to_string());
        let process = process.unwrap_or_else(|| "<unknown>".to_string());
        found.push(WindowReport {
            hwnd: hwnd.0 as isize,
            player,
            line: format!(
                "hwnd {:?} pid {pid} {process} vis {visible} icon {} cloak {cloaked} \
                 rect {},{} {}x{} ex {ex_style:#x} class {class}\n    aumid {app_id}\n    \
                 player {player}\n    title {title}",
                hwnd.0,
                unsafe { IsIconic(hwnd) }.as_bool(),
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
            ),
        });
        true
    });
    found
}

/// Whether a window is the player's own (see [`session::window_is_player`]); the process is read
/// only when the window has no app id.
pub fn is_player_window(hwnd: HWND) -> bool {
    let app_id = window_app_id(hwnd);
    let process = match app_id {
        Some(_) => None,
        None => window_process(hwnd),
    };
    session::window_is_player(app_id.as_deref(), process.as_deref())
}

/// The YouTube Music window: visible, titled `youtube music`, and the player's own (a browser window
/// with that tab active must not match — the toolbar put on it cannot be removed).
pub fn player_window() -> Option<HWND> {
    use windows::Win32::UI::WindowsAndMessaging::IsWindowVisible;

    let mut found = None;
    crate::win::enum_windows(|hwnd| {
        if !unsafe { IsWindowVisible(hwnd) }.as_bool() {
            return true;
        }
        // Title first: the identity is a cross-process call and this runs every poll.
        if crate::win::window_title(hwnd).to_lowercase().contains("youtube music") && is_player_window(hwnd) {
            found = Some(hwnd);
        }
        found.is_none()
    });
    found
}

/// Set the taskbar progress bar on the player's (another process's) window. `fraction` is 0.0–1.0,
/// `None` clears; `playing` picks normal vs paused colour. Call on an STA (the tray thread).
pub fn set_player_progress(fraction: Option<f64>, playing: bool) -> Result<()> {
    let hwnd = player_window().context("no YouTube Music window to put a progress bar on")?;
    // Cached per thread (apartment-affine); droppable because an Explorer restart kills the proxy.
    TASKBAR.with(|cell| {
        if cell.borrow().is_none() {
            *cell.borrow_mut() = Some(taskbar_list()?);
        }
        let borrowed = cell.borrow();
        let taskbar = borrowed.as_ref().context("caching ITaskbarList3")?;
        set_progress(taskbar, hwnd, fraction, playing)
    })
}

thread_local! {
    static TASKBAR: std::cell::RefCell<Option<windows::Win32::UI::Shell::ITaskbarList3>> =
        const { std::cell::RefCell::new(None) };
}

/// Drop the cached `ITaskbarList3` (a proxy into Explorer, which silently stops working after an
/// Explorer restart) so the next call builds a fresh one.
pub fn forget_taskbar_list() {
    TASKBAR.with(|cell| *cell.borrow_mut() = None);
}

/// A fresh, initialised `ITaskbarList3` (the thumbnail toolbar keeps its own on the feed thread).
pub fn taskbar_list() -> Result<windows::Win32::UI::Shell::ITaskbarList3> {
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::{ITaskbarList3, TaskbarList};

    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let taskbar: ITaskbarList3 = CoCreateInstance(&TaskbarList, None, CLSCTX_ALL)
            .context("CoCreateInstance(TaskbarList)")?;
        taskbar.HrInit().context("ITaskbarList3::HrInit")?;
        Ok(taskbar)
    }
}

/// Fill `hwnd`'s taskbar progress bar to `fraction` (paused state when not playing), or clear it.
fn set_progress(
    taskbar: &windows::Win32::UI::Shell::ITaskbarList3,
    hwnd: HWND,
    fraction: Option<f64>,
    playing: bool,
) -> Result<()> {
    use windows::Win32::UI::Shell::{TBPF_NOPROGRESS, TBPF_NORMAL, TBPF_PAUSED};

    /// The denominator. Fine enough that the shell's own rounding, not ours, decides the pixel.
    const TOTAL: u64 = 1000;

    unsafe {
        match fraction {
            Some(fraction) => {
                // Never 0: the shell treats zero as no progress and rebuilds `ProgressIndicator`, which
                // jumps until the TAP re-pins it.
                let completed = ((fraction.clamp(0.0, 1.0) * TOTAL as f64).round() as u64).max(1);
                taskbar
                    .SetProgressState(hwnd, if playing { TBPF_NORMAL } else { TBPF_PAUSED })
                    .context("SetProgressState")?;
                taskbar
                    .SetProgressValue(hwnd, completed, TOTAL)
                    .context("SetProgressValue")?;
            }
            None => taskbar
                .SetProgressState(hwnd, TBPF_NOPROGRESS)
                .context("SetProgressState(NOPROGRESS)")?,
        }
    }
    Ok(())
}

/// Which route actually brought the window forward, checked with `GetForegroundWindow` because
/// every call reports success while doing nothing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Raise {
    /// It was already the foreground window.
    AlreadyThere,
    /// `SetForegroundWindow` was honoured — meaning we had foreground rights.
    Foreground,
    /// Honoured only after borrowing the foreground thread's input queue.
    Attached,
    /// Honoured only by `SwitchToThisWindow`.
    Switched,
    /// Nothing worked: the window is still behind.
    Refused,
}

/// Bring a window to the front, restoring it if minimised, escalating `SetForegroundWindow` →
/// `AttachThreadInput` + retry → `SwitchToThisWindow` and verifying each rung. We usually lack
/// foreground rights (the click reached us as a message from Explorer).
fn raise(hwnd: HWND) -> Raise {
    use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, GetForegroundWindow, GetWindowThreadProcessId, IsIconic,
        SetForegroundWindow, ShowWindow, SwitchToThisWindow, SW_RESTORE, SW_SHOW,
    };

    let arrived = |hwnd: HWND| unsafe { GetForegroundWindow() } == hwnd;

    unsafe {
        // Restore first: needs no rights, and a minimised window cannot be foregrounded.
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        } else {
            let _ = ShowWindow(hwnd, SW_SHOW);
        }
        if arrived(hwnd) {
            return Raise::AlreadyThere;
        }

        let _ = BringWindowToTop(hwnd);
        let _ = SetForegroundWindow(hwnd);
        if arrived(hwnd) {
            return Raise::Foreground;
        }

        // Borrow the foreground thread's input queue; always detach immediately after.
        let foreground = GetForegroundWindow();
        let their_thread = GetWindowThreadProcessId(foreground, None);
        let ours = GetCurrentThreadId();
        if their_thread != 0 && their_thread != ours {
            let attached = AttachThreadInput(ours, their_thread, true).as_bool();
            let _ = BringWindowToTop(hwnd);
            let _ = SetForegroundWindow(hwnd);
            if attached {
                let _ = AttachThreadInput(ours, their_thread, false);
            }
            if arrived(hwnd) {
                return Raise::Attached;
            }
        }

        SwitchToThisWindow(hwnd, true);
        if arrived(hwnd) {
            return Raise::Switched;
        }
    }
    Raise::Refused
}

