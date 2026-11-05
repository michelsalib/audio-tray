//! `--keyboards [secs] [switch <device-substring>=<hkl> …]`: the measurement spike of
//! `docs/KEYBOARDS.md` step 2. Lists the keyboards and the installed layouts, then prints, live,
//! each key that comes from a *different* keyboard than the last: time, device, the foreground
//! process and its layout. Device and time only, never the key.
//!
//! With `switch` rules it also switches (only when the keyboard changes, decision 4) and prints
//! the layout read back ~50 ms later.

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, DispatchMessageW, GetMessageW, KillTimer, PostQuitMessage, SetTimer, TranslateMessage, MSG,
    WINDOW_EX_STYLE, WM_INPUT, WM_INPUT_DEVICE_CHANGE, WM_TIMER,
};

use super::{describe, device_names, device_path, foreground, keyboards, layouts, register, source, switch};

/// Prints a line, and appends it to [`transcript_path`]: a GUI-subsystem exe run without a console
/// (in the background) has nowhere to print.
macro_rules! say {
    ($($arg:tt)*) => {{
        let line = format!($($arg)*);
        println!("{line}");
        transcript(&line);
    }};
}

fn transcript_path() -> std::path::PathBuf {
    std::env::temp_dir().join("audio-tray-keyboards.txt")
}

fn transcript(line: &str) {
    use std::io::Write;
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(transcript_path()) {
        let _ = writeln!(file, "{line}");
    }
}

const TIMER_END: usize = 1;
const TIMER_READBACK: usize = 2;
/// When the layout is read back after a switch, in ms from the request, until it has switched.
const READBACKS: [u32; 3] = [50, 300, 1000];
/// `WM_INPUT_DEVICE_CHANGE`'s `wParam`.
const GIDC_ARRIVAL: usize = 1;

struct Rule {
    needle: String,
    hkl: u64,
}

struct Spike {
    start: Instant,
    rules: Vec<Rule>,
    paths: HashMap<usize, String>,
    last: Option<usize>,
    /// What was asked for, for the read-back.
    pending: Option<(Instant, u64, usize)>,
}

thread_local! {
    static SPIKE: RefCell<Option<Spike>> = const { RefCell::new(None) };
}

pub fn run(args: &[String]) -> Result<()> {
    let mut secs = 120u32;
    let mut rules = Vec::new();
    let mut switching = false;
    for arg in args {
        if let Ok(n) = arg.parse() {
            secs = n;
        } else if arg == "switch" {
            switching = true;
        } else if let Some((needle, hkl)) = arg.split_once('=').filter(|_| switching) {
            let digits = hkl.trim_start_matches("0x").trim_start_matches("0X");
            let hkl = u64::from_str_radix(digits, 16).with_context(|| format!("{hkl:?} is not a hex HKL"))?;
            rules.push(Rule { needle: needle.to_lowercase(), hkl });
        } else {
            bail!("usage: audio-tray --keyboards [secs] [switch <device-substring>=<hkl> …] (got {arg:?})");
        }
    }

    let _ = std::fs::remove_file(transcript_path());
    say!("keyboards (Raw Input):");
    for keyboard in keyboards() {
        let names = device_names(&keyboard.path, 4);
        say!("  {:#x}  {}", keyboard.handle, keyboard.path);
        if !names.is_empty() {
            say!("      {}", names.join("  <  "));
        }
    }
    say!("layouts (GetKeyboardLayoutList):");
    let installed = layouts();
    for layout in &installed {
        say!("  {:#010x}  klid {:<9} {}", layout.hkl, layout.klid.as_deref().unwrap_or("?"), layout);
    }
    for rule in &rules {
        if !installed.iter().any(|layout| layout.hkl == rule.hkl) {
            say!("warning: {:#010x} is not an installed layout", rule.hkl);
        }
        say!("rule: a keyboard whose path contains {:?} switches to {}", rule.needle, describe(rule.hkl));
    }

    let hwnd = crate::win::create_popup(
        w!("AudioTrayKeyboardSpike"),
        w!("audio-tray keyboards spike"),
        Some(window_proc),
        WINDOW_EX_STYLE(0),
        (0, 0, 0, 0),
    )
    .context("create the spike window")?;
    SPIKE.with(|spike| {
        *spike.borrow_mut() = Some(Spike { start: Instant::now(), rules, paths: HashMap::new(), last: None, pending: None })
    });
    register(hwnd, true).context("RegisterRawInputDevices")?;
    unsafe { SetTimer(Some(hwnd), TIMER_END, secs * 1000, None) };
    say!("listening for {secs} s — type on each keyboard (only the device is read, never the key)");

    let mut msg = MSG::default();
    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    let _ = register(hwnd, false);
    say!("done");
    Ok(())
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_INPUT => {
            if let Some(device) = source(lparam) {
                SPIKE.with(|spike| {
                    if let Some(spike) = spike.borrow_mut().as_mut() {
                        on_key(spike, hwnd, device);
                    }
                });
            }
        }
        WM_INPUT_DEVICE_CHANGE => {
            let device = lparam.0 as usize;
            let what = if wparam.0 == GIDC_ARRIVAL { "arrived" } else { "removed" };
            SPIKE.with(|spike| {
                if let Some(spike) = spike.borrow_mut().as_mut() {
                    let path = path_of(spike, device);
                    say!("{:>8.3}s  device {device:#x} {what}: {path}", spike.start.elapsed().as_secs_f32());
                }
            });
        }
        WM_TIMER if wparam.0 == TIMER_END => unsafe { PostQuitMessage(0) },
        WM_TIMER if wparam.0 == TIMER_READBACK => {
            unsafe {
                let _ = KillTimer(Some(hwnd), TIMER_READBACK);
            }
            SPIKE.with(|spike| {
                if let Some(spike) = spike.borrow_mut().as_mut() {
                    let Some((asked, wanted, step)) = spike.pending.take() else {
                        return;
                    };
                    let Some(now) = foreground() else {
                        return;
                    };
                    let switched = now.layout == wanted;
                    say!(
                        "{:>8.3}s    read back after {} ms: {:#010x} {}",
                        spike.start.elapsed().as_secs_f32(),
                        asked.elapsed().as_millis(),
                        now.layout,
                        if switched { "(switched)" } else { "(NOT switched)" }
                    );
                    // Not yet: look again later, to tell a slow switch from a refused one.
                    if let (false, Some(&next)) = (switched, READBACKS.get(step + 1)) {
                        spike.pending = Some((asked, wanted, step + 1));
                        let wait = next.saturating_sub(asked.elapsed().as_millis() as u32).max(10);
                        unsafe { SetTimer(Some(hwnd), TIMER_READBACK, wait, None) };
                    }
                }
            });
        }
        _ => {}
    }
    // `WM_INPUT` must reach DefWindowProc, which frees the input.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn path_of(spike: &mut Spike, device: usize) -> String {
    if device == 0 {
        return "(no device: injected input)".to_owned();
    }
    spike
        .paths
        .entry(device)
        .or_insert_with(|| device_path(device).unwrap_or_else(|| "(no path)".to_owned()))
        .clone()
}

fn on_key(spike: &mut Spike, hwnd: HWND, device: usize) {
    if spike.last == Some(device) {
        return;
    }
    spike.last = Some(device);
    let at = spike.start.elapsed().as_secs_f32();
    let path = path_of(spike, device);
    let Some(fg) = foreground() else {
        say!("{at:>8.3}s  key from {device:#x} {path}  (no foreground window)");
        return;
    };
    let exe = crate::win::window_exe(fg.window).unwrap_or_default();
    let exe = exe.rsplit('\\').next().unwrap_or_default().to_owned();
    // The process the keys really go to, when it is not the foreground window's (WebView2).
    let focus_exe = crate::win::window_exe(fg.target).unwrap_or_default();
    let focus_exe = focus_exe.rsplit('\\').next().unwrap_or_default();
    let exe = if focus_exe.is_empty() || focus_exe == exe { exe } else { format!("{exe} > focus in {focus_exe}") };
    say!("{at:>8.3}s  key from {device:#x} {path}");
    say!("            foreground {exe} (pid {}), layout {:#010x} {}", fg.pid, fg.layout, describe(fg.layout));
    let lower = path.to_lowercase();
    let Some(rule) = spike.rules.iter().find(|rule| lower.contains(&rule.needle)) else {
        return;
    };
    if fg.layout == rule.hkl {
        say!("            already {:#010x}", rule.hkl);
        return;
    }
    let posted = switch(&fg, rule.hkl);
    say!("            switch to {:#010x}: {}", rule.hkl, if posted { "posted" } else { "post FAILED" });
    spike.pending = Some((Instant::now(), rule.hkl, 0));
    unsafe { SetTimer(Some(hwnd), TIMER_READBACK, READBACKS[0], None) };
}
