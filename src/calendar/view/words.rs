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
    pub show_all: &'static str,
    pub back_to_calendar: &'static str,
    pub show_week: &'static str,
    pub show_month: &'static str,
    pub previous_month: &'static str,
    pub next_month: &'static str,
    pub settings: &'static str,
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
    /// "3 more from Teams".
    pub more_from: fn(usize, &str) -> String,
    /// "Focus 25 min", the timer to start.
    pub focus: fn(u32) -> String,
    /// "Stop · 24 min left", the timer running.
    pub focus_stop: fn(i64) -> String,
    /// "Focus · 24 min left", on the taskbar clock while it runs.
    pub focus_left: fn(i64) -> String,
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
    show_all: "Show all notifications",
    back_to_calendar: "Back to the calendar",
    show_week: "Show the week",
    show_month: "Show the month",
    previous_month: "Previous month",
    next_month: "Next month",
    settings: "Settings",
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
    more_from: |n, app| format!("{n} more from {app}"),
    focus: |m| format!("Focus {m} min"),
    focus_stop: |m| format!("Stop · {m} min left"),
    focus_left: |m| format!("Focus · {m} min left"),
    cleared: |n| if n == 1 { "Cleared 1 notification".into() } else { format!("Cleared {n} notifications") },
    next_in: |m| if m < 60 { format!("Next · in {m} min") } else { format!("Next · in {} h {:02}", m / 60, m % 60) },
    next_on: |day| format!("Next · {day}"),
    now_until: |end| format!("Now · until {end}"),
    you: |answer| format!("you: {answer}"),
    minutes: |m| format!("{m} min"),
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
    show_all: "Afficher toutes les notifications",
    back_to_calendar: "Revenir au calendrier",
    show_week: "Afficher la semaine",
    show_month: "Afficher le mois",
    previous_month: "Mois précédent",
    next_month: "Mois suivant",
    settings: "Paramètres",
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
    more_from: |n, app| format!("{n} de plus de {app}"),
    focus: |m| format!("Concentration {m} min"),
    focus_stop: |m| format!("Arrêter · {m} min"),
    focus_left: |m| format!("Concentration · {m} min"),
    cleared: |n| if n == 1 { "1 notification effacée".into() } else { format!("{n} notifications effacées") },
    next_in: |m| if m < 60 { format!("Prochain · dans {m} min") } else { format!("Prochain · dans {} h {:02}", m / 60, m % 60) },
    next_on: |day| format!("Prochain · {day}"),
    now_until: |end| format!("En cours · jusqu'à {end}"),
    you: |answer| format!("vous : {answer}"),
    minutes: |m| format!("{m} min"),
};

/// The words in Windows' display language (French for any French, else English).
pub fn words() -> &'static Words {
    static CHOSEN: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    // LANG_FRENCH, the primary language of the display language's id.
    let french = *CHOSEN.get_or_init(|| unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() } & 0x3ff == 0x0c);
    if french && !cfg!(test) {
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
        }
    }
}
