//! Discord through its own local RPC: no client mod. Audio Tray talks to the running Discord
//! over `\\.\pipe\discord-ipc-N` as the user's own developer application (`[discord]`
//! `client_id` / `client_secret`): the voice scopes are granted to the application's owner and
//! testers. Discord asks the user once to authorise it; the token is kept in
//! `discord-token.json` beside the config and refreshed as it expires.
//!
//! One thread, blocked on the pipe while nothing happens. It connects at start-up and whenever
//! Discord takes the microphone or opens an audio session ([`poke`]), never on a timer. Frames are
//! `op: u32, length: u32` (little endian) and a JSON body; ops 0 handshake, 1 frame, 2 close,
//! 3 ping, 4 pong.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_IO_PENDING, GENERIC_READ, GENERIC_WRITE, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, WriteFile, FILE_FLAG_OVERLAPPED, FILE_SHARE_NONE, OPEN_EXISTING,
};
use windows::Win32::System::Threading::CreateEventW;
use windows::Win32::System::IO::{GetOverlappedResult, OVERLAPPED};

use crate::hub::{Call, Event, Member, VoiceApp};

const NAME: &str = "Discord";
const SCOPES: [&str; 3] = ["rpc", "rpc.voice.read", "rpc.voice.write"];
/// The redirect the application must register (Discord insists on one, even over IPC); the code
/// exchange names it.
pub const REDIRECT: &str = "http://localhost";

/// The open pipe, shared with the tray thread for commands (overlapped, so a write never waits
/// behind the reader's pending read).
/// The flag says the session is being closed from outside (Discord turned off, new keys): no new
/// I/O starts, and the pending one was cancelled.
struct Pipe(HANDLE, std::sync::atomic::AtomicBool);
// The handle is a kernel object; overlapped I/O on it from two threads is supported.
unsafe impl Send for Pipe {}
unsafe impl Sync for Pipe {}

impl Pipe {
    fn open() -> Option<Self> {
        (0..10).find_map(|i| {
            let path = crate::win::wide(&format!(r"\\.\pipe\discord-ipc-{i}"));
            let handle = unsafe {
                CreateFileW(
                    PCWSTR(path.as_ptr()),
                    (GENERIC_READ | GENERIC_WRITE).0,
                    FILE_SHARE_NONE,
                    None,
                    OPEN_EXISTING,
                    FILE_FLAG_OVERLAPPED,
                    None,
                )
            };
            handle.ok().map(|handle| Pipe(handle, std::sync::atomic::AtomicBool::new(false)))
        })
    }

    /// Close the session from another thread: its blocked read returns, and nothing new starts.
    fn cancel(&self) {
        use windows::Win32::System::IO::CancelIoEx;
        self.1.store(true, std::sync::atomic::Ordering::SeqCst);
        unsafe {
            let _ = CancelIoEx(self.0, None);
        }
    }

    fn cancelled(&self) -> bool {
        self.1.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// One overlapped read or write, waited for.
    fn io(&self, read: Option<&mut [u8]>, write: Option<&[u8]>) -> Result<usize> {
        if self.cancelled() {
            bail!("the session was closed");
        }
        unsafe {
            let event = CreateEventW(None, true, false, None).context("CreateEvent")?;
            let mut overlapped = OVERLAPPED { hEvent: event, ..Default::default() };
            let started = match (read, write) {
                (Some(buf), _) => ReadFile(self.0, Some(buf), None, Some(&mut overlapped)),
                (_, Some(buf)) => WriteFile(self.0, Some(buf), None, Some(&mut overlapped)),
                _ => Ok(()),
            };
            let mut moved = 0u32;
            let done = match started {
                Ok(()) => GetOverlappedResult(self.0, &overlapped, &mut moved, true),
                Err(_) if GetLastError() == ERROR_IO_PENDING => GetOverlappedResult(self.0, &overlapped, &mut moved, true),
                Err(e) => Err(e),
            };
            let _ = CloseHandle(event);
            done.context("pipe I/O")?;
            Ok(moved as usize)
        }
    }

    fn read_exact(&self, buf: &mut [u8]) -> Result<()> {
        let mut at = 0;
        while at < buf.len() {
            let n = self.io(Some(&mut buf[at..]), None)?;
            if n == 0 {
                bail!("Discord closed the pipe");
            }
            at += n;
        }
        Ok(())
    }

    fn write_frame(&self, op: u32, body: &Value) -> Result<()> {
        let text = body.to_string();
        let mut frame = Vec::with_capacity(8 + text.len());
        frame.extend_from_slice(&op.to_le_bytes());
        frame.extend_from_slice(&(text.len() as u32).to_le_bytes());
        frame.extend_from_slice(text.as_bytes());
        self.io(None, Some(&frame)).map(|_| ())
    }

    fn read_frame(&self) -> Result<(u32, Value)> {
        let mut header = [0u8; 8];
        self.read_exact(&mut header)?;
        let op = u32::from_le_bytes(header[..4].try_into().expect("4 bytes"));
        let len = u32::from_le_bytes(header[4..].try_into().expect("4 bytes")) as usize;
        if len > 16 * 1024 * 1024 {
            bail!("a {len}-byte frame from Discord");
        }
        let mut body = vec![0u8; len];
        self.read_exact(&mut body)?;
        Ok((op, serde_json::from_slice(&body).unwrap_or(Value::Null)))
    }
}

impl Drop for Pipe {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// What the tray thread reaches: the live pipe (for commands) and the user's mute.
#[derive(Default)]
struct Shared {
    pipe: Option<Arc<Pipe>>,
    muted: bool,
    /// The application's id and secret, when Discord is on and both are set.
    app: Option<(String, String)>,
    /// The open session's pipe from the moment it opens (authorising included), to close it when
    /// the application changes.
    live: Option<Arc<Pipe>>,
    /// The last attempt failed on something only the user can fix (consent refused, bad keys, no
    /// redirect): automatic attempts stop until [`retry`] or new keys.
    blocked: bool,
}

fn shared() -> &'static Mutex<Shared> {
    static SHARED: OnceLock<Mutex<Shared>> = OnceLock::new();
    SHARED.get_or_init(Default::default)
}

static POKE: OnceLock<Sender<()>> = OnceLock::new();

/// Follow `[discord]`: connect as the application it names, now and on later pokes. Called again
/// when the keys change (Settings > Connectors); a token granted to another application is
/// dropped. The thread starts with the first application and lives with the process.
pub fn configure(settings: &crate::config::Discord) {
    let app = match (settings.enabled, &settings.client_id, &settings.client_secret) {
        (true, Some(id), Some(secret)) => Some((id.clone(), secret.clone())),
        _ => None,
    };
    let old = {
        let mut shared = shared().lock().unwrap_or_else(|p| p.into_inner());
        let old = std::mem::replace(&mut shared.app, app.clone());
        if old != app {
            shared.blocked = false;
            // A session as the old application (or with Discord now off) ends here.
            if let Some(live) = &shared.live {
                live.cancel();
            }
        }
        old
    };
    if old != app {
        crate::hub::emit(Event::IntegrationProblem { name: NAME, problem: None });
    }
    if let (Some((old_id, _)), Some((id, _))) = (&old, &app) {
        if old_id != id {
            Token::forget();
        }
    }
    if app.is_none() || old == app {
        return;
    }
    let poke = POKE.get_or_init(|| {
        let (poke, pokes) = std::sync::mpsc::channel();
        let _ = std::thread::Builder::new().name("discord".into()).spawn(move || run(pokes));
        poke
    });
    let _ = poke.send(());
}

/// The user asked (Settings opened its connectors): try again even after a refusal.
pub fn retry() {
    shared().lock().unwrap_or_else(|p| p.into_inner()).blocked = false;
    poke();
}

/// Try to connect now (Discord took the microphone or opened an audio session). Free when
/// connected, off, or stopped on a refusal (Discord would ask for consent again and again).
pub fn poke() {
    let idle = {
        let shared = shared().lock().unwrap_or_else(|p| p.into_inner());
        shared.live.is_none() && !shared.blocked
    };
    if idle {
        if let Some(poke) = POKE.get() {
            let _ = poke.send(());
        }
    }
}


/// The fixable cause behind a failed session, from Discord's own wording.
fn problem_of(error: &str) -> Option<&'static str> {
    let error = error.to_ascii_lowercase();
    if error.contains("redirect") {
        Some("Add the redirect http://localhost")
    } else if error.contains("invalid client id") || error.contains("4000") {
        Some("Wrong application ID")
    } else if error.contains("invalid_client") {
        Some("Wrong client secret")
    } else if error.contains("access_denied") || error.contains("cancel") {
        Some("Refused in Discord")
    } else {
        None
    }
}

/// Whether the client is connected and authenticated.
pub fn connected() -> bool {
    shared().lock().unwrap_or_else(|p| p.into_inner()).pipe.is_some()
}

/// Mute (`Some(true)`), unmute, or toggle (`None`) the user in Discord.
pub fn set_mute(mute: Option<bool>) {
    let (pipe, muted) = {
        let shared = shared().lock().unwrap_or_else(|p| p.into_inner());
        (shared.pipe.clone(), shared.muted)
    };
    let Some(pipe) = pipe else { return };
    let mute = mute.unwrap_or(!muted);
    let request = json!({ "cmd": "SET_VOICE_SETTINGS", "args": { "mute": mute }, "nonce": nonce() });
    if let Err(e) = pipe.write_frame(1, &request) {
        eprintln!("discord: could not set the mute ({e:#})");
    }
}

fn nonce() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst).to_string()
}

/// The thread: wait for a poke, connect, serve until Discord goes away, repeat.
fn run(pokes: Receiver<()>) {
    while pokes.recv().is_ok() {
        // Several pokes may have queued while connected: one attempt for all.
        while pokes.try_recv().is_ok() {}
        let Some((id, secret)) = shared().lock().unwrap_or_else(|p| p.into_inner()).app.clone() else { continue };
        let Some(pipe) = Pipe::open() else { continue };
        let pipe = Arc::new(pipe);
        shared().lock().unwrap_or_else(|p| p.into_inner()).live = Some(Arc::clone(&pipe));
        let mut session = Session {
            pipe: Arc::clone(&pipe),
            id,
            secret,
            me: String::new(),
            channel: None,
            call: None,
            avatars: HashMap::new(),
        };
        if let Err(e) = session.serve() {
            eprintln!("discord: {e:#}");
            // Only a failure before connecting says something about the setup.
            if !connected() && !pipe.cancelled() {
                let problem = problem_of(&format!("{e:#}"));
                shared().lock().unwrap_or_else(|p| p.into_inner()).blocked = problem.is_some();
                crate::hub::emit(Event::IntegrationProblem { name: NAME, problem });
            }
        }
        {
            let mut shared = shared().lock().unwrap_or_else(|p| p.into_inner());
            shared.pipe = None;
            shared.live = None;
        }
        if session.call.is_some() {
            crate::hub::emit(Event::Call { app: VoiceApp::Discord, call: None });
        }
        crate::hub::emit(Event::Integration { name: NAME, connected: false });
    }
}

struct Session {
    pipe: Arc<Pipe>,
    id: String,
    secret: String,
    me: String,
    channel: Option<String>,
    call: Option<Call>,
    /// Faces fetched, by user id and avatar hash.
    avatars: HashMap<(String, String), Option<Arc<Vec<u8>>>>,
}

impl Session {
    fn serve(&mut self) -> Result<()> {
        self.pipe.write_frame(0, &json!({ "v": 1, "client_id": self.id }))?;
        let (_, ready) = self.pipe.read_frame()?;
        if ready["evt"] != "READY" {
            bail!("Discord answered the handshake with {ready}");
        }
        self.me = ready["data"]["user"]["id"].as_str().unwrap_or_default().to_string();
        self.authenticate()?;
        println!("discord: connected over RPC");
        shared().lock().unwrap_or_else(|p| p.into_inner()).pipe = Some(Arc::clone(&self.pipe));
        crate::hub::emit(Event::IntegrationProblem { name: NAME, problem: None });
        crate::hub::emit(Event::Integration { name: NAME, connected: true });

        self.request("SUBSCRIBE", json!({}), Some("VOICE_CHANNEL_SELECT"))?;
        self.request("SUBSCRIBE", json!({}), Some("VOICE_SETTINGS_UPDATE"))?;
        let settings = self.request("GET_VOICE_SETTINGS", json!({}), None)?;
        self.set_self_mute(settings["mute"].as_bool().unwrap_or(false));
        let channel = self.request("GET_SELECTED_VOICE_CHANNEL", json!({}), None)?;
        self.joined(&channel)?;
        loop {
            let (op, frame) = self.pipe.read_frame()?;
            match op {
                2 => bail!("Discord closed the connection: {frame}"),
                3 => self.pipe.write_frame(4, &frame)?,
                _ => self.dispatch(&frame)?,
            }
        }
    }

    /// Send a command and wait for its answer, handling events that arrive meanwhile.
    fn request(&mut self, cmd: &str, args: Value, evt: Option<&str>) -> Result<Value> {
        let nonce = nonce();
        let mut body = json!({ "cmd": cmd, "args": args, "nonce": nonce });
        if let Some(evt) = evt {
            body["evt"] = json!(evt);
        }
        self.pipe.write_frame(1, &body)?;
        loop {
            let (op, frame) = self.pipe.read_frame()?;
            if op == 2 {
                bail!("Discord closed the connection: {frame}");
            }
            if frame["nonce"] == nonce {
                if frame["evt"] == "ERROR" {
                    bail!("{cmd}: {}", frame["data"]["message"]);
                }
                return Ok(frame["data"].clone());
            }
            self.dispatch(&frame)?;
        }
    }

    /// The saved token, refreshed when stale; else authorise (Discord asks the user) and exchange.
    fn authenticate(&mut self) -> Result<()> {
        if let Some(token) = Token::load() {
            let token = if token.stale() { token.refresh(&self.id, &self.secret).ok() } else { Some(token) };
            if let Some(token) = token {
                if self.request("AUTHENTICATE", json!({ "access_token": token.access }), None).is_ok() {
                    return Ok(());
                }
            }
        }
        println!("discord: asking you to authorise Audio Tray in Discord");
        // The consent prompt in Discord. No redirect here: only the WebSocket transport takes one,
        // and over IPC Discord uses the application's first registered redirect (which must exist)
        // and the code exchange below names it.
        let args = json!({ "client_id": self.id, "scopes": SCOPES, "response_type": "code", "prompt": "consent" });
        let code = self.request("AUTHORIZE", args, None)?;
        let code = code["code"].as_str().context("AUTHORIZE gave no code")?.to_string();
        let token = Token::exchange(&self.id, &self.secret, &[("grant_type", "authorization_code"), ("code", &code)])?;
        self.request("AUTHENTICATE", json!({ "access_token": token.access }), None)?;
        Ok(())
    }

    fn dispatch(&mut self, frame: &Value) -> Result<()> {
        let data = &frame["data"];
        match frame["evt"].as_str().unwrap_or_default() {
            "VOICE_CHANNEL_SELECT" => {
                let channel = match data["channel_id"].as_str() {
                    Some(id) => self.request("GET_CHANNEL", json!({ "channel_id": id }), None)?,
                    None => Value::Null,
                };
                self.joined(&channel)?;
            }
            "VOICE_SETTINGS_UPDATE" => self.set_self_mute(data["mute"].as_bool().unwrap_or(false)),
            "VOICE_STATE_CREATE" | "VOICE_STATE_UPDATE" => {
                let member = self.member(data);
                if let Some(call) = self.call.as_mut() {
                    match call.members.iter_mut().find(|m| m.id == member.id) {
                        Some(old) => *old = Member { speaking: old.speaking, ..member },
                        None => call.members.push(member),
                    }
                    self.emit();
                }
            }
            "VOICE_STATE_DELETE" => {
                let id = data["user"]["id"].as_str().unwrap_or_default().to_string();
                if let Some(call) = self.call.as_mut() {
                    call.members.retain(|m| m.id != id);
                    self.emit();
                }
            }
            evt @ ("SPEAKING_START" | "SPEAKING_STOP") => {
                let id = data["user_id"].as_str().unwrap_or_default();
                if let Some(m) = self.call.as_mut().and_then(|c| c.members.iter_mut().find(|m| m.id == id)) {
                    m.speaking = evt == "SPEAKING_START";
                    self.emit();
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// A channel was joined (or left, `Null`): follow its events and report who is there.
    fn joined(&mut self, channel: &Value) -> Result<()> {
        if let Some(old) = self.channel.take() {
            for evt in ["VOICE_STATE_CREATE", "VOICE_STATE_UPDATE", "VOICE_STATE_DELETE", "SPEAKING_START", "SPEAKING_STOP"] {
                let _ = self.request("UNSUBSCRIBE", json!({ "channel_id": old }), Some(evt));
            }
        }
        let Some(id) = channel["id"].as_str().map(str::to_string) else {
            println!("discord: not in a voice channel");
            if self.call.take().is_some() {
                crate::hub::emit(Event::Call { app: VoiceApp::Discord, call: None });
            }
            return Ok(());
        };
        for evt in ["VOICE_STATE_CREATE", "VOICE_STATE_UPDATE", "VOICE_STATE_DELETE", "SPEAKING_START", "SPEAKING_STOP"] {
            self.request("SUBSCRIBE", json!({ "channel_id": id }), Some(evt))?;
        }
        let states = channel["voice_states"].as_array().cloned().unwrap_or_default();
        self.fetch_faces(&states);
        let members = states.iter().map(|state| self.member(state)).collect();
        let self_muted = shared().lock().unwrap_or_else(|p| p.into_inner()).muted;
        self.call = Some(Call {
            channel: channel["name"].as_str().map(str::to_string),
            members,
            self_muted,
            self_deafened: false,
            icon: None,
        });
        if let Some(call) = &self.call {
            println!("discord: in {:?} with {} member(s)", call.channel.as_deref().unwrap_or("a call"), call.members.len());
        }
        self.channel = Some(id);
        self.emit();
        Ok(())
    }

    /// Fetch the faces of `states` not fetched yet, all at once: one after the other, a full
    /// channel took seconds before the call showed, and Discord's pings waited.
    fn fetch_faces(&mut self, states: &[Value]) {
        let missing: Vec<(String, String)> = states
            .iter()
            .filter_map(|state| Some((state["user"]["id"].as_str()?.to_string(), state["user"]["avatar"].as_str()?.to_string())))
            .filter(|key| !self.avatars.contains_key(key))
            .take(16)
            .collect();
        let fetched: Vec<_> = std::thread::scope(|scope| {
            let handles: Vec<_> = missing
                .iter()
                .map(|(id, hash)| scope.spawn(move || fetch(&format!("https://cdn.discordapp.com/avatars/{id}/{hash}.png?size=64")).ok().map(Arc::new)))
                .collect();
            handles.into_iter().map(|h| h.join().ok().flatten()).collect()
        });
        self.avatars.extend(missing.into_iter().zip(fetched));
    }

    /// A member from a voice state, with their face (fetched once per avatar).
    fn member(&mut self, state: &Value) -> Member {
        let user = &state["user"];
        let id = user["id"].as_str().unwrap_or_default().to_string();
        let name = state["nick"]
            .as_str()
            .or(user["global_name"].as_str())
            .or(user["username"].as_str())
            .unwrap_or("Someone")
            .to_string();
        let voice = &state["voice_state"];
        let muted = [voice["mute"].as_bool(), voice["self_mute"].as_bool(), state["mute"].as_bool()].into_iter().flatten().any(|m| m);
        let avatar = user["avatar"].as_str().map(str::to_string).and_then(|hash| {
            self.avatars
                .entry((id.clone(), hash.clone()))
                .or_insert_with(|| fetch(&format!("https://cdn.discordapp.com/avatars/{id}/{hash}.png?size=64")).ok().map(Arc::new))
                .clone()
        });
        Member { me: id == self.me, id, name, avatar, speaking: false, muted }
    }

    fn set_self_mute(&mut self, muted: bool) {
        let changed = {
            let mut shared = shared().lock().unwrap_or_else(|p| p.into_inner());
            std::mem::replace(&mut shared.muted, muted) != muted
        };
        if let Some(call) = self.call.as_mut() {
            call.self_muted = muted;
            if let Some(me) = call.members.iter_mut().find(|m| m.me) {
                me.muted = muted;
            }
        }
        if changed {
            crate::hub::emit(Event::SelfMuted { app: VoiceApp::Discord, muted });
        }
    }

    fn emit(&self) {
        crate::hub::emit(Event::Call { app: VoiceApp::Discord, call: self.call.clone() });
    }
}

/// The OAuth token, kept beside the config (not in it: the tray rewrites the config).
struct Token {
    access: String,
    refresh: String,
    /// Seconds since the epoch.
    expires: u64,
}

impl Token {
    fn path() -> Option<std::path::PathBuf> {
        Some(crate::config::Config::path().ok()?.parent()?.join("discord-token.json"))
    }

    fn load() -> Option<Self> {
        let value: Value = serde_json::from_str(&std::fs::read_to_string(Self::path()?).ok()?).ok()?;
        Some(Self {
            access: value["access_token"].as_str()?.to_string(),
            refresh: value["refresh_token"].as_str()?.to_string(),
            expires: value["expires_at"].as_u64()?,
        })
    }

    /// The application changed: its token is worthless to the new one.
    fn forget() {
        if let Some(path) = Self::path() {
            let _ = std::fs::remove_file(path);
        }
    }

    fn save(&self) {
        let Some(path) = Self::path() else { return };
        let body = json!({ "access_token": self.access, "refresh_token": self.refresh, "expires_at": self.expires });
        if let Err(e) = std::fs::write(&path, body.to_string()) {
            eprintln!("discord: could not keep the token ({e})");
        }
    }

    fn stale(&self) -> bool {
        now() + 3600 >= self.expires
    }

    fn refresh(&self, id: &str, secret: &str) -> Result<Self> {
        Self::exchange(id, secret, &[("grant_type", "refresh_token"), ("refresh_token", &self.refresh)])
    }

    /// Discord's token endpoint, for a code or a refresh; the result is saved.
    fn exchange(id: &str, secret: &str, grant: &[(&str, &str)]) -> Result<Self> {
        let mut form: Vec<(&str, &str)> = vec![("client_id", id), ("client_secret", secret), ("redirect_uri", REDIRECT)];
        form.extend_from_slice(grant);
        // A refusal's body says why (`invalid_client`, a redirect mismatch): read it, not the status.
        let mut response = agent()
            .post("https://discord.com/api/oauth2/token")
            .config()
            .http_status_as_error(false)
            .build()
            .send_form(form)
            .context("the token exchange")?;
        let value: Value = serde_json::from_str(&response.body_mut().read_to_string()?).context("the token answer")?;
        if let Some(error) = value["error"].as_str() {
            bail!("the token exchange: {error}: {}", value["error_description"].as_str().unwrap_or_default());
        }
        let token = Self {
            access: value["access_token"].as_str().context("no access token")?.to_string(),
            refresh: value["refresh_token"].as_str().unwrap_or_default().to_string(),
            expires: now() + value["expires_in"].as_u64().unwrap_or(0),
        };
        token.save();
        Ok(token)
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// One agent for the process: its connections (and their TLS sessions) are kept and reused.
fn agent() -> ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(new_agent).clone()
}

fn new_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        // SChannel with Windows' own certificate store (ureq's default roots are webpki's, not built).
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .provider(ureq::tls::TlsProvider::NativeTls)
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .build(),
        )
        .timeout_global(Some(Duration::from_secs(15)))
        .build()
        .into()
}

fn fetch(url: &str) -> Result<Vec<u8>> {
    let mut response = agent().get(url).call()?;
    Ok(response.body_mut().with_config().limit(2 * 1024 * 1024).read_to_vec()?)
}

#[cfg(test)]
mod tests {
    use super::problem_of;

    #[test]
    fn discords_refusals_become_what_to_fix() {
        let missing = r#"AUTHORIZE: "OAuth2 Error: invalid_request: Missing redirect_uri""#;
        assert_eq!(problem_of(missing), Some("Add the redirect http://localhost"));
        let mismatch = r#"the token exchange: invalid_request: Invalid "redirect_uri" in request."#;
        assert_eq!(problem_of(mismatch), Some("Add the redirect http://localhost"));
        let handshake = r#"Discord answered the handshake with {"code":4000,"message":"Invalid Client ID"}"#;
        assert_eq!(problem_of(handshake), Some("Wrong application ID"));
        assert_eq!(problem_of("the token exchange: invalid_client: "), Some("Wrong client secret"));
        assert_eq!(problem_of("Discord closed the pipe"), None, "Discord quitting is nothing to fix");
    }
}
