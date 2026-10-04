//! Developer and diagnostic modes, built only with `--features dev`.
//!
//! The release binary keeps just what users, the installer and the updater run (see `main`).
//!
//!   --list                         defaults by role, and every active device
//!   --set <q> / --set-icon <q> <IconId>
//!                                  switch the default output / store a device's icon
//!   --flyout [icons|update]        preview the panel (icons = the picker, update = fake a staged update)
//!   --vol <up|down|get>            nudge or read the default output volume by one scroll notch
//!   --osd [out|in] [level%]        preview the scroll readout beside the cursor until it fades
//!   --mic [secs]                   who holds the microphone, then watch it change
//!   --meter                        sample the default output+input peak meters for 4 s
//!   --taskbar-click <out|in|panel> post a strip gesture to the running tray (clicks on the
//!                                  taskbar cannot be synthesised)
//!   --taskbar-scroll <out|in> [notches]
//!                                  post a strip scroll (the only way to drive touchpad deltas)
//!   --music-probe                  list SMTC sessions with their YouTube Music verdicts
//!   --music-timeline [next|prev|toggle] [secs]
//!                                  trace the followed session every 100 ms, optionally commanding it
//!   --music-progress <pct|off> [paused]
//!                                  set the player's taskbar progress bar by hand
//!   --music-windows [all]          survey the player's windows, from an STA and an MTA
//!   --music-thumbbar [playing|paused]
//!                                  put transport buttons on the player's thumbnail toolbar

use anyhow::{bail, Context, Result};

use crate::audio::wasapi::WasapiBackend;
use crate::audio::{self, Device, DeviceId, Flow};
use crate::config::Config;
use crate::icons::IconId;
use crate::{flyout, music, osd, taskbar, tray, update};

/// Run a dev mode. `Ok(false)`: `args` names none, so `main` carries on.
pub fn run(args: &[String], backend: &WasapiBackend) -> Result<bool> {
    match args.first().map(String::as_str) {
        Some("--flyout") => {
            // `icons` opens on the first device's picker; `update` fakes a staged update.
            let mut config = Config::load();
            let outcome = match args.get(1).map(String::as_str) {
                Some("icons") => flyout::show_icons_preview(backend, &mut config, None),
                Some("update") => {
                    update::set_pending_version("9.9.9");
                    flyout::show(backend, &mut config, None)
                }
                _ => flyout::show(backend, &mut config, None),
            };
            if outcome.config_changed {
                config.save()?;
            }
            println!(
                "flyout: closed (config_changed={}, quit={}, restart={})",
                outcome.config_changed, outcome.quit, outcome.restart
            );
        }
        Some("--list") => list(backend)?,
        Some("--set") => {
            let Some(query) = args.get(1) else {
                bail!("usage: audio-tray --set <name-substring-or-id>");
            };
            let devices = backend.enumerate_flow(Flow::Output)?;
            let target = find_device(&devices, query)?;
            println!("Switching default to: {} [{}]", target.friendly_name, target.id.0);
            backend.set_default_of(&target.id)?;
            println!("Done. Verify in Windows sound settings.");
        }
        Some("--set-icon") => {
            let (Some(query), Some(icon_str)) = (args.get(1), args.get(2)) else {
                bail!("usage: audio-tray --set-icon <name-substring-or-id> <IconId>");
            };
            let icon = IconId::parse(icon_str)
                .with_context(|| format!("unknown icon {icon_str:?}; one of {:?}", IconId::ALL))?;
            let devices = backend.enumerate_flow(Flow::Output)?;
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
            let master = || -> Result<f32> {
                let default = backend.default_of(Flow::Output)?.context("no default output")?;
                backend.volume_of(&default)
            };
            let before = master()?;
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
            let after = master()?;
            println!("volume: {:.0}% -> {:.0}%", before * 100.0, after * 100.0);
        }
        Some("--osd") => {
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
            osd::preview(backend, flow, level)?;
        }
        Some("--mic") => {
            // The watcher prints the flips itself; this reports the start state and waits.
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
        Some("--taskbar-click") => {
            // Real taskbar clicks cannot be synthesised (see FINDINGS.md), so post the gesture.
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
            // Stands in for touchpad deltas from inside Explorer, fractional notches included.
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
        Some("--music-probe") => music::probe()?,
        Some("--music-timeline") => {
            let skip = match args.get(1).map(String::as_str) {
                Some("next") => Some(music::smtc::Command::Next),
                Some("prev") => Some(music::smtc::Command::Previous),
                Some("toggle") => Some(music::smtc::Command::TogglePlayPause),
                _ => None,
            };
            let seconds = args.iter().skip(1).find_map(|a| a.parse().ok()).unwrap_or(9);
            music::report_timeline(seconds, skip)?
        }
        Some("--music-progress") => {
            let value = args.get(1).map(String::as_str).unwrap_or("off");
            let fraction = match value {
                "off" | "none" => None,
                percent => Some(
                    percent.parse::<f64>().with_context(|| {
                        format!("--music-progress wants a percentage or 'off', got {percent:?}")
                    })? / 100.0,
                ),
            };
            music::player::set_player_progress(fraction, args.get(2).map(String::as_str) != Some("paused"))?;
            println!("music: progress -> {fraction:?}");
        }
        // Probe: does the shell's thumbnail toolbar accept a window we do not own?
        Some("--music-thumbbar") => {
            let playing = !matches!(args.get(1).map(String::as_str), Some("paused"));
            music::thumbbar::probe(playing)?;
        }
        Some("--music-windows") => {
            // `all` lists every visible titled window, not just the ones the title rule would
            // match — the view that shows a browser window's app id next to the player's.
            let all = matches!(args.get(1).map(String::as_str), Some("all"));
            let windows = music::player::player_windows(all);
            if windows.is_empty() {
                println!("no window with 'youtube' in its title");
            }
            for window in &windows {
                println!("{}", window.line);
            }
            // This is an STA; the feed asks from an MTA, so report any disagreement.
            let handles: Vec<isize> = windows.iter().map(|window| window.hwnd).collect();
            match music::player_verdicts_from_mta(handles) {
                Ok(from_mta) => {
                    let disagreed: Vec<&music::player::WindowReport> = windows
                        .iter()
                        .zip(&from_mta)
                        .filter(|(window, mta)| window.player != **mta)
                        .map(|(window, _)| window)
                        .collect();
                    if disagreed.is_empty() {
                        println!("\nsame verdicts from an MTA thread.");
                    } else {
                        println!("\nan MTA thread disagrees — the identity does not read there:");
                        for window in disagreed {
                            println!("{}", window.line);
                        }
                    }
                }
                Err(err) => println!("\ncould not ask an MTA thread: {err:#}"),
            }
        }
        _ => return Ok(false),
    }
    Ok(true)
}


fn list(backend: &WasapiBackend) -> Result<()> {
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
