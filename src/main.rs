// GUI subsystem: no console window flashes when the tray is launched. The dev CLI modes
// re-attach to the launching console at runtime (see `main`) so their output still prints.
#![windows_subsystem = "windows"]

//! Windows audio output tray app.
//!
//! Mode dispatch (plan §6): default = tray. Left-click opens the acrylic control panel
//! (volume, mute, output/input switching, per-device icons); right-click opens the quick
//! menu (Sound settings + Quit).
//! Dev utilities retained from the early slices:
//!   audio-tray            run the tray (default)
//!   audio-tray --list     print current default + active output devices
//!   audio-tray --set <q>  switch default output to the device whose friendly name
//!                         contains <q> (case-insensitive), or whose id equals <q>
//!   audio-tray --flyout [menu]  preview the control panel (or the right-click menu)
//!   audio-tray --update   check GitHub releases and self-update now (see update.rs)

use anyhow::{bail, Context, Result};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};

mod audio;
mod config;
mod flyout;
mod icons;
mod tray;
mod update;

use audio::wasapi::WasapiBackend;
use audio::{AudioBackend, Device, DeviceId, Flow};
use config::Config;
use icons::IconId;

fn main() -> Result<()> {
    // STA: conventional for the GUI/tray thread that owns the message pump.
    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()? };
    // Per-monitor DPI aware for every windowed path (tray icon and the flyout, incl. the
    // `--flyout` dev preview) so glyphs and the acrylic panel render crisp at the real size
    // instead of being bitmap-scaled by the OS.
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let backend = WasapiBackend::new()?;

    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.is_empty() {
        // GUI-subsystem binaries don't inherit the parent console; re-attach so the dev
        // CLI (--list/--set/--set-icon/--vol) can print to the launching terminal.
        unsafe {
            let _ = AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
    match args.first().map(String::as_str) {
        Some("--flyout") => {
            // Dev: show the flyout once. `--flyout menu` previews the right-click menu.
            let trigger = match args.get(1).map(String::as_str) {
                Some("menu") => flyout::Trigger::RightClick,
                _ => flyout::Trigger::LeftClick,
            };
            let mut config = Config::load();
            let outcome = flyout::show(&backend, &mut config, None, trigger);
            if outcome.config_changed {
                config.save()?;
            }
            println!(
                "flyout: closed (config_changed={}, quit={})",
                outcome.config_changed, outcome.quit
            );
        }
        Some("--list") => list(&backend)?,
        Some("--set") => {
            let Some(query) = args.get(1) else {
                bail!("usage: audio-tray --set <name-substring-or-id>");
            };
            let devices = backend.enumerate()?;
            let target = find_device(&devices, query)?;
            println!("Switching default to: {} [{}]", target.friendly_name, target.id.0);
            backend.set_default(&target.id)?;
            println!("Done. Verify in Windows sound settings.");
        }
        Some("--set-icon") => {
            let (Some(query), Some(icon_str)) = (args.get(1), args.get(2)) else {
                bail!("usage: audio-tray --set-icon <name-substring-or-id> <IconId>");
            };
            let icon = IconId::parse(icon_str)
                .with_context(|| format!("unknown icon {icon_str:?}; one of {:?}", IconId::ALL))?;
            let devices = backend.enumerate()?;
            let target = find_device(&devices, query)?;
            let mut cfg = Config::load();
            cfg.set_icon(target.id.0.clone(), icon);
            cfg.save()?;
            println!(
                "saved: {} -> {icon:?}\n  at {}",
                target.friendly_name,
                Config::path()?.display()
            );
        }
        Some("--vol") => {
            let before = backend.master_volume()?;
            match args.get(1).map(String::as_str) {
                Some("up") => backend.step_volume(true)?,
                Some("down") => backend.step_volume(false)?,
                Some("get") | None => {}
                Some(other) => bail!("usage: audio-tray --vol <up|down|get> (got {other:?})"),
            }
            let after = backend.master_volume()?;
            println!("volume: {:.0}% -> {:.0}%", before * 100.0, after * 100.0);
        }
        Some("--update") => update::run_manual()?,
        _ => {
            // Fire-and-forget auto-update: checks GitHub releases in the background
            // and self-replaces the on-disk exe (applied on next launch). No-op in
            // debug builds. See src/update.rs.
            update::spawn_background_check();
            tray::run(backend)?;
        }
    }
    Ok(())
}

fn list(backend: &WasapiBackend) -> Result<()> {
    let devices = backend.enumerate()?;
    let name_of = |id: &DeviceId| {
        devices
            .iter()
            .find(|d| &d.id == id)
            .map(|d| d.friendly_name.clone())
            .unwrap_or_else(|| id.0.clone())
    };

    println!("Default output by role:");
    for (role, result) in backend.defaults_by_role() {
        match result {
            Ok(Some(id)) => println!("  {role:<16} {}", name_of(&id)),
            Ok(None) => println!("  {role:<16} (none)"),
            Err(e) => println!("  {role:<16} <error: {e:#}>"),
        }
    }

    for (flow, title) in [(Flow::Output, "output"), (Flow::Input, "input")] {
        let default = backend.default_of(flow).ok().flatten();
        println!("\nActive {title} devices:");
        for d in backend.enumerate_flow(flow)?.iter() {
            let marker = if Some(&d.id) == default.as_ref() { "*" } else { " " };
            let level = match backend.volume_of(&d.id) {
                Ok(v) => format!("{:>3.0}%", v * 100.0),
                Err(_) => "  ? ".to_string(),
            };
            let mute = if backend.is_muted(&d.id).unwrap_or(false) { " muted" } else { "" };
            println!("  {marker} [{:?}] {level}{mute}  {}", d.form_factor, d.friendly_name);
            println!("      id: {}", d.id.0);
        }
    }
    Ok(())
}

/// Resolve a device by exact endpoint id, else by case-insensitive friendly-name
/// substring. Errors if nothing matches or the substring is ambiguous.
fn find_device<'a>(devices: &'a [Device], query: &str) -> Result<&'a Device> {
    if let Some(d) = devices.iter().find(|d| d.id == DeviceId(query.to_string())) {
        return Ok(d);
    }
    let q = query.to_lowercase();
    let matches: Vec<&Device> = devices
        .iter()
        .filter(|d| d.friendly_name.to_lowercase().contains(&q))
        .collect();
    match matches.as_slice() {
        [one] => Ok(one),
        [] => bail!("no output device matches {query:?}"),
        many => {
            let names: Vec<&str> = many.iter().map(|d| d.friendly_name.as_str()).collect();
            bail!("{query:?} is ambiguous, matches: {names:?}")
        }
    }
}
