//! The Notifications page (the bell's panel).

use crate::lang::tr;

use super::super::choice::FOCUS_LENGTHS;
use super::super::column::{Column, Control, Gate, RowButton};
use super::super::{plain, Button, Choice, Model, Page, Toggle, Tone};
use super::{bell_title, note, off_label, open_label};

/// The Notifications page (the bell's panel): whether the bell opens Audio Tray's, whether Windows
/// lets it read them, then Audio Tray's own banners, then keeping them quiet: now, in meetings and
/// calls, the focus timer.
pub(in crate::settings::layout) fn notifications(col: &mut Column, model: &Model) {
    let c = &model.config.calendar;
    let s = &model.calendar;
    let off = Gate::needs(!c.bell, tr("Needs the bell's notifications, above", "Nécessite les notifications de la cloche, ci-dessus"), None);
    let open = |accent: bool, button: Button| Control::Button(RowButton::new(open_label(), button).accent(accent).glyph('\u{E8A7}'));
    let mut body = col.group();
    let what = plain(tr("Every app's, and Audio Tray's own (batteries, audio devices, reminders, focus)", "Celles de toutes les applications et d’Audio Tray (batteries, périphériques audio, rappels, concentration)"));
    col.row(&mut body, None, bell_title(), what, Gate::Open, Control::Toggle(Toggle::Bell));
    let banners = plain(tr(
        "Its banners over the bell replace Windows': each app's are turned off in Windows' settings, and back on with this",
        "Ses bannières au-dessus de la cloche remplacent celles de Windows : celles de chaque application sont désactivées dans les paramètres de Windows, et réactivées avec ce réglage",
    ));
    col.row(&mut body, None, tr("Audio Tray shows every app's banners", "Audio Tray affiche les bannières de toutes les applications"), banners, off, Control::Toggle(Toggle::TakeOver));
    if c.bell && s.notifications_denied {
        let why = Some((tr("Windows does not let Audio Tray read them", "Windows ne laisse pas Audio Tray les lire").to_string(), Tone::Warn));
        col.row(&mut body, None, tr("Allow Audio Tray", "Autoriser Audio Tray"), why, Gate::Open, open(true, Button::AllowNotifications));
    }
    col.end_group(body);
    note(col, tr("With a screen reader, Windows keeps its own banners: they are the ones it reads aloud", "Avec un lecteur d’écran, Windows garde ses bannières : ce sont elles qu’il lit à voix haute"));

    // Audio Tray's own banners, whatever the bell (never on the pill: it is about what plays).
    col.section(tr("Audio banners", "Bannières audio"));
    let mut body = col.group();
    let listening = plain(tr(
        "A red banner when an app records outside a call; a click mutes the microphone",
        "Une bannière rouge quand une application enregistre hors appel ; un clic coupe le micro",
    ));
    col.row(&mut body, None, tr("When an app listens", "Quand une application écoute"), listening, Gate::Open, Control::Toggle(Toggle::ListeningAlerts));
    let song = plain(tr("Its title and cover, for a moment; a click brings the player to the front", "Son titre et sa pochette, un instant ; un clic affiche le lecteur"));
    col.row(&mut body, None, tr("Each new song", "Chaque nouveau morceau"), song, Gate::Open, Control::Toggle(Toggle::AnnounceTracks));
    col.end_group(body);

    col.section(tr("Quiet", "Silence"));
    let mut body = col.group();
    let now = if s.quiet { (tr("On: Windows keeps notifications quiet", "Activé : Windows met les notifications en silence").to_string(), Tone::Good) } else { (off_label().to_string(), Tone::Plain) };
    let switch = if s.quiet { tr("Turn off", "Désactiver") } else { tr("Turn on", "Activer") };
    col.row(&mut body, None, tr("Do not disturb", "Ne pas déranger"), Some(now), Gate::Open, Control::Button(RowButton::new(switch, Button::Quiet).glyph('\u{E708}')));
    let no_calendar = Gate::needs(!c.enabled, tr("Needs Audio Tray's calendar, on the Taskbar page", "Nécessite l’agenda d’Audio Tray, page Barre des tâches"), Some(Page::Taskbar));
    col.row(&mut body, None, tr("Do not disturb in meetings", "Ne pas déranger en réunion"), plain(tr("While a calendar event runs", "Pendant un événement de l’agenda")), no_calendar, Control::Toggle(Toggle::QuietMeetings));
    col.row(&mut body, None, tr("Do not disturb in calls", "Ne pas déranger en appel"), plain(tr("Teams, Discord, or a meeting app on the microphone", "Teams, Discord, ou une application de réunion sur le micro")), Gate::Open, Control::Toggle(Toggle::QuietCalls));
    let focus_at = FOCUS_LENGTHS.iter().position(|f| *f == c.focus_minutes).unwrap_or(1);
    let labels = FOCUS_LENGTHS.iter().map(|f| format!("{f} min")).collect();
    col.row(&mut body, None, tr("Focus timer", "Minuteur de concentration"), plain(tr("Its length; notifications stay quiet while it runs", "Sa durée ; les notifications restent silencieuses pendant ce temps")), off, Control::Segments(labels, focus_at, Choice::FocusLength));
    let rules = plain(tr("A full-screen app, a game, a duplicated display: the bell and the moon follow them too", "Une application en plein écran, un jeu, un écran dupliqué : la cloche et la lune les suivent aussi"));
    col.row(&mut body, None, tr("Windows' own rules", "Règles de Windows"), rules, Gate::Open, open(false, Button::QuietRules));
    col.end_group(body);
}
