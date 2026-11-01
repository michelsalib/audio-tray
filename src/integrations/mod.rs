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

use std::cell::RefCell;
use std::rc::Rc;

use crate::config::Config;
use crate::hub::VoiceApp;

thread_local! {
    static DISCORD: RefCell<Option<Rc<RefCell<pipe::Server<discord::Discord>>>>> = const { RefCell::new(None) };
    static BROWSER: RefCell<Option<Rc<RefCell<pipe::Server<browser::Tabs>>>>> = const { RefCell::new(None) };
    static BROWSER_COMMANDS: RefCell<Option<Rc<RefCell<pipe::Server<browser::Commands>>>>> = const { RefCell::new(None) };
    static TEAMS: RefCell<Option<Rc<RefCell<teams::Client>>>> = const { RefCell::new(None) };
    static TEAMS_TOKEN: RefCell<Option<Option<String>>> = const { RefCell::new(None) };
}

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
            teams_ui::meeting(false);
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
/// Try Teams again: called when Teams takes the microphone, since a meeting is starting. A no-op
/// when it is off or already connected.
pub fn poke_teams() {
    let enabled = TEAMS_TOKEN.with(|t| t.borrow().is_some());
    if enabled && TEAMS.with(|t| t.borrow().is_none()) {
        connect_teams();
    }
}

/// Teams took the microphone (`true`) or let it go: without its local API, follow the meeting
/// window instead (when Teams is on in config).
pub fn teams_meeting(in_meeting: bool) {
    let enabled = TEAMS_TOKEN.with(|t| t.borrow().is_some());
    let api = TEAMS.with(|t| t.borrow().is_some());
    teams_ui::meeting(in_meeting && enabled && !api);
}

/// Teams handed over a new token.
pub fn teams_token(token: String) {
    TEAMS_TOKEN.with(|t| *t.borrow_mut() = Some(Some(token)));
}

fn connect_teams() {
    let token = TEAMS_TOKEN.with(|t| t.borrow().clone().flatten());
    match teams::connect(token.as_deref()) {
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
                }
            });
            TEAMS.with(|t| *t.borrow_mut() = Some(client));
        }
        Err(e) => {
            println!("teams: not connected ({e:#})");
            // Nothing listening while Teams runs: its API is off, or an admin removed the switch.
            let problem = teams::running().then_some("API blocked: follows meetings");
            crate::hub::emit(crate::hub::Event::IntegrationProblem { name: "Teams", problem });
        }
    }
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
