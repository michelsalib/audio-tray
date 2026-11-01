//! Every tab playing video or audio, through a browser extension (`integrations/browser`).
//! Windows' media controls get one session per browser, so a second Twitch stream never reaches
//! SMTC; the extension sees each `<video>`/`<audio>` itself.
//!
//! The extension talks native messaging to `com.audiotray.media`, which is this exe in host mode
//! ([`run_host`], launched by the browser). The host relays over two pipes the tray serves:
//! `audio-tray-browser` carries the tab list in, `audio-tray-browser-cmd` carries play/pause out
//! (two pipes, so the host never needs simultaneous I/O on one handle).
//!
//! Extension → app: `{"type":"tabs","tabs":[{"id":"12:0","site":"www.twitch.tv","title":…,
//! "artist":…,"art":"<base64>","playing":true,"muted":false,"duration":0,"position":12.5}]}`.
//! App → extension: `{"type":"command","id":"12:0","command":"toggle"}` (or `"focus"`).
//!
//! Each browser runs its own host, so the pipes have several instances; a host opens both with
//! `{"type":"hello","host":<pid>}`, and its tabs' app ids carry that id
//! (`browser:<host>:<tab>:<frame>`), so commands go back to the browser that has the tab. A host
//! waits for the tray on a named event ([`READY_EVENT`]) the tray sets while it serves.

use std::sync::Arc;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::hub::{Event, Media};

pub const PIPE: &str = "audio-tray-browser";
pub const COMMAND_PIPE: &str = "audio-tray-browser-cmd";
/// Browsers served at once (each pipe has this many instances).
pub const INSTANCES: u32 = 4;
/// Set by the tray while it serves the pipes; a host waits on it rather than retry on a timer.
pub const READY_EVENT: &str = r"Local\AudioTray.BrowserPipes";
/// Hosts connected now, for one status across browsers.
static CONNECTED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
/// Its name in the hub, for Settings' status.
pub const NAME: &str = "Browser";
/// The native messaging host's name, as the extension asks for it.
const HOST: &str = "com.audiotray.media";
/// The extension's id, fixed by the `key` in its manifest.
const EXTENSION_ID: &str = "eajikcpfpkjjbdfibhfnnacflhgklgll";
/// Prefix of the app id a browser tab's media carries, so commands find their way back.
pub const APP_ID_PREFIX: &str = "browser:";

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum Message {
    Hello { host: u32 },
    Tabs { tabs: Vec<Tab> },
}

#[derive(Deserialize)]
struct Tab {
    id: String,
    #[serde(default)]
    site: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    artist: String,
    art: Option<String>,
    #[serde(default)]
    playing: bool,
    #[serde(default)]
    duration: f64,
    #[serde(default)]
    position: f64,
}

/// The tab list side.
#[derive(Default)]
pub struct Tabs {
    /// The last list, to keep decoded covers across updates.
    media: Vec<Media>,
    /// The host's id, from its hello (0 for a host from before hellos).
    host: u32,
}

impl super::pipe::Handler for Tabs {
    fn connected(&mut self) {
        println!("browser: extension connected");
        self.host = 0;
        if CONNECTED.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
            crate::hub::emit(Event::Integration { name: NAME, connected: true });
        }
    }

    fn disconnected(&mut self) {
        println!("browser: extension went away");
        self.media.clear();
        crate::hub::emit(Event::BrowserMedia { host: Some(self.host), media: Vec::new() });
        if CONNECTED.fetch_sub(1, std::sync::atomic::Ordering::SeqCst) == 1 {
            crate::hub::emit(Event::Integration { name: NAME, connected: false });
        }
    }

    fn line(&mut self, text: &str) -> Result<()> {
        let tabs = match serde_json::from_str(text).context("not a message")? {
            Message::Hello { host } => {
                self.host = host;
                return Ok(());
            }
            Message::Tabs { tabs } => tabs,
        };
        let now = crate::music::smtc::now_ticks();
        let media: Vec<Media> = tabs
            .into_iter()
            .filter(|t| !t.title.trim().is_empty())
            .map(|t| {
                let app_id = format!("{APP_ID_PREFIX}{}:{}", self.host, t.id);
                // Covers arrive as base64 each time: reuse the decoded bytes when unchanged.
                let old = self.media.iter().find(|m| m.app_id == app_id && m.title == t.title);
                let cover = match (old, t.art.as_deref()) {
                    (Some(old), Some(_)) if old.cover.is_some() => old.cover.clone(),
                    (_, art) => art.and_then(super::discord::base64_decode).map(Arc::new),
                };
                let ticks = |seconds: f64| (seconds.max(0.0) * 1e7) as i64;
                let fresh = crate::music::smtc::Timeline { start: 0, end: ticks(t.duration), position: ticks(t.position), last_updated: now };
                // A tab's reading that agrees with its last one (within a second, same state) keeps
                // the old timeline: re-stamping every tab on every message made all of them differ,
                // and the pill repainted on any tab's change.
                let timeline = old
                    .filter(|old| old.playing == t.playing)
                    .and_then(|old| old.timeline)
                    .filter(|kept| {
                        let moved = if t.playing { now - kept.last_updated } else { 0 };
                        kept.end == fresh.end && (kept.position + moved - fresh.position).abs() < 10_000_000
                    })
                    .unwrap_or(fresh);
                Media {
                    app: crate::hub::names::site_name(&t.site),
                    app_id,
                    title: t.title,
                    artist: t.artist,
                    playing: t.playing,
                    cover,
                    timeline: Some(timeline),
                    // The page's own previous/next handlers are out of a content script's reach.
                    skip: false,
                }
            })
            .collect();
        let summary: Vec<String> = media
            .iter()
            .map(|m| format!("{} on {}{}", m.artist, m.app, if m.playing { "" } else { " (paused)" }))
            .collect();
        println!("browser: {} tab(s): {}", media.len(), summary.join(", "));
        self.media = media.clone();
        crate::hub::emit(Event::BrowserMedia { host: Some(self.host), media });
        Ok(())
    }
}

/// The command side: only the host's hello comes in, saying which browser this pipe reaches.
#[derive(Default)]
pub struct Commands {
    pub host: u32,
}

impl super::pipe::Handler for Commands {
    fn connected(&mut self) {
        self.host = 0;
    }

    fn line(&mut self, text: &str) -> Result<()> {
        if let Message::Hello { host } = serde_json::from_str(text).context("not a message")? {
            self.host = host;
        }
        Ok(())
    }
}

/// The browser and the tab frame a tab's id (the part after [`APP_ID_PREFIX`]) names:
/// `"<host>:<tab>:<frame>"` → `(host, "<tab>:<frame>")`.
pub fn split_id(id: &str) -> Option<(u32, &str)> {
    let (host, frame) = id.split_once(':')?;
    Some((host.parse().ok()?, frame))
}

/// The JSON for `command` (`toggle`, `focus`) on tab frame `frame` (`"<tab>:<frame>"`).
pub fn command(frame: &str, command: &str) -> String {
    serde_json::json!({ "type": "command", "id": frame, "command": command }).to_string()
}

/// Register this exe as the extension's native messaging host for Edge, Chrome, Brave and
/// Vivaldi (per user, no administrator rights): a manifest next to the config, and a registry
/// key naming it, where each looks.
pub fn register_host() -> Result<()> {
    use windows::core::PCWSTR;
    use windows::Win32::System::Registry::{RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ};

    let exe = std::env::current_exe().context("current_exe")?;
    let dir = crate::config::Config::path()?.parent().context("config directory")?.to_path_buf();
    std::fs::create_dir_all(&dir)?;
    let manifest = dir.join("browser-host.json");
    let json = serde_json::json!({
        "name": HOST,
        "description": "Audio Tray: the media playing in your tabs",
        "path": exe,
        "type": "stdio",
        "allowed_origins": [format!("chrome-extension://{EXTENSION_ID}/")],
    });
    std::fs::write(&manifest, serde_json::to_string_pretty(&json)?)?;
    let value = crate::win::wide(&manifest.to_string_lossy());
    for browser in [r"Microsoft\Edge", r"Google\Chrome", r"BraveSoftware\Brave-Browser", r"Vivaldi"] {
        let key = crate::win::wide(&format!(r"Software\{browser}\NativeMessagingHosts\{HOST}"));
        let status = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                PCWSTR(key.as_ptr()),
                PCWSTR::null(),
                REG_SZ.0,
                Some(value.as_ptr() as *const core::ffi::c_void),
                (value.len() * 2) as u32,
            )
        };
        if status.is_err() {
            eprintln!("browser: could not register the host for {browser} ({status:?})");
        }
    }
    Ok(())
}

/// The extension's folder, to load unpacked: `browser-extension` beside the installed exe, else
/// `integrations/browser` above it (a build from the repository).
pub fn extension_dir() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let installed = exe.parent()?.join("browser-extension");
    if installed.join("manifest.json").is_file() {
        return Some(installed);
    }
    exe.ancestors().map(|dir| dir.join("integrations").join("browser")).find(|dir| dir.join("manifest.json").is_file())
}

/// Show the extension's folder in Explorer, its path on the clipboard for the browser's
/// "Load unpacked" dialog. `false` when there is no folder to show.
pub fn show_extension_dir() -> bool {
    let Some(dir) = extension_dir() else {
        eprintln!("browser: the extension's folder is missing");
        return false;
    };
    let path = dir.to_string_lossy();
    crate::win::set_clipboard_text(&path);
    crate::win::shell_open(&path, None)
}

/// Open the default browser's extensions page (Edge's when the default is not a Chromium one
/// we know): where Developer mode and "Load unpacked" are.
pub fn open_extensions_page() {
    let (exe, page) = match default_browser().as_deref() {
        Some(id) if id.starts_with("ChromeHTML") => ("chrome.exe", "chrome://extensions"),
        Some(id) if id.starts_with("BraveHTML") => ("brave.exe", "brave://extensions"),
        Some(id) if id.starts_with("VivaldiHTM") => ("vivaldi.exe", "vivaldi://extensions"),
        _ => ("msedge.exe", "edge://extensions"),
    };
    if !crate::win::shell_open(exe, Some(page)) {
        eprintln!("browser: could not open {page} with {exe}");
    }
}

/// The ProgId the user picked for https links.
fn default_browser() -> Option<String> {
    use windows::core::w;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ};

    let mut buf = [0u16; 128];
    let mut size = std::mem::size_of_val(&buf) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\Shell\Associations\UrlAssociations\https\UserChoice"),
            w!("ProgId"),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut core::ffi::c_void),
            Some(&mut size),
        )
    };
    if status.is_err() {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..len]))
}

/// Whether the browser launched us as the native messaging host: its first argument is the
/// calling extension's origin.
pub fn is_host_launch(args: &[String]) -> bool {
    args.first().is_some_and(|a| a.starts_with("chrome-extension://"))
}

/// The tray's "serving" event ([`READY_EVENT`]), created if the tray has not yet: manual reset.
pub fn ready_event() -> Option<windows::Win32::Foundation::HANDLE> {
    use windows::core::PCWSTR;
    use windows::Win32::System::Threading::CreateEventW;
    let name = crate::win::wide(READY_EVENT);
    unsafe { CreateEventW(None, true, false, PCWSTR(name.as_ptr())) }.ok()
}

/// Host mode: relay the extension's messages (stdin, length-prefixed JSON) to the tray's pipe,
/// and the tray's commands back (stdout). Never prints anything else to stdout: it is the
/// channel. Runs until the browser closes stdin; waits for the tray (on [`READY_EVENT`], no
/// timer) when it is not running.
pub fn run_host() -> Result<()> {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::sync::Mutex;

    let pipe = Arc::new(Mutex::new(None::<std::fs::File>));
    let last = Arc::new(Mutex::new(None::<String>));
    // The tray side: connect (and reconnect), replay the last list, pass commands to stdout.
    {
        let (pipe, last) = (Arc::clone(&pipe), Arc::clone(&last));
        // A kernel handle, sent to the relay thread as its value.
        let ready = ready_event().map(|event| event.0 as isize);
        let hello = serde_json::json!({ "type": "hello", "host": std::process::id() }).to_string();
        std::thread::spawn(move || loop {
            if let Some(ready) = ready {
                use windows::Win32::System::Threading::{WaitForSingleObject, INFINITE};
                unsafe { WaitForSingleObject(windows::Win32::Foundation::HANDLE(ready as *mut core::ffi::c_void), INFINITE) };
            }
            let data = std::fs::OpenOptions::new().write(true).open(format!(r"\\.\pipe\{PIPE}"));
            let commands = std::fs::OpenOptions::new().read(true).write(true).open(format!(r"\\.\pipe\{COMMAND_PIPE}"));
            if let (Ok(mut data), Ok(mut commands)) = (data, commands) {
                let _ = writeln!(data, "{hello}");
                let _ = writeln!(commands, "{hello}");
                if let Some(line) = last.lock().unwrap_or_else(|p| p.into_inner()).as_ref() {
                    let _ = writeln!(data, "{line}");
                }
                *pipe.lock().unwrap_or_else(|p| p.into_inner()) = Some(data);
                for line in BufReader::new(commands).lines() {
                    let Ok(line) = line else { break };
                    let mut out = std::io::stdout().lock();
                    let _ = out.write_all(&(line.len() as u32).to_le_bytes());
                    let _ = out.write_all(line.as_bytes());
                    let _ = out.flush();
                }
                *pipe.lock().unwrap_or_else(|p| p.into_inner()) = None;
            }
            // Set but nothing to open: every instance busy, or a tray that died without clearing
            // the event. A slow retry, only then; a tray that quit cleared it, and the wait sleeps.
            std::thread::sleep(std::time::Duration::from_secs(3));
        });
    }
    // The browser side: one message per frame, until stdin closes with the browser.
    let mut stdin = std::io::stdin().lock();
    loop {
        let mut len = [0u8; 4];
        if stdin.read_exact(&mut len).is_err() {
            return Ok(());
        }
        let mut body = vec![0u8; u32::from_le_bytes(len) as usize];
        if stdin.read_exact(&mut body).is_err() {
            return Ok(());
        }
        // JSON from the extension has no raw newlines, but be sure: the pipe is line-framed.
        let line = String::from_utf8_lossy(&body).replace(['\n', '\r'], " ");
        let mut pipe = pipe.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(data) = pipe.as_mut() {
            if writeln!(data, "{line}").is_err() {
                *pipe = None;
            }
        }
        *last.lock().unwrap_or_else(|p| p.into_inner()) = Some(line);
    }
}
