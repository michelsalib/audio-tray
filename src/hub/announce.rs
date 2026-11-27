//! How the hub's news is told: Audio Tray's own Windows notifications (shown by its banners while
//! they replace Windows', by Windows' else), or a glance that is not kept.

use crate::config::Config;
use crate::toast::{Outgoing, TAG};

use super::Notice;

/// A notice that is news (never the pill's: it is about audio activity alone): a Windows
/// notification of Audio Tray's own (listed with the others and shown by Audio Tray's banners while
/// they replace Windows', by Windows' else), or for a layout switch a glance that is not kept. A new
/// song is the tray's to announce (it needs the session's cover).
pub(crate) fn hand_over(notice: &Notice, config: &Config) {
    use crate::lang::tr;
    // Battery alerts as Settings has them.
    let b = &config.batteries;
    if matches!(notice, Notice::LowBattery { .. }) && !(b.enabled && b.toast) {
        return;
    }
    let plain = |title: String, body: String, tag: String| Outgoing { title, body, link: None, reminder: false, tag };
    let outgoing = match notice {
        Notice::KeyboardSwitched { keyboard, layout } => {
            crate::calendar::glance(layout, keyboard);
            return;
        }
        // Each tagged with what it is (`toast::TAG`), for the banner that draws it.
        Notice::AudioMoved { to, from } => plain(
            crate::tf!("Sound now on {to}", "Son sur {to}"),
            crate::tf!("{from} disconnected.", "{from} est déconnecté."),
            format!("{TAG}audio"),
        ),
        Notice::LowBattery { name, level, .. } => low_battery(name, *level),
        // Its start in its tag: its banner counts down to it.
        Notice::Meeting { title, line, url, start } => Outgoing {
            title: title.clone(),
            body: line.clone(),
            link: url.clone().map(|url| (tr("Join", "Rejoindre").to_string(), url)),
            reminder: true,
            tag: format!("{TAG}meeting/{start}"),
        },
        Notice::FocusDone { minutes } => plain(
            tr("Focus done", "Concentration terminée").to_string(),
            crate::tf!("{minutes} minutes of focus. Take a break.", "{minutes} min de concentration. Faites une pause."),
            format!("{TAG}focus/{minutes}"),
        ),
        Notice::NewTrack { .. } => return,
    };
    // Off the tray thread: Windows' notification platform can take its time.
    crate::toast::send(outgoing);
}

/// A device's low battery, as a notification in Windows' language: tagged (`toast::TAG`) so the
/// banner draws its battery, one per device (two low at once are two notifications), a device's
/// next warning replacing its last (the level is in the words, not the tag: the banner reads it
/// back from them, a test there holds the two together).
pub(crate) fn low_battery(name: &str, level: u8) -> Outgoing {
    let device = crate::icons::name_hash(name);
    Outgoing {
        title: crate::tf!("{name} battery low", "Batterie faible : {name}"),
        body: crate::tf!("{level}% left. Time to charge it.", "Il reste {level} %. Pensez à la recharger."),
        link: None,
        reminder: false,
        tag: format!("{TAG}battery/{device:x}"),
    }
}
