//! Optional per-app integrations. Each reports through [`crate::hub`] and takes commands back
//! (mute toggles); each can be off in config without touching the rest, and the app works with
//! none of them. Discord, the browser and Teams live on the tray thread, their handles served by
//! [`crate::wait`].

pub mod browser;
pub mod discord;
pub mod pipe;
pub mod teams;

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
    if config.discord.enabled {
        if let Some(server) = serve(discord::PIPE, discord::Discord::default()) {
            DISCORD.with(|d| *d.borrow_mut() = Some(server));
            println!("discord: waiting for the Vencord plugin on \\\\.\\pipe\\{}", discord::PIPE);
        }
    }
    if config.browser.enabled {
        if let Err(e) = browser::register_host() {
            eprintln!("browser: could not register the native messaging host ({e:#})");
        }
        let tabs = serve(browser::PIPE, browser::Tabs::default());
        let commands = serve(browser::COMMAND_PIPE, browser::Commands);
        BROWSER.with(|b| *b.borrow_mut() = tabs);
        BROWSER_COMMANDS.with(|b| *b.borrow_mut() = commands);
    }
    if config.teams.enabled {
        TEAMS_TOKEN.with(|t| *t.borrow_mut() = Some(config.teams.token.clone()));
        connect_teams();
    }
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

pub fn stop() {
    DISCORD.with(|d| {
        if let Some(server) = d.borrow_mut().take() {
            crate::wait::unregister(server.borrow().event());
        }
    });
    BROWSER.with(|b| {
        if let Some(server) = b.borrow_mut().take() {
            crate::wait::unregister(server.borrow().event());
        }
    });
    BROWSER_COMMANDS.with(|b| {
        if let Some(server) = b.borrow_mut().take() {
            crate::wait::unregister(server.borrow().event());
        }
    });
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
            crate::wait::register(event, move || {
                if !served.borrow_mut().signalled() {
                    let event = served.borrow().event();
                    crate::wait::unregister(event);
                    TEAMS.with(|t| *t.borrow_mut() = None);
                }
            });
            TEAMS.with(|t| *t.borrow_mut() = Some(client));
        }
        Err(e) => println!("teams: not connected ({e:#})"),
    }
}

/// Toggle the user's mute in a voice app.
pub fn toggle_mute(app: VoiceApp) {
    match app {
        VoiceApp::Discord => discord(|server| server.send(r#"{"type":"toggleMute"}"#)),
        VoiceApp::Teams => teams(|client| client.action("toggle-mute")),
    }
}

/// Set the user's mute in a voice app.
pub fn set_muted(app: VoiceApp, muted: bool) {
    match app {
        VoiceApp::Discord => discord(|server| server.send(&format!(r#"{{"type":"setMute","muted":{muted}}}"#))),
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
