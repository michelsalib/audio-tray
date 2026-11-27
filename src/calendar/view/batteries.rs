//! The batteries' card's state: the devices as the tray last had them, this PC's battery as last
//! read, and the lines the card shows from them.

use std::cell::RefCell;

use super::{scene, with_view, words, Panel, View};
use crate::batteries::Tone;

thread_local! {
    /// Every device's battery as the tray last had them, for the batteries' card.
    static DEVICES: RefCell<Vec<crate::hub::Battery>> = const { RefCell::new(Vec::new()) };
}

/// The devices' levels moved (or the tray read them first): the batteries' card follows. Tray
/// thread.
pub fn set_batteries(devices: Vec<crate::hub::Battery>) {
    if DEVICES.with(|d| *d.borrow() == devices) {
        return;
    }
    DEVICES.with(|d| *d.borrow_mut() = devices);
    power_changed();
}

/// This PC's charge, its power source or the energy saver changed: the batteries' card follows.
pub fn power_changed() {
    with_view(|v| {
        if v.panel == Panel::Batteries {
            v.pc = crate::batteries::system();
            v.relayout();
        }
    });
}

/// The batteries' card's lines: this PC's (with a battery), then each device's, lowest first.
pub(super) fn lines(view: &View) -> (Option<scene::PcBattery>, Vec<scene::DeviceBattery>) {
    if view.panel != Panel::Batteries {
        return (None, Vec::new());
    }
    let t = words::words();
    let low_at = view.config.batteries.low_at;
    let pc = view.pc.map(|pc| scene::PcBattery { level: pc.level, tone: Tone::of(pc.level, low_at), charging: pc.charging, state: t.pc_state(&pc) });
    let mut devices: Vec<crate::hub::Battery> = DEVICES.with(|d| d.borrow().clone());
    devices.sort_by_key(|b| b.level);
    let devices = devices
        .into_iter()
        .map(|b| scene::DeviceBattery { glyph: b.kind.glyph(), name: b.name, level: b.level, tone: Tone::of(b.level, low_at) })
        .collect();
    (pc, devices)
}

/// Windows' settings the card's footer opens: power on a PC with a battery, the devices' on one
/// without.
pub(super) fn windows_settings(view: &View) -> &'static str {
    if view.pc.is_some() {
        "ms-settings:batterysaver"
    } else {
        "ms-settings:bluetooth"
    }
}
