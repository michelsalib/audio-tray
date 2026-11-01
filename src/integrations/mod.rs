//! Optional per-app integrations. Each reports through [`crate::hub`] and takes commands back
//! (mute toggles); each can be off in config without touching the rest, and the app works with
//! none of them. Discord, the browser and Teams live on the tray thread, their handles served by
//! [`crate::wait`].

pub mod browser;
pub mod discord;
pub mod discord_rpc;
pub mod pipe;
pub mod teams;
pub mod teams_ui;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Mutex;

use crate::config::Config;
use crate::hub::VoiceApp;

thread_local! {
    static DISCORD: RefCell<Option<Rc<RefCell<pipe::Server<discord::Discord>>>>> = const { RefCell::new(None) };
    static BROWSER: RefCell<Option<Rc<RefCell<pipe::Server<browser::Tabs>>>>> = const { RefCell::new(None) };
    static BROWSER_COMMANDS: RefCell<Option<Rc<RefCell<pipe::Server<browser::Commands>>>>> = const { RefCell::new(None) };
    static TEAMS: RefCell<Option<Rc<RefCell<teams::Client>>>> = const { RefCell::new(None) };
    static TEAMS_TOKEN: RefCell<Option<Option<String>>> = const { RefCell::new(None) };
    /// Whether Teams holds the microphone (a meeting), as the tray last said.
    static TEAMS_ON_MIC: Cell<bool> = const { Cell::new(false) };
    /// A connection attempt is out on its worker thread.
    static TEAMS_CONNECTING: Cell<bool> = const { Cell::new(false) };
    /// Signalled by the worker when its attempt is done (created and registered once).
    static TEAMS_DONE: Cell<Option<isize>> = const { Cell::new(None) };
}

/// The worker's outcome, collected on the tray thread when [`TEAMS_DONE`] fires.
static TEAMS_ATTEMPT: Mutex<Option<tungstenite::WebSocket<std::net::TcpStream>>> = Mutex::new(None);

/// Start what config turns on. Tray thread, before [`crate::wait::run`].
pub fn start(config: &Config) {
    discord_rpc::configure(&config.discord);
    if config.discord.enabled {
        start_discord();
    }
    if config.browser.enabled {
        start_browser();
    }
    if config.teams.enabled {
        TEAMS_TOKEN.with(|t| *t.borrow_mut() = Some(config.teams.token.clone()));
        connect_teams();
    }
}

fn start_discord() {
    if let Some(server) = serve(discord::PIPE, discord::Discord::default()) {
        DISCORD.with(|d| *d.borrow_mut() = Some(server));
        println!("discord: waiting for the Vencord plugin on \\\\.\\pipe\\{}", discord::PIPE);
    }
}

fn start_browser() {
    if let Err(e) = browser::register_host() {
        eprintln!("browser: could not register the native messaging host ({e:#})");
    }
    let tabs = serve(browser::PIPE, browser::Tabs::default());
    let commands = serve(browser::COMMAND_PIPE, browser::Commands);
    BROWSER.with(|b| *b.borrow_mut() = tabs);
    BROWSER_COMMANDS.with(|b| *b.borrow_mut() = commands);
}

/// Host a pipe and serve it from the tray thread's wait.
fn serve<H: pipe::Handler + 'static>(name: &'static str, handler: H) -> Option<Rc<RefCell<pipe::Server<H>>>> {
    match pipe::Server::new(name, handler) {
        Ok(server) => {
            let event = server.event();
            let server = Rc::new(RefCell::new(server));
            let served = Rc::clone(&server);
            crate::wait::register(event, move || served.borrow_mut().signalled());
            Some(server)
        }
        Err(e) => {
            eprintln!("{name}: unavailable ({e:#})");
            None
        }
    }
}

/// The connectors changed in Settings (`old` to `new`): start or stop only what was switched,
/// so a live connection is not dropped for nothing, and follow new Discord keys.
pub fn reconfigure(old: &Config, new: &Config) {
    discord_rpc::configure(&new.discord);
    if old.discord.enabled != new.discord.enabled {
        if new.discord.enabled {
            start_discord();
        } else {
            unserve(&DISCORD);
            crate::hub::emit(crate::hub::Event::Call { app: VoiceApp::Discord, call: None });
            crate::hub::emit(crate::hub::Event::Integration { name: "Discord", connected: false });
        }
    }
    if old.teams.enabled != new.teams.enabled {
        if new.teams.enabled {
            TEAMS_TOKEN.with(|t| *t.borrow_mut() = Some(new.teams.token.clone()));
            connect_teams();
            follow_teams_window();
        } else {
            TEAMS_TOKEN.with(|t| *t.borrow_mut() = None);
            TEAMS.with(|t| {
                if let Some(client) = t.borrow_mut().take() {
                    crate::wait::unregister(client.borrow().event());
                }
            });
            crate::hub::emit(crate::hub::Event::Call { app: VoiceApp::Teams, call: None });
            crate::hub::emit(crate::hub::Event::Integration { name: "Teams", connected: false });
            crate::hub::emit(crate::hub::Event::IntegrationProblem { name: "Teams", problem: None });
            follow_teams_window();
        }
    }
    if old.browser.enabled != new.browser.enabled {
        if new.browser.enabled {
            start_browser();
        } else {
            unserve(&BROWSER);
            unserve(&BROWSER_COMMANDS);
            crate::hub::emit(crate::hub::Event::BrowserMedia(Vec::new()));
            crate::hub::emit(crate::hub::Event::Integration { name: "Browser", connected: false });
        }
    }
}

type Served<H> = std::thread::LocalKey<RefCell<Option<Rc<RefCell<pipe::Server<H>>>>>>;

/// Stop serving a pipe: its wait goes, and the pipe closes with the last reference.
fn unserve<H: pipe::Handler>(slot: &'static Served<H>) {
    slot.with(|s| {
        if let Some(server) = s.borrow_mut().take() {
            crate::wait::unregister(server.borrow().event());
        }
    });
}

pub fn stop() {
    unserve(&DISCORD);
    unserve(&BROWSER);
    unserve(&BROWSER_COMMANDS);
    TEAMS.with(|t| {
        if let Some(client) = t.borrow_mut().take() {
            crate::wait::unregister(client.borrow().event());
        }
    });
}
/// Try Teams again (Settings opened its connectors): a no-op when it is off, connected, or an
/// attempt is already out.
pub fn poke_teams() {
    let enabled = TEAMS_TOKEN.with(|t| t.borrow().is_some());
    if enabled && TEAMS.with(|t| t.borrow().is_none()) {
        connect_teams();
    }
}

/// Whether Teams holds the microphone, on each change of the microphone's users. Taking it starts
/// a meeting: the moment to connect to its API; either way, the meeting window is followed while
/// there is no API.
pub fn teams_on_mic(on: bool) {
    let was = TEAMS_ON_MIC.with(|m| m.replace(on));
    if on && !was {
        poke_teams();
    }
    if on != was {
        follow_teams_window();
    }
}

/// Follow Teams' meeting window exactly while Teams has the microphone, is on in config, and its
/// API is not connected. Re-checked whenever one of those changes.
fn follow_teams_window() {
    let enabled = TEAMS_TOKEN.with(|t| t.borrow().is_some());
    let api = TEAMS.with(|t| t.borrow().is_some());
    teams_ui::meeting(TEAMS_ON_MIC.with(Cell::get) && enabled && !api);
}

/// Teams handed over a new token.
pub fn teams_token(token: String) {
    TEAMS_TOKEN.with(|t| *t.borrow_mut() = Some(Some(token)));
}

/// Start a connection attempt on a worker thread; [`teams_attempted`] picks it up on the tray
/// thread. Connecting blocks (a refused or filtered port costs the whole timeout), which the tray
/// thread, pumping the pill and the flyout, must never do.
fn connect_teams() {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Threading::{CreateEventW, SetEvent};

    if TEAMS_CONNECTING.with(|c| c.replace(true)) {
        return;
    }
    let done = match TEAMS_DONE.with(Cell::get) {
        Some(done) => done,
        None => {
            let Ok(event) = (unsafe { CreateEventW(None, false, false, None) }) else {
                TEAMS_CONNECTING.with(|c| c.set(false));
                return;
            };
            crate::wait::register(event, teams_attempted);
            TEAMS_DONE.with(|d| d.set(Some(event.0 as isize)));
            event.0 as isize
        }
    };
    let token = TEAMS_TOKEN.with(|t| t.borrow().clone().flatten());
    let _ = std::thread::Builder::new().name("teams-connect".into()).spawn(move || {
        match teams::handshake(token.as_deref()) {
            Ok(socket) => *TEAMS_ATTEMPT.lock().unwrap_or_else(|p| p.into_inner()) = Some(socket),
            Err(e) => {
                println!("teams: not connected ({e:#})");
                // Nothing listening while Teams runs: its API is off, or an admin removed the switch.
                let problem = teams::running().then_some(TEAMS_API_BLOCKED);
                crate::hub::emit(crate::hub::Event::IntegrationProblem { name: "Teams", problem });
            }
        }
        unsafe {
            let _ = SetEvent(HANDLE(done as *mut core::ffi::c_void));
        }
    });
}

/// Settings' status when Teams runs but its API does not answer.
const TEAMS_API_BLOCKED: &str = "API off: uses the meeting window";

/// A connection attempt finished (tray thread): serve the socket it opened, if Teams is still
/// wanted, and settle whether the meeting window is followed instead.
fn teams_attempted() {
    TEAMS_CONNECTING.with(|c| c.set(false));
    let socket = TEAMS_ATTEMPT.lock().unwrap_or_else(|p| p.into_inner()).take();
    let token = TEAMS_TOKEN.with(|t| t.borrow().clone());
    if let (Some(socket), Some(token)) = (socket, token) {
        match teams::client(socket, token.is_some()) {
            Ok(client) => {
                let event = client.event();
                let client = Rc::new(RefCell::new(client));
                let served = Rc::clone(&client);
                crate::hub::emit(crate::hub::Event::IntegrationProblem { name: "Teams", problem: None });
                crate::wait::register(event, move || {
                    if !served.borrow_mut().signalled() {
                        let event = served.borrow().event();
                        crate::wait::unregister(event);
                        TEAMS.with(|t| *t.borrow_mut() = None);
                        // Mid-meeting, the meeting window takes over.
                        follow_teams_window();
                    }
                });
                TEAMS.with(|t| *t.borrow_mut() = Some(client));
            }
            Err(e) => eprintln!("teams: could not serve the connection ({e:#})"),
        }
    }
    follow_teams_window();
}

/// Toggle the user's mute in a voice app.
pub fn toggle_mute(app: VoiceApp) {
    match app {
        VoiceApp::Discord if discord_rpc::connected() => discord_rpc::set_mute(None),
        VoiceApp::Discord => discord(|server| server.send(r#"{"type":"toggleMute"}"#)),
        VoiceApp::Teams if TEAMS.with(|t| t.borrow().is_none()) && teams_ui::attached() => teams_ui::set_mute(None),
        VoiceApp::Teams => teams(|client| client.action("toggle-mute")),
    }
}

/// Set the user's mute in a voice app.
pub fn set_muted(app: VoiceApp, muted: bool) {
    match app {
        VoiceApp::Discord if discord_rpc::connected() => discord_rpc::set_mute(Some(muted)),
        VoiceApp::Discord => discord(|server| server.send(&format!(r#"{{"type":"setMute","muted":{muted}}}"#))),
        VoiceApp::Teams if TEAMS.with(|t| t.borrow().is_none()) && teams_ui::attached() => teams_ui::set_mute(Some(muted)),
        VoiceApp::Teams => teams(|client| client.action(if muted { "mute" } else { "unmute" })),
    }
}

/// A command for a browser tab (`id` is the part after [`browser::APP_ID_PREFIX`]):
/// `toggle` plays or pauses its media, `focus` brings the tab and its window to the front.
pub fn browser_command(id: &str, command: &str) {
    match BROWSER_COMMANDS.with(|b| b.borrow().clone()) {
        Some(server) => server.borrow_mut().send(&browser::command(id, command)),
        None => eprintln!("integrations: the browser extension is off; cannot send it a command"),
    }
}

fn discord(f: impl FnOnce(&mut pipe::Server<discord::Discord>)) {
    match DISCORD.with(|d| d.borrow().clone()) {
        Some(server) => f(&mut server.borrow_mut()),
        None => eprintln!("integrations: Discord is off; cannot send it a command"),
    }
}

fn teams(f: impl FnOnce(&mut teams::Client)) {
    match TEAMS.with(|t| t.borrow().clone()) {
        Some(client) => f(&mut client.borrow_mut()),
        None => eprintln!("integrations: Teams is not connected; cannot send it a command"),
    }
}
