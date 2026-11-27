//! One module per page, and the words and helpers more than one of them use.

pub(super) mod about;
pub(super) mod batteries;
pub(super) mod calendar;
pub(super) mod connectors;
pub(super) mod devices;
pub(super) mod home;
pub(super) mod keyboards;
pub(super) mod motion;
pub(super) mod notifications;
pub(super) mod pill;
pub(super) mod taskbar;

use crate::lang::tr;
use crate::tf;

use super::column::{Column, Gate};
use super::{Kind, Page, Tone, R};

/// A line of secondary text under what is above it.
fn note(col: &mut Column, text: &str) {
    col.laid.push(R::new(col.x + 2.0, col.y - 4.0, col.w, 18.0), Kind::Text(text.into()));
    col.y += 20.0;
}

/// "Off", as a state.
fn off_label() -> &'static str {
    tr("Off", "Désactivé")
}

/// A connector's state, as its card's second line says it.
fn off() -> (String, Tone) {
    (off_label().to_string(), Tone::Plain)
}

fn not_set_up(why: &str) -> (String, Tone) {
    (tf!("Not set up: {why}", "Non configuré : {why}"), Tone::Warn)
}

fn set_up(how: &str) -> (String, Tone) {
    (how.to_string(), Tone::Good)
}

/// A key not pasted yet, as its row's second line.
fn not_set() -> Option<(String, Tone)> {
    super::plain(tr("Not set", "Non défini"))
}

/// The label of a button that opens a page outside Settings.
fn open_label() -> &'static str {
    tr("Open", "Ouvrir")
}

fn paste_label() -> &'static str {
    tr("Paste", "Coller")
}

fn remove_label() -> &'static str {
    tr("Remove", "Supprimer")
}

fn cancel_label() -> &'static str {
    tr("Cancel", "Annuler")
}

/// "A, B and C".
fn join_names(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [one] => one.to_string(),
        [rest @ .., last] => tf!("{} and {last}", "{} et {last}", rest.join(", ")),
    }
}

/// The bell's switch, named alike wherever it shows.
fn bell_title() -> &'static str {
    tr("The bell opens Audio Tray's notifications", "La cloche ouvre les notifications d’Audio Tray")
}

/// The clock's switch, named alike wherever it shows.
fn clock_title() -> &'static str {
    tr("The clock opens Audio Tray's calendar", "L’horloge ouvre l’agenda d’Audio Tray")
}

/// What a switch drawn by the code in the taskbar waits on, while none of Audio Tray's parts are
/// there.
fn needs_tap() -> &'static str {
    tr("Needs one of Audio Tray's buttons in the taskbar", "Nécessite un bouton d’Audio Tray dans la barre des tâches")
}

/// What a switch drawn with the audio buttons waits on: the buttons, on the Taskbar page.
fn needs_strip(c: &crate::config::Config) -> Gate {
    Gate::needs(!c.strip.enabled, tr("Needs the audio buttons, on the Taskbar page", "Nécessite les boutons audio, page Barre des tâches"), Some(Page::Taskbar))
}
