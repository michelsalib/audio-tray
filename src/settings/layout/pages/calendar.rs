//! The Calendar page (the clock's panel): where its events come from, how it looks, its reminders.

use crate::lang::tr;

use super::super::choice::{AGENDA_ROWS, CALENDAR_LAYOUTS, REMINDERS, SWATCHES, WEEK_STARTS};
use super::super::column::{switch_room, Column, Control, Gate, Group, RowButton};
use super::super::{plain, Button, Cal, Choice, Kind, Model, Page, Target, Toggle, Tone, CARD_PAD, CARD_TEXT_X, R, ROW_H};
use crate::settings::model::masked_tail;
use super::{cancel_label, not_set, off_label, open_label, paste_label, remove_label};

/// The Calendar page (the clock's panel): where its events come from first (the calendars found,
/// the accounts, the links), then how it looks, then its reminders. Whether the clock opens it is
/// with the clock, on the Taskbar page.
pub(in crate::settings::layout) fn calendar(col: &mut Column, model: &Model) {
    use crate::calendar::Provider;
    let c = &model.config.calendar;
    let s = &model.calendar;
    // Off, what it shows is moot; the card on top says where it is turned on.
    let gate = Gate::from(!c.enabled);
    if gate.dim() {
        let why = plain(tr("Turn it on with the clock, on the Taskbar page", "Activez-le avec l’horloge, page Barre des tâches"));
        col.card(Some('\u{E787}'), tr("The clock opens Windows' calendar", "L’horloge ouvre le calendrier de Windows"), why, false, Some(Target::Nav(Page::Taskbar)));
        col.gap(8.0);
    }

    // Each calendar found: shown or not, and its colour.
    if !s.calendars.is_empty() {
        col.section(tr("Calendars", "Agendas"));
        let mut body = col.group();
        for (i, calendar) in s.calendars.iter().enumerate() {
            let shown = !c.hidden.contains(&calendar.key);
            let chosen = c.colors.get(&calendar.key).and_then(|hex| crate::calendar::parse_hex(hex));
            col.calendar_row(&mut body, i, &calendar.name, calendar.color, chosen, shown, gate);
        }
        col.end_group(body);
    }

    let open = |button: Cal| Control::Button(RowButton::new(open_label(), Button::Cal(button)).glyph('\u{E8A7}'));
    let paste = |button: Cal| Control::Button(RowButton::new(paste_label(), Button::Cal(button)).glyph('\u{E77F}'));
    for provider in Provider::ALL {
        let account = s.account(provider);
        let google = provider == Provider::Google;
        col.section(if google { tr("Google account", "Compte Google") } else { tr("Microsoft account (Outlook, work or school)", "Compte Microsoft (Outlook, professionnel ou scolaire)") });
        let mut body = col.group();
        let sign_in = Button::Cal(Cal::SignIn(provider));
        let sign_out = RowButton::new(tr("Sign out", "Se déconnecter"), Button::Cal(Cal::SignOut(provider)));
        let cancel = RowButton::new(cancel_label(), Button::Cal(Cal::CancelSignIn(provider)));
        let signing_in = plain(tr("Finish signing in in your browser", "Terminez la connexion dans votre navigateur"));
        if let Some(email) = &account.email {
            let (desc, buttons) = match &account.problem {
                Some(problem) => ((problem.clone(), Tone::Warn), vec![RowButton::new(tr("Sign in again", "Se reconnecter"), sign_in).accent(true), sign_out]),
                None => ((tr("Signed in", "Connecté").to_string(), Tone::Good), vec![sign_out]),
            };
            let (desc, control) = if account.signing_in { (signing_in, Control::Button(cancel)) } else { (Some(desc), Control::Buttons(buttons)) };
            col.row(&mut body, None, email, desc, Gate::Open, control);
            col.end_group(body);
            continue;
        }
        let registered = if google { c.google_client_id.is_some() && c.google_client_secret.is_some() } else { c.microsoft_client_id.is_some() };
        if google {
            let api = plain(tr("Enable it, then set up the consent screen: External, and Publish it", "Activez-la, puis configurez l’écran de consentement : Externe, et publiez-le"));
            col.row(&mut body, Some((1, registered)), tr("Enable the Calendar API", "Activer l’API Calendar"), api, Gate::Open, open(Cal::GoogleApi));
            let client = plain(tr("Credentials > Create > OAuth client ID > Desktop app", "Identifiants > Créer > ID client OAuth > Application de bureau"));
            col.row(&mut body, Some((2, registered)), tr("Create a desktop client", "Créer un client de bureau"), client, Gate::Open, open(Cal::GoogleCredentials));
            let id = model.hint_for(Button::Cal(Cal::GoogleId)).or_else(|| c.google_client_id.clone().map(|id| (id, Tone::Plain)));
            col.row(&mut body, None, tr("Client ID", "ID client"), id.or(not_set()), Gate::Open, paste(Cal::GoogleId));
            let secret = model.hint_for(Button::Cal(Cal::GoogleSecret)).or_else(|| c.google_client_secret.as_deref().map(|s| (masked_tail(s), Tone::Plain)));
            col.row(&mut body, None, tr("Client secret", "Secret client"), secret.or(not_set()), Gate::Open, paste(Cal::GoogleSecret));
        } else {
            let register = plain(tr("New registration: any organisation + personal; then Calendars.Read", "Nouvelle inscription : toute organisation + personnel ; puis Calendars.Read"));
            col.row(&mut body, Some((1, registered)), tr("Register an app", "Inscrire une application"), register, Gate::Open, open(Cal::MicrosoftPortal));
            let redirect = model.hint_for(Button::Cal(Cal::MicrosoftRedirect)).or(plain(tr("Authentication > Mobile and desktop applications: http://localhost", "Authentification > Applications mobiles et de bureau : http://localhost")));
            col.row(&mut body, Some((2, false)), tr("Add the redirect", "Ajouter la redirection"), redirect, Gate::Open, Control::Button(RowButton::new(tr("Copy", "Copier"), Button::Cal(Cal::MicrosoftRedirect)).glyph('\u{E8C8}')));
            let id = model.hint_for(Button::Cal(Cal::MicrosoftId)).or_else(|| c.microsoft_client_id.clone().map(|id| (id, Tone::Plain)));
            col.row(&mut body, None, tr("Application (client) ID", "ID d’application (client)"), id.or(not_set()), Gate::Open, paste(Cal::MicrosoftId));
        }
        let (desc, control) = if account.signing_in {
            (signing_in, Control::Button(cancel))
        } else {
            let desc = account.problem.clone().map(|p| (p, Tone::Warn)).or(plain(tr("Opens the sign-in in your browser", "Ouvre la connexion dans votre navigateur")));
            (desc, Control::Button(RowButton::new(tr("Sign in", "Se connecter"), sign_in).accent(registered)))
        };
        // Not before the app is registered (the steps above): the sign-in would have nothing to use.
        col.row(&mut body, Some((3, false)), tr("Sign in", "Se connecter"), desc, !registered, control);
        col.end_group(body);
    }

    col.section(tr("Calendar links", "Liens d’agenda"));
    let mut body = col.group();
    for (i, link) in c.links.iter().enumerate() {
        let desc = match s.links.get(i).cloned().flatten() {
            Some(problem) => (problem, Tone::Warn),
            None => (link_host(&link.url), Tone::Plain),
        };
        col.row(&mut body, None, &link.name, Some(desc), Gate::Open, Control::Button(RowButton::new(remove_label(), Button::Cal(Cal::RemoveLink(i))).glyph('\u{E74D}')));
    }
    let add = model.hint_for(Button::Cal(Cal::AddLink))
        .or(plain(tr("A secret iCal address: Google's Integrate calendar, Outlook's Publish", "Une adresse iCal secrète : Intégrer l’agenda de Google, Publier d’Outlook")));
    col.row(&mut body, None, tr("Add a link", "Ajouter un lien"), add, Gate::Open, Control::Button(RowButton::new(tr("Paste link", "Coller le lien"), Button::Cal(Cal::AddLink)).accent(c.links.is_empty()).glyph('\u{E77F}')));
    col.end_group(body);

    col.section(tr("How it looks", "Apparence"));
    let mut body = col.group();
    let layout_at = CALENDAR_LAYOUTS.iter().position(|l| *l == c.layout).unwrap_or(0);
    col.row(&mut body, None, tr("Layout", "Disposition"), None, gate, Control::Segments(vec![tr("Comfortable", "Confortable").into(), "Compact".into(), tr("Agenda only", "Agenda seul").into()], layout_at, Choice::CalendarLayout));
    let rows_at = AGENDA_ROWS.iter().position(|r| *r == c.rows).unwrap_or(1);
    col.row(&mut body, None, tr("Agenda rows", "Lignes de l’agenda"), plain(tr("At most; fewer on a short screen", "Au plus ; moins sur un petit écran")), gate, Control::Segments(AGENDA_ROWS.iter().map(|r| r.to_string()).collect(), rows_at, Choice::AgendaRows));
    // Read as the calendar reads it (past Sunday is Sunday). Another day, set by hand in
    // config.toml, is none of the three: no segment is selected.
    let week_at = WEEK_STARTS.iter().position(|w| *w == c.first_weekday.map(|d| d.min(6))).unwrap_or(usize::MAX);
    col.row(&mut body, None, tr("Week starts", "Début de semaine"), None, gate, Control::Segments(vec![tr("As Windows", "Comme Windows").into(), tr("Monday", "Lundi").into(), tr("Sunday", "Dimanche").into()], week_at, Choice::WeekStart));
    col.row(&mut body, None, tr("Week numbers", "Numéros de semaine"), None, gate, Control::Toggle(Toggle::WeekNumbers));
    let clocks = model.hint_for(Button::Cal(Cal::EditClocks)).or(plain(tr("Windows' additional clocks, under the date", "Les horloges supplémentaires de Windows, sous la date")));
    col.row(&mut body, None, tr("World clocks", "Horloges supplémentaires"), clocks, gate, Control::Toggle(Toggle::WorldClocks));
    col.row(&mut body, None, tr("Choose the world clocks", "Choisir les horloges supplémentaires"), plain(tr("In Windows' Date and time settings", "Dans les paramètres Date et heure de Windows")), gate, open(Cal::EditClocks));
    col.end_group(body);

    col.section(tr("Reminders", "Rappels"));
    let mut body = col.group();
    let remind_at = REMINDERS.iter().position(|r| *r == c.remind_minutes).unwrap_or(2);
    let labels = REMINDERS.iter().map(|r| if *r == 0 { off_label().to_string() } else { format!("{r} min") }).collect();
    // A notification, under the bell when the bell is Audio Tray's (never on the pill: it is about
    // audio activity alone).
    let (remind, how) = (tr("Remind me", "Me rappeler"), tr("Before an event, a notification with Join", "Avant un événement, une notification avec Rejoindre"));
    col.row(&mut body, None, remind, plain(how), gate, Control::Segments(labels, remind_at, Choice::Reminder));
    col.end_group(body);
}

impl Column<'_> {
    /// A calendar's row: its name, its colours to pick from (its own first), and its switch.
    #[allow(clippy::too_many_arguments)]
    fn calendar_row(&mut self, group: &mut Group, i: usize, name: &str, own: [u8; 3], chosen: Option<[u8; 3]>, shown: bool, gate: Gate) {
        let dim = gate.dim();
        let row = self.begin_row(group);
        self.toggle_in(row, row.right() - CARD_PAD, shown, dim, Toggle::CalendarShown(i));
        let switch = switch_room();
        if !dim {
            self.laid.hits.push((R::new(row.right() - CARD_PAD - switch, row.y, switch, row.h), Target::Toggle(Toggle::CalendarShown(i))));
        }
        const DOT: f32 = 18.0;
        const STEP: f32 = 24.0;
        let colours: Vec<[u8; 3]> = std::iter::once(own).chain(SWATCHES).collect();
        let mut x = row.right() - CARD_PAD - switch - 20.0 - colours.len() as f32 * STEP;
        let current = chosen.unwrap_or(own);
        for (j, rgb) in colours.iter().enumerate() {
            let r = R::new(x + (STEP - DOT) / 2.0, row.y + (row.h - DOT) / 2.0, DOT, DOT);
            let target = Target::Swatch(i, j);
            let selected = if j == 0 { chosen.is_none() } else { *rgb == current && chosen.is_some() };
            self.laid.push(r, Kind::Swatch { rgb: *rgb, selected, target });
            if !dim {
                self.laid.hits.push((r, target));
            }
            x += STEP;
        }
        let end = row.right() - CARD_PAD - switch - 20.0 - colours.len() as f32 * STEP - 12.0;
        let tx = row.x + CARD_TEXT_X;
        self.laid.push(R::new(tx, row.y, end - tx, ROW_H), Kind::Label { title: name.to_string(), desc: None, dim: dim || !shown });
        self.y += ROW_H;
    }
}

/// Where a link points, without the secret part of its address.
fn link_host(url: &str) -> String {
    let url = crate::calendar::feed_url(url);
    let host = url.split("://").nth(1).and_then(|rest| rest.split('/').next()).unwrap_or_default();
    format!("{host}/••••")
}

#[cfg(test)]
mod tests {
    use super::super::super::tests::{measure, model};
    use super::super::super::page;
    use super::*;

    #[test]
    fn a_week_set_by_hand_is_not_shown_as_sunday() {
        let mut m = model(Page::Calendar);
        let selected = |m: &Model| -> Vec<String> {
            page(m, 900.0, &measure).items.into_iter().filter_map(|i| match i.kind {
                Kind::Segment { label, selected: true, target: Target::Choice(Choice::WeekStart, _), .. } => Some(label),
                _ => None,
            }).collect()
        };
        assert_eq!(selected(&m), ["As Windows"]);
        m.config.calendar.first_weekday = Some(6);
        assert_eq!(selected(&m), ["Sunday"]);
        m.config.calendar.first_weekday = Some(2);
        assert!(selected(&m).is_empty(), "Wednesday is none of the three");
    }
}
