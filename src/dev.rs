//! Developer and diagnostic modes, built only with `--features dev`.
//!
//! The release binary keeps just what users, the installer and the updater run (see `main`).
//!
//! Any mode also takes `volume=`, `ring=`, `bar=` (`none`, `google` or `samsung`) and
//! `colour=cover`: the motion to show, over the saved one, for this run.
//!
//!   --list                         defaults by role, and every active device
//!   --set <q> / --set-icon <q> <IconId>
//!                                  switch the default output / store a device's icon
//!   --flyout [update]              preview the panel (update = fake a staged update)
//!   --pill-preview <left|right|top> <start|center|end> [secs] [small] [earbuds] [live] [shots]
//!                  [call] [crowd] [cover] [paused] [toggle] [card]
//!                                  the edge pill alone there, with a song and the devices
//!   --settings-preview [page]      the settings window alone (taskbar, pill, devices, connectors,
//!                                  about); changes are not applied, there is no tray to apply them
//!   --calendar [bell|batteries] [ics-url …]
//!                                  the calendar flyout alone (with `bell`: the notifications; with
//!                                  `batteries`: the batteries, read once), on the config's accounts
//!                                  and links plus these links (this run only)
//!   --activator <aumid>            where an app's notification buttons and replies go
//!   --payloads [count]             the newest notifications in Windows' store, as their apps sent them
//!   --click-notification <id> [dry]
//!                                  a listed notification's banner clicked (dry: not launched)
//!   --banners [count] [secs]       banners of what is listed
//!   --banner-sim                   the banners' simulation, failing when a check does
//!   --listening-preview            the "an app is listening" banner, for a few seconds
//!   --digest [secs]                the digest's banner over everything listed now
//!   --vol <up|down|get>            nudge or read the default output volume by one scroll notch
//!   --osd [out|in] [level%]        preview the scroll readout beside the cursor until it fades
//!   --mic [secs]                   who holds the microphone, then watch it change
//!   --batteries [secs]             the connected devices' battery levels, then each change
//!   --battery-icon [dir]           Audio Tray's own battery icon in each style, as raw RGBA files
//!   --battery-toast                the low-battery Windows notification, with a made-up device
//!   --own-toast [battery|audio|meeting|focus|call|clear]
//!                                  one of Audio Tray's own notifications, tagged as the tray sends
//!                                  it (call: an incoming call; clear: Audio Tray's taken off the list)
//!   --meter                        sample the default output+input peak meters for 4 s
//!   --taskbar-click <out|in|panel|clock|bell|battery>
//!                                  post a strip gesture to the running tray (clicks on the
//!                                  taskbar cannot be synthesised)
//!   --taskbar-scroll <out|in> [notches]
//!                                  post a strip scroll (the only way to drive touchpad deltas)
//!   --keyboards [secs] [switch <device-substring>=<hkl> …]
//!                                  keyboards and layouts, then which keyboard types (never the
//!                                  key) and the foreground layout; with rules, switch per keyboard
//!   --music-probe                  list SMTC sessions with their YouTube Music verdicts
//!   --music-timeline [next|prev|toggle] [secs]
//!                                  trace the followed session every 100 ms, optionally commanding it
//!   --music-events [secs]          print each SMTC event and the reading it leads to, unpublished
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
    crate::config::set_motion(motion_from(args));
    match args.first().map(String::as_str) {
        Some("--flyout") => {
            // `update` fakes a staged update.
            if args.get(1).is_some_and(|a| a == "update") {
                update::set_pending_version("9.9.9");
            }
            let outcome = flyout::show(backend, &mut Config::load(), None);
            println!("flyout: closed (restart={}, settings={})", outcome.restart, outcome.settings);
        }
        Some("--pill-preview") => pill_preview(backend, &args[1..])?,
        Some("--calendar") => {
            // Extra calendar links for this run only (nothing is saved).
            let mut config = Config::load();
            let bell = args.get(1).is_some_and(|a| a == "bell");
            let batteries = args.get(1).is_some_and(|a| a == "batteries");
            for url in &args[1 + usize::from(bell || batteries)..] {
                config.calendar.links.push(crate::calendar::Link { name: crate::calendar::link_name(url), url: url.clone() });
            }
            let panel = if bell {
                crate::calendar::Panel::Notifications
            } else if batteries {
                // The levels read once, as the tray's source would report them.
                crate::calendar::set_batteries(crate::batteries::read());
                crate::calendar::Panel::Batteries
            } else {
                crate::calendar::Panel::Calendar
            };
            crate::calendar::preview(&config, panel);
        }
        Some("--activator") => crate::calendar::rich::print_activator(args.get(1).map_or("", String::as_str)),
        Some("--payloads") => crate::calendar::rich::print_latest(args.get(1).and_then(|n| n.parse().ok()).unwrap_or(5))?,
        Some("--click-notification") => crate::calendar::preview_click(args.get(1).and_then(|n| n.parse().ok()).unwrap_or(0), args.get(2).is_some_and(|a| a == "dry")),
        Some("--banners") => crate::calendar::preview_banners(args.get(1).and_then(|n| n.parse().ok()).unwrap_or(2), args.get(2).and_then(|s| s.parse().ok()).unwrap_or(10)),
        Some("--settings-preview") => crate::settings::preview(&Config::load(), args.get(1).map_or("", String::as_str)),
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
                Some("clock") => taskbar::Action::OpenCalendar,
                Some("bell") => taskbar::Action::OpenNotifications,
                Some("battery") => taskbar::Action::OpenBatteries,
                other => bail!("usage: audio-tray --taskbar-click <out|in|panel|clock|bell|battery> (got {other:?})"),
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
        Some("--batteries") => crate::batteries::trace(args.get(1).and_then(|s| s.parse().ok()).unwrap_or(0)),
        Some("--battery-icon") => {
            // Audio Tray's own battery icon (a desktop's), each style for one, two and three devices,
            // as raw RGBA files: <dir>/battery-<style>-<n>-<size>.rgba
            use crate::tray::battery::render::Style;
            let dir = std::path::PathBuf::from(args.get(1).map_or(".", String::as_str));
            let cases: [&[u8]; 3] = [&[78], &[35, 78], &[12, 35, 78]];
            let styles = [(Style::Bars, "bars"), (Style::Rings, "rings"), (Style::Bubbles, "bubbles"), (Style::Battery, "battery"), (Style::Number, "number")];
            for (style, label) in styles {
                for levels in cases {
                    let (rgba, size) = crate::tray::battery::icon_pixels(style, levels, 20);
                    let name = format!("battery-{label}-{}-{size}.rgba", levels.len());
                    std::fs::write(dir.join(&name), rgba)?;
                    println!("{name}");
                }
            }
        }
        Some("--battery-toast") => crate::toast::show("G705 Mouse battery low", "12% left. Time to charge it.")?,
        // Audio Tray's own notices as the tray sends them (tagged), and a call and a chat.
        Some("--own-toast") => {
            let tag = |kind: &str| format!("{}{kind}", crate::toast::TAG);
            match args.get(1).map_or("battery", String::as_str) {
                "battery" => crate::toast::show_with("G705 Mouse battery low", "12% left. Time to charge it.", None, false, &tag("battery/a1"))?,
                "audio" => crate::toast::show_with("Sound now on WH-1000XM5", "Speakers disconnected.", None, false, &tag("audio"))?,
                "meeting" => {
                    let start = crate::calendar::time::now() + 300;
                    crate::toast::show_with("Sync produit", "In 5 min · Teams", Some(("Join", "https://teams.microsoft.com/")), true, &tag(&format!("meeting/{start}")))?
                }
                "focus" => crate::toast::show_with("Focus done", "25 minutes of focus. Take a break.", None, false, &tag("focus/25"))?,
                "call" => crate::toast::show_xml(
                    r#"<toast scenario="incomingCall"><visual><binding template="ToastGeneric"><text>Camille Bernard</text><text>Incoming call</text></binding></visual><actions><action content="Accept" arguments="accept" activationType="foreground"/><action content="Decline" arguments="decline" activationType="foreground"/></actions></toast>"#,
                    "",
                )?,
                // Audio Tray's own test notifications gone from Windows' list.
                "clear" => windows::UI::Notifications::ToastNotificationManager::History()?.ClearWithId(&windows::core::HSTRING::from(crate::toast::AUMID))?,
                other => anyhow::bail!("--own-toast battery|audio|meeting|focus|call|clear, not {other:?}"),
            }
        }
        Some("--banner-sim") => {
            if !crate::calendar::simulate_banners() {
                anyhow::bail!("the banner simulation failed");
            }
        }
        Some("--listening-preview") => {
            crate::calendar::listening(Some(("SystemSettings".into(), "SystemSettings.exe".into())));
            crate::win::pump_for(std::time::Duration::from_secs(6));
        }
        Some("--digest") => crate::calendar::preview_digest(args.get(1).and_then(|s| s.parse().ok()).unwrap_or(8)),
        Some("--keyboards") => crate::keyboards::spike::run(&args[1..])?,
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
        Some("--music-events") => {
            let seconds = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(60);
            music::report_events(seconds)?
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

/// The saved motion, with the styles `args` name over it: `volume=` (the flyout's and the readout's
/// sliders), `ring=` and `bar=` (the pill's) as `none`, `google` or `samsung`, and `colour=cover`.
fn motion_from(args: &[String]) -> crate::config::Motion {
    let mut motion = Config::load().motion;
    for (key, value) in args.iter().filter_map(|a| a.split_once('=')) {
        let style = crate::config::MotionStyle::ALL.into_iter().find(|s| format!("{s:?}").eq_ignore_ascii_case(value));
        match (key, style) {
            ("volume", Some(style)) => motion.volume = style,
            ("ring", Some(style)) => motion.ring = style,
            ("bar", Some(style)) => motion.bar = style,
            ("colour", _) if value == "cover" => motion.colour = crate::config::WaveColour::Cover,
            _ => {}
        }
    }
    motion
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

/// The edge pill at a place, showing a song, for `secs` (default 20).
fn pill_preview(backend: &WasapiBackend, args: &[String]) -> Result<()> {
    use crate::config::{PillAlign, PillEdge, PillSize};
    let mut config = Config::load().pill;
    config.enabled = true;
    config.monitor = None;
    config.edge = match args.first().map(String::as_str) {
        Some("right") => PillEdge::Right,
        Some("top") => PillEdge::Top,
        _ => PillEdge::Left,
    };
    config.align = match args.get(1).map(String::as_str) {
        Some("start") => PillAlign::Start,
        Some("end") => PillAlign::End,
        _ => PillAlign::Center,
    };
    config.size = if args.iter().any(|a| a == "small") { PillSize::Small } else { PillSize::Big };
    let secs: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(20);
    // The motion (`ring=`, `bar=`, `colour=`) is `run`'s.
    crate::pill::create(&config)?;
    let now = music::smtc::now_ticks();
    let live = args.iter().any(|a| a == "live");
    let mut state = crate::hub::State::default();
    // The defaults as the tray reads them, unmuted; `earbuds`: the output's hand-drawn icon,
    // whatever the device.
    let read = tray::endpoints::read(backend, &Config::load());
    state.endpoints.output = read.output.map(|e| crate::hub::Endpoint { muted: false, icon: if args.iter().any(|a| a == "earbuds") { IconId::RoundEarbuds } else { e.icon }, ..e });
    state.endpoints.input = read.input.map(|e| crate::hub::Endpoint { muted: false, ..e });
    state.media.push(crate::hub::Media {
        app_id: "preview".into(),
        // `live`: a stream, as a Twitch tab reports it (no end, no previous or next).
        app: if live { "Twitch" } else { "YouTube Music" }.into(),
        title: if live { "A stream" } else { "A song" }.into(),
        artist: "An artist".into(),
        playing: true,
        cover: None,
        timeline: (!live).then_some(music::smtc::Timeline { start: 0, end: 200 * 10_000_000, position: 140 * 10_000_000, last_updated: now }),
        skip: !live,
        video: live,
    });
    // `call`: a Teams call with no faces (its icon in their place; Audio Tray's, here).
    if args.iter().any(|a| a == "call") {
        let icon = std::env::current_exe().ok().and_then(|exe| crate::image::exe_icon_png(&exe.to_string_lossy(), 64).ok()).map(std::sync::Arc::new);
        let call = crate::hub::Call { channel: None, members: Vec::new(), self_muted: false, self_deafened: false, icon };
        state.calls.push((crate::hub::VoiceApp::Teams, call));
        state.media.clear();
    }
    // `cover`: the song's cover from %TEMP%\audio-tray-shots\cover1.png; `paused`: paused.
    if let Some(song) = state.media.first_mut() {
        if args.iter().any(|a| a == "cover") {
            song.cover = std::fs::read(std::env::temp_dir().join("audio-tray-shots").join("cover1.png")).ok().map(std::sync::Arc::new);
        }
    }
    // A paused session shows only beside a playing one: a second session, playing, under it.
    if args.iter().any(|a| a == "paused") && !state.media.is_empty() {
        let mut other = state.media[0].clone();
        state.media[0].playing = false;
        other.app_id = "preview-2".into();
        other.app = "Spotify".into();
        other.title = "Another song".into();
        other.cover = std::fs::read(std::env::temp_dir().join("audio-tray-shots").join("cover2.png")).ok().map(std::sync::Arc::new);
        if let Some(timeline) = other.timeline.as_mut() {
            timeline.position = 60 * 10_000_000;
        }
        state.media.push(other);
    }
    // `crowd`: a Discord call of nine (initials, the third speaking), beside the song.
    if args.iter().any(|a| a == "crowd") {
        let members = ["Ana", "Ben", "Cy", "Dee", "Eli", "Fay", "Gus", "Hal", "Ivy"]
            .iter()
            .enumerate()
            .map(|(i, name)| crate::hub::Member { id: i.to_string(), name: name.to_string(), avatar: None, speaking: i == 2, muted: false, me: i == 0 })
            .collect();
        let call = crate::hub::Call { channel: Some("General".into()), members, self_muted: false, self_deafened: false, icon: None };
        state.calls.push((crate::hub::VoiceApp::Discord, call));
    }
    // `shots`: two invented sessions with covers from %TEMP%\audio-tray-shots (cover1.png,
    // cover2.png), for the website: nothing of anyone's.
    if args.iter().any(|a| a == "shots") {
        let dir = std::env::temp_dir().join("audio-tray-shots");
        let cover = |n: u32| std::fs::read(dir.join(format!("cover{n}.png"))).ok().map(std::sync::Arc::new);
        state.media.clear();
        state.media.push(crate::hub::Media {
            app_id: "shots-1".into(),
            app: "YouTube Music".into(),
            title: "Night Drive".into(),
            artist: "Low Orbit".into(),
            playing: true,
            cover: cover(1),
            timeline: Some(music::smtc::Timeline { start: 0, end: 214 * 10_000_000, position: 92 * 10_000_000, last_updated: now }),
            skip: true,
            video: false,
        });
        state.media.push(crate::hub::Media {
            app_id: "shots-2".into(),
            app: "Twitch".into(),
            title: "Building a synth from scratch".into(),
            artist: "Workbench".into(),
            playing: true,
            cover: cover(2),
            timeline: None,
            skip: false,
            video: true,
        });
    }
    crate::pill::update(&state);
    // `card`: the card open, as when the pointer is on the pill.
    if args.iter().any(|a| a == "card") {
        crate::pill::preview_hover();
    }
    println!("pill: {:?} {:?}, {}s", config.edge, config.align, secs);
    let until = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    // `toggle`: pause and resume the first song every 3 s, to watch its ripple settle and rise.
    let toggle = args.iter().any(|a| a == "toggle");
    let mut flip_at = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while std::time::Instant::now() < until {
        if toggle && std::time::Instant::now() >= flip_at {
            if let Some(song) = state.media.first_mut() {
                song.playing = !song.playing;
                if let Some(timeline) = song.timeline.as_mut() {
                    timeline.last_updated = music::smtc::now_ticks();
                }
            }
            crate::pill::update(&state);
            flip_at += std::time::Duration::from_secs(3);
        }
        crate::win::pump_messages();
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
    crate::pill::destroy();
    Ok(())
}
