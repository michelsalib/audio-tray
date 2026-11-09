//! The runtime switcher: on a thread of its own with a hidden window that receives Raw Input,
//! because the tray thread does slow COM work and a switch that lands after the user's second key
//! defeats the purpose. It is the process's only keyboard Raw Input registration (one per usage
//! per process: a second would replace it).
//!
//! On a key from a different keyboard than the last (decision 4) that has a layout assigned
//! (decision 8): if the foreground layout differs, switch it and confirm on the pill. Only the
//! device is read, never the key.
//!
//! It also announces a layout changed by anything else (Win+Space, Alt+Shift, the language bar):
//! Windows tells no other process, so the focus window's layout is polled ([`WATCH_EVERY`]).

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Mutex;

use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, PostThreadMessageW, SetTimer, MSG,
    WINDOW_EX_STYLE, WM_INPUT, WM_INPUT_DEVICE_CHANGE, WM_QUIT, WM_TIMER,
};

use crate::config::Keyboards;

/// `WM_INPUT_DEVICE_CHANGE`'s `wParam` for an arrival (else a removal).
const GIDC_ARRIVAL: usize = 1;

/// The layout watch's timer, and its period: a banner within a glance of the shortcut.
const TIMER_WATCH: usize = 1;
const WATCH_EVERY: u32 = 150;

/// How long the focus must stay on one thread before its layout counts. A thread that gets the
/// focus may still hold its old layout until Windows syncs it, which is not a change to announce;
/// and Win+Space's own picker takes the focus while it is open.
const FOCUS_SETTLE: std::time::Duration = std::time::Duration::from_millis(400);

/// The switcher thread; dropping it unregisters and stops the thread.
pub struct Switcher {
    thread: u32,
    join: Option<std::thread::JoinHandle<()>>,
}

/// What the switcher acts on, shared with the tray (which replaces it when settings change).
struct Rules {
    show_readout: bool,
    /// `(device path, name, layout)`: one per path, so a keyboard with several connections has several.
    assigned: Vec<(String, String, u64)>,
}

static RULES: Mutex<Rules> = Mutex::new(Rules { show_readout: true, assigned: Vec::new() });

/// The switcher thread's own state.
#[derive(Default)]
struct Seen {
    /// `hDevice → interface path`, for this session (handles change across reconnects).
    paths: HashMap<usize, String>,
    /// The keyboard that typed last; a switch happens only when this changes.
    last: Option<usize>,
    /// The layout last announced or switched to: a change from it is announced.
    shown: Option<u64>,
    /// The focus window's thread, and since when it has had the focus.
    focus: Option<(u32, std::time::Instant)>,
}

thread_local! {
    static SEEN: RefCell<Seen> = RefCell::new(Seen::default());
}

impl Switcher {
    /// Start listening with `config`. `None` if Raw Input could not be registered.
    pub fn spawn(config: &Keyboards) -> Option<Self> {
        configure(config);
        let (ready, started) = std::sync::mpsc::channel();
        let join = std::thread::Builder::new()
            .name("keyboards".into())
            .spawn(move || {
                let window = crate::win::create_popup(
                    w!("AudioTrayKeyboards"),
                    w!("Audio Tray keyboards"),
                    Some(window_proc),
                    WINDOW_EX_STYLE(0),
                    (0, 0, 0, 0),
                );
                let window = match window.and_then(|window| super::register(window, true).map(|()| window)) {
                    Ok(window) => window,
                    Err(e) => {
                        eprintln!("keyboards: cannot listen for keyboards ({e:#})");
                        let _ = ready.send(None);
                        return;
                    }
                };
                unsafe { SetTimer(Some(window), TIMER_WATCH, WATCH_EVERY, None) };
                let _ = ready.send(Some(unsafe { GetCurrentThreadId() }));
                let mut msg = MSG::default();
                while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {
                    unsafe { DispatchMessageW(&msg) };
                }
                let _ = super::register(window, false);
                let _ = unsafe { DestroyWindow(window) };
            })
            .ok()?;
        match started.recv().ok().flatten() {
            Some(thread) => Some(Self { thread, join: Some(join) }),
            None => {
                let _ = join.join();
                None
            }
        }
    }

    /// Take new settings without restarting the thread.
    pub fn configure(&self, config: &Keyboards) {
        configure(config);
    }
}

impl Drop for Switcher {
    fn drop(&mut self) {
        let _ = unsafe { PostThreadMessageW(self.thread, WM_QUIT, WPARAM(0), LPARAM(0)) };
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn configure(config: &Keyboards) {
    let assigned = config
        .assigned
        .iter()
        .filter_map(|keyboard| Some((keyboard, keyboard.hkl()?)))
        .flat_map(|(keyboard, hkl)| keyboard.paths().map(move |path| (path.to_owned(), keyboard.name.clone(), hkl)))
        .collect();
    *RULES.lock().unwrap_or_else(|p| p.into_inner()) = Rules { show_readout: config.show_readout, assigned };
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_INPUT => {
            if let Some(device) = super::source(lparam) {
                on_key(device);
            }
        }
        WM_INPUT_DEVICE_CHANGE => {
            let device = lparam.0 as usize;
            SEEN.with(|seen| {
                let mut seen = seen.borrow_mut();
                if wparam.0 == GIDC_ARRIVAL {
                    if let Some(path) = super::device_path(device) {
                        seen.paths.insert(device, path);
                    }
                } else {
                    // A handle can be reused by the next device that arrives.
                    seen.paths.remove(&device);
                    if seen.last == Some(device) {
                        seen.last = None;
                    }
                }
            });
        }
        WM_TIMER if wparam.0 == TIMER_WATCH => watch(),
        _ => {}
    }
    // `WM_INPUT` must reach DefWindowProc, which frees the input.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Settings asked for the next keyboard typed on: where to say so, `(window, message)`.
static CAPTURE: Mutex<Option<(isize, u32)>> = Mutex::new(None);
static CAPTURED: Mutex<Option<Captured>> = Mutex::new(None);

/// A keyboard typed on during a capture.
#[derive(Clone, Debug)]
pub struct Captured {
    pub path: String,
    /// Its names, nearest first (see [`super::device_names`]).
    pub names: Vec<String>,
    /// The name it already has, when it is already assigned (the capture stays armed).
    pub assigned: Option<String>,
}

/// Take the next keyboard typed on, and post `message` to `window` when one is ([`take_captured`]).
/// Only the device is read, as always. Needs the switcher running.
pub fn capture(window: HWND, message: u32) {
    *CAPTURE.lock().unwrap_or_else(|p| p.into_inner()) = Some((window.0 as isize, message));
    *CAPTURED.lock().unwrap_or_else(|p| p.into_inner()) = None;
}

pub fn cancel_capture() {
    *CAPTURE.lock().unwrap_or_else(|p| p.into_inner()) = None;
}

pub fn take_captured() -> Option<Captured> {
    CAPTURED.lock().unwrap_or_else(|p| p.into_inner()).take()
}

/// During a capture: report `device` to settings. An unassigned keyboard ends the capture.
fn report_capture(device: usize) {
    let Some((window, message)) = *CAPTURE.lock().unwrap_or_else(|p| p.into_inner()) else {
        return;
    };
    let Some(path) = SEEN.with(|seen| seen.borrow().paths.get(&device).cloned()).or_else(|| super::device_path(device)) else {
        return;
    };
    let assigned = {
        let rules = RULES.lock().unwrap_or_else(|p| p.into_inner());
        rules.assigned.iter().find(|(known, _, _)| known.eq_ignore_ascii_case(&path)).map(|(_, name, _)| name.clone())
    };
    // The same keyboard again, already reported as taken: say nothing new.
    let mut captured = CAPTURED.lock().unwrap_or_else(|p| p.into_inner());
    if assigned.is_some() && captured.as_ref().is_some_and(|c| c.path == path) {
        return;
    }
    if assigned.is_none() {
        *CAPTURE.lock().unwrap_or_else(|p| p.into_inner()) = None;
    }
    let names = super::device_names(&path, 4);
    *captured = Some(Captured { path, names, assigned });
    drop(captured);
    let _ = unsafe {
        windows::Win32::UI::WindowsAndMessaging::PostMessageW(Some(HWND(window as *mut _)), message, WPARAM(0), LPARAM(0))
    };
}

/// A key from `device`: switch if it is another keyboard than the last, and has a layout.
fn on_key(device: usize) {
    // No device: injected input (`SendInput`), which no keyboard typed.
    if device == 0 {
        return;
    }
    report_capture(device);
    let path = SEEN.with(|seen| {
        let mut seen = seen.borrow_mut();
        if seen.last == Some(device) {
            return None;
        }
        seen.last = Some(device);
        if let Some(path) = seen.paths.get(&device) {
            return Some(path.clone());
        }
        let path = super::device_path(device)?;
        seen.paths.insert(device, path.clone());
        Some(path)
    });
    let Some(path) = path else {
        return;
    };
    let (wanted, show_readout) = {
        let rules = RULES.lock().unwrap_or_else(|p| p.into_inner());
        let wanted = rules.assigned.iter().find(|(known, _, _)| known.eq_ignore_ascii_case(&path)).map(|(_, name, hkl)| (name.clone(), *hkl));
        (wanted, rules.show_readout)
    };
    // A keyboard with no layout switches nothing (decision 8).
    let Some((name, hkl)) = wanted else {
        return;
    };
    let installed = super::layouts();
    // A layout uninstalled since it was assigned: leave the current one.
    let Some(layout) = installed.iter().find(|layout| layout.hkl == hkl) else {
        return;
    };
    let Some(foreground) = super::foreground() else {
        return;
    };
    if foreground.layout == hkl {
        return;
    }
    if !super::switch(&foreground, hkl) {
        return;
    }
    // Announced here; the watch must not announce it again when it lands.
    SEEN.with(|seen| seen.borrow_mut().shown = Some(hkl));
    if show_readout {
        crate::hub::emit(crate::hub::Event::KeyboardSwitched { keyboard: name, layout: super::label(layout, &installed) });
    }
}

/// Every [`WATCH_EVERY`]: announce a layout the user changed by other means (Win+Space,
/// Alt+Shift), named after the keyboard that typed last when it has a name.
fn watch() {
    let Some(foreground) = super::foreground() else {
        return;
    };
    let now = std::time::Instant::now();
    let (changed, last) = SEEN.with(|seen| {
        let mut seen = seen.borrow_mut();
        match seen.focus {
            Some((thread, _)) if thread == foreground.thread => {}
            _ => seen.focus = Some((foreground.thread, now)),
        }
        let settled = seen.focus.is_some_and(|(_, since)| now.duration_since(since) >= FOCUS_SETTLE);
        if !settled || seen.shown == Some(foreground.layout) {
            return (false, None);
        }
        // The first reading is where things stand, not a change.
        let changed = seen.shown.is_some();
        seen.shown = Some(foreground.layout);
        let last = seen.last.and_then(|device| seen.paths.get(&device).cloned());
        (changed, last)
    });
    if !changed {
        return;
    }
    let keyboard = {
        let rules = RULES.lock().unwrap_or_else(|p| p.into_inner());
        if !rules.show_readout {
            return;
        }
        last.and_then(|path| rules.assigned.iter().find(|(known, _, _)| known.eq_ignore_ascii_case(&path)).map(|(_, name, _)| name.clone()))
    };
    let installed = super::layouts();
    let layout = installed.iter().find(|layout| layout.hkl == foreground.layout).cloned().unwrap_or_else(|| super::describe(foreground.layout));
    crate::hub::emit(crate::hub::Event::KeyboardSwitched { keyboard: keyboard.unwrap_or_default(), layout: super::label(&layout, &installed) });
}
