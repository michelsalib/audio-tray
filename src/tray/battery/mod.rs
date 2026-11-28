//! The batteries' icon on the taskbar, where the levels live while `[batteries] taskbar_icon` is on:
//! Windows' own battery icon on a PC with a battery (the TAP takes its click, and is told here what
//! its tooltip says and whether it wears the low dot: `taskbar::send_battery`), or, on a PC Windows
//! gives no battery icon, Audio Tray's own ([`render`]), shown while a device reports a battery.
//!
//! Told again only when what it says changes: a level, the power source, the energy saver, a setting.
//! A TAP that does not take it (loading, or not there) is asked again on a short, ending back-off,
//! and as soon as it reports in.

mod promote;
pub(crate) mod render;

use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use crate::batteries::{SystemBattery, Tone};
use crate::calendar::Words;
use crate::config::Config;
use crate::hub::Battery;
use crate::taskbar;

/// Audio Tray's own battery icon's id, which tells its clicks from the audio icon's.
pub(super) const ICON_ID: &str = "batteries";
/// The tooltip the own icon is *registered* with: with the exe path, how Windows knows it (its "show
/// on the taskbar" choice). **Do not change it.**
const REGISTERED_TOOLTIP: &str = "Batteries";
/// What a notification icon's tooltip holds (UTF-16 units): Windows shows 127 of its 128 and cuts
/// the rest.
pub(super) const TIP_MAX: usize = 127;
/// How long before each try again, in seconds: then it waits for the TAP to report in.
const RETRY_AFTER: [u32; 5] = [2, 4, 8, 16, 32];

/// What Audio Tray's own icon is drawn for.
#[derive(Clone, Debug, PartialEq)]
struct OwnLook {
    style: render::Style,
    /// Each shown device's level and tone, lowest first.
    levels: Vec<(u8, Tone)>,
    /// The taskbar is light.
    light: bool,
}

/// Audio Tray's own icon, and what it shows.
struct Own {
    icon: TrayIcon,
    look: OwnLook,
    tip: String,
}

/// Both battery icons' state, owned by the tray.
#[derive(Default)]
pub(super) struct BatteryIcons {
    own: Option<Own>,
    /// What Windows' icon was last told: its tooltip and whether a device is low. `None`: nothing
    /// yet, or the TAP reloaded since.
    sent: Option<(String, bool)>,
    /// Tries made since the TAP last took it.
    tries: usize,
}

impl BatteryIcons {
    /// Bring both icons in line with the settings, this PC's battery and the devices'.
    pub(super) fn sync(&mut self, config: &Config, devices: &[Battery]) {
        self.sync_own(config, devices);
        self.sync_windows(config, devices);
    }

    /// The TAP was reloaded (an Explorer restart, a fresh injection): Windows' icon is told again.
    pub(super) fn forget_windows(&mut self) {
        self.sent = None;
        self.tries = 0;
    }

    /// Explorer restarted: Audio Tray's own icon went with the old shell (added again by the next
    /// sync), and Windows' icon must be told again.
    pub(super) fn taskbar_restarted(&mut self) {
        self.own = None;
        super::hook::set_own_battery_icon(None);
        self.forget_windows();
    }

    fn sync_windows(&mut self, config: &Config, devices: &[Battery]) {
        if !config.uses_windows_battery_icon() {
            // Not ours: the TAP was told so by its init data, and gave the icon back.
            self.forget_windows();
            return;
        }
        let words = crate::calendar::words();
        let low_at = config.batteries.low_at;
        let wanted = (tip(crate::batteries::system().as_ref(), devices, low_at, words), devices.iter().any(|d| d.level <= low_at));
        if self.sent.as_ref() == Some(&wanted) {
            return;
        }
        let look = taskbar::BatteryLook { tip: wanted.0.clone(), dot: if wanted.1 { crate::calendar::battery_dot() } else { None } };
        if taskbar::send_battery(&look) {
            println!("batteries: Windows' battery icon says {:?}{}", look.tip, if wanted.1 { ", with the low dot" } else { "" });
            self.sent = Some(wanted);
            self.tries = 0;
        } else if let Some(after) = RETRY_AFTER.get(self.tries) {
            // Loading, or not there: again soon, a few times; then when the TAP reports in.
            self.tries += 1;
            super::messages::set_timer(super::messages::BATTERY_RETRY_TIMER, after * 1000);
        }
    }

    /// Audio Tray's own icon (a PC without a battery): shown while a device reports one, each device
    /// (the lowest three) a bar or a ring in its level's colour, every level in its tooltip.
    fn sync_own(&mut self, config: &Config, devices: &[Battery]) {
        if !config.uses_own_battery_icon() || devices.is_empty() {
            if self.own.take().is_some() {
                super::hook::set_own_battery_icon(None);
                println!("batteries: Audio Tray's battery icon put away");
            }
            return;
        }
        let low_at = config.batteries.low_at;
        let mut levels: Vec<u8> = devices.iter().map(|d| d.level).collect();
        levels.sort_unstable();
        let look = OwnLook {
            style: config.batteries.icon_style,
            levels: levels.iter().take(render::MAX).map(|l| (*l, Tone::of(*l, low_at))).collect(),
            light: super::icon::taskbar_is_light(),
        };
        let tip = tip(None, devices, low_at, crate::calendar::words());
        match &mut self.own {
            Some(own) => {
                if own.look != look {
                    let drawn = image(&look).and_then(|image| own.icon.set_icon(Some(image)).map_err(anyhow::Error::from));
                    match drawn {
                        Ok(()) => own.look = look,
                        // Left as it was: drawn again at the next sync.
                        Err(e) => eprintln!("batteries: could not redraw the battery icon ({e:#})"),
                    }
                }
                if own.tip != tip && own.icon.set_tooltip(Some(&tip)).is_ok() {
                    own.tip = tip;
                }
            }
            None => {
                let built = image(&look).and_then(|image| TrayIconBuilder::new().with_id(ICON_ID).with_tooltip(REGISTERED_TOOLTIP).with_icon(image).build().map_err(anyhow::Error::from));
                match built {
                    Ok(icon) => {
                        let _ = icon.set_tooltip(Some(&tip));
                        println!("batteries: Audio Tray's battery icon shown ({tip:?})");
                        self.own = Some(Own { icon, look, tip });
                        promote::once();
                    }
                    Err(e) => eprintln!("batteries: could not show the battery icon ({e:#})"),
                }
            }
        }
        // Where it is, so its click is not taken for one outside an open card.
        super::hook::set_own_battery_icon(self.own.as_ref().and_then(|own| super::icon::icon_rect(&own.icon)));
    }
}

/// Audio Tray's own icon's picture, at the tray's exact icon size.
fn image(look: &OwnLook) -> anyhow::Result<Icon> {
    let size = crate::win::small_icon_size();
    Ok(Icon::from_rgba(render::render(look.style, &look.levels, size, look.light), size, size)?)
}

/// A device's line: its name and level, "low" said in words where it is (not by colour alone).
pub(crate) fn device_line(device: &Battery, low_at: u8, words: &Words) -> String {
    (words.labelled)(&device.name, &words.level(device.level, device.level <= low_at))
}

/// A battery icon's tooltip: this PC's battery (with one), then each device's, lowest first; at
/// most what Windows shows, the devices left out counted on the last line.
pub(crate) fn tip(pc: Option<&SystemBattery>, devices: &[Battery], low_at: u8, words: &Words) -> String {
    let head = pc.map(|pc| (words.labelled)(words.this_pc, &format!("{} · {}", (words.percent)(pc.level), words.pc_state(pc))));
    fit_devices(head.into_iter().collect(), devices, low_at, words, true)
}

/// A tooltip: `head`'s lines, then a line per device ([`device_line`]), lowest first, while they fit
/// in what Windows shows ([`TIP_MAX`]); with `count_left_out`, the devices that did not fit counted
/// on the last line.
pub(super) fn fit_devices(mut lines: Vec<String>, devices: &[Battery], low_at: u8, words: &Words, count_left_out: bool) -> String {
    let mut by_level: Vec<&Battery> = devices.iter().collect();
    by_level.sort_by_key(|d| d.level);
    let device_lines: Vec<String> = by_level.iter().map(|d| device_line(d, low_at, words)).collect();
    let length = |lines: &[String]| lines.iter().map(|l| l.encode_utf16().count() + 1).sum::<usize>();
    for (n, line) in device_lines.iter().enumerate() {
        let left = device_lines.len() - n - 1;
        let more = if count_left_out && left > 0 { (words.more)(left).encode_utf16().count() + 1 } else { 0 };
        if length(&lines) + line.encode_utf16().count() + more > TIP_MAX {
            if count_left_out {
                lines.push((words.more)(device_lines.len() - n));
            }
            break;
        }
        lines.push(line.clone());
    }
    lines.join("\n")
}

/// Audio Tray's own icon's right-click menu: the batteries, their settings, Windows' devices.
pub(super) fn menu() {
    const SHOW: usize = 1;
    const SETTINGS: usize = 2;
    const DEVICES: usize = 3;
    let words = crate::calendar::words();
    let items = [(SHOW, words.show_batteries, false), (SETTINGS, words.battery_settings, false), (DEVICES, words.bluetooth_settings, true)];
    match super::clicks::popup_menu(&items) {
        Some(SHOW) => {
            super::post(taskbar::WM_TASKBAR_ACTION, tap_proto::ACTION_OPEN_BATTERIES, 0);
        }
        Some(SETTINGS) => {
            super::post(crate::settings::WM_OPEN_SETTINGS, crate::settings::OPEN_BATTERIES, 0);
        }
        Some(DEVICES) => {
            crate::win::shell_open("ms-settings:bluetooth", None);
        }
        _ => {}
    }
}

/// Audio Tray's own icon in `style`, `px` square, for three sample devices (one low, one fair and
/// one good at `low_at`), in the taskbar's theme: Settings' pictures of the styles. With whether
/// the taskbar is light, for what they are drawn on.
pub(crate) fn preview(style: render::Style, low_at: u8, px: u32) -> (Vec<u8>, bool) {
    let light = super::icon::taskbar_is_light();
    let levels = [low_at.saturating_sub(5).max(3), low_at.saturating_mul(3) / 2, 80];
    let toned: Vec<_> = levels.iter().map(|l| (*l, Tone::of(*l, low_at))).collect();
    (render::render(style, &toned, px, light), light)
}

/// Dev: Audio Tray's own icon in `style` for `levels` (of `low_at`), as raw RGBA, for previews.
#[cfg(feature = "dev")]
pub(crate) fn icon_pixels(style: render::Style, levels: &[u8], low_at: u8) -> (Vec<u8>, u32) {
    let size = crate::win::small_icon_size();
    let toned: Vec<_> = levels.iter().map(|l| (*l, Tone::of(*l, low_at))).collect();
    (render::render(style, &toned, size, false), size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::BatteryKind;

    fn device(name: &str, level: u8) -> Battery {
        Battery { id: name.into(), name: name.into(), kind: BatteryKind::Mouse, level }
    }

    fn english() -> &'static Words {
        crate::calendar::english()
    }

    /// This PC first, then the devices lowest first, a low one said low.
    #[test]
    fn the_tooltip_lists_this_pc_then_the_lowest_first() {
        let pc = SystemBattery { level: 62, plugged: true, charging: true, saver: false, left: None };
        let tip = tip(Some(&pc), &[device("Mouse", 78), device("Headset", 12)], 20, english());
        assert_eq!(tip, "This PC: 62% · Charging\nHeadset: 12% (low)\nMouse: 78%");
    }

    /// More devices than Windows shows: the tooltip ends on how many were left out, within its room.
    #[test]
    fn a_long_list_ends_on_how_many_more() {
        let devices: Vec<Battery> = (0..12).map(|n| device(&format!("Wireless device number {n}"), 30 + n)).collect();
        let tip = tip(None, &devices, 20, english());
        assert!(tip.encode_utf16().count() <= TIP_MAX, "{} units", tip.encode_utf16().count());
        assert!(tip.ends_with("more"), "{tip}");
    }
}
