//! The About page: the app, its version, where it keeps things.

use crate::lang::tr;
use crate::tf;

use super::super::column::{button_width, Column, RowButton};
use super::super::{plain, Button, Kind, Model, Target, Tone, CARD_GAP, CARD_PAD, CONTROL_H, R};

/// About's header card.
const ABOUT_H: f32 = 104.0;

/// The app: its logo, version and whether an update waits, then where it keeps things.
pub(in crate::settings::layout) fn about(col: &mut Column, model: &Model) {
    let hero = R::new(col.x, col.y, col.w, ABOUT_H);
    col.laid.push(hero, Kind::Card { glyph: None, title: String::new(), desc: None, dim: false, target: None });
    col.laid.push(R::new(hero.x + CARD_PAD + 4.0, hero.y + (ABOUT_H - 64.0) / 2.0, 64.0, 64.0), Kind::Logo);
    let x = hero.x + CARD_PAD + 4.0 + 64.0 + 20.0;
    let version = format!("Version {}", env!("CARGO_PKG_VERSION"));
    let state = match &model.update {
        Some(next) => (tf!("{version}; {next} is ready", "{version} ; la {next} est prête"), Tone::Good),
        None => (tf!("{version}; up to date", "{version} ; à jour"), Tone::Plain),
    };
    let mut right = hero.right() - CARD_PAD;
    if model.update.is_some() {
        let label = tr("Restart to update", "Redémarrer pour mettre à jour");
        let w = button_width(col.measure, label, None);
        col.push_button(R::new(right - w, hero.y + (ABOUT_H - CONTROL_H) / 2.0, w, CONTROL_H), label, true, None, false, Target::Button(Button::Restart));
        right -= w + 16.0;
    }
    col.laid.push(R::new(x, hero.y + (ABOUT_H - 46.0) / 2.0, right - x, 46.0), Kind::Label { title: "Audio Tray".into(), desc: Some(state), dim: false });
    col.y += ABOUT_H + CARD_GAP;
    col.section(tr("More", "Plus"));
    col.link(Some('\u{E71B}'), tr("Releases and source", "Versions et code source"), plain("github.com/michelsalib/audio-tray"), '\u{E8A7}', Button::Releases);
    col.link(Some('\u{E8B7}'), tr("Settings folder", "Dossier des paramètres"), plain(tr("Where config.toml is kept", "Là où se trouve config.toml")), '\u{E8A7}', Button::ConfigFolder);
    col.button_card(Some('\u{E7E8}'), tr("Quit Audio Tray", "Quitter Audio Tray"), plain(tr("Puts the taskbar back as Windows draws it", "Rend la barre des tâches telle que Windows la dessine")), RowButton::new(tr("Quit", "Quitter"), Button::Quit));
}
