//! The Edge pill page: whether it shows, its place on the map of the displays, its size at rest.

use crate::config::{PillAlign, PillEdge, PillSize};
use crate::lang::tr;

use super::super::choice::PILL_SIZES;
use super::super::column::{choice_tiles, Column, Gate};
use super::super::{plain, Choice, Kind, Model, Picture, Target, Toggle, CARD_GAP, R};

const MAP_H: f32 = 260.0;
/// A size tile's height on the pill's page.
const SIZE_TILE_H: f32 = 156.0;

pub(in crate::settings) const EDGES: [PillEdge; 3] = [PillEdge::Left, PillEdge::Top, PillEdge::Right];
pub(in crate::settings) const ALIGNS: [PillAlign; 3] = [PillAlign::Start, PillAlign::Center, PillAlign::End];

/// The labels for the places along `edge`.
pub(in crate::settings) fn align_labels(edge: PillEdge) -> [&'static str; 3] {
    if edge.vertical() {
        [tr("Top", "Haut"), tr("Middle", "Milieu"), tr("Bottom", "Bas")]
    } else {
        [tr("Left", "Gauche"), tr("Middle", "Milieu"), tr("Right", "Droite")]
    }
}

pub(in crate::settings::layout) fn pill(col: &mut Column, model: &Model) {
    let c = &model.config;
    let p = &c.pill;
    col.toggle(Some('\u{E7F4}'), tr("Show the edge pill", "Afficher la pastille"), plain(tr("What plays and your calls at a glance, on the edge of a screen", "Ce qui joue et vos appels en un coup d’œil, au bord d’un écran")), Gate::Open, Toggle::Pill);
    let off = Gate::needs(!p.enabled, tr("Needs the edge pill, above", "Nécessite la pastille, ci-dessus"), None);
    // The map is where the place is picked: click one of the dots along a screen's edge.
    col.section(tr("Place", "Position"));
    let map = R::new(col.x, col.y, col.w, MAP_H);
    col.laid.push(map, Kind::Map { dim: off.dim() });
    for (r, target) in map_targets(model, map) {
        if let Some(target) = off.target(target) {
            col.laid.hits.push((r, target));
        }
    }
    col.y += MAP_H + CARD_GAP;
    // Automatic, then the displays as numbered on the map; a display not connected is automatic.
    let mut displays = vec![tr("Automatic", "Automatique").to_string()];
    displays.extend((1..=model.monitors.len()).map(|n| n.to_string()));
    let display = p.monitor.as_ref().and_then(|name| model.monitors.iter().position(|m| &m.name == name)).map_or(0, |i| i + 1);
    let auto = plain(tr("Automatic is the screen at that edge of the desktop", "Automatique : l’écran à ce bord du bureau"));
    col.segments(tr("Display", "Écran"), auto, &displays, display, off, Choice::Display);

    col.section(tr("Size at rest", "Taille au repos"));
    let sizes = [
        (PillSize::Hidden, tr("Hidden", "Masquée"), tr("Only a sliver on the edge", "Juste un liseré sur le bord")),
        (PillSize::Small, tr("Small", "Petite"), tr("Small covers", "Petites pochettes")),
        (PillSize::Big, tr("Big", "Grande"), tr("Covers with their progress rings", "Pochettes avec leur anneau de progression")),
    ];
    let tiles = choice_tiles(&sizes, &PILL_SIZES, p.size, Picture::Size, Choice::PillSize);
    col.tiles(None, &tiles, off, SIZE_TILE_H);
    col.laid.push(R::new(col.x, col.y - 4.0, col.w, 20.0), Kind::Text(tr("Hovering the edge always opens the full card; \"you're muted\" in a call always shows in full.", "Survoler le bord ouvre toujours la carte complète ; « micro coupé » en appel s’affiche toujours en entier.").into()));
    col.y += 24.0;
}

/// The displays scaled into `area`, keeping their arrangement: one rectangle per display.
pub(in crate::settings) fn map_rects(model: &Model, area: R) -> Vec<R> {
    let ms = &model.monitors;
    if ms.is_empty() {
        return Vec::new();
    }
    let left = ms.iter().map(|m| m.rect.left).min().unwrap_or(0) as f32;
    let top = ms.iter().map(|m| m.rect.top).min().unwrap_or(0) as f32;
    let right = ms.iter().map(|m| m.rect.right).max().unwrap_or(1) as f32;
    let bottom = ms.iter().map(|m| m.rect.bottom).max().unwrap_or(1) as f32;
    let inner = R::new(area.x + 24.0, area.y + 20.0, area.w - 48.0, area.h - 56.0);
    let k = (inner.w / (right - left).max(1.0)).min(inner.h / (bottom - top).max(1.0));
    let (dx, dy) = (inner.x + (inner.w - (right - left) * k) / 2.0, inner.y + (inner.h - (bottom - top) * k) / 2.0);
    // A hairline apart, so displays side by side read as two.
    ms.iter()
        .map(|m| {
            let x = dx + (m.rect.left as f32 - left) * k;
            let y = dy + (m.rect.top as f32 - top) * k;
            R::new(x + 2.0, y + 2.0, (m.rect.right - m.rect.left) as f32 * k - 4.0, (m.rect.bottom - m.rect.top) as f32 * k - 4.0)
        })
        .collect()
}

/// Where the pill would sit on a display drawn at `screen`, at `edge` and `align`.
pub(in crate::settings) fn slot_rect(screen: R, edge: PillEdge, align: PillAlign) -> R {
    const THICK: f32 = 6.0;
    let len = if edge.vertical() { (screen.h * 0.2).clamp(10.0, 36.0) } else { (screen.w * 0.14).clamp(10.0, 36.0) };
    let span = if edge.vertical() { (screen.y, screen.h) } else { (screen.x, screen.w) };
    // Clear of the corners, where the other edges' places are.
    let margin = THICK + 8.0;
    let at = match align {
        PillAlign::Start => span.0 + margin,
        PillAlign::Center => span.0 + (span.1 - len) / 2.0,
        PillAlign::End => span.0 + span.1 - margin - len,
    };
    match edge {
        PillEdge::Left => R::new(screen.x + 3.0, at, THICK, len),
        PillEdge::Right => R::new(screen.right() - 3.0 - THICK, at, THICK, len),
        PillEdge::Top => R::new(at, screen.y + 3.0, len, THICK),
    }
}

/// The map's click targets: each display, then the nine places on it (on top, padded to be easy
/// to hit).
fn map_targets(model: &Model, map: R) -> Vec<(R, Target)> {
    let mut targets = Vec::new();
    for (i, screen) in map_rects(model, map).into_iter().enumerate() {
        targets.push((screen, Target::Monitor(i)));
        for edge in EDGES {
            for align in ALIGNS {
                let s = slot_rect(screen, edge, align);
                targets.push((R::new(s.x - 6.0, s.y - 6.0, s.w + 12.0, s.h + 12.0), Target::Slot(i, edge, align)));
            }
        }
    }
    targets
}

#[cfg(test)]
mod tests {
    use super::super::super::tests::{measure, model};
    use super::super::super::{page, Page};
    use super::*;

    #[test]
    fn a_switch_is_hit_anywhere_on_its_card() {
        let laid = page(&model(Page::Pill), 900.0, &measure);
        let card = laid.items.iter().find(|i| matches!(&i.kind, Kind::Card { title, .. } if title == "Show the edge pill")).unwrap();
        assert_eq!(laid.hit(card.r.x + 10.0, card.r.y + 10.0), Some(Target::Toggle(Toggle::Pill)));
        let switch = laid.items.iter().find(|i| matches!(i.kind, Kind::Toggle { toggle: Toggle::Pill, .. })).unwrap();
        assert_eq!(laid.hit(switch.r.x + 2.0, switch.r.y + 2.0), Some(Target::Toggle(Toggle::Pill)));
    }

    #[test]
    fn choices_are_hit_where_they_are_drawn_and_not_while_dimmed() {
        let mut m = model(Page::Pill);
        let laid = page(&m, 900.0, &measure);
        // The size, as picture tiles.
        for i in 0..3 {
            let tile = laid.items.iter().find(|it| matches!(it.kind, Kind::Tile { target: Target::Choice(Choice::PillSize, j), .. } if j == i)).unwrap();
            assert_eq!(laid.hit(tile.r.x + tile.r.w / 2.0, tile.r.y + tile.r.h / 2.0), Some(Target::Choice(Choice::PillSize, i)));
        }
        m.config.pill.enabled = false;
        let laid = page(&m, 900.0, &measure);
        assert!(!laid.hits.iter().any(|(_, t)| matches!(t, Target::Choice(..) | Target::Slot(..) | Target::Monitor(_))));
        // On Motion, the volume sliders' style stays: they are the panel's and the readout's.
        m.page = Page::Motion;
        let laid = page(&m, 900.0, &measure);
        assert!(laid.hits.iter().any(|(_, t)| matches!(t, Target::Choice(Choice::VolumeMotion, _))));
        assert!(!laid.hits.iter().any(|(_, t)| matches!(t, Target::Choice(Choice::RingMotion, _))));
    }

    #[test]
    fn the_map_keeps_the_displays_apart_and_its_places_on_them() {
        let m = model(Page::Pill);
        let area = R::new(0.0, 0.0, 800.0, MAP_H);
        let screens = map_rects(&m, area);
        assert_eq!(screens.len(), 2);
        assert!(screens[0].right() <= screens[1].x, "side by side, not overlapping");
        assert!(screens[1].y > screens[0].y, "the second sits lower, as arranged");
        for screen in &screens {
            assert!(screen.x >= area.x && screen.right() <= area.right() && screen.bottom() <= area.bottom());
            for edge in EDGES {
                for align in ALIGNS {
                    let s = slot_rect(*screen, edge, align);
                    assert!(s.x >= screen.x && s.right() <= screen.right() && s.y >= screen.y && s.bottom() <= screen.bottom(), "{edge:?} {align:?}");
                }
            }
        }
        // A place wins over the display under it.
        let laid = page(&m, 900.0, &measure);
        let map = laid.items.iter().find(|i| matches!(i.kind, Kind::Map { .. })).unwrap().r;
        let screen = map_rects(&m, map)[1];
        let s = slot_rect(screen, PillEdge::Right, PillAlign::End);
        assert_eq!(laid.hit(s.x + 1.0, s.y + 1.0), Some(Target::Slot(1, PillEdge::Right, PillAlign::End)));
        assert_eq!(laid.hit(screen.x + screen.w / 2.0, screen.y + screen.h / 2.0), Some(Target::Monitor(1)));
    }
}
