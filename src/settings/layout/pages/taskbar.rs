//! The Taskbar page: Audio Tray's buttons there, and Windows' shortcuts for what they replace.

use crate::config::{BellMiddleClick, MiddleClick};
use crate::lang::tr;

use super::super::choice::{BELL_MIDDLE_CLICKS, MIDDLE_CLICKS};
use super::super::column::{choice_tiles, Column, Gate, Switch};
use super::super::{plain, Choice, Model, Page, Picture, Toggle, Tone};
use super::{bell_title, clock_title, needs_tap, note};

/// A taskbar picture tile's height.
pub(super) const BUTTON_TILE_H: f32 = 140.0;

/// The Taskbar page: Audio Tray's three buttons in the order they sit on the taskbar (the audio
/// buttons, the clock, the bell), each on or off, what a middle click does, what the button shows;
/// then Windows' shortcuts for what they replace.
pub(in crate::settings::layout) fn taskbar(col: &mut Column, model: &Model) {
    let c = &model.config;
    if let Some(notice) = model.status.notice {
        col.card(Some('\u{E7BA}'), tr("The taskbar needs attention", "La barre des tâches nécessite votre attention"), Some((notice.to_string(), Tone::Warn)), false, None);
        col.gap(8.0);
    }
    note(col, tr("On each button, a left click opens its panel and a right click its menu", "Sur chaque bouton, un clic gauche ouvre son panneau et un clic droit son menu"));

    // The audio buttons.
    col.section(tr("Audio buttons", "Boutons audio"));
    col.toggle(
        Some('\u{E767}'),
        tr("Audio buttons in the tray", "Boutons audio dans la zone de notification"),
        plain(tr("Output and input buttons, over Audio Tray's icon", "Boutons de sortie et d’entrée, par-dessus l’icône d’Audio Tray")),
        Gate::Open,
        Toggle::Strip,
    );
    let off = Gate::needs(!c.strip.enabled, tr("Needs the audio buttons, above", "Nécessite les boutons audio, ci-dessus"), None);
    let clicks = [
        (MiddleClick::Mute, tr("Mute", "Couper le son"), tr("Mutes it; again unmutes", "Coupe le son ; rétablit au second clic")),
        (MiddleClick::Switch, tr("Switch device", "Changer de périphérique"), tr("Goes to the next device", "Passe au périphérique suivant")),
        (MiddleClick::SwitchMute, tr("Switch, then mute", "Changer, puis couper"), tr("Each device, then muted", "Chaque périphérique, puis coupé")),
    ];
    let tiles = choice_tiles(&clicks, &MIDDLE_CLICKS, c.strip.middle_click, Picture::Click, Choice::MiddleClick);
    col.tiles(Some((tr("Middle click", "Clic du milieu"), tr("On the output or the input button", "Sur le bouton de sortie ou d’entrée"))), &tiles, off, BUTTON_TILE_H);
    // The keyboard layout indicator is Windows' icon the buttons sit beside; hiding it needs the
    // code drawn into the taskbar (any of Audio Tray's parts there).
    let no_tap = Gate::needs(!c.injects(), needs_tap(), None);
    col.toggle_grid(&[
        Switch::new(c, tr("Next to Wi-Fi", "À côté du Wi-Fi"), tr("Between the keyboard layout and Wi-Fi", "Entre la disposition du clavier et le Wi-Fi"), off, Toggle::NextToWifi),
        Switch::new(c, tr("Hide Windows' volume icon", "Masquer l’icône de volume de Windows"), tr("The output button replaces it", "Le bouton de sortie la remplace"), off, Toggle::HideVolume),
        Switch::new(c, tr("Hide Windows' microphone icon", "Masquer l’icône du micro de Windows"), tr("The input button's dot says who listens", "Le point du bouton d’entrée indique qui écoute"), off, Toggle::HideMic),
        Switch::new(c, tr("Hide Windows' keyboard layout", "Masquer la disposition de clavier de Windows"), tr("Win+Space still switches", "Win+Espace change toujours"), no_tap, Toggle::HideLanguage),
    ]);

    // The clock.
    let cal = &c.calendar;
    col.section(tr("Clock", "Horloge"));
    col.toggle(
        Some('\u{E787}'),
        clock_title(),
        plain(tr("The month, your events and meetings, in place of Windows' calendar", "Le mois, vos événements et réunions, à la place du calendrier de Windows")),
        Gate::Open,
        Toggle::Calendar,
    );
    note(col, tr("A middle click on the clock is Windows'", "Le clic du milieu sur l’horloge reste à Windows"));
    col.toggle_grid(&[
        Switch::new(c, tr("Seconds", "Secondes"), tr("After the minutes", "Après les minutes"), Gate::Open, Toggle::ClockSeconds),
        Switch::new(c, tr("Day of the week", "Jour de la semaine"), tr("Before the date", "Avant la date"), Gate::Open, Toggle::ClockWeekday),
        Switch::new(c, tr("One line", "Une ligne"), tr("The date after the time", "La date après l’heure"), Gate::Open, Toggle::ClockOneLine),
        Switch::new(c, tr("Hide the date", "Masquer la date"), tr("The time alone", "L’heure seule"), Gate::Open, Toggle::ClockHideDate),
    ]);
    note(col, tr("Your own patterns: time_pattern and date_pattern under [clock] in config.toml", "Vos propres formats : time_pattern et date_pattern sous [clock] dans config.toml"));

    // The bell: its switch is with its notifications; what the button does is here.
    col.section(tr("Bell", "Cloche"));
    let state = if cal.bell { tr("On; its notifications have their own page", "Activé ; ses notifications ont leur page") } else { tr("Off: Windows' own. Turn it on on the Notifications page", "Désactivé : celles de Windows. À activer page Notifications") };
    col.nav_link(Some('\u{EA8F}'), bell_title(), plain(state), Page::Notifications);
    let bell_off = Gate::needs(!cal.bell, tr("Needs the bell's notifications, on the Notifications page", "Nécessite les notifications de la cloche, page Notifications"), Some(Page::Notifications));
    let clicks = [
        (BellMiddleClick::DoNotDisturb, tr("Do not disturb", "Ne pas déranger"), tr("On, and off again", "Activer, puis désactiver")),
        (BellMiddleClick::ClearAll, tr("Clear all", "Tout effacer"), tr("Every notification", "Toutes les notifications")),
        (BellMiddleClick::Nothing, tr("Nothing", "Rien"), tr("The click stays Windows'", "Le clic reste à Windows")),
    ];
    let tiles = choice_tiles(&clicks, &BELL_MIDDLE_CLICKS, cal.bell_middle_click, Picture::Bell, Choice::BellMiddle);
    col.tiles(Some((tr("Middle click", "Clic du milieu"), tr("On the bell", "Sur la cloche"))), &tiles, bell_off, BUTTON_TILE_H);
    col.toggle(None, tr("How many wait", "Nombre en attente"), plain(tr("A count on the bell; crossed out while Windows keeps them quiet", "Un compteur sur la cloche ; barré quand Windows les met en silence")), bell_off, Toggle::ClockUnread);

    // Windows' shortcuts for what the buttons replace.
    col.section(tr("Shortcuts", "Raccourcis"));
    col.toggle(None, tr("Win+Ctrl+V opens the audio panel", "Win+Ctrl+V ouvre le panneau audio"), plain(tr("Windows' shortcut for its sound output", "Le raccourci de Windows pour sa sortie audio")), Gate::Open, Toggle::StripShortcut);
    let neither = Gate::needs(!crate::calendar::takes_clock_button(c), tr("Needs the calendar or the bell, above", "Nécessite l’agenda ou la cloche, ci-dessus"), None);
    col.toggle(None, tr("Win+Alt+D opens the calendar, Win+N the notifications", "Win+Alt+D ouvre l’agenda, Win+N les notifications"), plain(tr("Windows' shortcuts, each while Audio Tray's is on", "Les raccourcis de Windows, chacun quand celui d’Audio Tray est activé")), neither, Toggle::CalendarShortcut);
}

#[cfg(test)]
mod tests {
    use super::super::super::tests::{measure, model};
    use super::super::super::{attention, page, Button, Kind, Target};
    use super::*;

    #[test]
    fn the_calendar_turns_on_from_off() {
        // Off is the default: its own switch, with the clock's, must still take a click (it once
        // dimmed itself).
        let mut m = model(Page::Taskbar);
        m.config.calendar.enabled = false;
        let laid = page(&m, 900.0, &measure);
        assert!(laid.hits.iter().any(|(_, t)| *t == Target::Toggle(Toggle::Calendar)));
        // The Calendar page says where.
        m.page = Page::Calendar;
        let laid = page(&m, 900.0, &measure);
        assert!(laid.hits.iter().any(|(_, t)| *t == Target::Nav(Page::Taskbar)));
    }

    #[test]
    fn every_taskbar_button_has_its_section_and_the_bell_its_page() {
        let mut m = model(Page::Taskbar);
        // The bell off: its tiles are dimmed and lead to the page that turns it on.
        let laid = page(&m, 900.0, &measure);
        assert!(!laid.hits.iter().any(|(_, t)| matches!(t, Target::Choice(Choice::BellMiddle, _))));
        assert!(laid.hits.iter().any(|(_, t)| *t == Target::Nav(Page::Notifications)));
        // On, without the calendar: the bell stands alone.
        m.config.calendar.bell = true;
        let laid = page(&m, 900.0, &measure);
        let sections: Vec<&str> = laid.items.iter().filter_map(|i| if let Kind::Section(s) = i.kind { Some(s) } else { None }).collect();
        assert_eq!(&sections[..4], ["Audio buttons", "Clock", "Bell", "Shortcuts"]);
        assert!(laid.hits.iter().any(|(_, t)| matches!(t, Target::Choice(Choice::BellMiddle, _))));
        m.page = Page::Notifications;
        m.calendar.notifications_denied = true;
        let laid = page(&m, 900.0, &measure);
        assert!(laid.hits.iter().any(|(_, t)| *t == Target::Button(Button::AllowNotifications)));
        assert!(laid.hits.iter().any(|(_, t)| *t == Target::Button(Button::Quiet)));
        assert_eq!(attention(&m, Page::Notifications), 1);
    }
}
