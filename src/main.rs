// GUI subsystem: no console window flashes when the tray is launched. The CLI modes
// re-attach to the launching console at runtime (see `main`) so their output still prints.
#![windows_subsystem = "windows"]

//! Windows audio tray app: a notification icon decorated by a taskbar strip (output and input
//! buttons), an acrylic control flyout, a scroll readout, and a YouTube Music tile.
//!
//! Modes in the release binary — what users, the installer and the updater run:
//!   audio-tray                     run the tray (also with `--relaunched`, from `restart_app`)
//!   audio-tray --update            check GitHub releases and self-update now (see update.rs)
//!   audio-tray --tap-version       which TAP is on disk next to the exe
//!   audio-tray --taskbar-restart   restart explorer.exe (frees the TAP, placing a staged one)
//!   audio-tray --taskbar-revert    ask an injected TAP to put the taskbar back
//!
//! Developer and diagnostic modes live in `dev.rs`, behind the `dev` cargo feature.

use anyhow::Result;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};

mod audio;
mod canvas;
mod config;
#[cfg(feature = "dev")]
mod dev;
mod flyout;
mod icons;
mod instance;
mod layered;
mod music;
mod osd;
mod taskbar;
mod tray;
mod update;
mod win;

use audio::wasapi::WasapiBackend;

fn main() -> Result<()> {
    // STA: conventional for the GUI/tray thread that owns the message pump.
    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()? };
    // Per-monitor DPI aware, so nothing is bitmap-scaled by the OS.
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let backend = WasapiBackend::new()?;

    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.is_empty() {
        // GUI-subsystem binaries don't inherit the parent console; re-attach so CLI output prints.
        unsafe {
            let _ = AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
    #[cfg(feature = "dev")]
    if dev::run(&args, &backend)? {
        return Ok(());
    }
    match args.first().map(String::as_str) {
        Some("--update") => update::run_manual()?,
        Some("--tap-version") => update::report_tap_version(),
        Some("--taskbar-restart") => {
            // Restarts the shell, as the flyout footer's button does. Leaves the running tray
            // alone — it puts the strip back on `TaskbarCreated`.
            println!("taskbar: restarting Explorer...");
            taskbar::restart_explorer()?;
            println!("taskbar: done.");
        }
        Some("--taskbar-revert") => {
            // Escape hatch: put the taskbar back without touching the running tray.
            taskbar::revert(0);
            println!("taskbar: controls removed.");
        }
        _ => {
            // One tray per session. A second launch exits quietly once the wait runs out.
            let Some(_instance) = instance::acquire(instance::wait_budget(&args)) else {
                eprintln!("audio-tray: already running — exiting");
                return Ok(());
            };
            // Background self-update, applied on next launch. No-op in debug builds.
            update::spawn_background_check();
            tray::run(backend)?;
        }
    }
    Ok(())
}
