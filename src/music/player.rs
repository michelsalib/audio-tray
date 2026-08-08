//! The player behind the strip: which window it is, how to bring it forward, and its progress bar.
//!
//! **There is no menu of ours any more.** A right-click used to open one — a Win32 popup, then an
//! owner-drawn flyout — and both are gone with the self-updater they mostly existed to offer. On an
//! app's own taskbar button the right click now falls through to the shell, which answers it with
//! that app's jump list; that is a better answer than anything we had in there.

use anyhow::{bail, Context, Result};
use windows::Win32::Foundation::HWND;
use windows_core::PCWSTR;

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(core::iter::once(0)).collect()
}

/// Where the player's AUMID is remembered between runs.
///
/// **Needed because the identity outlives the session.** A media session only exists while
/// something is playing, so with YouTube Music shut down there is no app id to activate — and
/// falling back to the `https://music.youtube.com` URL is wrong in a specific, visible way: the
/// shell hands it to the default browser, which opens it as a *tab in Edge's last-used profile*
/// rather than launching the installed PWA.
fn remembered_path() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")?;
    Some(std::path::PathBuf::from(base).join("audio-tray").join("player-aumid.txt"))
}

/// Remember a packaged app id, so the PWA can be launched later from cold.
///
/// Only packaged identities are kept: a bare `Chrome`/`MSEdge` id names a browser, not an app,
/// and could not be activated anyway.
pub fn remember_player(app_id: &str) {
    if !app_id.contains('!') {
        return;
    }
    let Some(path) = remembered_path() else { return };
    if remembered_player().as_deref() == Some(app_id) {
        return;
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, app_id);
}

/// The last packaged player identity we saw.
pub fn remembered_player() -> Option<String> {
    let path = remembered_path()?;
    let id = std::fs::read_to_string(path).ok()?.trim().to_string();
    (!id.is_empty()).then_some(id)
}

/// What [`activate_player`] actually did — worth reporting, because the three outcomes fail in
/// different places and only one of them can be checked afterwards by looking for a window.
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
    // Find and raise the existing window first, because activation is **not** idempotent: each
    // call starts a *fresh* YouTube Music window rather than activating the running one. Measured
    // the hard way — four activations left four cascaded windows, all live at once. So launching
    // is only for when there is genuinely no window to raise.
    if let Some(hwnd) = player_window() {
        return Ok(Activation::Raised(raise(hwnd)));
    }

    // The live session id if there is one, otherwise the identity remembered from when there
    // last was. Only a packaged identity can be launched as an app; the URL is the last resort
    // for a machine where YouTube Music has never played, and it will open as a browser tab.
    let identity = app_id
        .filter(|id| id.contains('!'))
        .map(str::to_string)
        .or_else(remembered_player);
    match identity {
        // A packaged identity goes through the activation manager, which is what the shell's own
        // app list uses and the only route that reports *why* it failed.
        Some(aumid) => activate_packaged(&aumid).map(Activation::Started),
        None => launch("https://music.youtube.com").map(|()| Activation::OpenedUrl),
    }
}

/// Activate a packaged app by AUMID, the way the Start menu does.
///
/// `IApplicationActivationManager` rather than a shell target, because it is the one route that
/// answers back: it returns an `HRESULT` **and the pid it started**, where `ShellExecuteW` on
/// `shell:AppsFolder\<aumid>` returns only a handle-shaped number that says the shell accepted the
/// request. Measured: `Started(14812)`, and a window 3 s later.
///
/// The pid is the point. A long hunt for a launch that "silently did nothing" turned out to be a
/// launch that worked and a *measurement* that was broken — see FINDINGS.md — and no amount of
/// staring at `ShellExecuteW`'s return value could have told the two apart. This one can.
fn activate_packaged(aumid: &str) -> Result<u32> {
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_LOCAL_SERVER, COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::{
        ApplicationActivationManager, IApplicationActivationManager, AO_NONE,
    };

    unsafe {
        // Already-initialized is not an error here: the caller may or may not have set the
        // apartment up, and this call works in either one.
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let manager: IApplicationActivationManager =
            CoCreateInstance(&ApplicationActivationManager, None, CLSCTX_LOCAL_SERVER)
                .context("CoCreateInstance(ApplicationActivationManager)")?;
        let aumid_w = wide(aumid);
        manager
            .ActivateApplication(PCWSTR(aumid_w.as_ptr()), PCWSTR::null(), AO_NONE)
            .with_context(|| format!("ActivateApplication({aumid})"))
    }
}

/// Open a URL in the default browser — the last resort, for a machine where YouTube Music has
/// never played and so has left no packaged identity to activate.
///
/// **`ShellExecuteW`, not `Command::new("explorer.exe")`.** Spawning explorer only reports that a
/// process started, so explorer exiting without honouring its argument reads as success — which is
/// exactly how the strip could claim "brought the player forward" and do nothing. `ShellExecuteW`
/// performs the activation itself and returns a value ≤ 32 on failure.
///
/// That makes it the better of the two, not a proof of anything: a return above 32 says the shell
/// took the request, not that a window appeared. Where that difference matters — activating the
/// packaged player — [`activate_packaged`] is used instead, because it hands back a pid.
fn launch(target: &str) -> Result<()> {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let target_w = wide(target);
    let verb = wide("open");
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
    // The documented convention: an `HINSTANCE` of 32 or less is an error code, not a handle.
    if result.0 as usize <= 32 {
        bail!("ShellExecute refused {target} (code {})", result.0 as usize);
    }
    Ok(())
}

/// Every top-level window with `youtube` in its title, described — for `--activate`.
///
/// The point is the *rejected* ones. `player_window` takes the first visible match and
/// `raise` then reports success, so a window that is visible but cloaked (another virtual desktop,
/// or Edge holding a PWA window it is not showing) makes the strip claim it brought the player
/// forward while nothing appears. This is what distinguishes those cases from "no window at all",
/// which needs the opposite fix.
pub fn player_windows() -> Vec<String> {
    use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetClassNameW, GetWindowLongW, GetWindowRect, GetWindowTextW,
        GetWindowThreadProcessId, IsIconic, IsWindowVisible, GWL_EXSTYLE,
    };
    use windows_core::BOOL;

    unsafe extern "system" fn visit(
        hwnd: HWND,
        lparam: windows::Win32::Foundation::LPARAM,
    ) -> BOOL {
        let found = unsafe { &mut *(lparam.0 as *mut Vec<String>) };
        let mut title = [0u16; 512];
        let len = unsafe { GetWindowTextW(hwnd, &mut title) };
        if len == 0 {
            return BOOL(1);
        }
        let title = String::from_utf16_lossy(&title[..len as usize]);
        if !title.to_lowercase().contains("youtube") {
            return BOOL(1);
        }
        let mut pid = 0u32;
        let _ = unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        let mut cloaked = 0u32;
        let _ = unsafe {
            DwmGetWindowAttribute(
                hwnd,
                DWMWA_CLOAKED,
                &mut cloaked as *mut u32 as *mut core::ffi::c_void,
                size_of::<u32>() as u32,
            )
        };
        let mut rect = windows::Win32::Foundation::RECT::default();
        let _ = unsafe { GetWindowRect(hwnd, &mut rect) };
        let mut class = [0u16; 128];
        let class_len = unsafe { GetClassNameW(hwnd, &mut class) };
        let class = String::from_utf16_lossy(&class[..class_len.max(0) as usize]);
        let ex_style = unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) };
        found.push(format!(
            "hwnd {:?} pid {pid} vis {} icon {} cloak {cloaked} \
             rect {},{} {}x{} ex {ex_style:#x} class {class} : {title}",
            hwnd.0,
            unsafe { IsWindowVisible(hwnd) }.as_bool(),
            unsafe { IsIconic(hwnd) }.as_bool(),
            rect.left,
            rect.top,
            rect.right - rect.left,
            rect.bottom - rect.top,
        ));
        BOOL(1)
    }

    let mut found: Vec<String> = Vec::new();
    let _ = unsafe {
        EnumWindows(
            Some(visit),
            windows::Win32::Foundation::LPARAM(&mut found as *mut Vec<String> as isize),
        )
    };
    found
}

/// The YouTube Music window, if it is open.
///
/// Matched on the window title rather than the process, because the PWA runs inside an
/// `msedge.exe` shared with every other Edge window — the title is what distinguishes it. Only
/// visible top-level windows are considered, so hidden helper windows never match.
pub fn player_window() -> Option<HWND> {
    use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowTextW, IsWindowVisible};
    use windows_core::BOOL;

    struct Search {
        found: HWND,
    }

    unsafe extern "system" fn visit(
        hwnd: HWND,
        lparam: windows::Win32::Foundation::LPARAM,
    ) -> BOOL {
        let search = unsafe { &mut *(lparam.0 as *mut Search) };
        if !unsafe { IsWindowVisible(hwnd) }.as_bool() {
            return BOOL(1);
        }
        let mut title = [0u16; 512];
        let len = unsafe { GetWindowTextW(hwnd, &mut title) };
        if len > 0 {
            let title = String::from_utf16_lossy(&title[..len as usize]).to_lowercase();
            if title.contains("youtube music") {
                search.found = hwnd;
                return BOOL(0);
            }
        }
        BOOL(1)
    }

    let mut search = Search {
        found: HWND(std::ptr::null_mut()),
    };
    let _ = unsafe {
        EnumWindows(
            Some(visit),
            windows::Win32::Foundation::LPARAM(&mut search as *mut Search as isize),
        )
    };
    (!search.found.0.is_null()).then_some(search.found)
}

/// Set the taskbar progress bar on the player's window — the line MPC-HC draws under its icon.
///
/// **Cross-process, which is the part that had to be measured.** `ITaskbarList3` is normally an app
/// reporting its *own* progress, and nothing in the documentation says a different process may report
/// it for someone else's window. If the shell accepts it, the bar we get is the shell's own: right
/// colour, right place, right animation, and no drawing of ours to keep in step with it.
///
/// `fraction` is 0.0–1.0 and `playing` picks the colour, matching what the taskbar already means by
/// them elsewhere: green while it runs, yellow when it is paused. A fraction outside the track — no
/// timeline, or nothing playing — clears the bar rather than drawing a zero-length one.
pub fn set_player_progress(fraction: Option<f64>, playing: bool) -> Result<()> {
    let hwnd = player_window().context("no YouTube Music window to put a progress bar on")?;
    // **Cached per thread, not rebuilt per call.** `CoCreateInstance` plus `HrInit` is a broker
    // round-trip, and this is called from a poll; the object is apartment-affine, so a thread-local
    // is exactly the right lifetime for it — it lives as long as the apartment that may use it.
    //
    // A `RefCell` rather than a `OnceCell` because it has to be droppable: this is a **proxy into
    // explorer.exe**, so an Explorer restart leaves it pointing at a process that no longer exists.
    // See [`forget_taskbar_list`].
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

/// Drop the cached `ITaskbarList3` so the next call builds a fresh one.
///
/// **An `ITaskbarList3` is a proxy into `explorer.exe`.** When Explorer restarts, every interface
/// pointer we are holding refers to a dead process — and the calls do not necessarily *fail* in a way
/// that shows up, they simply stop having any effect. That is the whole reason a progress bar and a
/// thumbnail toolbar could both come back "successfully" after a restart and neither appear.
pub fn forget_taskbar_list() {
    TASKBAR.with(|cell| *cell.borrow_mut() = None);
}

/// The shell's taskbar list, initialised. Kept by the caller — see [`crate::progress::Progress`].
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

/// Fill `hwnd`'s taskbar progress bar to `fraction`, or clear it.
///
/// The state picks the colour, matching what the taskbar already means by them: green while it runs,
/// yellow when it is paused — which is why MPC-HC's bar is yellow in the screenshot that prompted
/// this.
pub fn set_progress(
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
                let completed = (fraction.clamp(0.0, 1.0) * TOTAL as f64).round() as u64;
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

/// Which route actually brought the window forward.
///
/// Reported rather than swallowed because **every one of these calls returns success while doing
/// nothing.** `SetForegroundWindow` returns non-zero, `SwitchToThisWindow` returns nothing at all,
/// and the window stays behind whatever the user was looking at — which is exactly how the strip
/// logged `Activate -> Raised` six times in a row against a player that never came forward.
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

/// Bring a window to the front, restoring it if minimised — and **check that it worked**.
///
/// Windows refuses a foreground change from a process the user has not just interacted with, and
/// audio-tray is exactly that process: the click lands in Explorer, which posts us a message, so by
/// the time we ask we have no rights. There is no single call that fixes it, so this escalates and
/// reports which rung it got to:
///
/// 1. `SetForegroundWindow`, which works when the TAP managed to hand its rights over
///    (`AllowSetForegroundWindow`, called from Explorer where the click actually happened).
/// 2. `AttachThreadInput` to the foreground window's thread, then ask again. Sharing an input queue
///    makes the check see one process where there were two — the standard route for this, and the
///    reason it is second rather than first is that it perturbs another process's input queue.
/// 3. `SwitchToThisWindow`, which the shell uses for Alt-Tab. Undocumented and increasingly ignored
///    on Windows 11, so last.
///
/// `GetForegroundWindow` after each rung is the only honest test, because none of the calls tell
/// the truth about whether they did anything.
fn raise(hwnd: HWND) -> Raise {
    use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, GetForegroundWindow, GetWindowThreadProcessId, IsIconic,
        SetForegroundWindow, ShowWindow, SwitchToThisWindow, SW_RESTORE, SW_SHOW,
    };

    let arrived = |hwnd: HWND| unsafe { GetForegroundWindow() } == hwnd;

    unsafe {
        // Restoring first, always: a minimised window cannot be foregrounded, and this is also the
        // one step that needs no rights at all.
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

        // Borrow the input queue of whoever holds the foreground. Detached again immediately: a
        // thread left attached shares keyboard focus with ours, which would be a real bug in
        // somebody else's app.
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

