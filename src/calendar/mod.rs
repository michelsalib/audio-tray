//! The calendar that replaces the taskbar clock's: a click on the clock (caught by the TAP, which
//! keeps the shell's own flyout shut) opens [`view`], a month and the selected day's agenda, with
//! the events of a Google account, a Microsoft account and calendar links.
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
pub mod time;
mod view;
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

/// The open calendar's screen rectangle, for the mouse hook: a click outside it closes it even
/// when Windows kept the foreground from it.
pub static OPEN_RECT: Mutex<Option<windows::Win32::Foundation::RECT>> = Mutex::new(None);

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
}

static STATE: Mutex<State> = Mutex::new(State {
    events: Vec::new(),
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

/// Open the calendar (a click on the taskbar clock), or close it when it is open.
pub fn open(config: &crate::config::Config) {
    view::toggle(config);
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

/// The events fetched, and the days they cover.
pub fn events() -> (Vec<Event>, Option<(i32, i32)>) {
    let state = state();
    (state.events.clone(), state.range)
}

/// Whether a fetch is running.
pub fn fetching() -> bool {
    state().fetching
}

/// What Settings shows.
pub fn status(setup: &Setup) -> Status {
    let state = state();
    let mut status = Status { fetched: state.fetched, calendars: state.calendars.clone(), ..Default::default() };
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
            let Fetched { events, problems, done, calendars } = fetch_all(&setup, days);
            {
                let mut state = state();
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
        state.events.retain(|e| e.source != provider.source());
        state.problems.retain(|(s, _)| *s != provider.source());
        state.sign_in_problems.retain(|(p, _)| *p != provider);
    }
    notify();
}

/// The links changed (one added or removed): their events, by index, are no longer right.
pub fn links_changed() {
    let mut state = state();
    state.events.retain(|e| !matches!(e.source, Source::Link(_)));
    state.problems.retain(|(s, _)| !matches!(s, Source::Link(_)));
    state.fetched = None;
}

/// Dev: the calendar alone (no tray, no taskbar click), on the config's accounts and links.
#[cfg(feature = "dev")]
pub fn preview(config: &crate::config::Config) {
    view::preview(config);
}

/// Win+Alt+D opens this calendar (taken by the tray's keyboard hook) while this is on.
pub static SHORTCUT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_shortcut(on: bool) {
    SHORTCUT.store(on, Ordering::SeqCst);
}

/// `#RRGGBB` as a colour.
pub fn parse_hex(text: &str) -> Option<[u8; 3]> {
    let hex = text.trim().trim_start_matches('#');
    let n = u32::from_str_radix(hex.get(..6)?, 16).ok()?;
    Some([(n >> 16) as u8, (n >> 8) as u8, n as u8])
}
