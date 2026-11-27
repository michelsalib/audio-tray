//! Home: what Audio Tray is doing, at a glance.

use crate::config::{PillAlign, PillEdge, PillSize, TileStyle};
use crate::lang::tr;
use crate::tf;

use super::super::column::{button_width, Column, Gate, Switch};
use super::super::{attention, plain, Kind, Model, Page, Target, Toggle, Tone, CARD_PAD, CONTROL_H, R};
use super::{join_names, needs_strip, off_label};

const GLANCE_H: f32 = 100.0;
/// Between two of Home's tiles, across and down.
const GLANCE_GAP: f32 = 10.0;

/// What Audio Tray is doing, at a glance: a tile per part of the app saying how it is and opening
/// its page (in the navigation's order, three by three), then the switches most often changed.
pub(in crate::settings::layout) fn home(col: &mut Column, model: &Model) {
    let c = &model.config;
    first_run(col, model);
    let glances = glances(model);
    let rows = glances.len().div_ceil(3) as f32;
    let w = (col.w - 2.0 * GLANCE_GAP) / 3.0;
    for (i, glance) in glances.into_iter().enumerate() {
        let r = R::new(col.x + (i % 3) as f32 * (w + GLANCE_GAP), col.y + (i / 3) as f32 * (GLANCE_H + GLANCE_GAP), w, GLANCE_H);
        col.laid.push(r, Kind::Glance { title: glance.title, head: glance.head, line: glance.line, tone: glance.tone, target: glance.target });
        col.laid.hits.push((r, glance.target));
    }
    col.y += rows * (GLANCE_H + GLANCE_GAP);

    col.section(tr("Quick settings", "Paramètres rapides"));
    // Short titles, each with its part's glyph: half a card leaves no room for a sentence.
    col.toggle_grid(&[
        Switch::new(c, tr("Notifications at the bell", "Notifications à la cloche"), tr("Every app's, and Audio Tray's", "De toutes les applications et d’Audio Tray"), Gate::Open, Toggle::Bell).glyph(Page::Notifications.glyph()),
        Switch::new(c, tr("Calendar at the clock", "Agenda à l’horloge"), tr("The month and your meetings", "Le mois et vos réunions"), Gate::Open, Toggle::Calendar).glyph(Page::Calendar.glyph()),
        Switch::new(c, tr("Edge pill", "Pastille"), tr("What plays and your calls", "Ce qui joue et vos appels"), Gate::Open, Toggle::Pill).glyph(Page::Pill.glyph()),
        Switch::new(c, tr("Each new song", "Chaque nouveau morceau"), tr("Its title and cover, for a moment", "Son titre et sa pochette, un instant"), Gate::Open, Toggle::AnnounceTracks).glyph('\u{EC4F}'),
        Switch::new(c, tr("Do not disturb in calls", "Ne pas déranger en appel"), tr("Teams, Discord, a meeting app", "Teams, Discord, une application de réunion"), Gate::Open, Toggle::QuietCalls).glyph('\u{E708}'),
        Switch::new(c, tr("Hide Windows' volume", "Masquer le volume de Windows"), tr("The output button replaces it", "Le bouton de sortie le remplace"), needs_strip(c), Toggle::HideVolume).glyph('\u{E74F}'),
    ]);
}

/// "Audio Tray's buttons in the taskbar": the first thing to turn on, while nothing is drawn there.
fn first_run(col: &mut Column, model: &Model) {
    if model.config.injects() {
        return;
    }
    let card = col.card(
        Some('\u{E75B}'),
        tr("Put Audio Tray in the taskbar", "Mettre Audio Tray dans la barre des tâches"),
        plain(tr("Output and input buttons beside the clock; the clock and the bell can follow", "Des boutons de sortie et d’entrée près de l’horloge ; l’horloge et la cloche peuvent suivre")),
        false,
        None,
    );
    let label = tr("Turn on", "Activer");
    let w = button_width(col.measure, label, None);
    let r = R::new(card.right() - CARD_PAD - w, card.y + (card.h - CONTROL_H) / 2.0, w, CONTROL_H);
    col.push_button(r, label, true, None, false, Target::Toggle(Toggle::Strip));
}

/// "Left edge, middle".
fn place_words(edge: PillEdge, align: PillAlign) -> String {
    let edge_word = match edge {
        PillEdge::Left => tr("Left edge", "Bord gauche"),
        PillEdge::Right => tr("Right edge", "Bord droit"),
        PillEdge::Top => tr("Top edge", "Bord supérieur"),
    };
    let labels = super::pill::align_labels(edge);
    let at = super::pill::ALIGNS.iter().position(|a| *a == align).unwrap_or(1);
    let place = if align == PillAlign::Center { tr("middle", "milieu").to_string() } else { labels[at].to_lowercase() };
    format!("{edge_word}, {place}")
}

/// One of Home's tiles: a part of the app, its state in a few words and a line more, how it is
/// going, and the page it opens.
struct Glance {
    title: &'static str,
    head: String,
    line: String,
    tone: Tone,
    target: Target,
}

/// Home's tiles.
fn glances(model: &Model) -> Vec<Glance> {
    let c = &model.config;
    let s = &model.status;
    let mut tiles = Vec::new();
    let mut tile = |title: &'static str, head: String, line: String, tone: Tone, page: Page| tiles.push(Glance { title, head, line, tone, target: Target::Nav(page) });
    let (head, tone) = match (c.strip.enabled, s.notice) {
        (_, Some(_)) => (tr("Needs attention", "Nécessite votre attention").to_string(), Tone::Warn),
        (true, None) => (tr("Output and input buttons", "Boutons de sortie et d’entrée").to_string(), Tone::Good),
        (false, None) => (off_label().to_string(), Tone::Plain),
    };
    let line = if !c.strip.enabled { tr("Windows' own icons only", "Seulement les icônes de Windows") } else if c.strip.next_to_wifi { tr("Next to Wi-Fi", "À côté du Wi-Fi") } else { tr("Beside the notification icons", "À côté des icônes de notification") };
    tile(tr("Taskbar", "Barre des tâches"), head, line.to_string(), tone, Page::Taskbar);

    // The clock's and the bell's panels.
    let cal = &c.calendar;
    let cs = &model.calendar;
    let problem = [&cs.google, &cs.microsoft].into_iter().find_map(|a| a.problem.clone()).or_else(|| cs.links.iter().flatten().next().cloned());
    let (head, line, tone) = match (cal.enabled, problem) {
        (false, _) => (off_label().to_string(), tr("The clock opens Windows' calendar", "L’horloge ouvre le calendrier de Windows").to_string(), Tone::Plain),
        (true, Some(problem)) => (tr("Needs attention", "Nécessite votre attention").to_string(), problem, Tone::Warn),
        (true, None) if cs.calendars.is_empty() && cal.links.is_empty() => (tr("No calendar yet", "Pas encore d’agenda").to_string(), tr("Sign in, or add a link", "Connectez-vous, ou ajoutez un lien").to_string(), Tone::Plain),
        (true, None) => {
            let n = cs.calendars.len();
            (if n == 1 { tr("1 calendar", "1 agenda").to_string() } else { tf!("{n} calendars", "{n} agendas") }, tr("Opened from the clock", "Ouvert depuis l’horloge").to_string(), Tone::Good)
        }
    };
    tile(tr("Calendar", "Agenda"), head, line, tone, Page::Calendar);
    let (head, line, tone) = match (cal.bell, cs.notifications_denied, cs.quiet) {
        (false, _, _) => (off_label().to_string(), tr("The bell opens Windows' own", "La cloche ouvre celles de Windows").to_string(), Tone::Plain),
        (true, true, _) => (tr("Not allowed", "Non autorisé").to_string(), tr("Windows keeps them from Audio Tray", "Windows les refuse à Audio Tray").to_string(), Tone::Warn),
        (true, false, true) => (tr("Quiet", "Silence").to_string(), tr("Do not disturb is on", "Ne pas déranger est activé").to_string(), Tone::Plain),
        (true, false, false) => (tr("Opened from the bell", "Ouvertes depuis la cloche").to_string(), tr("Do not disturb is off", "Ne pas déranger est désactivé").to_string(), Tone::Good),
    };
    tile("Notifications", head, line, tone, Page::Notifications);

    let p = &c.pill;
    let (head, tone) = match (p.enabled, p.size) {
        (false, _) => (off_label().to_string(), Tone::Plain),
        (true, PillSize::Hidden) => (tr("Hidden at rest", "Masquée au repos").to_string(), Tone::Good),
        (true, PillSize::Small) => (tr("Small at rest", "Petite au repos").to_string(), Tone::Good),
        (true, PillSize::Big) => (tr("Big at rest", "Grande au repos").to_string(), Tone::Good),
    };
    tile(Page::Pill.title(), head, place_words(p.edge, p.align), tone, Page::Pill);

    let b = &c.batteries;
    let lowest = s.batteries.iter().min_by_key(|x| x.level);
    let (head, line, tone) = match (b.enabled, lowest) {
        (false, _) => (off_label().to_string(), tr("Battery levels are not followed", "Les niveaux de batterie ne sont pas suivis").to_string(), Tone::Plain),
        (true, None) => (tr("None reporting", "Aucun ne le signale").to_string(), tr("Wireless devices show once connected", "Les appareils sans fil apparaissent une fois connectés").to_string(), Tone::Plain),
        (true, Some(low)) => {
            let others = s.batteries.len() - 1;
            let line = match others {
                0 => tr("The only one reporting", "Le seul à le signaler").to_string(),
                1 => tr("1 other device", "1 autre appareil").to_string(),
                n => tf!("{n} other devices", "{n} autres appareils"),
            };
            (tf!("{} at {}%", "{} à {} %", low.name, low.level), line, if low.level <= b.low_at { Tone::Warn } else { Tone::Good })
        }
    };
    tile("Batteries", head, line, tone, Page::Batteries);

    let k = &c.keyboards;
    let head = match (k.enabled, k.assigned.len()) {
        (false, _) => off_label().to_string(),
        (true, 0) => tr("No keyboard yet", "Pas encore de clavier").to_string(),
        (true, 1) => tr("1 keyboard", "1 clavier").to_string(),
        (true, n) => tf!("{n} keyboards", "{n} claviers"),
    };
    let line = if k.enabled { tr("A layout for each keyboard", "Une disposition par clavier") } else { tr("One layout for every keyboard", "Une disposition pour tous les claviers") };
    tile(tr("Keyboards", "Claviers"), head, line.to_string(), if k.enabled { Tone::Good } else { Tone::Plain }, Page::Keyboards);

    let (head, tone) = if c.music.enabled { (tr("Following YouTube Music", "Suit YouTube Music").to_string(), Tone::Good) } else { (tr("YouTube Music not followed", "YouTube Music non suivi").to_string(), Tone::Plain) };
    let line = match c.music.tile_style {
        TileStyle::Off => tr("Its taskbar button: the icon only", "Son bouton : l’icône seule"),
        TileStyle::Cover => tr("Its taskbar button: the cover", "Son bouton : la pochette"),
        TileStyle::Full => tr("Its taskbar button: cover and title", "Son bouton : pochette et titre"),
    };
    tile(tr("Music", "Musique"), head, line.to_string(), tone, Page::Connectors);

    let connected: Vec<&str> = s.integrations.iter().filter(|(_, on)| *on).map(|(name, _)| *name).collect();
    let needs = attention(model, Page::Connectors);
    let (head, tone) = match (needs, connected.len()) {
        (0, 0) => (tr("None connected", "Aucun connecté").to_string(), Tone::Plain),
        (0, 1) => (tr("1 connected", "1 connecté").to_string(), Tone::Good),
        (0, n) => (tf!("{n} connected", "{n} connectés"), Tone::Good),
        (1, _) => (tr("One needs a step", "Un demande une étape").to_string(), Tone::Warn),
        (n, _) => (tf!("{n} need a step", "{n} demandent une étape"), Tone::Warn),
    };
    let line = if connected.is_empty() { tr("Browser, Discord, Teams, MPC-HC", "Navigateur, Discord, Teams, MPC-HC").to_string() } else { join_names(&connected) };
    tile(tr("Connectors", "Connecteurs"), head, line, tone, Page::Connectors);

    let version = env!("CARGO_PKG_VERSION");
    let (head, line, tone) = match &model.update {
        Some(next) => (tf!("Version {next} is ready", "La version {next} est prête"), tr("Restart Audio Tray to use it", "Redémarrez Audio Tray pour l’utiliser").to_string(), Tone::Warn),
        None => (tr("Up to date", "À jour").to_string(), format!("Version {version}"), Tone::Good),
    };
    tile(tr("Updates", "Mises à jour"), head, line, tone, Page::About);
    tiles
}
