//! The panel's words, in Windows' display language: French or English (dates and times come from
//! the locale already, [`super::locale`]; a panel saying "Coming up" over "Mercredi 18 novembre"
//! read as two languages).

/// Every word the panel says that is not a date or a time.
pub struct Words {
    pub join: &'static str,
    pub reply: &'static str,
    pub notifications: &'static str,
    pub clear_all: &'static str,
    pub no_notifications: &'static str,
    pub not_allowed: &'static str,
    pub do_not_disturb: &'static str,
    pub turn_off_dnd: &'static str,
    pub previous_month: &'static str,
    pub next_month: &'static str,
    pub settings: &'static str,
    /// The footers' links to Windows' own settings.
    pub date_settings: &'static str,
    pub notification_settings: &'static str,
    pub back: &'static str,
    pub dismiss: &'static str,
    pub clear_app: &'static str,
    pub more_options: &'static str,
    pub send: &'static str,
    pub change_clocks: &'static str,
    pub go_to_today: &'static str,
    pub connect: &'static str,
    pub loading: &'static str,
    pub directions: &'static str,
    pub coming_up: &'static str,
    pub nothing_else: &'static str,
    pub nothing: &'static str,
    pub no_events: &'static str,
    pub today: &'static str,
    pub all_day: &'static str,
    pub today_all_day: &'static str,
    pub now: &'static str,
    pub open: &'static str,
    pub open_google: &'static str,
    pub open_outlook: &'static str,
    pub calendar: &'static str,
    pub dnd_refused_on: &'static str,
    pub dnd_refused_off: &'static str,
    pub timer_not_quiet: &'static str,
    pub app_did_not_take: &'static str,
    /// "Focus 25 min", the timer to start.
    pub focus: fn(u32) -> String,
    /// "Stop · 24 min left", the timer running.
    pub focus_stop: fn(i64) -> String,
    /// "Cleared 14 notifications".
    pub cleared: fn(usize) -> String,
    /// "Next · in 12 min", "Next · in 2 h 05".
    pub next_in: fn(i64) -> String,
    /// "Next · Monday".
    pub next_on: fn(&str) -> String,
    /// "Now · until 21:03".
    pub now_until: fn(&str) -> String,
    /// "you: accepted".
    pub you: fn(&str) -> String,
    /// "12 min" ago.
    pub minutes: fn(i64) -> String,
    /// A conversation's banner: how many more came.
    pub more: fn(usize) -> String,
    /// A reply or reaction sent.
    pub sent: &'static str,
    /// The digest after a quiet time: its title, its count, its button.
    pub away: &'static str,
    pub away_count: fn(usize) -> String,
    pub latest_from: fn(&str) -> String,
    pub show_them: &'static str,
    /// An app listening outside a call, and what a click on its alert does.
    pub listening: fn(&str) -> String,
    pub click_to_mute: &'static str,
    /// A meeting's countdown, and once it has begun.
    pub starts_in: fn(i64) -> String,
    pub started: fn(i64) -> String,
    /// The batteries' card: its title, this PC's line, the footer's link, and with no device.
    pub batteries: &'static str,
    pub this_pc: &'static str,
    pub charging: &'static str,
    pub plugged_in: &'static str,
    pub on_battery: &'static str,
    pub energy_saver: &'static str,
    pub power_settings: &'static str,
    pub no_devices: &'static str,
    /// A level at or under the low one, said in words (not by its colour alone).
    pub low: &'static str,
    pub fully_charged: &'static str,
    /// The footer's link on a PC without a battery: the devices' own page.
    pub bluetooth_settings: &'static str,
    /// Audio Tray's own battery icon's menu.
    pub show_batteries: &'static str,
    pub battery_settings: &'static str,
    /// "2 h 05 min left", from minutes.
    pub time_left: fn(u32) -> String,
    /// "43%" (French puts a space before it).
    pub percent: fn(u8) -> String,
    /// "Name: 43%" (French puts a space before the colon).
    pub labelled: fn(&str, &str) -> String,
    /// A reminder's "in 5 min".
    pub in_minutes: fn(i64) -> String,
    /// An event's length, from minutes: "45 min", "2 h", "1 h 30".
    pub duration: fn(i64) -> String,
    /// A source that failed, said over the list ("Outlook: sign in again"): a link without a name,
    /// and what is wrong.
    pub a_calendar_link: &'static str,
    pub sign_in_again: &'static str,
    pub offline: &'static str,
    pub could_not_refresh: &'static str,
}

static EN: Words = Words {
    join: "Join",
    reply: "Reply",
    notifications: "Notifications",
    clear_all: "Clear all",
    no_notifications: "No new notifications",
    not_allowed: "Allow Audio Tray to read notifications",
    do_not_disturb: "Do not disturb",
    turn_off_dnd: "Turn off Do not disturb",
    previous_month: "Previous month",
    next_month: "Next month",
    settings: "Settings",
    date_settings: "Date & time settings",
    notification_settings: "Notification settings",
    back: "Back",
    dismiss: "Dismiss",
    clear_app: "Clear all from this app",
    more_options: "More options",
    send: "Send",
    change_clocks: "Change the clocks",
    go_to_today: "Go to today",
    connect: "Connect a calendar in Settings",
    loading: "Loading…",
    directions: "Directions",
    coming_up: "Coming up",
    nothing_else: "Nothing else in the next 7 days",
    nothing: "Nothing in the next 7 days",
    no_events: "No events",
    today: "Today",
    all_day: "All day",
    today_all_day: "Today · all day",
    now: "now",
    open: "Open",
    open_google: "Open in Google Calendar",
    open_outlook: "Open in Outlook",
    calendar: "Calendar",
    dnd_refused_on: "Windows would not turn on Do not disturb",
    dnd_refused_off: "Windows would not turn off Do not disturb",
    timer_not_quiet: "Timer on; Windows kept notifications on",
    app_did_not_take: "The app did not take it: open the app to finish",
    focus: |m| format!("Focus {m} min"),
    focus_stop: |m| format!("Stop · {m} min left"),
    cleared: |n| if n == 1 { "Cleared 1 notification".into() } else { format!("Cleared {n} notifications") },
    next_in: |m| if m < 60 { format!("Next · in {m} min") } else { format!("Next · in {} h {:02}", m / 60, m % 60) },
    next_on: |day| format!("Next · {day}"),
    now_until: |end| format!("Now · until {end}"),
    you: |answer| format!("you: {answer}"),
    minutes: |m| format!("{m} min"),
    more: |n| format!("+{n} more"),
    sent: "Sent",
    away: "While you were away",
    away_count: |n| if n == 1 { "1 notification".into() } else { format!("{n} notifications") },
    latest_from: |who| format!("Latest from {who}"),
    show_them: "Show them",
    listening: |app| format!("{app} is listening"),
    click_to_mute: "Click to mute",
    starts_in: |s| format!("Starts in {}:{:02}", s / 60, s % 60),
    started: |m| if m < 1 { "Starting now".into() } else { format!("Started {m} min ago") },
    batteries: "Batteries",
    this_pc: "This PC",
    charging: "Charging",
    plugged_in: "Plugged in",
    on_battery: "On battery",
    energy_saver: "Energy saver on",
    power_settings: "Power & battery",
    no_devices: "No connected device reports its battery",
    low: "low",
    fully_charged: "Fully charged",
    bluetooth_settings: "Bluetooth & devices",
    show_batteries: "Show batteries",
    battery_settings: "Battery settings",
    time_left: |m| if m < 60 { format!("{m} min left") } else { format!("{} h {:02} min left", m / 60, m % 60) },
    percent: |n| format!("{n}%"),
    labelled: |name, value| format!("{name}: {value}"),
    in_minutes: |m| format!("in {m} min"),
    duration: duration_words,
    a_calendar_link: "A calendar link",
    sign_in_again: "sign in again",
    offline: "offline",
    could_not_refresh: "could not refresh",
};

static FR: Words = Words {
    join: "Rejoindre",
    reply: "Répondre",
    notifications: "Notifications",
    clear_all: "Tout effacer",
    no_notifications: "Aucune nouvelle notification",
    not_allowed: "Autoriser Audio Tray à lire les notifications",
    do_not_disturb: "Ne pas déranger",
    turn_off_dnd: "Désactiver Ne pas déranger",
    previous_month: "Mois précédent",
    next_month: "Mois suivant",
    settings: "Paramètres",
    date_settings: "Paramètres de date et d'heure",
    notification_settings: "Paramètres de notification",
    back: "Retour",
    dismiss: "Ignorer",
    clear_app: "Tout effacer pour cette application",
    more_options: "Plus d'options",
    send: "Envoyer",
    change_clocks: "Changer les horloges",
    go_to_today: "Aller à aujourd'hui",
    connect: "Connecter un calendrier dans les paramètres",
    loading: "Chargement…",
    directions: "Itinéraire",
    coming_up: "À venir",
    nothing_else: "Rien d'autre dans les 7 prochains jours",
    nothing: "Rien dans les 7 prochains jours",
    no_events: "Aucun événement",
    today: "Aujourd'hui",
    all_day: "Toute la journée",
    today_all_day: "Aujourd'hui · toute la journée",
    now: "maintenant",
    open: "Ouvrir",
    open_google: "Ouvrir dans Google Agenda",
    open_outlook: "Ouvrir dans Outlook",
    calendar: "Calendrier",
    dnd_refused_on: "Windows n'a pas activé Ne pas déranger",
    dnd_refused_off: "Windows n'a pas désactivé Ne pas déranger",
    timer_not_quiet: "Minuteur lancé ; Windows garde les notifications",
    app_did_not_take: "L'application ne l'a pas pris : ouvrez-la pour finir",
    focus: |m| format!("Concentration {m} min"),
    focus_stop: |m| format!("Arrêter · {m} min"),
    cleared: |n| if n == 1 { "1 notification effacée".into() } else { format!("{n} notifications effacées") },
    next_in: |m| if m < 60 { format!("Prochain · dans {m} min") } else { format!("Prochain · dans {} h {:02}", m / 60, m % 60) },
    next_on: |day| format!("Prochain · {day}"),
    now_until: |end| format!("En cours · jusqu'à {end}"),
    you: |answer| format!("vous : {answer}"),
    minutes: |m| format!("{m} min"),
    more: |n| format!("+{n} autres"),
    sent: "Envoyé",
    away: "Pendant votre absence",
    away_count: |n| if n == 1 { "1 notification".into() } else { format!("{n} notifications") },
    latest_from: |who| format!("Dernière de {who}"),
    show_them: "Les afficher",
    listening: |app| format!("{app} écoute"),
    click_to_mute: "Cliquer pour couper le micro",
    starts_in: |s| format!("Commence dans {}:{:02}", s / 60, s % 60),
    started: |m| if m < 1 { "Commence maintenant".into() } else { format!("Commencée il y a {m} min") },
    batteries: "Batteries",
    this_pc: "Ce PC",
    charging: "En charge",
    plugged_in: "Branché",
    on_battery: "Sur batterie",
    energy_saver: "Économiseur d'énergie activé",
    power_settings: "Alimentation et batterie",
    no_devices: "Aucun appareil connecté n'indique sa batterie",
    low: "faible",
    fully_charged: "Entièrement chargée",
    bluetooth_settings: "Bluetooth et appareils",
    show_batteries: "Afficher les batteries",
    battery_settings: "Paramètres des batteries",
    time_left: |m| if m < 60 { format!("{m} min restantes") } else { format!("{} h {:02} min restantes", m / 60, m % 60) },
    percent: |n| format!("{n}\u{a0}%"),
    labelled: |name, value| format!("{name}\u{a0}: {value}"),
    in_minutes: |m| format!("dans {m} min"),
    duration: duration_words,
    a_calendar_link: "Un lien d'agenda",
    sign_in_again: "reconnectez-vous",
    offline: "hors ligne",
    could_not_refresh: "actualisation impossible",
};

/// "1 h 30", "45 min", "2 h": the same in both languages.
fn duration_words(minutes: i64) -> String {
    match (minutes / 60, minutes % 60) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m:02}"),
    }
}

impl Words {
    /// A battery's level, "low" said in words where it is ("12% (low)"): not by its colour alone.
    pub fn level(&self, level: u8, low: bool) -> String {
        if low {
            format!("{} ({})", (self.percent)(level), self.low)
        } else {
            (self.percent)(level)
        }
    }

    /// This PC's battery's state, each part that holds ("Charging · Energy saver on").
    pub fn pc_state(&self, pc: &crate::batteries::SystemBattery) -> String {
        let first = if pc.charging {
            self.charging.to_string()
        } else if pc.plugged && pc.level >= 100 {
            self.fully_charged.to_string()
        } else if pc.plugged {
            self.plugged_in.to_string()
        } else {
            pc.left.map_or_else(|| self.on_battery.to_string(), |secs| (self.time_left)(secs / 60))
        };
        if pc.saver {
            format!("{first} · {}", self.energy_saver)
        } else {
            first
        }
    }
}

/// The English words, for tests that check a wording.
#[cfg(test)]
pub fn english() -> &'static Words {
    &EN
}

/// The words in Windows' display language (French for any French, else English).
pub fn words() -> &'static Words {
    // The rule every panel and Settings follow ([`crate::lang`]).
    if crate::lang::french() {
        &FR
    } else {
        &EN
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn both_languages_say_the_same_things() {
        for words in [&super::EN, &super::FR] {
            assert!(!words.join.is_empty() && !words.clear_all.is_empty());
            assert!((words.next_in)(125).contains("2 h 05"));
            assert!((words.cleared)(3).contains('3'));
            assert!((words.in_minutes)(5).contains("5 min"));
            assert_eq!((words.duration)(90), "1 h 30");
            assert!(!words.sign_in_again.is_empty() && !words.offline.is_empty() && !words.could_not_refresh.is_empty());
        }
    }
}
