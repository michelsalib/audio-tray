//! The Batteries page: the devices reporting one, each with its level, then where they show and
//! when one counts as low.

use crate::lang::tr;

use super::super::column::{Column, Gate, Switch};
use super::super::{plain, Choice, Kind, Model, Page, Toggle, Tone, CARD_H, CARD_PAD, R};
use super::{needs_tap, note};

/// A battery level's bar and number.
const LEVEL_W: f32 = 180.0;

/// The batteries: the devices reporting one first, each with its level, then where they show and
/// when one counts as low.
pub(in crate::settings::layout) fn batteries(col: &mut Column, model: &Model) {
    let b = &model.config.batteries;
    col.toggle(
        Some('\u{E83F}'),
        tr("Battery levels", "Niveaux de batterie"),
        plain(tr("Keyboards, mice, headsets and other wireless devices that report one", "Claviers, souris, casques et autres appareils sans fil qui la signalent")),
        Gate::Open,
        Toggle::Batteries,
    );
    let off = !b.enabled;
    col.section(tr("Connected now", "Connectés maintenant"));
    let batteries = &model.status.batteries;
    if batteries.is_empty() {
        let why = if off { tr("Turned off", "Désactivé") } else { tr("Bluetooth keyboards, mice and headsets report one once connected", "Les claviers, souris et casques Bluetooth la signalent une fois connectés") };
        col.card(None, tr("No device reports a battery", "Aucun appareil ne signale de batterie"), plain(why), true, None);
    }
    // Lowest first: the one to charge.
    let mut sorted: Vec<_> = batteries.iter().collect();
    sorted.sort_by_key(|x| x.level);
    for battery in sorted {
        let low = battery.level <= b.low_at;
        let desc = low.then(|| (tr("Low: charge it soon", "Faible : rechargez-le bientôt").to_string(), Tone::Warn));
        let card = col.card(Some(battery.kind.glyph()), battery.name.clone(), desc, off, None);
        let r = R::new(card.right() - CARD_PAD - LEVEL_W, card.y, LEVEL_W, CARD_H);
        col.laid.push(r, Kind::Level { level: battery.level, low });
    }
    col.section(tr("Where they show", "Où ils s’affichent"));
    let c = &model.config;
    use crate::batteries::Presence;
    let laptop = crate::batteries::presence() == Presence::Battery;
    // Moved to the battery icon, a switch reads off and says so: it does not show the levels.
    let moved = !c.levels_in_audio();
    // On a laptop, Windows' icon is taken by the code drawn into the taskbar: with none, the
    // switch would do nothing.
    let icon_gate = if off { Gate::Off } else { Gate::needs(laptop && !c.injects(), needs_tap(), Some(Page::Taskbar)) };
    // The audio surfaces (the panel, the audio icon's tooltip): no levels followed, or their home
    // is a battery icon.
    let audio_gate = if off {
        Gate::Needs(tr("Needs battery levels", "Nécessite les niveaux de batterie"), None)
    } else {
        Gate::needs(moved, tr("On the battery icon instead", "Sur l’icône de batterie à la place"), None)
    };
    col.toggle_grid(&[
        Switch::new(c, tr("On the battery icon", "Sur l’icône de batterie"), tr("Its levels on a click and in its tooltip", "Ses niveaux au clic et dans l’info-bulle"), icon_gate, Toggle::BatteryTaskbarIcon),
        Switch { on: b.flyout && !moved, ..Switch::new(c, tr("In the audio panel", "Dans le panneau audio"), tr("Under the audio devices", "Sous les périphériques audio"), audio_gate, Toggle::BatteryFlyout) },
        Switch { on: b.tooltip && !moved, ..Switch::new(c, tr("In the audio icon's tooltip", "Dans l’info-bulle de l’icône audio"), tr("Lowest first", "Le plus faible en premier"), audio_gate, Toggle::BatteryTooltip) },
        Switch::new(c, tr("Notify when low", "Notification si faible"), tr("Once, until it is charged again", "Une fois, jusqu’à la prochaine charge"), off, Toggle::BatteryToast),
    ]);
    note(
        col,
        if laptop {
            tr("With the battery icon on, its levels leave the audio panel and the audio icon's tooltip", "Avec l’icône de batterie, les niveaux quittent le panneau audio et l’info-bulle de l’icône audio")
        } else {
            tr("Without a battery of its own, this PC gets Audio Tray's battery icon while a device reports one", "Sans batterie, ce PC reçoit l’icône de batterie d’Audio Tray tant qu’un appareil en signale une")
        },
    );
    let percent = crate::calendar::words().percent;
    let labels: Vec<String> = crate::config::Batteries::LOW_LEVELS.iter().map(|level| percent(*level)).collect();
    let at = crate::config::Batteries::LOW_LEVELS.iter().position(|level| *level == b.low_at).unwrap_or(2);
    col.segments(tr("Low at", "Faible à"), plain(tr("At or under this level", "À ce niveau ou en dessous")), &labels, at, off, Choice::LowBattery);
    // Audio Tray's own battery icon: only where Windows gives none (a desktop); a laptop never
    // shows it, so it has nothing to choose.
    if laptop {
        return;
    }
    let styles = [tr("Bars", "Barres").to_string(), tr("Rings", "Anneaux").to_string()];
    let style = crate::config::BatteryIconStyle::ALL.iter().position(|s| *s == b.icon_style).unwrap_or(0);
    let gate = Gate::needs(off || !b.taskbar_icon, tr("Needs the battery icon, above", "Nécessite l’icône de batterie, ci-dessus"), None);
    col.segments(tr("Battery icon style", "Style de l’icône de batterie"), plain(tr("A bar or a ring per device", "Une barre ou un anneau par appareil")), &styles, style, gate, Choice::BatteryIconStyle);
}
