//! How the hub's news is told: a glance that is not kept (a layout switch, the sound moved, a
//! battery low, focus done), or Audio Tray's own Windows notification (a meeting; and the glances'
//! news while glances keep quiet), shown by its banners while they replace Windows', by Windows'
//! else.

use crate::config::Config;
use crate::toast::{Outgoing, TAG};

use super::Notice;

/// A notice that is news (never the pill's: it is about audio activity alone). A layout switch is
/// a glance, dropped while glances keep quiet. The sound moved, a battery low and focus done are
/// glances in their colours too, but while glances keep quiet (Do not disturb, a full-screen app,
/// the open panel) they go out as notifications instead, which the list keeps and the digest sums
/// up: none is lost. A meeting is always a notification (it counts down, and has its Join). A new
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
        _ if glanced(notice) => return,
        // Each tagged with what it is (`toast::TAG`), for the banner that draws it.
        Notice::AudioMoved { to, from } => plain(moved_title(to), moved_body(from), format!("{TAG}audio")),
        Notice::LowBattery { name, level, .. } => low_battery(name, *level),
        // Its start in its tag: its banner counts down to it.
        Notice::Meeting { title, line, url, start } => Outgoing {
            title: title.clone(),
            body: line.clone(),
            link: url.clone().map(|url| (tr("Join", "Rejoindre").to_string(), url)),
            reminder: true,
            tag: format!("{TAG}meeting/{start}"),
        },
        Notice::FocusDone { minutes } => plain(focus_title(), focus_body(*minutes), format!("{TAG}focus/{minutes}")),
        Notice::NewTrack { .. } => return,
    };
    // Off the tray thread: Windows' notification platform can take its time.
    crate::toast::send(outgoing);
}

/// The sound moved, a battery low or focus done told by a glance in its colour, with the words
/// its notification would have; `false`: not one of those, or glances keep quiet now.
fn glanced(notice: &Notice) -> bool {
    use crate::calendar::{notice as glance, Note};
    let said = |o: Outgoing, note: Note| glance(&o.title, &o.body, note);
    match notice {
        Notice::AudioMoved { to, from } => glance(&moved_title(to), &moved_body(from), Note::Audio),
        Notice::LowBattery { name, level, .. } => said(low_battery(name, *level), Note::Battery(*level)),
        Notice::FocusDone { minutes } => glance(&focus_title(), &focus_body(*minutes), Note::Focus),
        _ => false,
    }
}

fn moved_title(to: &str) -> String {
    crate::tf!("Sound now on {to}", "Son sur {to}")
}

fn moved_body(from: &str) -> String {
    crate::tf!("{from} disconnected.", "{from} est déconnecté.")
}

fn focus_title() -> String {
    crate::lang::tr("Focus done", "Concentration terminée").to_string()
}

fn focus_body(minutes: u32) -> String {
    crate::tf!("{minutes} minutes of focus. Take a break.", "{minutes} min de concentration. Faites une pause.")
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
