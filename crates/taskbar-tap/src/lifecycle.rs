//! Getting a "put it back" request onto the one thread allowed to answer it.
//!
//! Everything the TAP changes about the shell — the presenter's content, the
//! notification area's column, Explorer's own volume slot — can only be touched
//! from the visual-tree callback thread. audio-tray, which knows *when* to revert
//! (it is quitting, or the user asked for the taskbar back), is a different
//! process entirely.
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

/// "The devices changed — redraw with these glyphs."
///
/// The init data is read once in `SetSite`, so it cannot carry a device switch.
/// Both glyphs fit in the message parameters, so nothing has to be shared: `wParam`
/// carries the output codepoint with its muted flag at bit 24, `lParam` the input.
/// Must match `WM_TAP_RESTYLE` on the audio-tray side.
pub const WM_TAP_RESTYLE: u32 = WM_APP + 23;

/// Timer id for the periodic check that the strip is still there.
const SWEEP_TIMER: usize = 1;

/// How often that check runs while there is still work to do.
///
/// The sweep is the *only* thing that mutates — the visual-tree callback no longer
/// does, because mutating from inside the event stream wedges the shell — so this
/// interval also decides how quickly the strip appears, and how quickly a redraw
/// lands when the sweep `restyle` asks for declines because the tree is mid-burst.
///
/// 250ms rather than a second because that wait is user-visible: a click's new
/// glyph arriving a second later reads as the switch itself being slow. A tick that
/// has nothing to do is one handle resolve and a runtime-class read, and this pace
/// only runs while there is something outstanding.
const SWEEP_FAST_MS: u32 = 250;

/// How often it runs once everything is applied.
///
/// A settled tick is one handle resolve and a runtime-class read, but it runs on
/// Explorer's UI thread, so it should not run more often than it needs to. Its only
/// remaining job is noticing that the shell has overwritten our strip, and a few
/// seconds is well inside "before the user finishes noticing".
const SWEEP_IDLE_MS: u32 = 4000;

/// The interval currently armed, so the timer is only re-armed when it changes.
static SWEEP_INTERVAL: AtomicU32 = AtomicU32::new(0);

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
    if msg == WM_TAP_RESTYLE {
        let caught = std::panic::catch_unwind(|| {
            // Unpack: codepoint in the low bits, muted flag at bit 24.
            let glyph = |packed: usize| {
                (
                    char::from_u32((packed & 0x00FF_FFFF) as u32),
                    packed & (1 << 24) != 0,
                )
            };
            let (out, out_muted) = glyph(wparam.0);
            let (input, in_muted) = glyph(lparam.0 as usize);
            unsafe { crate::restyle(out, out_muted, input, in_muted) };
        });
        if caught.is_err() {
            logf!("restyle handler panicked");
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

/// Slows the sweep down once there is nothing left to apply, and speeds it back up
/// if there is.
///
/// `SetTimer` with an existing id replaces that timer, so re-arming is how the
/// cadence changes. Only called when the interval actually differs, to avoid
/// resetting the countdown on every tick — which would delay the next sweep
/// indefinitely.
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
