//! The Audio devices page: each output and input, and the icon it is drawn with.

use crate::icons::IconId;
use crate::lang::tr;

use super::super::column::Column;
use super::super::{plain, Button, Kind, Model, Target, Tone, CARD_GAP, CARD_H, CARD_PAD, CARD_TEXT_X, R};

/// An icon's chip in a device's grid, and the room between two.
const CHIP: f32 = 48.0;
const CHIP_GAP: f32 = 8.0;

pub(in crate::settings::layout) fn devices(col: &mut Column, model: &Model) {
    for (flow, title) in [(crate::audio::Flow::Output, tr("Output", "Sortie")), (crate::audio::Flow::Input, tr("Input", "Entrée"))] {
        let rows: Vec<usize> = (0..model.devices.len()).filter(|&i| model.devices[i].flow == flow).collect();
        if rows.is_empty() {
            continue;
        }
        col.section(title);
        for i in rows {
            let d = &model.devices[i];
            let open = model.expanded == Some(i);
            let desc = d.default.then(|| (tr("Default device", "Périphérique par défaut").to_string(), Tone::Plain));
            let card = col.card(None, d.name.clone(), desc, false, Some(Target::Device(i)));
            col.laid.push(R::new(card.x + CARD_PAD, card.y, 24.0, CARD_H), Kind::Icon(d.icon));
            let label = if open { tr("Done", "Terminé") } else { tr("Change icon", "Changer l’icône") };
            let r = R::new(card.right() - CARD_PAD - 28.0 - 140.0, card.y, 140.0, CARD_H);
            col.laid.push(r, Kind::Value(label.to_string()));
            let chevron = R::new(card.right() - CARD_PAD - 20.0, card.y, 20.0, CARD_H);
            col.laid.push(chevron, Kind::Trailing(if open { '\u{E70E}' } else { '\u{E70D}' }));
            if open {
                icon_grid(col, i, d.icon);
            }
        }
    }
}

/// Windows' own sound settings, at the end of the audio devices.
pub(in crate::settings::layout) fn windows_sound(col: &mut Column) {
    col.section("Windows");
    col.link(Some('\u{E713}'), tr("Windows sound settings", "Paramètres de son de Windows"), plain(tr("Volume levels, device properties, spatial sound", "Niveaux de volume, propriétés des périphériques, son spatial")), '\u{E8A7}', Button::SoundSettings);
}

/// The icons for device `device`, wrapping, in a card under its own.
fn icon_grid(col: &mut Column, device: usize, current: IconId) {
    let inner = col.w - 2.0 * CARD_PAD - CARD_TEXT_X + CARD_PAD;
    let per_row = (((inner + CHIP_GAP) / (CHIP + CHIP_GAP)).floor() as usize).max(1);
    let rows = IconId::ALL.len().div_ceil(per_row);
    let h = CARD_PAD * 2.0 + rows as f32 * CHIP + (rows as f32 - 1.0) * CHIP_GAP;
    let card = R::new(col.x, col.y, col.w, h);
    col.laid.push(card, Kind::Card { glyph: None, title: String::new(), desc: None, dim: false, target: None });
    for (k, icon) in IconId::ALL.into_iter().enumerate() {
        let (row, c) = (k / per_row, k % per_row);
        let r = R::new(card.x + CARD_TEXT_X + c as f32 * (CHIP + CHIP_GAP), card.y + CARD_PAD + row as f32 * (CHIP + CHIP_GAP), CHIP, CHIP);
        let target = Target::Icon(device, icon);
        col.laid.push(r, Kind::Chip { icon, selected: icon == current, target });
        col.laid.hits.push((r, target));
    }
    col.y += h + CARD_GAP;
}

#[cfg(test)]
mod tests {
    use super::super::super::tests::{measure, model};
    use super::super::super::{page, Page};
    use super::*;

    #[test]
    fn an_open_device_shows_every_icon_once() {
        let mut m = model(Page::Devices);
        m.expanded = Some(0);
        let laid = page(&m, 700.0, &measure);
        let icons: Vec<IconId> = laid.hits.iter().filter_map(|(_, t)| match t {
            Target::Icon(0, icon) => Some(*icon),
            _ => None,
        }).collect();
        assert_eq!(icons, IconId::ALL.to_vec());
    }
}
