//! Teams, through its local third-party app API: a WebSocket on `127.0.0.1:8124` that the user
//! turns on in Teams (Settings > Privacy > Manage API). Unpaired, the first command sent during a
//! meeting asks the user to allow Audio Tray inside Teams, which then hands over a token kept in
//! config: so when Teams says pairing is possible, a harmless `query-meeting-state` is sent. The
//! wire format is only community-documented; it reports the meeting and the user's mute, not who
//! is there.
//!
//! The socket's readiness is a `WSAEventSelect` event waited on by the tray thread. Connecting is
//! attempted at start-up and whenever Teams takes the microphone (a meeting starting), never on a
//! timer.

use std::io::ErrorKind;
use std::net::TcpStream;
use std::os::windows::io::AsRawSocket;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::Deserialize;
use tungstenite::{Message, WebSocket};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Networking::WinSock::{
    WSACloseEvent, WSACreateEvent, WSAEnumNetworkEvents, WSAEventSelect, FD_CLOSE, FD_READ, FD_WRITE, SOCKET, WSAEVENT,
    WSANETWORKEVENTS,
};

use crate::hub::{Call, Event, VoiceApp};

/// Its name in the hub, for Settings' status.
pub const NAME: &str = "Teams";
const PORT: u16 = 8124;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Incoming {
    token_refresh: Option<String>,
    meeting_update: Option<MeetingUpdate>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MeetingUpdate {
    meeting_state: Option<MeetingState>,
    meeting_permissions: Option<MeetingPermissions>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct MeetingPermissions {
    /// In a meeting, unpaired: a command now shows Teams' "allow" prompt.
    #[serde(default)]
    can_pair: bool,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct MeetingState {
    #[serde(default)]
    is_muted: bool,
    #[serde(default)]
    is_in_meeting: bool,
}

pub struct Client {
    socket: WebSocket<TcpStream>,
    event: WSAEVENT,
    in_meeting: bool,
    request: u32,
    /// Whether Teams handed over a token (now or on an earlier run).
    paired: bool,
    /// Whether this connection already asked Teams to pair.
    asked: bool,
}

/// The blocking half of connecting: TCP and the WebSocket handshake, with the saved token when
/// there is one. Off the tray thread (a refused or filtered port costs the whole timeout).
pub fn handshake(token: Option<&str>) -> Result<WebSocket<TcpStream>> {
    let address = std::net::SocketAddr::from(([127, 0, 0, 1], PORT));
    let stream = TcpStream::connect_timeout(&address, Duration::from_millis(300)).context("Teams is not listening")?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    let version = env!("CARGO_PKG_VERSION");
    let url = format!(
        "ws://127.0.0.1:{PORT}?token={}&protocol-version=2.0.0&manufacturer=Audio%20Tray&device=Audio%20Tray&app=Audio%20Tray&app-version={version}",
        token.unwrap_or("")
    );
    let (socket, _) = tungstenite::client(url.as_str(), stream).map_err(|e| anyhow::anyhow!("WebSocket handshake: {e}"))?;
    Ok(socket)
}

/// The tray-thread half: drive a handshaken `socket` from an event the tray waits on. `paired`:
/// it was opened with a token.
pub fn client(socket: WebSocket<TcpStream>, paired: bool) -> Result<Client> {
    // From here every read is driven by the event; WSAEventSelect makes the socket non-blocking.
    let event = unsafe { WSACreateEvent() }.context("WSACreateEvent")?;
    let raw = SOCKET(socket.get_ref().as_raw_socket() as usize);
    // FD_WRITE too: a send that would block stays queued in tungstenite until the next flush.
    if unsafe { WSAEventSelect(raw, Some(event), (FD_READ | FD_WRITE | FD_CLOSE) as i32) } != 0 {
        unsafe {
            let _ = WSACloseEvent(event);
        }
        anyhow::bail!("WSAEventSelect failed");
    }
    println!("teams: connected");
    crate::hub::emit(Event::Integration { name: NAME, connected: true });
    let mut client = Client { socket, event, in_meeting: false, request: 0, paired, asked: false };
    // The meeting as it is now; unpaired and in a meeting, this also asks to pair.
    client.action("query-meeting-state");
    client.asked = !client.paired;
    Ok(client)
}

/// Whether Teams is running (new Teams or classic), to tell "the API is off" from "Teams is not
/// open" when nothing listens. Only asked after a failed connection.
pub fn running() -> bool {
    let mut found = false;
    crate::win::enum_windows(|hwnd| {
        found = matches!(crate::win::window_process(hwnd).as_deref(), Some("ms-teams.exe" | "teams.exe"));
        !found
    });
    found
}

impl Client {
    pub fn event(&self) -> HANDLE {
        HANDLE(self.event.0 as *mut core::ffi::c_void)
    }

    /// The socket has data, or closed. Returns `false` once the connection is gone.
    pub fn signalled(&mut self) -> bool {
        let raw = SOCKET(self.socket.get_ref().as_raw_socket() as usize);
        let mut events = WSANETWORKEVENTS::default();
        // Also resets the event.
        unsafe { WSAEnumNetworkEvents(raw, self.event, &mut events) };
        loop {
            match self.socket.read() {
                Ok(Message::Text(text)) => self.message(text.as_str()),
                Ok(Message::Close(_)) => return self.closed(),
                Ok(_) => {}
                Err(tungstenite::Error::Io(e)) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) => {
                    eprintln!("teams: connection lost ({e})");
                    return self.closed();
                }
            }
        }
        // Pongs queued by the reader go out now.
        let _ = self.socket.flush();
        if events.lNetworkEvents & FD_CLOSE as i32 != 0 {
            return self.closed();
        }
        true
    }

    fn closed(&mut self) -> bool {
        if self.in_meeting {
            crate::hub::emit(Event::Call { app: VoiceApp::Teams, call: None });
        }
        crate::hub::emit(Event::Integration { name: NAME, connected: false });
        false
    }

    fn message(&mut self, text: &str) {
        let Ok(incoming) = serde_json::from_str::<Incoming>(text) else {
            return;
        };
        if let Some(token) = incoming.token_refresh {
            println!("teams: Audio Tray was allowed; keeping the token");
            self.paired = true;
            crate::tray::set_teams_token(token);
        }
        let update = incoming.meeting_update;
        let can_pair = update.as_ref().and_then(|u| u.meeting_permissions.as_ref()).is_some_and(|p| p.can_pair);
        if can_pair && !self.paired && !self.asked {
            println!("teams: asking to pair (Teams shows an allow prompt)");
            self.asked = true;
            self.action("query-meeting-state");
        }
        if let Some(state) = update.and_then(|u| u.meeting_state) {
            let was = std::mem::replace(&mut self.in_meeting, state.is_in_meeting);
            let call = state.is_in_meeting.then(|| Call { self_muted: state.is_muted, ..Default::default() });
            if call.is_some() || was {
                crate::hub::emit(Event::Call { app: VoiceApp::Teams, call });
            }
        }
    }

    /// Send an action (`toggle-mute`, `mute`, `unmute`). The first one pairs Audio Tray.
    pub fn action(&mut self, action: &str) {
        self.request += 1;
        let json = format!(r#"{{"action":"{action}","parameters":{{}},"requestId":{}}}"#, self.request);
        if let Err(e) = self.socket.send(Message::text(json)) {
            eprintln!("teams: could not send {action} ({e})");
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.socket.close(None);
        let _ = self.socket.flush();
        unsafe {
            let _ = WSACloseEvent(self.event);
        }
    }
}
