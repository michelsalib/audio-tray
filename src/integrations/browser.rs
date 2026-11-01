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
//! App → extension: `{"type":"command","id":"12:0","command":"toggle"}`.

use std::sync::Arc;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::hub::{Event, Media};

pub const PIPE: &str = "audio-tray-browser";
pub const COMMAND_PIPE: &str = "audio-tray-browser-cmd";
const NAME: &str = "Browser";
/// The native messaging host's name, as the extension asks for it.
const HOST: &str = "com.audiotray.media";
/// The extension's id, fixed by the `key` in its manifest.
const EXTENSION_ID: &str = "eajikcpfpkjjbdfibhfnnacflhgklgll";
/// Prefix of the app id a browser tab's media carries, so commands find their way back.
pub const APP_ID_PREFIX: &str = "browser:";

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum Message {
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
}

impl super::pipe::Handler for Tabs {
    fn connected(&mut self) {
        println!("browser: extension connected");
        crate::hub::emit(Event::Integration { name: NAME, connected: true });
    }

    fn disconnected(&mut self) {
        println!("browser: extension went away");
        self.media.clear();
        crate::hub::emit(Event::BrowserMedia(Vec::new()));
        crate::hub::emit(Event::Integration { name: NAME, connected: false });
    }

    fn line(&mut self, text: &str) -> Result<()> {
        let Message::Tabs { tabs } = serde_json::from_str(text).context("not a message")?;
        let now = crate::music::smtc::now_ticks();
        let media: Vec<Media> = tabs
            .into_iter()
            .filter(|t| !t.title.trim().is_empty())
            .map(|t| {
                let app_id = format!("{APP_ID_PREFIX}{}", t.id);
                // Covers arrive as base64 each time: reuse the decoded bytes when unchanged.
                let old = self.media.iter().find(|m| m.app_id == app_id && m.title == t.title);
                let cover = match (old, t.art.as_deref()) {
                    (Some(old), Some(_)) if old.cover.is_some() => old.cover.clone(),
                    (_, art) => art.and_then(super::discord::base64_decode).map(Arc::new),
                };
                let ticks = |seconds: f64| (seconds.max(0.0) * 1e7) as i64;
                Media {
                    app: crate::hub::names::site_name(&t.site),
                    app_id,
                    title: t.title,
                    artist: t.artist,
                    playing: t.playing,
                    cover,
                    timeline: Some(crate::music::smtc::Timeline {
                        start: 0,
                        end: ticks(t.duration),
                        position: ticks(t.position),
                        last_updated: now,
                    }),
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
        crate::hub::emit(Event::BrowserMedia(media));
        Ok(())
    }
}

/// The command side: nothing comes in.
#[derive(Default)]
pub struct Commands;

impl super::pipe::Handler for Commands {
    fn line(&mut self, _: &str) -> Result<()> {
        Ok(())
    }
}

/// The JSON for a play/pause on tab frame `id` (the part after [`APP_ID_PREFIX`]).
pub fn command(id: &str) -> String {
    serde_json::json!({ "type": "command", "id": id, "command": "toggle" }).to_string()
}

/// Register this exe as the extension's native messaging host for Edge and Chrome (per user, no
/// administrator rights): a manifest next to the config, and a registry key naming it.
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
    for browser in [r"Microsoft\Edge", r"Google\Chrome"] {
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

/// Whether the browser launched us as the native messaging host: its first argument is the
/// calling extension's origin.
pub fn is_host_launch(args: &[String]) -> bool {
    args.first().is_some_and(|a| a.starts_with("chrome-extension://"))
}

/// Host mode: relay the extension's messages (stdin, length-prefixed JSON) to the tray's pipe,
/// and the tray's commands back (stdout). Never prints anything else to stdout: it is the
/// channel. Runs until the browser closes stdin; waits for the tray when it is not running.
pub fn run_host() -> Result<()> {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::sync::Mutex;

    let pipe = Arc::new(Mutex::new(None::<std::fs::File>));
    let last = Arc::new(Mutex::new(None::<String>));
    // The tray side: connect (and reconnect), replay the last list, pass commands to stdout.
    {
        let (pipe, last) = (Arc::clone(&pipe), Arc::clone(&last));
        std::thread::spawn(move || loop {
            let data = std::fs::OpenOptions::new().write(true).open(format!(r"\\.\pipe\{PIPE}"));
            let commands = std::fs::OpenOptions::new().read(true).open(format!(r"\\.\pipe\{COMMAND_PIPE}"));
            if let (Ok(mut data), Ok(commands)) = (data, commands) {
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
            // The tray is not running (or just quit): look again in a moment.
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
