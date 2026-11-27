//! The Connectors page: the browser, MPC-HC, Discord and Teams, each with its steps; then YouTube
//! Music.

use crate::config::TileStyle;
use crate::lang::tr;
use crate::tf;

use super::super::choice::TILE_STYLES;
use super::super::column::{choice_tiles, Column, ConnectorCard, Control, Gate, RowButton};
use crate::settings::model::masked_tail;
use super::super::{plain, Button, Choice, Connector, Kind, Model, Picture, Toggle, Tone, R};
use super::taskbar::BUTTON_TILE_H;
use super::{join_names, not_set, not_set_up, off, open_label, paste_label, set_up};

/// The connectors two by two, each with its state and its switch; the one opened shows its steps
/// under the grid.
pub(in crate::settings::layout) fn connectors(col: &mut Column, model: &Model) {
    use crate::integrations::{browser, discord, discord_rpc, teams, teams_ui};
    let c = &model.config;
    let s = &model.status;

    // Edge freezes a paused player it has put to sleep: play from here does nothing until its window
    // is opened. Only while one of the players is not on its "never sleep" list.
    if !model.edge_sleep.is_empty() {
        let names = join_names(&model.edge_sleep);
        let card = col.card(
            Some('\u{E7BA}'),
            tf!("Edge can put {names} to sleep", "Edge peut mettre {names} en veille"),
            Some((tr("Paused a while, play then does nothing until you open it. In Edge: Never put these sites to sleep", "Après une pause, la lecture ne fait rien tant que vous ne l’ouvrez pas. Dans Edge : Ne jamais mettre ces sites en veille").to_string(), Tone::Warn)),
            false,
            None,
        );
        col.buttons_in(card, &[RowButton::new(tr("Open Edge settings", "Ouvrir les paramètres d’Edge"), Button::EdgeSleep).glyph('\u{E8A7}')], false);
        col.gap(8.0);
    }

    let mut cards = Vec::new();
    let state = match (c.browser.enabled, s.connected(browser::NAME)) {
        (false, _) => off(),
        (true, true) => set_up(tr("Connected", "Connecté")),
        (true, false) => not_set_up(tr("the extension", "l’extension")),
    };
    cards.push(ConnectorCard { glyph: '\u{E774}', title: tr("Browser tabs", "Onglets du navigateur"), state, on: c.browser.enabled, toggle: Toggle::Browser, connector: Connector::Browser });
    // A player that shares what it plays only once told to; no card where it is not installed.
    if let Some(shares) = model.mpc {
        let state = match (c.mpc.enabled, shares) {
            (false, _) => off(),
            (true, true) => set_up(tr("Followed like any player", "Suivi comme tout lecteur")),
            (true, false) => not_set_up(tr("its media controls", "ses contrôles multimédias")),
        };
        cards.push(ConnectorCard { glyph: '\u{E714}', title: "MPC-HC", state, on: c.mpc.enabled, toggle: Toggle::Mpc, connector: Connector::Mpc });
    }
    let discord_on = s.connected(discord_rpc::NAME) || s.connected(discord::NAME);
    let keys = c.discord.client_id.is_some() && c.discord.client_secret.is_some();
    let state = match (c.discord.enabled, discord_on, keys && s.discord_problem.is_none()) {
        (false, _, _) => off(),
        (true, true, _) => set_up(tr("Connected", "Connecté")),
        (true, false, true) => (tr("Set up, waiting for Discord", "Configuré, en attente de Discord").to_string(), Tone::Plain),
        (true, false, false) => not_set_up(if keys { tr("see its steps", "voir ses étapes") } else { tr("an application of your own", "une application à vous") }),
    };
    cards.push(ConnectorCard { glyph: '\u{E8BD}', title: tr("Discord calls", "Appels Discord"), state, on: c.discord.enabled, toggle: Toggle::Discord, connector: Connector::Discord });
    let (api, window, paired) = (s.connected(teams::NAME), s.connected(teams_ui::NAME), c.teams.token.is_some());
    let state = match (c.teams.enabled, api, paired, window) {
        (false, ..) => off(),
        (true, true, true, _) => set_up(tr("Connected", "Connecté")),
        (true, _, _, true) => set_up(tr("Following the meeting window", "Suit la fenêtre de réunion")),
        (true, true, false, _) => not_set_up(tr("allow it in a meeting", "autorisez-le en réunion")),
        (true, false, true, false) => (tr("Set up, waiting for Teams", "Configuré, en attente de Teams").to_string(), Tone::Plain),
        (true, false, false, false) => not_set_up(tr("its API is off", "son API est désactivée")),
    };
    cards.push(ConnectorCard { glyph: '\u{E716}', title: tr("Teams meetings", "Réunions Teams"), state, on: c.teams.enabled, toggle: Toggle::Teams, connector: Connector::Teams });

    col.connector_grid(&cards, model.open);
    if let Some(card) = model.open.and_then(|open| cards.iter().find(|card| card.connector == open)) {
        col.section(card.title);
        connector_steps(col, model, card.connector);
    } else {
        col.laid.push(R::new(col.x, col.y, col.w, 20.0), Kind::Text(tr("Click a connector for its steps and options.", "Cliquez sur un connecteur pour ses étapes et options.").into()));
        col.y += 24.0;
    }
    youtube_music(col, model);
}

/// What an opened connector shows under the grid: its setup steps until it is set up, then what can
/// still be changed. While it is off, all of it is dimmed and moot, its buttons too: its switch is
/// right above, on its card.
fn connector_steps(col: &mut Column, model: &Model, connector: Connector) {
    use crate::integrations::{browser, discord, discord_rpc, teams};
    let c = &model.config;
    let s = &model.status;
    let mut body = col.group();
    match connector {
        Connector::Browser => {
            let gate = Gate::from(!c.browser.enabled);
            if !s.connected(browser::NAME) {
                let why = model.hint_for(Button::ExtensionFolder).or(plain(tr("Opens the extensions page, and the folder to Load unpacked (its path copied)", "Ouvre la page des extensions, et le dossier à charger décompressé (son chemin copié)")));
                col.row(&mut body, Some((1, false)), tr("Install the extension", "Installer l’extension"), why, gate, Control::Button(RowButton::new(open_label(), Button::ExtensionFolder).accent(true)));
                col.row(&mut body, Some((2, false)), tr("Play something in a tab", "Lisez quelque chose dans un onglet"), plain(tr("The extension connects with the first video or song", "L’extension se connecte à la première vidéo ou au premier morceau")), gate, Control::None);
            }
            let labels = vec![tr("Every site", "Tous les sites").to_string(), tr("Chosen sites", "Sites choisis").to_string()];
            col.row(&mut body, None, tr("Tabs followed", "Onglets suivis"), plain(tr("Every tab that plays, or only the sites below", "Chaque onglet qui joue, ou seulement les sites ci-dessous")), gate, Control::Segments(labels, usize::from(!c.browser.every_site), Choice::BrowserTabs));
            // One switch per site, so more can be offered later.
            for (i, (_, name, _)) in browser::SITES.iter().enumerate() {
                col.row(&mut body, None, name, None, Gate::from(gate.dim() || c.browser.every_site), Control::Toggle(Toggle::Site(i)));
            }
        }
        Connector::Mpc => {
            let gate = Gate::from(!c.mpc.enabled);
            if model.mpc == Some(true) {
                col.row(&mut body, None, tr("Its Windows media controls are on", "Ses contrôles multimédias Windows sont activés"), plain(tr("What it plays is shared with Windows; Off here only leaves it out of Audio Tray", "Ce qu’il lit est partagé avec Windows ; Désactivé ici l’écarte seulement d’Audio Tray")), gate, Control::None);
            } else {
                col.row(&mut body, Some((1, false)), tr("Close MPC-HC", "Fermer MPC-HC"), plain(tr("It saves its own settings as it closes, over any change made meanwhile", "Il enregistre ses paramètres en se fermant, par-dessus toute modification faite entre-temps")), gate, Control::None);
                let why = model.hint_for(Button::Mpc).or(plain(tr("Its Options > Player > Control via Windows UI (SMTC)", "Ses Options > Player > Control via Windows UI (SMTC)")));
                col.row(&mut body, Some((2, false)), tr("Turn on its Windows media controls", "Activer ses contrôles multimédias Windows"), why, gate, Control::Button(RowButton::new(tr("Turn on", "Activer"), Button::Mpc).accent(true)));
            }
        }
        Connector::Discord => {
            let gate = Gate::from(!c.discord.enabled);
            let discord_on = s.connected(discord_rpc::NAME) || s.connected(discord::NAME);
            let has_id = c.discord.client_id.is_some();
            let keys = has_id && c.discord.client_secret.is_some();
            if !(keys && s.discord_problem.is_none()) && !discord_on {
                if let Some(problem) = s.discord_problem.filter(|_| keys) {
                    col.row(&mut body, None, tr("Not connected yet", "Pas encore connecté"), Some((problem.to_string(), Tone::Warn)), gate, Control::None);
                }
                let create = plain(tr("Discord shares calls only with an app of your own: make one on its developer portal", "Discord ne partage les appels qu’avec votre propre application : créez-en une sur son portail développeur"));
                col.row(&mut body, Some((1, has_id)), tr("Create an application", "Créer une application"), create, gate, Control::Button(RowButton::new(tr("Open portal", "Ouvrir le portail"), Button::DiscordPortal).accent(!has_id).glyph('\u{E8A7}')));
                let redirect = model.hint_for(Button::DiscordRedirect).or(plain(&tf!("In OAuth2 > Redirects, add {}", "Dans OAuth2 > Redirects, ajoutez {}", discord_rpc::REDIRECT)));
                col.row(&mut body, Some((2, false)), tr("Add the redirect", "Ajouter la redirection"), redirect, gate, Control::Button(RowButton::new(tr("Copy", "Copier"), Button::DiscordRedirect).glyph('\u{E8C8}')));
                col.row(&mut body, Some((3, keys)), tr("Paste its keys", "Coller ses clés"), plain(tr("From OAuth2: copy each one, then click Paste beside it", "Depuis OAuth2 : copiez chacune, puis cliquez sur Coller en face")), gate, Control::None);
            }
            let id = model.hint_for(Button::DiscordId).or_else(|| c.discord.client_id.clone().map(|id| (id, Tone::Plain)));
            col.row(&mut body, None, tr("Application ID", "ID d’application"), id.or(not_set()), gate, Control::Button(RowButton::new(paste_label(), Button::DiscordId).glyph('\u{E77F}')));
            let secret = model.hint_for(Button::DiscordSecret).or_else(|| c.discord.client_secret.as_deref().map(|s| (masked_tail(s), Tone::Plain)));
            col.row(&mut body, None, tr("Client secret", "Secret client"), secret.or(not_set()), gate, Control::Button(RowButton::new(paste_label(), Button::DiscordSecret).glyph('\u{E77F}')));
        }
        Connector::Teams => {
            let gate = Gate::from(!c.teams.enabled);
            if c.teams.token.is_some() {
                col.row(&mut body, None, tr("Audio Tray is allowed", "Audio Tray est autorisé"), plain(tr("Teams asked once, in a meeting", "Teams l’a demandé une fois, en réunion")), gate, Control::None);
            } else {
                if let Some(problem) = s.teams_problem {
                    col.row(&mut body, None, tr("Not connected yet", "Pas encore connecté"), Some((problem.to_string(), Tone::Warn)), gate, Control::None);
                }
                col.row(&mut body, Some((1, s.connected(teams::NAME))), tr("Turn on Teams' API", "Activer l’API de Teams"), plain(tr("In Teams: Settings > Privacy > Third-party app API", "Dans Teams : Paramètres > Confidentialité > API d’application tierce")), gate, Control::None);
                col.row(&mut body, Some((2, false)), tr("Allow Audio Tray", "Autoriser Audio Tray"), plain(tr("In a meeting, when Teams asks", "En réunion, quand Teams le demande")), gate, Control::None);
                col.row(&mut body, None, tr("Or with no setup", "Ou sans configuration"), plain(tr("Without its API, Teams' meeting window is followed: the mute, not who talks", "Sans son API, la fenêtre de réunion de Teams est suivie : le micro coupé, pas qui parle")), gate, Control::None);
            }
        }
    }
    col.end_group(body);
}

/// YouTube Music: followed or not, and what its taskbar button shows.
fn youtube_music(col: &mut Column, model: &Model) {
    let c = &model.config;
    col.section("YouTube Music");
    col.toggle(
        Some('\u{EC4F}'),
        tr("Follow YouTube Music", "Suivre YouTube Music"),
        plain(tr("Its progress on the player's taskbar button", "Sa progression sur le bouton du lecteur dans la barre des tâches")),
        Gate::Open,
        Toggle::Music,
    );
    let styles = [
        (TileStyle::Off, tr("Icon only", "Icône seule"), tr("The player's own icon", "L’icône du lecteur")),
        (TileStyle::Cover, tr("Cover", "Pochette"), tr("The song's cover in its button", "La pochette du morceau dans son bouton")),
        (TileStyle::Full, tr("Full", "Complet"), tr("The cover and the title", "La pochette et le titre")),
    ];
    let tiles = choice_tiles(&styles, &TILE_STYLES, c.music.tile_style, Picture::PlayerButton, Choice::Tile);
    let off = Gate::needs(!c.music.enabled, tr("Needs YouTube Music followed, above", "Nécessite le suivi de YouTube Music, ci-dessus"), None);
    col.tiles(Some((tr("Player's button", "Bouton du lecteur"), tr("In the taskbar", "Dans la barre des tâches"))), &tiles, off, BUTTON_TILE_H);
}

#[cfg(test)]
mod tests {
    use super::super::super::tests::{measure, model};
    use super::super::super::{page, Page, Target};
    use super::*;

    #[test]
    fn a_grouped_card_holds_its_rows_and_their_buttons_work() {
        let mut m = model(Page::Connectors);
        m.open = Some(Connector::Discord);
        let laid = page(&m, 900.0, &measure);
        let copy = laid.items.iter().find(|i| matches!(i.kind, Kind::Button { target: Target::Button(Button::DiscordRedirect), .. })).unwrap().r;
        assert_eq!(laid.hit(copy.x + 4.0, copy.y + 4.0), Some(Target::Button(Button::DiscordRedirect)));
        let group = laid.items.iter().find(|i| matches!(&i.kind, Kind::Card { title, .. } if title.is_empty()) && i.r.contains(copy.x, copy.y)).unwrap().r;
        let rows: Vec<R> = laid
            .items
            .iter()
            .filter(|i| matches!(i.kind, Kind::Label { .. }) && i.r.y >= group.y && i.r.bottom() <= group.bottom() + 0.5)
            .map(|i| i.r)
            .collect();
        assert_eq!(rows.len(), 5, "three steps, two keys");
        let beside = rows.iter().find(|r| r.contains(r.x, copy.y + copy.h / 2.0)).unwrap();
        assert!(beside.right() <= copy.x, "a row's text stops before its button");
        // Off, its steps are moot: drawn, not a click away.
        m.config.discord.enabled = false;
        let laid = page(&m, 900.0, &measure);
        assert!(laid.items.iter().any(|i| matches!(i.kind, Kind::Button { target: Target::Button(Button::DiscordRedirect), dim: true, .. })));
        assert!(!laid.hits.iter().any(|(_, t)| matches!(t, Target::Button(Button::DiscordRedirect | Button::DiscordId | Button::DiscordSecret))));
    }
}
