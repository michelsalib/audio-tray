//! The notification icon: registering it (and again when an Explorer restart lost it), its glyph,
//! its tooltip, and where it is on the taskbar.

use std::time::Duration;

use anyhow::Result;
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};
use windows::Win32::Foundation::RECT;

use crate::audio::wasapi::WasapiBackend;
use crate::config::Config;
use crate::icons::IconId;

use super::messages::WM_ICON_CHECK;
use super::{battery, endpoints, hook, post_after, Tray};

/// Stable marker appended to the tray icon's tooltip, which is also its accessible name: the only
/// thing the TAP can pick *our* icon out by. Keep in step with `tooltip=` in [`crate::taskbar`].
pub const TRAY_MARKER: &str = "Audio Tray";

/// The tooltip's first line: the device, and the marker.
fn tooltip_for(device: &str) -> String {
    format!("{device} — {TRAY_MARKER}")
}

/// The tooltip: the device and the marker on the first line, then a line per battery while they
/// fit. Lowest first, so the one that needs charging is the one that shows.
fn tooltip_text(device: &str, batteries: &[crate::hub::Battery], low_at: u8, words: &crate::calendar::Words) -> String {
    battery::fit_devices(vec![tooltip_for(device)], batteries, low_at, words, false)
}

/// The tooltip the icon is *registered* with. **Do not change it**: Windows keys the icon's
/// "always show on the taskbar" choice on the exe path plus this string, and a new string drops
/// every user's icon (and with it the strip) into the overflow. See docs/NOTES.md.
const INITIAL_TOOLTIP: &str = "Audio output";

/// Re-registration checks after a restart, and the first gap (doubling): ~1 min in all.
const ICON_CHECKS: usize = 6;
pub(super) const ICON_CHECK_FIRST: Duration = Duration::from_secs(1);

/// Register the tray icon, retrying until the shell actually accepts it.
///
/// At logon audio-tray can start before Explorer takes icons, and a failed add is terminal
/// (`TaskbarCreated` has already been and gone). `build` reports success even when the add did
/// not land, so setting a property is the real test. Retries without a deadline: an audio-tray
/// with no icon has nothing to offer, so waiting beats quitting.
pub(super) fn build_tray(backend: &WasapiBackend, config: &Config) -> Result<TrayIcon> {
    const FIRST_GAP: Duration = Duration::from_millis(500);
    const MAX_GAP: Duration = Duration::from_secs(5);
    const NAG_EVERY: u32 = 12;

    let mut gap = FIRST_GAP;
    for attempt in 1.. {
        let output = endpoints::default_output(backend, config);
        let (name, icon) = endpoints::output_face(output.as_ref());
        let built = TrayIconBuilder::new()
            .with_tooltip(INITIAL_TOOLTIP)
            .with_icon(icon_image(icon)?)
            .build();
        let failure = match built {
            // Before the hub has any battery: the device alone, until the first `refresh`.
            Ok(tray) => match show_output(&tray, name, icon, &tooltip_for(name)) {
                Ok(()) => {
                    if attempt > 1 {
                        println!("tray: registered on attempt {attempt}");
                    }
                    return Ok(tray);
                }
                Err(e) => e,
            },
            Err(e) => e.into(),
        };
        if attempt == 1 || attempt % NAG_EVERY == 0 {
            println!("tray: the shell is not taking icons yet, retrying… ({failure:#})");
        }
        std::thread::sleep(gap);
        gap = (gap * 2).min(MAX_GAP);
    }
    unreachable!("the retry loop only exits by returning")
}

/// Put the default output on the notification icon: its glyph, and `tooltip` (its name and the rest).
fn show_output(tray: &TrayIcon, name: &str, icon: IconId, tooltip: &str) -> Result<()> {
    tray.set_icon(Some(icon_image(icon)?))?;
    tray.set_tooltip(Some(tooltip))?;
    println!("refresh: default \"{name}\" -> icon {icon:?}");
    Ok(())
}

fn icon_image(id: IconId) -> Result<Icon> {
    // Monochrome like the shell's own tray icons, at the exact small-icon size.
    let tint = if taskbar_is_light() { [0x20, 0x20, 0x20] } else { [0xff, 0xff, 0xff] };
    let (rgba, w, h) = id.render(crate::win::small_icon_size(), tint)?;
    Ok(Icon::from_rgba(rgba, w, h)?)
}

/// Whether the taskbar uses the light theme (`SystemUsesLightTheme`); absent means dark.
pub(super) fn taskbar_is_light() -> bool {
    use windows::core::w;

    crate::win::hkcu_dword(
        w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
        w!("SystemUsesLightTheme"),
    ) == Some(1)
}

/// Where a notification icon is on the screen; `None` while it is not on the taskbar.
pub(super) fn icon_rect(icon: &TrayIcon) -> Option<RECT> {
    let rect = icon.rect()?;
    let (left, top) = (rect.position.x as i32, rect.position.y as i32);
    Some(RECT { left, top, right: left + rect.size.width as i32, bottom: top + rect.size.height as i32 })
}

impl Tray {
    /// Put the default output on the icon, its name heading the tooltip. Failure is non-fatal — the
    /// shell refuses icons while it restarts, and `TaskbarCreated` brings us back here.
    pub(super) fn show_output(&mut self, output: Option<&crate::hub::Endpoint>) {
        let (name, icon) = endpoints::output_face(output);
        self.tooltip_device = name.to_string();
        if let Err(e) = show_output(&self.icon, name, icon, &self.tooltip()) {
            eprintln!("tray: could not update the icon, will retry when the taskbar is back ({e:#})");
        }
    }

    /// The tooltip as it stands: the default output, then the levels when they are shown there.
    fn tooltip(&self) -> String {
        let b = &self.config.batteries;
        let shown: &[crate::hub::Battery] = if self.config.levels_in_audio() && b.tooltip { &self.hub.batteries } else { &[] };
        tooltip_text(&self.tooltip_device, shown, b.low_at, crate::calendar::words())
    }

    pub(super) fn apply_tooltip(&self) {
        if self.tooltip_device.is_empty() {
            return; // not read yet: `refresh` sets it with the icon
        }
        if let Err(e) = self.icon.set_tooltip(Some(&self.tooltip())) {
            eprintln!("tray: could not update the tooltip ({e:#})");
        }
    }

    /// Ask the shell where our icon is (it moves with every tray change and Explorer restart),
    /// record it for the wheel hook, and return it for placing the readout. `None` while the icon
    /// is not on the taskbar.
    pub(super) fn refresh_icon_rect(&self) -> Option<RECT> {
        let slot = icon_rect(&self.icon)?;
        if hook::set_icon_rect(Some(slot)) {
            println!("tray: icon slot at {},{} {}x{}", slot.left, slot.top, slot.right - slot.left, slot.bottom - slot.top);
        }
        Some(slot)
    }

    /// tray-icon re-adds the icon on `TaskbarCreated` but ignores the result, and a shell that is
    /// not taking icons yet (two restarts in a row) refuses it: the icon is then simply gone.
    /// `rect()` is `None` only for an unregistered icon (an overflowed one reports the chevron),
    /// so a missing one is rebuilt, retrying with a doubling gap.
    pub(super) fn ensure_icon(&mut self, attempt: usize) {
        if self.icon.rect().is_some() {
            if attempt > 0 {
                println!("tray: icon back after an Explorer restart (check {attempt})");
            }
            return;
        }
        let output = endpoints::default_output(&self.backend, &self.config);
        let rebuilt = icon_image(endpoints::output_face(output.as_ref()).1)
            .and_then(|image| {
                TrayIconBuilder::new()
                    .with_tooltip(INITIAL_TOOLTIP)
                    .with_icon(image)
                    .build()
                    .map_err(anyhow::Error::from)
            });
        match rebuilt {
            Ok(icon) => {
                // The old one is already gone from the shell; dropping it only frees its window.
                self.icon = icon;
                println!("tray: the icon did not survive the Explorer restart — registered it again");
                self.refresh();
            }
            Err(e) => println!("tray: icon still missing after an Explorer restart ({e:#})"),
        }
        if attempt + 1 < ICON_CHECKS {
            post_after(WM_ICON_CHECK, attempt + 1, ICON_CHECK_FIRST * 2u32.pow(attempt as u32 + 1));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tooltip_keeps_its_marker_and_lists_the_lowest_battery_first_while_it_fits() {
        let device = |name: &str, level: u8| crate::hub::Battery { id: name.into(), name: name.into(), kind: crate::hub::BatteryKind::Other, level };
        let text = tooltip_text("Speakers", &[device("Keyboard", 96), device("Mouse", 12)], 20, crate::calendar::english());
        assert_eq!(text, "Speakers — Audio Tray\nMouse: 12% (low)\nKeyboard: 96%");
        let many: Vec<_> = (0..20).map(|i| device(&format!("A device with a long name {i}"), i)).collect();
        let text = tooltip_text("Speakers", &many, 20, crate::calendar::english());
        assert!(text.starts_with("Speakers — Audio Tray\n"));
        assert!(text.encode_utf16().count() <= battery::TIP_MAX);
        assert!(text.contains("name 0: 0% (low)"), "the lowest is kept");
    }
}
