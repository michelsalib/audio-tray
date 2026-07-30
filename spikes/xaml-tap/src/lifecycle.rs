//! Getting a "put it back" request onto the one thread allowed to answer it.
//!
//! Everything the TAP changes about the shell — the presenter's content, the
//! notification area's column, Explorer's own volume slot — can only be touched
//! from the visual-tree callback thread. audio-tray, which knows *when* to revert
//! (the user toggled the feature off, or quit), is a different process entirely.
//!
//! The bridge is a hidden window created **on the callback thread itself**.
//! Explorer already pumps messages there, so a cross-process `PostMessage` is
//! delivered by that pump onto exactly the thread that may touch XAML — no
//! dispatcher, no marshalling, no extra thread of our own. `GetDispatcher` points
//! at a different island and posting through it fails with RPC_E_WRONG_THREAD,
//! so this is the only queue available to us.
//!
//! The window is deliberately *not* created during `SetSite`: that runs on the
//! injector's marshalling thread, and a window created there would deliver its
//! messages to a thread that cannot touch XAML — the exact bug this exists to
//! avoid.

use core::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};

use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{
    OpenProcess, WaitForSingleObject, INFINITE, PROCESS_SYNCHRONIZE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, PostMessageW, RegisterClassW, SetTimer, HMENU, WM_APP,
    WM_TIMER, WNDCLASSW, WS_EX_TOOLWINDOW, WS_POPUP,
};

use crate::log::logf;

/// Class name audio-tray looks for. Must match `TAP_CONTROL_CLASS` on the
/// audio-tray side.
pub const CONTROL_CLASS: &str = "AudioTrayTapControl";

/// "Undo everything and stand down." Sent when the user turns the feature off,
/// and when audio-tray exits.
pub const WM_TAP_REVERT: u32 = WM_APP + 21;

/// Timer id for the periodic check that the strip is still there.
const SWEEP_TIMER: usize = 1;

/// How often that check runs.
///
/// This is now the *only* thing that decorates — the visual-tree callback no
/// longer does, because mutating from inside the event stream wedges the shell —
/// so the interval also decides how quickly the strip appears. One second keeps
/// that imperceptible while staying cheap: in the steady state a tick is one
/// handle resolve and a runtime-class read.
const SWEEP_MS: u32 = 1000;

/// The control window, or 0 before it exists. Also the "already created" flag.
static WINDOW: AtomicIsize = AtomicIsize::new(0);

/// The thread that owns the taskbar's XAML island.
///
/// Explorer runs several islands and calls back on more than one thread. Only one
/// of them owns the tray, and a WinRT call against a tray element from any other
/// simply never returns — measured repeatedly as a decoration that stops dead at
/// "setting content on …" while Explorer carries on repainting.
///
/// Learned rather than assumed, and it has to be learned from an element that
/// exists in **one** island only — see [`adopt_tray_thread`]. Filtering the work
/// by element type is not enough on its own, because a trigger like "a
/// ContentPresenter was added" matches elements in every island.
static TRAY_TID: AtomicU32 = AtomicU32::new(0);

/// Claims the calling thread as the tray's. **Last caller wins — deliberately.**
///
/// First-wins was tried and froze the shell hard. The initial replay is delivered
/// on a marshalling thread rather than the island's UI thread (`HasThreadAccess`
/// reads false there), so the frames announced during it pin the value to a thread
/// that cannot touch the tray — and `put_Content` from there never returns,
/// blocking Explorer's UI thread with it: CPU flat at 0.0s over 70 seconds and the
/// taskbar clock frozen. Last-wins drifts onto the thread delivering the most
/// recent tray event, which is the one that can actually act.
///
/// Called for any `SystemTray.*` element for the same reason: a narrower test
/// (`SystemTray.SystemTrayFrame`, `Taskbar.TaskbarFrame`) only ever matches during
/// the replay, which is exactly the window where the answer is wrong.
pub fn adopt_tray_thread() {
    let me = crate::tid();
    if TRAY_TID.swap(me, Ordering::SeqCst) != me {
        logf!("tray island is thread {me}");
    }
}

/// Whether the caller is on the thread that owns the tray, and may therefore
/// touch it. False until [`adopt_tray_thread`] has run.
pub fn on_tray_thread() -> bool {
    let owner = TRAY_TID.load(Ordering::SeqCst);
    owner != 0 && owner == crate::tid()
}

/// A revert that arrived before there was anywhere to post it.
///
/// The window is only created on the first visual-tree callback, and an owner can
/// die before that — audio-tray failing during startup does exactly this, having
/// already injected. Dropping the request there would leave whatever had been
/// applied in place with nobody left to undo it.
static PENDING_REVERT: AtomicBool = AtomicBool::new(false);

/// Process id of whoever currently owns the strip.
///
/// Read by the watcher threads to decide whether their own owner's death still
/// means anything. It usually does — but audio-tray restarting itself (after a
/// self-update) spawns the new process *before* the old one exits, so the old
/// watcher can wake up to find the strip already re-claimed. Reverting then would
/// tear down the new process's strip and leave it inert.
static OWNER_PID: AtomicU32 = AtomicU32::new(0);

unsafe extern "system" fn control_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_TAP_REVERT {
        // This runs inside Explorer's own message pump; a panic escaping here
        // would take the shell down with it.
        let caught = std::panic::catch_unwind(|| {
            logf!("revert requested — on thread {}", crate::tid());
            unsafe { crate::stand_down() };
        });
        if caught.is_err() {
            logf!("revert handler panicked");
        }
        return LRESULT(0);
    }
    if msg == WM_TIMER && wparam.0 == SWEEP_TIMER {
        let caught = std::panic::catch_unwind(|| unsafe { crate::sweep() });
        if caught.is_err() {
            logf!("sweep panicked");
        }
        return LRESULT(0);
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Whether a revert arrived with nowhere to go and still needs running.
///
/// Consumed by the visual-tree callback, which is on the only thread that may
/// act on it.
pub fn take_pending_revert() -> bool {
    PENDING_REVERT.swap(false, Ordering::SeqCst)
}

/// Creates the control window, once, on the calling thread.
///
/// Call only from the visual-tree callback — the whole point is which thread
/// owns the window.
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
        // Registering twice is harmless — the second call just fails, and the
        // class from the first is still there.
        let descriptor = WNDCLASSW {
            lpfnWndProc: Some(control_proc),
            hInstance: instance.into(),
            lpszClassName: windows_core::PCWSTR(class.as_ptr()),
            ..Default::default()
        };
        RegisterClassW(&descriptor);

        // Never shown, zero-sized, kept out of the taskbar and Alt-Tab. It is a
        // top-level window rather than message-only because message-only windows
        // are not reachable by the cross-process `EnumWindows` scan that finds it.
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
            // The sweep timer belongs to this window, so its `WM_TIMER` lands on
            // this thread — the only one that may touch XAML. Set here rather
            // than from `SetSite`, which runs elsewhere.
            unsafe { SetTimer(Some(hwnd), SWEEP_TIMER, SWEEP_MS, None) };
            logf!(
                "control window 0x{:x} created on thread {}, sweeping every {SWEEP_MS}ms",
                hwnd.0 as usize,
                crate::tid()
            );
        }
        Ok(_) => logf!("control window: CreateWindowEx returned null"),
        Err(err) => logf!("control window: CreateWindowEx failed ({err})"),
    }
}

/// Watches the process that asked for the strip, and reverts when it goes away.
///
/// A clean quit posts [`WM_TAP_REVERT`] itself, but a kill from Task Manager or a
/// crash posts nothing — and the strip left behind would be a dead control, since
/// every click it answers is a message to a process that no longer exists. So the
/// owner's exit is treated as a revert request in its own right.
///
/// One blocking wait on the process handle, no polling: `WaitForSingleObject`
/// returns the moment the process dies, however it dies.
///
/// The revert itself is *not* done on this thread — it posts to the control
/// window, so the work still happens on the XAML thread like every other
/// mutation.
pub fn watch_owner(pid: Option<String>) {
    let Some(pid) = pid.and_then(|value| value.parse::<u32>().ok()) else {
        logf!("no owner pid in the init data — the strip will outlive its app");
        return;
    };
    OWNER_PID.store(pid, Ordering::SeqCst);
    std::thread::spawn(move || {
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) };
        let Ok(handle) = handle else {
            logf!("cannot watch owner pid {pid}: {:?}", handle.err());
            return;
        };
        logf!("watching owner pid {pid}");
        let waited = unsafe { WaitForSingleObject(handle, INFINITE) };
        let _ = unsafe { CloseHandle(handle) };
        // Someone else owns the strip now — our owner handed over rather than
        // going away. Reverting here would dismantle the new owner's strip.
        let current = OWNER_PID.load(Ordering::SeqCst);
        if current != pid {
            logf!("owner pid {pid} exited, but pid {current} owns the strip now — no revert");
            return;
        }
        logf!("owner pid {pid} exited (wait -> {}) — asking for a revert", waited.0);
        request_revert();
    });
}

/// Posts a revert to the control window, from any thread.
///
/// `PostMessage` rather than `SendMessage`: the caller must not block on the
/// shell's UI thread, and has nothing to learn from the answer.
pub fn request_revert() {
    let hwnd = WINDOW.load(Ordering::SeqCst);
    if hwnd == 0 {
        // Nowhere to post it yet. Leave it for the next visual-tree callback,
        // which runs on the right thread anyway.
        PENDING_REVERT.store(true, Ordering::SeqCst);
        logf!("revert requested before the control window existed — deferred");
        return;
    }
    let posted = unsafe {
        PostMessageW(
            Some(HWND(hwnd as *mut core::ffi::c_void)),
            WM_TAP_REVERT,
            WPARAM(0),
            LPARAM(0),
        )
    };
    if let Err(err) = posted {
        logf!("posting the revert failed: {err}");
    }
}
