//! The control window: a hidden window created on the tray island's thread, so cross-process
//! messages (revert, restyle, handover) and the sweep timer are dispatched by Explorer's own pump
//! on the one thread that may touch the tray's XAML. Never create it from `SetSite`, which runs on
//! a marshalling thread. Also watches the owner process and reverts when it exits.
//! See FINDINGS.md, "Getting "revert now" onto the XAML thread".

use core::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};

use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{
    OpenProcess, WaitForSingleObject, INFINITE, PROCESS_SYNCHRONIZE,
};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, KillTimer, PostMessageW, RegisterClassW, SetTimer, HMENU,
    WM_COPYDATA, WM_TIMER, WNDCLASSW, WS_EX_TOOLWINDOW, WS_POPUP,
};

use crate::log::logf;

pub use tap_proto::CONTROL_CLASS;

pub use tap_proto::WM_TAP_REVERT;

pub use tap_proto::WM_TAP_RESTYLE;

pub use tap_proto::WM_TAP_WIRE_TRANSPORT;

pub use tap_proto::WM_TAP_REPIN;

/// Retries a re-pin that met a busy thread or a stream that was not yet quiet.
const REPIN_TIMER: usize = 2;
const REPIN_RETRY_MS: u32 = 16;
/// About a second of retries; past that the sweep (the safety net) takes over.
const REPIN_MAX_TRIES: u32 = 60;
static REPIN_TRIES: AtomicU32 = AtomicU32::new(0);

/// Timer id for the periodic check that the strip is still there.
const SWEEP_TIMER: usize = 1;

/// Sweep interval while there is work outstanding. The sweep is the only mutator, so this bounds
/// how fast the strip appears or a declined redraw lands (user-visible).
const SWEEP_FAST_MS: u32 = 250;

/// Sweep interval once everything is applied (it runs on Explorer's UI thread; its remaining job
/// is noticing the shell overwrote our strip).
const SWEEP_IDLE_MS: u32 = 4000;

/// The interval currently armed, so the timer is only re-armed when it changes.
static SWEEP_INTERVAL: AtomicU32 = AtomicU32::new(0);

/// The control window, or 0 before it exists. Also the "already created" flag.
static WINDOW: AtomicIsize = AtomicIsize::new(0);

/// The thread that owns the taskbar's XAML island (Explorer calls back on several islands'
/// threads), learned from `SystemTray.*` events by [`adopt_tray_thread`].
static TRAY_TID: AtomicU32 = AtomicU32::new(0);

/// Claims the calling thread as the tray's. **Last caller wins, deliberately**: the replay arrives
/// on a marshalling thread that cannot touch the tray, and first-wins pinned that one and froze
/// the shell. Any `SystemTray.*` element counts; narrower types only match during the replay.
pub fn adopt_tray_thread() {
    let me = crate::tid();
    if TRAY_TID.swap(me, Ordering::SeqCst) != me {
        logf!("tray island is thread {me}");
    }
}

/// Whether the caller is on the tray's thread. False until [`adopt_tray_thread`] has run.
pub fn on_tray_thread() -> bool {
    let owner = TRAY_TID.load(Ordering::SeqCst);
    owner != 0 && owner == crate::tid()
}

/// A revert that arrived before the control window existed (the owner died early); run by the
/// next tray callback.
static PENDING_REVERT: AtomicBool = AtomicBool::new(false);

/// Process id of whoever currently owns the strip. A watcher whose owner is no longer this (a
/// restart spawns its successor first) must not revert.
static OWNER_PID: AtomicU32 = AtomicU32::new(0);

unsafe extern "system" fn control_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_TAP_REVERT {
        // Inside Explorer's message pump: nothing may unwind.
        let caught = std::panic::catch_unwind(|| {
            let from = wparam.0 as u32;
            if !revert_is_current(from) {
                logf!("revert from pid {from} ignored — pid {} owns the strip now", OWNER_PID.load(Ordering::SeqCst));
                return;
            }
            logf!("revert requested — on thread {}", crate::tid());
            unsafe { crate::revert() };
        });
        if caught.is_err() {
            logf!("revert handler panicked");
        }
        return LRESULT(0);
    }
    if msg == WM_TAP_RESTYLE {
        let caught = std::panic::catch_unwind(|| {
            // Unpack: codepoint in the low 24 bits, muted at bit 24, and — the input's
            // alone — "an app is recording" at bit 25.
            let glyph = |packed: usize| {
                (
                    char::from_u32((packed & tap_proto::RESTYLE_GLYPH_MASK) as u32),
                    packed & tap_proto::RESTYLE_MUTED != 0,
                    packed & tap_proto::RESTYLE_RECORDING != 0,
                )
            };
            let (out, out_muted, _) = glyph(wparam.0);
            let (input, in_muted, in_recording) = glyph(lparam.0 as usize);
            unsafe { crate::restyle(out, out_muted, input, in_muted, in_recording) };
        });
        if caught.is_err() {
            logf!("restyle handler panicked");
        }
        return LRESULT(0);
    }
    if msg == WM_TAP_WIRE_TRANSPORT {
        // Cleared before the work, so buttons announced meanwhile get another pass.
        WIRE_PENDING.store(false, Ordering::SeqCst);
        let caught = std::panic::catch_unwind(|| unsafe { crate::wire_transport() });
        if caught.is_err() {
            logf!("transport wiring panicked");
        }
        return LRESULT(0);
    }
    if msg == WM_COPYDATA {
        let caught = std::panic::catch_unwind(|| {
            let copy = unsafe { (lparam.0 as *const COPYDATASTRUCT).as_ref() }?;
            if copy.dwData != HANDOVER_MAGIC || copy.lpData.is_null() {
                return None;
            }
            let units = unsafe { core::slice::from_raw_parts(copy.lpData as *const u16, copy.cbData as usize / 2) };
            let data = String::from_utf16_lossy(units);
            Some(crate::hand_over(data.trim_end_matches('\0')))
        });
        match caught {
            Ok(Some(code)) => return LRESULT(code),
            Ok(None) => {}
            Err(_) => {
                logf!("handover handler panicked");
                return LRESULT(0);
            }
        }
    }
    if msg == WM_TAP_REPIN || (msg == WM_TIMER && wparam.0 == REPIN_TIMER) {
        if msg == WM_TAP_REPIN {
            REPIN_PENDING.store(false, Ordering::SeqCst);
            REPIN_TRIES.store(0, Ordering::SeqCst);
        }
        let done = std::panic::catch_unwind(|| unsafe { crate::repin() }).unwrap_or_else(|_| {
            logf!("re-pin panicked");
            true
        });
        if done || REPIN_TRIES.fetch_add(1, Ordering::SeqCst) >= REPIN_MAX_TRIES {
            let _ = unsafe { KillTimer(Some(hwnd), REPIN_TIMER) };
        } else {
            unsafe { SetTimer(Some(hwnd), REPIN_TIMER, REPIN_RETRY_MS, None) };
        }
        return LRESULT(0);
    }
    if msg == WM_TIMER && wparam.0 == SWEEP_TIMER {
        let caught = std::panic::catch_unwind(|| unsafe { crate::timed_sweep() });
        if caught.is_err() {
            logf!("sweep panicked");
        }
        return LRESULT(0);
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Takes a deferred revert, returning the pid that asked for it.
pub fn take_pending_revert() -> Option<u32> {
    PENDING_REVERT
        .swap(false, Ordering::SeqCst)
        .then(|| PENDING_REVERT_PID.load(Ordering::SeqCst))
}

/// The pid that asked for the deferred revert (0 = unconditional).
static PENDING_REVERT_PID: AtomicU32 = AtomicU32::new(0);

/// Whether a revert asked for by `pid` still applies: 0 is unconditional (`--taskbar-revert`, an
/// older audio-tray), anything else only while that process still owns the strip — a revert from
/// an owner that has since handed over must not dismantle its successor's strip.
pub fn revert_is_current(pid: u32) -> bool {
    pid == 0 || pid == OWNER_PID.load(Ordering::SeqCst)
}

/// Whether a wiring request is already queued, so the three buttons of one rebuild cost one message.
static WIRE_PENDING: AtomicBool = AtomicBool::new(false);

/// Ask for the hover preview's transport buttons to be wired as soon as the pump is free (until
/// wired, a press goes to the player's window and does nothing). Safe from any thread, including
/// inside the visual-tree callback: it only posts.
pub fn nudge_transport() {
    let hwnd = WINDOW.load(Ordering::SeqCst);
    if hwnd == 0 {
        return;
    }
    if WIRE_PENDING.swap(true, Ordering::SeqCst) {
        return;
    }
    let posted = unsafe {
        PostMessageW(
            Some(HWND(hwnd as *mut core::ffi::c_void)),
            WM_TAP_WIRE_TRANSPORT,
            WPARAM(0),
            LPARAM(0),
        )
    };
    if posted.is_err() {
        // Nothing else clears the flag; the sweep is the fallback.
        WIRE_PENDING.store(false, Ordering::SeqCst);
    }
}

/// Whether a re-pin is already queued, so a burst of indicator rebuilds costs one message.
static REPIN_PENDING: AtomicBool = AtomicBool::new(false);

/// Ask for the music tile's indicators to be re-pinned as soon as the stream allows. Safe from inside
/// the visual-tree callback and from any thread: it only posts.
pub fn nudge_repin() {
    let hwnd = WINDOW.load(Ordering::SeqCst);
    if hwnd == 0 || REPIN_PENDING.swap(true, Ordering::SeqCst) {
        return;
    }
    let posted = unsafe { PostMessageW(Some(HWND(hwnd as *mut core::ffi::c_void)), WM_TAP_REPIN, WPARAM(0), LPARAM(0)) };
    if posted.is_err() {
        REPIN_PENDING.store(false, Ordering::SeqCst);
    }
}

/// Creates the control window, once, on the calling thread. Call only on the tray thread.
pub fn ensure_window() {
    if WINDOW.load(Ordering::SeqCst) != 0 {
        return;
    }
    let class = crate::wide(CONTROL_CLASS);
    let hwnd = unsafe {
        let Ok(instance) = GetModuleHandleW(None) else {
            logf!("control window: GetModuleHandle failed");
            return;
        };
        // Registering twice is harmless (the second call fails).
        let descriptor = WNDCLASSW {
            lpfnWndProc: Some(control_proc),
            hInstance: instance.into(),
            lpszClassName: windows_core::PCWSTR(class.as_ptr()),
            ..Default::default()
        };
        RegisterClassW(&descriptor);

        // Never shown. Top-level, not message-only: the app finds it with `EnumWindows`.
        CreateWindowExW(
            WS_EX_TOOLWINDOW,
            windows_core::PCWSTR(class.as_ptr()),
            windows_core::PCWSTR(class.as_ptr()),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None::<HMENU>,
            Some(instance.into()),
            None,
        )
    };
    match hwnd {
        Ok(hwnd) if !hwnd.0.is_null() => {
            WINDOW.store(hwnd.0 as isize, Ordering::SeqCst);
            // The sweep timer belongs to this window, so it fires on the tray thread.
            SWEEP_INTERVAL.store(SWEEP_FAST_MS, Ordering::SeqCst);
            unsafe { SetTimer(Some(hwnd), SWEEP_TIMER, SWEEP_FAST_MS, None) };
            logf!(
                "control window 0x{:x} created on thread {}, sweeping every {SWEEP_FAST_MS}ms",
                hwnd.0 as usize,
                crate::tid()
            );
        }
        Ok(_) => logf!("control window: CreateWindowEx returned null"),
        Err(err) => logf!("control window: CreateWindowEx failed ({err})"),
    }
}

/// Sets the sweep to the idle or fast pace. Re-arms only when the interval changes: re-arming
/// resets the countdown, and doing it every tick would postpone the sweep indefinitely.
///
/// # Safety
/// Must run on the thread that owns the control window.
pub unsafe fn set_sweep_pace(settled: bool) {
    let wanted = if settled { SWEEP_IDLE_MS } else { SWEEP_FAST_MS };
    if SWEEP_INTERVAL.swap(wanted, Ordering::SeqCst) == wanted {
        return;
    }
    let hwnd = WINDOW.load(Ordering::SeqCst);
    if hwnd == 0 {
        return;
    }
    unsafe {
        SetTimer(
            Some(HWND(hwnd as *mut core::ffi::c_void)),
            SWEEP_TIMER,
            wanted,
            None,
        )
    };
    logf!("sweeping every {wanted}ms");
}

/// Watches the process that asked for the strip and requests a revert when it exits, however it
/// exits (a killed owner posts nothing). The revert is posted to the control window, not run here.
pub fn watch_owner(pid: Option<String>) {
    let Some(pid) = pid.and_then(|value| value.parse::<u32>().ok()) else {
        logf!("no owner pid in the init data — the strip will outlive its app");
        return;
    };
    OWNER_PID.store(pid, Ordering::SeqCst);
    // A handover can name the same owner twice (restart_app transfers to its child, which then
    // hands over itself); one watcher per pid is enough.
    {
        let mut watched = crate::lock(&WATCHED);
        if watched.contains(&pid) {
            return;
        }
        watched.push(pid);
    }
    std::thread::spawn(move || {
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) };
        let Ok(handle) = handle else {
            logf!("cannot watch owner pid {pid}: {:?}", handle.err());
            return;
        };
        logf!("watching owner pid {pid}");
        let waited = unsafe { WaitForSingleObject(handle, INFINITE) };
        let _ = unsafe { CloseHandle(handle) };
        crate::lock(&WATCHED).retain(|&watched| watched != pid);
        // Our owner handed over; reverting would dismantle the new owner's strip.
        let current = OWNER_PID.load(Ordering::SeqCst);
        if current != pid {
            logf!("owner pid {pid} exited, but pid {current} owns the strip now — no revert");
            return;
        }
        logf!("owner pid {pid} exited (wait -> {}) — asking for a revert", waited.0);
        request_revert(pid);
    });
}

/// Posts (never sends: do not block on the shell's UI thread) a revert to the control window.
pub fn request_revert(pid: u32) {
    let hwnd = WINDOW.load(Ordering::SeqCst);
    if hwnd == 0 {
        // Nowhere to post it yet: left for the next tray callback.
        PENDING_REVERT_PID.store(pid, Ordering::SeqCst);
        PENDING_REVERT.store(true, Ordering::SeqCst);
        logf!("revert requested before the control window existed — deferred");
        return;
    }
    let posted = unsafe {
        PostMessageW(
            Some(HWND(hwnd as *mut core::ffi::c_void)),
            WM_TAP_REVERT,
            WPARAM(pid as usize),
            LPARAM(0),
        )
    };
    if let Err(err) = posted {
        logf!("posting the revert failed: {err}");
    }
}

/// Owner pids with a watcher thread running.
static WATCHED: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());

pub use tap_proto::HANDOVER_MAGIC;

/// Full path of this DLL, to tell our own copy from another build's.
pub fn own_module_path() -> Option<String> {
    use windows::Win32::System::LibraryLoader::{
        GetModuleFileNameW, GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
        GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
    };
    let mut module = windows::Win32::Foundation::HMODULE::default();
    let anchor = own_module_path as *const () as *const u16;
    unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            windows_core::PCWSTR(anchor),
            &mut module,
        )
        .ok()?;
        let mut buf = [0u16; 1024];
        let len = GetModuleFileNameW(Some(module), &mut buf) as usize;
        (len > 0).then(|| String::from_utf16_lossy(&buf[..len]))
    }
}
