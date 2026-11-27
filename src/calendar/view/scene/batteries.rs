//! The batteries' card laid out: this PC's battery (with one), then each device's level as a bar,
//! its number and, when low, the word. Nothing on it is a target but the footer: it is read, not
//! worked (the tooltip of the battery icon says the same for a screen reader).

use super::*;
use crate::batteries::{Tone, MIN_SHOWN};

/// This PC's line.
pub(super) const PC_H: f32 = 72.0;
/// A device's line.
pub(super) const DEVICE_H: f32 = 44.0;
/// The PC's battery drawing, and where the text beside it starts.
const PC_BODY: (f32, f32) = (44.0, 24.0);
const PC_TEXT_X: f32 = 80.0;
/// A device's glyph column, its name's start, its level's bar and its number.
const GLYPH_W: f32 = 24.0;
const NAME_X: f32 = 44.0;
const BAR_W: f32 = 64.0;
const BAR_H: f32 = 6.0;
const NUMBER_W: f32 = 44.0;
/// The number with its word ("12% (low)"), wider.
const NUMBER_LOW_W: f32 = 96.0;

/// The card's height: its header, this PC's line, every device's (or the line saying there is
/// none), the footer; at most the screen's.
pub(super) fn batteries_height(input: &Input) -> f32 {
    let mut h = CENTER_HEAD_H;
    if input.pc.is_some() {
        h += PC_H + 8.0;
    }
    h += if input.devices.is_empty() { EMPTY_H } else { input.devices.len() as f32 * DEVICE_H };
    (h + 8.0 + FOOTER_H).min(input.room)
}

/// A battery drawn in `body`: its outline and tip, filled to `level` in its tone's colour (green
/// while it charges), a bolt over it while charging.
fn battery_shape(ink: &mut Layout, body: R, pc: &PcBattery, light: bool) {
    ink.draw(body, Draw::Outline { ink: Ink::Text2, radius: 4.0 });
    let tip_h = body.h * 0.4;
    ink.draw(R::new(body.right() + 1.0, body.y + (body.h - tip_h) / 2.0, 2.5, tip_h), Draw::Rect { fill: Fill::Mark, radius: 1.0 });
    let inner = R::new(body.x + 3.0, body.y + 3.0, (body.w - 6.0) * (pc.level as f32 / 100.0).clamp(MIN_SHOWN, 1.0), body.h - 6.0);
    let tone = if pc.charging { Tone::Good } else { pc.tone };
    ink.draw(inner, Draw::Rect { fill: Fill::Rgb(tone.rgb(light)), radius: 2.0 });
    if pc.charging {
        ink.glyph(body, '\u{E945}', Font::IconSmall, Ink::Text);
    }
}

/// The card: its title, this PC's battery, each device's, the footer (Windows' power settings on a
/// PC with a battery, its devices' on one without).
pub(super) fn batteries(ink: &mut Layout, card: R, input: &Input) {
    let t = words();
    let x = card.x + 8.0;
    let w = card.w - 16.0;
    let head = R::new(x, card.y + 10.0, w, 32.0);
    ink.text(R::new(head.x + 8.0, head.y, head.w - 16.0, head.h), t.batteries, Font::Strong, Ink::Text, Align::Left);
    match input.pc {
        Some(_) => footer(ink, card, '\u{E83F}', t.power_settings),
        None => footer(ink, card, '\u{E702}', t.bluetooth_settings),
    }
    let area = R::new(card.x, head.bottom() + 6.0, card.w, card.bottom() - FOOTER_H - 4.0 - (head.bottom() + 6.0));
    ink.draw(area, Draw::Clip);
    let mut y = area.y;
    if let Some(pc) = &input.pc {
        let r = R::new(x, y, w, PC_H);
        ink.draw(r, Draw::Rect { fill: Fill::Raised, radius: 6.0 });
        battery_shape(ink, R::new(r.x + 16.0, r.y + (PC_H - PC_BODY.1) / 2.0, PC_BODY.0, PC_BODY.1), pc, input.light);
        let tx = r.x + PC_TEXT_X;
        let low = pc.tone == Tone::Low && !pc.charging;
        let level_ink = if low { Ink::Rgba(Tone::Low.rgb(input.light), 255) } else { Ink::Text };
        ink.text(R::new(tx, r.y + 10.0, r.right() - tx - 12.0, 30.0), t.level(pc.level, low), Font::Big, level_ink, Align::Left);
        let line = if pc.state.is_empty() { t.this_pc.to_string() } else { format!("{} · {}", t.this_pc, pc.state) };
        ink.text(R::new(tx, r.y + 40.0, r.right() - tx - 12.0, 20.0), line, Font::Caption, Ink::Text2, Align::Left);
        y += PC_H + 8.0;
    }
    if input.devices.is_empty() {
        ink.text(R::new(x + 8.0, y, w - 16.0, EMPTY_H - 8.0), t.no_devices, Font::Body, Ink::Text3, Align::Left);
    }
    for device in &input.devices {
        let row = R::new(x, y, w, DEVICE_H);
        let low = device.tone == Tone::Low;
        ink.glyph(R::new(row.x + 8.0, row.y + (row.h - GLYPH_W) / 2.0, GLYPH_W, GLYPH_W), device.glyph, Font::Icon, Ink::Text);
        let number_w = if low { NUMBER_LOW_W } else { NUMBER_W };
        let number = R::new(row.right() - 8.0 - number_w, row.y, number_w, row.h);
        let track = R::new(number.x - 6.0 - BAR_W, row.y + (row.h - BAR_H) / 2.0, BAR_W, BAR_H);
        ink.text(R::new(row.x + NAME_X, row.y, track.x - 12.0 - (row.x + NAME_X), row.h), device.name.clone(), Font::Body, Ink::Text, Align::Left);
        ink.draw(track, Draw::Rect { fill: Fill::Mark, radius: BAR_H / 2.0 });
        let fill = Fill::Rgb(device.tone.rgb(input.light));
        ink.draw(R::new(track.x, track.y, track.w * (device.level as f32 / 100.0).clamp(MIN_SHOWN, 1.0), track.h), Draw::Rect { fill, radius: BAR_H / 2.0 });
        let number_ink = if low { Ink::Rgba(Tone::Low.rgb(input.light), 255) } else { Ink::Text2 };
        ink.text(number, t.level(device.level, low), Font::Caption, number_ink, Align::Right);
        y += DEVICE_H;
    }
    ink.draw(area, Draw::Unclip);
}
