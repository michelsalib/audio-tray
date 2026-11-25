//! The calendar that replaces the taskbar clock's: a click on the clock (caught by the TAP, which
//! keeps the shell's own flyout shut) opens [`view`], a month and the selected day's agenda, with
//! the events of a Google account, a Microsoft account and calendar links. A click on the bell
//! beside the clock opens the same panel's other card: Windows' notifications.
//!
//! **Threads.** Everything here is called on the tray thread except the work: a fetch and a
//! sign-in each run on a thread of their own and post [`WM_CALENDAR`] to the tray when they are
//! done; [`changed`] then repaints the calendar and Settings from [`STATE`]. Events are fetched when
//! the calendar opens (from the cache at once, refreshed when stale), when its month moves out of
//! what was fetched, and every quarter hour in the background ([`schedule`]) for reminders.

pub mod focus;
mod ics;
mod model;
pub mod notifications;
mod oauth;
pub mod rich;
pub mod schedule;
mod sources;
pub mod takeover;
pub mod time;
mod view;

pub use view::{Badge, Panel};
pub use view::banner::{display_changed, glance, listening};
pub mod zones;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use windows::Win32::UI::WindowsAndMessaging::WM_APP;

pub use model::{Calendar, Event, Source};
pub use sources::{feed_url, link_name, Link, Provider, Setup};
use time::{Local, Zone, DAY};

/// Posted to the tray's window: a fetch or a sign-in finished (`wParam` 0), or a click landed
/// outside the open calendar ([`CLOSE`]).
pub const WM_CALENDAR: u32 = WM_APP + 42;
/// [`WM_CALENDAR`]'s `wParam` to close the calendar.
pub const CLOSE: usize = 1;
/// [`WM_CALENDAR`]'s `wParam`: Windows' notifications changed.
pub const NOTIFICATIONS: usize = 2;
/// [`WM_CALENDAR`]'s `wParam`: Do not disturb was switched (by anyone).
pub const QUIET: usize = 3;

/// The open panel's card on screen, for the mouse hook: a click outside it closes it even when
/// Windows kept the foreground from it.
pub static OPEN_CARD: Mutex<Option<windows::Win32::Foundation::RECT>> = Mutex::new(None);

/// A cache this old is fetched again when the calendar opens.
const STALE_AFTER: i64 = 5 * 60;

/// An account's state, as Settings shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Account {
    /// The signed-in address.
    pub email: Option<String>,
    /// The browser is open on the provider's sign-in.
    pub signing_in: bool,
    /// What went wrong last (signing in, or fetching).
    pub problem: Option<String>,
}

/// What Settings shows about the calendar.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    pub google: Account,
    pub microsoft: Account,
    /// Each link's problem, by index, when its last fetch failed.
    pub links: Vec<Option<String>>,
    /// When events were last fetched.
    pub fetched: Option<i64>,
    /// The calendars found, to show or hide and colour.
    pub calendars: Vec<Calendar>,
    /// Windows keeps notifications quiet (Do not disturb, a focus session, one of its rules).
    pub quiet: bool,
    /// Windows does not let Audio Tray read its notifications.
    pub notifications_denied: bool,
}

impl Status {
    pub fn account(&self, provider: Provider) -> &Account {
        match provider {
            Provider::Google => &self.google,
            Provider::Microsoft => &self.microsoft,
        }
    }

    fn account_mut(&mut self, provider: Provider) -> &mut Account {
        match provider {
            Provider::Google => &mut self.google,
            Provider::Microsoft => &mut self.microsoft,
        }
    }
}

#[derive(Default)]
struct State {
    events: Vec<Event>,
    /// The local days fetched, and with what.
    range: Option<(i32, i32)>,
    setup: Option<Setup>,
    fetched: Option<i64>,
    fetching: bool,
    /// A fetch asked for while one was running: done next.
    again: Option<(Setup, (i32, i32))>,
    /// Problems by source, from the last fetch.
    problems: Vec<(Source, String)>,
    /// Sign-ins in progress, each with its way to give up.
    signing_in: Vec<(Provider, Arc<AtomicBool>)>,
    sign_in_problems: Vec<(Provider, String)>,
    /// The calendars the last fetch found.
    calendars: Vec<Calendar>,
    /// Bumped by a sign-out or a change of links: a fetch begun before brings back what is no
    /// longer wanted, and is dropped.
    generation: u64,
}

static STATE: Mutex<State> = Mutex::new(State {
    events: Vec::new(),
    generation: 0,
    range: None,
    setup: None,
    fetched: None,
    fetching: false,
    again: None,
    problems: Vec::new(),
    signing_in: Vec::new(),
    sign_in_problems: Vec::new(),
    calendars: Vec::new(),
});

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

fn notify() {
    crate::tray::post(WM_CALENDAR, 0, 0);
}

/// Open `panel` (the clock's calendar, the bell's notifications), close it when it is open, or
/// turn the open panel into it.
pub fn open(config: &crate::config::Config, panel: Panel) {
    view::toggle(config, panel);
}

/// Have Windows show its notification bell beside the clock (Taskbar settings' "Show notification
/// bell icon"): the bell is the notifications' button, and Windows keeps its place.
pub fn show_bell() {
    use windows::core::w;
    use windows::Win32::System::Registry::{RegSetKeyValueW, HKEY_CURRENT_USER, REG_DWORD};
    const KEY: windows::core::PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced");
    const NAME: windows::core::PCWSTR = w!("ShowNotificationIcon");
    // Shown already (or never set: Windows' default shows it).
    if crate::win::hkcu_dword(KEY, NAME) != Some(0) {
        return;
    }
    let one = 1u32;
    let set = unsafe { RegSetKeyValueW(HKEY_CURRENT_USER, KEY, NAME, REG_DWORD.0, Some(&one as *const u32 as *const _), 4) };
    if set.is_err() {
        eprintln!("calendar: could not show Windows' notification bell ({set:?})");
        return;
    }
    println!("calendar: Windows' notification bell shown, for the notifications");
    // The taskbar takes it at once on being told; a hung window must not hold the tray thread.
    std::thread::spawn(|| {
        use windows::Win32::Foundation::{LPARAM, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE};
        let area = w!("TraySettings");
        unsafe { SendMessageTimeoutW(HWND_BROADCAST, WM_SETTINGCHANGE, WPARAM(0), LPARAM(area.as_ptr() as isize), SMTO_ABORTIFHUNG, 3000, None) };
    });
}

/// Draw the panel once, hidden, ahead of its next open (see [`view::prewarm`]).
pub fn prewarm(config: &crate::config::Config) {
    view::prewarm(config);
}

/// Close the calendar if it is open (the app is quitting).
pub fn close() {
    view::close();
}

/// A fetch or a sign-in finished: show what changed. Tray thread.
pub fn changed() {
    view::refresh();
    crate::settings::calendar_changed();
}

/// The notifications changed: the panel shows them, and the banners what is new (Settings lists
/// accounts and fetches). Apps Windows heard from for the first time have their banners taken.
pub fn notifications_changed(config: &crate::config::Config) {
    view::refresh();
    view::banner::list_changed(config);
    // New apps taken; and a screen reader started since gives Windows its banners back.
    if config.calendar.take_over {
        follow_takeover(config);
    }
}

/// Dev: the banners' simulation ([`view::banner::simulate`]); `false` when a check failed.
#[cfg(feature = "dev")]
pub fn simulate_banners() -> bool {
    view::banner::simulate()
}

/// Dev: the digest's banner ([`view::banner::preview_digest`]).
#[cfg(feature = "dev")]
pub fn preview_digest(seconds: u64) {
    view::banner::preview_digest(seconds);
}

/// Dev: a listed notification's banner clicked ([`view::banner::preview_click`]).
#[cfg(feature = "dev")]
pub fn preview_click(id: u32, dry: bool) {
    view::banner::preview_click(id, dry);
}

/// Dev: banners of what is listed ([`view::banner::preview`]).
#[cfg(feature = "dev")]
pub fn preview_banners(count: usize, seconds: u64) {
    view::banner::preview(count, seconds);
}

/// Audio Tray's own notices (batteries, keyboards, audio devices, reminders, focus) are
/// notifications, listed under the bell with every app's, not the pill's: whenever the bell is
/// Audio Tray's.
pub fn merges_notices(config: &crate::config::Config) -> bool {
    config.calendar.enabled && config.calendar.notifications
}

/// Audio Tray shows every notification itself: the bell's list is on, and Settings' take-over.
/// Not while a screen reader runs: Windows' own banners are the ones it reads aloud, so they stay.
pub fn handed_over(config: &crate::config::Config) -> bool {
    merges_notices(config) && config.calendar.take_over && !screen_reader()
}

/// A screen reader (Narrator, NVDA, JAWS) is running, as it tells Windows.
fn screen_reader() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_GETSCREENREADER, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS};
    let mut on = windows::core::BOOL(0);
    let asked = unsafe { SystemParametersInfoW(SPI_GETSCREENREADER, 0, Some(&mut on as *mut _ as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0)) };
    asked.is_ok() && on.as_bool()
}

/// Take Windows' banners, or give them back, as the config says (at start, on a change in
/// Settings); the banners showing go when Audio Tray no longer shows them.
pub fn follow_takeover(config: &crate::config::Config) {
    let on = handed_over(config) && !notifications::denied();
    takeover::apply(on);
    // Banners want each notification as it comes.
    notifications::set_fast(on);
    if !on {
        view::banner::clear();
    }
}

/// Do not disturb was switched (by anyone): the open panel's moon follows, and Settings' switch.
pub fn quiet_changed() {
    view::quiet_changed();
    crate::settings::calendar_changed();
}

/// The events fetched, and the days they cover.
pub fn events() -> (Vec<Event>, Option<(i32, i32)>) {
    let state = state();
    (state.events.clone(), state.range)
}

/// The first source that failed, in a few words for the panel (`Outlook: sign in again`).
pub fn problem(setup: &Setup) -> Option<String> {
    let state = state();
    let (source, why) = state.problems.first()?;
    let name = match source {
        Source::Google => "Google".to_string(),
        Source::Microsoft => "Outlook".to_string(),
        Source::Link(i) => setup.links.get(*i).map_or_else(|| "A calendar link".into(), |l| l.name.clone()),
    };
    let short = if why.contains("sign in again") { "sign in again" } else if why.starts_with("Offline") { "offline" } else { "could not refresh" };
    Some(format!("{name}: {short}"))
}

/// When events were last fetched.
pub fn fetched_at() -> Option<i64> {
    state().fetched
}

/// Whether a fetch is running.
pub fn fetching() -> bool {
    state().fetching
}

/// What Settings shows.
pub fn status(setup: &Setup) -> Status {
    let state = state();
    let mut status = Status {
        fetched: state.fetched,
        calendars: state.calendars.clone(),
        quiet: focus::active(),
        notifications_denied: notifications::denied(),
        ..Default::default()
    };
    for provider in Provider::ALL {
        let account = status.account_mut(provider);
        account.email = sources::signed_in(provider);
        account.signing_in = state.signing_in.iter().any(|(p, _)| *p == provider);
        account.problem = state
            .sign_in_problems
            .iter()
            .find(|(p, _)| *p == provider)
            .map(|(_, why)| why.clone())
            .or_else(|| state.problems.iter().find(|(s, _)| *s == provider.source()).map(|(_, why)| why.clone()))
            .filter(|_| !account.signing_in);
    }
    status.links = (0..setup.links.len())
        .map(|i| state.problems.iter().find(|(s, _)| *s == Source::Link(i)).map(|(_, why)| why.clone()))
        .collect();
    status
}

/// Whether anything is set up to fetch from.
pub fn has_sources(setup: &Setup) -> bool {
    !setup.links.is_empty() || Provider::ALL.iter().any(|&p| setup.registered(p) && sources::signed_in(p).is_some())
}

/// The days fetched for a calendar showing month `(y, m)`: from the month before to the one after,
/// so moving a month either way needs no fetch.
pub fn days_around(y: i32, m: u32) -> (i32, i32) {
    let (py, pm) = time::add_months(y, m, -1);
    let (ny, nm) = time::add_months(y, m, 2);
    (time::days_from_civil(py, pm, 1) - 7, time::days_from_civil(ny, nm, 1) + 7)
}

/// Fetch the events of `days` unless the cache already has them fresh (`force`: regardless).
pub fn fetch(setup: &Setup, days: (i32, i32), force: bool) {
    let mut state = state();
    let covered = state.range.is_some_and(|(a, b)| a <= days.0 && days.1 <= b);
    let fresh = state.fetched.is_some_and(|at| time::now() - at < STALE_AFTER);
    if !force && covered && fresh && state.setup.as_ref() == Some(setup) {
        return;
    }
    if state.fetching {
        state.again = Some((setup.clone(), days));
        return;
    }
    state.fetching = true;
    drop(state);
    spawn_fetch(setup.clone(), days);
}

fn spawn_fetch(setup: Setup, days: (i32, i32)) {
    std::thread::spawn(move || {
        // A panic (a feed nobody foresaw) must not leave `fetching` set, or nothing is fetched
        // again until a restart.
        struct Done;
        impl Drop for Done {
            fn drop(&mut self) {
                if std::thread::panicking() {
                    eprintln!("calendar: the fetch panicked");
                    let mut state = state();
                    state.fetching = false;
                    state.again = None;
                    drop(state);
                    notify();
                }
            }
        }
        let _done = Done;
        let (mut setup, mut days) = (setup, days);
        loop {
            let generation = state().generation;
            let Fetched { events, problems, done, calendars } = fetch_all(&setup, days);
            {
                let mut state = state();
                if state.generation != generation {
                    println!("calendar: a fetch begun before a sign-out or a change of links is dropped");
                } else {
                    // A source that failed keeps what it had, when it was for the same days.
                    let same = state.range == Some(days);
                    let failed: Vec<Source> = problems.iter().map(|(s, _)| *s).collect();
                    let mut kept: Vec<Event> = if same { state.events.iter().filter(|e| failed.contains(&e.source)).cloned().collect() } else { Vec::new() };
                    kept.extend(events.into_iter().filter(|e| done.contains(&e.source)));
                    state.events = kept;
                    state.range = Some(days);
                    state.setup = Some(setup.clone());
                    state.fetched = Some(time::now());
                    state.problems = problems;
                    // Calendars of a source that failed stay listed.
                    state.calendars.retain(|c| failed.contains(&c.source));
                    state.calendars.extend(calendars);
                }
                match state.again.take() {
                    Some((next_setup, next_days)) => {
                        setup = next_setup;
                        days = next_days;
                    }
                    None => {
                        state.fetching = false;
                        drop(state);
                        notify();
                        return;
                    }
                }
            }
            notify();
        }
    });
}

/// What a fetch brings back: the events, the problems met, which sources answered, and their
/// calendars.
struct Fetched {
    events: Vec<Event>,
    problems: Vec<(Source, String)>,
    done: Vec<Source>,
    calendars: Vec<Calendar>,
}

fn fetch_all(setup: &Setup, days: (i32, i32)) -> Fetched {
    let zone = Local;
    let range = (zone.to_utc(days.0 as i64 * DAY), zone.to_utc(days.1 as i64 * DAY));
    let mut events = Vec::new();
    let mut problems = Vec::new();
    let mut done = Vec::new();
    let mut calendars = Vec::new();
    for provider in Provider::ALL {
        if !setup.registered(provider) || sources::signed_in(provider).is_none() {
            continue;
        }
        match sources::fetch(provider, setup, range, &zone) {
            Ok((list, found)) => {
                events.extend(list);
                calendars.extend(found);
                done.push(provider.source());
            }
            Err(e) => {
                eprintln!("calendar: {} ({e:#})", provider.name());
                problems.push((provider.source(), describe(&e)));
            }
        }
    }
    for (i, link) in setup.links.iter().enumerate() {
        match sources::fetch_link(i, link, days, &zone) {
            Ok((list, calendar)) => {
                events.extend(list);
                calendars.push(calendar);
                done.push(Source::Link(i));
            }
            Err(e) => {
                eprintln!("calendar: link {i} ({e:#})");
                problems.push((Source::Link(i), describe_link(&e)));
            }
        }
    }
    Fetched { events, problems, done, calendars }
}

/// A link's problem: there is no account to sign in again with, only the address to fix.
fn describe_link(error: &anyhow::Error) -> String {
    match error.downcast_ref::<oauth::Refused>().map(|r| r.0) {
        Some(401 | 403) => "Refused: use the calendar's secret iCal address".into(),
        _ if format!("{error:#}").contains("not a calendar") => "Not a calendar: use its iCal (.ics) address".into(),
        _ => describe(error),
    }
}

/// A problem in a few words, for Settings and the calendar's footer.
fn describe(error: &anyhow::Error) -> String {
    if error.chain().any(|e| e.is::<sources::SignInAgain>()) {
        return "Signed out: sign in again".into();
    }
    if let Some(refused) = error.downcast_ref::<oauth::Refused>() {
        return match refused.0 {
            401 | 403 => "Access refused: sign in again".into(),
            404 => "Not found: check the address".into(),
            code => format!("The server answered {code}"),
        };
    }
    let text = format!("{error:#}");
    if text.contains("could not reach") {
        return "Offline, or the service could not be reached".into();
    }
    text.chars().take(120).collect()
}

/// Sign in with `provider` in the browser, on a thread of its own.
pub fn sign_in(provider: Provider, setup: &Setup) {
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut state = state();
        if state.signing_in.iter().any(|(p, _)| *p == provider) {
            return;
        }
        state.signing_in.push((provider, cancel.clone()));
        state.sign_in_problems.retain(|(p, _)| *p != provider);
    }
    notify();
    let setup = setup.clone();
    std::thread::spawn(move || {
        let result = sources::sign_in(provider, &setup, &cancel);
        let mut state = state();
        state.signing_in.retain(|(p, _)| *p != provider);
        match result {
            Ok(email) => {
                println!("calendar: signed in to {} as {email}", provider.name());
                state.problems.retain(|(s, _)| *s != provider.source());
                // Fetch again with the new account.
                state.fetched = None;
            }
            Err(e) if cancel.load(Ordering::SeqCst) => println!("calendar: {} sign-in cancelled ({e:#})", provider.name()),
            Err(e) => {
                eprintln!("calendar: {} sign-in failed ({e:#})", provider.name());
                state.sign_in_problems.push((provider, describe_sign_in(&e)));
            }
        }
        drop(state);
        notify();
    });
}

/// A sign-in's failure in a few words: the provider's own error code says most.
fn describe_sign_in(error: &anyhow::Error) -> String {
    let text = format!("{error:#}");
    if text.contains("invalid_client") {
        return "The client ID or secret was refused".into();
    }
    if text.contains("redirect_uri") || text.contains("AADSTS50011") || text.contains("AADSTS500113") {
        return "The redirect is not registered: add http://localhost".into();
    }
    if text.contains("AADSTS50194") {
        return "Single-tenant app: allow any organisation in its Authentication".into();
    }
    if text.contains("AADSTS700016") {
        return "No such application: check its ID".into();
    }
    if text.contains("AADSTS65001") || text.contains("consent") {
        return "Your organisation must approve the app first".into();
    }
    if text.contains("access_denied") {
        return "Access was not allowed".into();
    }
    describe(error)
}

/// Give up a sign-in waiting on the browser.
pub fn cancel_sign_in(provider: Provider) {
    if let Some((_, cancel)) = state().signing_in.iter().find(|(p, _)| *p == provider) {
        cancel.store(true, Ordering::SeqCst);
    }
}

/// Forget `provider`'s account and its events.
pub fn sign_out(provider: Provider) {
    sources::sign_out(provider);
    {
        let mut state = state();
        state.generation += 1;
        state.calendars.retain(|c| c.source != provider.source());
        // Fetched again, without it.
        state.fetched = None;
        state.events.retain(|e| e.source != provider.source());
        state.problems.retain(|(s, _)| *s != provider.source());
        state.sign_in_problems.retain(|(p, _)| *p != provider);
    }
    notify();
}

/// The links changed (one added or removed): their events, by index, are no longer right.
pub fn links_changed() {
    let mut state = state();
    state.generation += 1;
    state.calendars.retain(|c| !matches!(c.source, Source::Link(_)));
    state.events.retain(|e| !matches!(e.source, Source::Link(_)));
    state.problems.retain(|(s, _)| !matches!(s, Source::Link(_)));
    state.fetched = None;
}

/// Dev: the calendar or the notifications alone (no tray, no taskbar click), on the config's
/// accounts and links.
#[cfg(feature = "dev")]
pub fn preview(config: &crate::config::Config, panel: Panel) {
    view::preview(config, panel);
}

/// Win+Alt+D and Win+N (Windows' shortcuts for its calendar and notification centre) open the
/// calendar and the notifications while this is on, taken by the tray's keyboard hook (installed
/// only then; Win+N only while the notifications are on).
pub static SHORTCUT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_shortcut(on: bool) {
    SHORTCUT.store(on, Ordering::SeqCst);
}

/// Whether `link` is a web address, the only kind an event's link or join opens: a feed's text
/// could name a program or a share, which the shell would run.
pub fn web_link(link: &str) -> bool {
    let link = link.trim_start().to_ascii_lowercase();
    link.starts_with("https://") || link.starts_with("http://")
}

/// Whether `link` is an address a notification's button may open: a URI with a scheme of its own
/// (`ms-teams:`, `https:`), never a path, a file, or a scheme that runs or searches things.
pub fn app_link(link: &str) -> bool {
    const REFUSED: [&str; 9] = ["file", "shell", "ms-msdt", "search-ms", "search", "ms-search", "javascript", "vbscript", "ms-officecmd"];
    let Some((scheme, _)) = link.trim_start().split_once(':') else { return false };
    let scheme = scheme.to_ascii_lowercase();
    // One letter is a drive (`C:\…`).
    scheme.len() >= 2
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        && !REFUSED.contains(&scheme.as_str())
}

/// `#RRGGBB` as a colour.
pub fn parse_hex(text: &str) -> Option<[u8; 3]> {
    let hex = text.trim().trim_start_matches('#');
    // Six hex digits, nothing else (`from_str_radix` alone takes a sign).
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let n = u32::from_str_radix(hex, 16).ok()?;
    Some([(n >> 16) as u8, (n >> 8) as u8, n as u8])
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_web_and_app_addresses_open() {
        assert!(super::web_link("https://teams.microsoft.com/l/meetup-join/x"));
        assert!(super::web_link("  HTTP://example.com"));
        assert!(!super::web_link(r"\host\share\x.exe"));
        assert!(!super::web_link(r"C:\Windows\System32\cmd.exe"));
        assert!(!super::web_link("file:///C:/x.exe"));
        assert!(super::app_link("ms-teams:/l/chat"));
        assert!(super::app_link("https://example.com"));
        assert!(super::app_link("whatsapp://send?phone=1"));
        assert!(!super::app_link(r"C:\Windows\System32\cmd.exe"));
        assert!(!super::app_link(r"\host\share\x.exe"));
        assert!(!super::app_link("file:///C:/x.exe"));
        assert!(!super::app_link("search-ms:query=x"));
        assert!(!super::app_link("SHELL:startup"));
        assert!(!super::app_link("no scheme here"));
    }

    #[test]
    fn a_month_s_fetch_covers_the_months_either_side_and_a_week_more() {
        let (from, to) = super::days_around(2026, 1);
        assert_eq!(from, super::time::days_from_civil(2025, 12, 1) - 7);
        assert_eq!(to, super::time::days_from_civil(2026, 3, 1) + 7);
    }

    #[test]
    fn colours_are_six_hex_digits() {
        assert_eq!(super::parse_hex("#1a2B3c"), Some([0x1a, 0x2b, 0x3c]));
        assert_eq!(super::parse_hex("+12345"), None);
        assert_eq!(super::parse_hex("#1234567"), None);
        assert_eq!(super::parse_hex("#12345"), None);
    }
}
