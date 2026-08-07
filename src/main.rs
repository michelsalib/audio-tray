// GUI subsystem: no console window flashes when the tray is launched. The dev CLI modes
// re-attach to the launching console at runtime (see `main`) so their output still prints.
#![windows_subsystem = "windows"]

//! Windows audio output tray app.
//!
//! Mode dispatch (plan §6): default = tray, which draws the taskbar strip (an output
//! and an input button) over its notification icon. Left-click a button steps that
//! endpoint around its own cycle — each active device in turn, then muted — and does
//! nothing else; opening the acrylic control panel (volume, mute, output/input
//! switching, per-device icons, Sound settings, Quit) is the right click's alone. On
//! the plain tray icon we fall back to — see `taskbar` — either button opens the panel,
//! there being no segment to cycle.
//! Dev utilities retained from the early slices:
//!   audio-tray            run the tray (default)
//!   audio-tray --list     print current default + active output devices
//!   audio-tray --set <q>  switch default output to the device whose friendly name
//!                         contains <q> (case-insensitive), or whose id equals <q>
//!   audio-tray --flyout [menu|icons|update]  preview the panel (menu = right-click;
//!                         icons = the first device's icon-picker screen; update = fake a
//!                         staged update so the footer's restart button shows)
//!   audio-tray --meter    sample the default output+input peak meters for 4s (diagnostic)
//!   audio-tray --mic [secs]
//!                         report which apps hold the microphone open, then watch for
//!                         changes — what the mic icon's red recording dot follows
//!   audio-tray --vol <up|down|get>
//!                         nudge (or read) the default output volume, one scroll notch
//!   audio-tray --osd [out|in] [level%]
//!                         preview the scroll readout — the level bar a scroll puts up
//!                         beside the buttons — beside the cursor, until it fades
//!   audio-tray --update   check GitHub releases and self-update now (see update.rs)
//!   audio-tray --taskbar-click <out|in|panel>
//!                         send a running tray the gesture a strip click would —
//!                         the only way to exercise the cycling, since clicks on the
//!                         taskbar itself cannot be synthesised
//!   audio-tray --taskbar-scroll <out|in> [notches]
//!                         send a running tray the scroll a strip button would, which is
//!                         the only way to drive the touchpad half of that gesture
//!   audio-tray --taskbar-revert
//!                         ask an injected TAP to put the taskbar back. Leaves the
//!                         running tray alone — it will put the strip up again the
//!                         next time it starts, or when Explorer restarts.
//!   audio-tray --taskbar-restart
//!                         restart explorer.exe, as the flyout footer's button does.
//!                         Frees the TAP DLL, so it also applies a staged update to it.

use anyhow::{bail, Context, Result};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};

mod audio;
mod canvas;
mod config;
mod flyout;
mod icons;
mod layered;
mod music;
mod osd;
mod taskbar;
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
            // Dev: show the flyout once. `--flyout menu` previews the right-click menu;
            // `--flyout icons` jumps straight to the first device's icon-picker screen;
            // `--flyout update` fakes a staged update so the footer's restart button shows.
            let mut config = Config::load();
            let outcome = match args.get(1).map(String::as_str) {
                                Some("icons") => flyout::show_icons_preview(&backend, &mut config, None),
                Some("update") => {
                    update::set_pending_version("9.9.9");
                    flyout::show(&backend, &mut config, None)
                }
                _ => flyout::show(&backend, &mut config, None),
            };
            if outcome.config_changed {
                config.save()?;
            }
            println!(
                "flyout: closed (config_changed={}, quit={}, restart={})",
                outcome.config_changed, outcome.quit, outcome.restart
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
            // One notch, so this moves the volume by exactly as much as a scroll over the
            // output button does.
            let before = backend.master_volume()?;
            match args.get(1).map(String::as_str) {
                Some("up") => {
                    backend.nudge_volume(Flow::Output, tray::SCROLL_STEP)?;
                }
                Some("down") => {
                    backend.nudge_volume(Flow::Output, -tray::SCROLL_STEP)?;
                }
                Some("get") | None => {}
                Some(other) => bail!("usage: audio-tray --vol <up|down|get> (got {other:?})"),
            }
            let after = backend.master_volume()?;
            println!("volume: {:.0}% -> {:.0}%", before * 100.0, after * 100.0);
        }
        Some("--osd") => {
            // Dev: show the scroll readout on its own, next to the cursor, and wait for it
            // to fade. The only way to iterate on it (and screenshot it) without a taskbar
            // strip to scroll — and with an explicit level, without touching the device.
            let flow = match args.get(1).map(String::as_str) {
                Some("in") => Flow::Input,
                Some("out") | None => Flow::Output,
                Some(other) => bail!("usage: audio-tray --osd [out|in] [level%] (got {other:?})"),
            };
            let level = match args.get(2) {
                Some(value) => Some(
                    value
                        .parse::<f32>()
                        .with_context(|| format!("{value:?} is not a level in percent"))?
                        / 100.0,
                ),
                None => None,
            };
            osd::preview(&backend, flow, level)?;
        }
        Some("--mic") => {
            // Dev: what the red recording dot is driven by — which apps hold the
            // microphone open now, and then every change as it happens. The watcher
            // itself prints the flips (see `audio::mic`), so this only has to report the
            // starting state and stay alive to hear them.
            let seconds = match args.get(1) {
                Some(value) => value
                    .parse::<u64>()
                    .with_context(|| format!("{value:?} is not a number of seconds"))?,
                None => 20,
            };
            let users = audio::mic::users();
            match users.as_slice() {
                [] => println!("mic: idle"),
                users => println!("mic: in use by {}", users.join(", ")),
            }
            println!("watching for {seconds}s (start or stop a recording app)...");
            audio::mic::in_use(); // starts the watcher
            std::thread::sleep(std::time::Duration::from_secs(seconds));
        }
        Some("--meter") => {
            // Dev: sample the default output + input peak meters (IAudioMeterInformation)
            // for a few seconds, to confirm they report live activity.
            let out = backend.default_of(Flow::Output).ok().flatten();
            let inp = backend.default_of(Flow::Input).ok().flatten();
            let om = out.as_ref().and_then(|id| backend.meter_for(id, Flow::Output).ok());
            let im = inp.as_ref().and_then(|id| backend.meter_for(id, Flow::Input).ok());
            println!("sampling meters for 4s (out_meter={}, in_meter={})...", om.is_some(), im.is_some());
            for _ in 0..80 {
                let o = om.as_ref().map(|m| m.peak()).unwrap_or(-1.0);
                let i = im.as_ref().map(|m| m.peak()).unwrap_or(-1.0);
                println!("out={o:.3}  in={i:.3}");
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
        Some("--update") => update::run_manual()?,
        Some("--taskbar-click") => {
            // Dev: drive the strip's gestures against the running tray. Real clicks
            // on the taskbar cannot be synthesised (see `crates/taskbar-tap/FINDINGS.md`),
            // so this is the only way to exercise the cycling from a script.
            let action = match args.get(1).map(String::as_str) {
                Some("out") => taskbar::Action::CycleOutput,
                Some("in") => taskbar::Action::CycleInput,
                Some("panel") => taskbar::Action::OpenPanel,
                other => bail!("usage: audio-tray --taskbar-click <out|in|panel> (got {other:?})"),
            };
            taskbar::post_action(action)?;
            println!("taskbar: posted {action:?} to the running tray.");
        }
        Some("--taskbar-scroll") => {
            // Dev: the wheel/touchpad half of the strip's gestures. The wheel can be tested
            // by hand; the touchpad's sub-notch deltas arrive from inside Explorer and
            // cannot be synthesised, so this stands in for them — fractional notches
            // included.
            let flow = match args.get(1).map(String::as_str) {
                Some("out") => Flow::Output,
                Some("in") => Flow::Input,
                other => {
                    bail!("usage: audio-tray --taskbar-scroll <out|in> [notches] (got {other:?})")
                }
            };
            let notches = match args.get(2) {
                Some(value) => value
                    .parse::<f32>()
                    .with_context(|| format!("{value:?} is not a number of notches"))?,
                None => 1.0,
            };
            taskbar::post_scroll(flow, notches)?;
            println!("taskbar: posted a {notches} notch {flow:?} scroll to the running tray.");
        }
        Some("--taskbar-restart") => {
            // Restarts the shell, the same as the flyout footer's button. Here for the same
            // reason `--taskbar-revert` is: the click that normally triggers it cannot be
            // synthesised, so this is how the path gets exercised end to end. Leaves the
            // running tray alone — it puts the strip back on `TaskbarCreated`.
            println!("taskbar: restarting Explorer...");
            taskbar::restart_explorer()?;
            println!("taskbar: done.");
        }
        // The music half's diagnostics. They exist because the interesting failures are all in *other*
        // processes: which SMTC session is YouTube Music (Chromium decides the app id), whether the
        // player publishes a position at all, and whether the shell still lets us put a progress bar
        // on somebody else's window.
        Some("--music-probe") => music::probe()?,
        Some("--music-timeline") => music::report_timeline()?,
        Some("--music-progress") => {
            let value = args.get(2).map(String::as_str).unwrap_or("off");
            let fraction = match value {
                "off" | "none" => None,
                percent => Some(
                    percent.parse::<f64>().with_context(|| {
                        format!("--music-progress wants a percentage or 'off', got {percent:?}")
                    })? / 100.0,
                ),
            };
            music::player::set_player_progress(fraction, true)?;
            println!("music: progress -> {fraction:?}");
        }
        Some("--music-windows") => {
            let windows = music::player::player_windows();
            if windows.is_empty() {
                println!("no window with 'youtube' in its title");
            }
            for line in windows {
                println!("{line}");
            }
        }
        Some("--taskbar-revert") => {
            // Ask whatever TAP is loaded to put the taskbar back, without touching
            // the running tray. Both an escape hatch (a strip left behind by a
            // process that died badly) and how the revert path gets exercised end
            // to end — the strip's own gestures cannot be synthesised, see
            // `crates/taskbar-tap/FINDINGS.md`.
            taskbar::revert();
            println!("taskbar: controls removed.");
        }
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
