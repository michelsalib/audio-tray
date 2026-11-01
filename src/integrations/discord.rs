//! Discord, through a Vencord user plugin (`integrations/vencord/audioTray`). Audio Tray hosts
//! `\\.\pipe\audio-tray`; the plugin's native half connects and writes one JSON object per line
//! describing the voice call, and reads commands (mute toggles) back.
//!
//! The pipe is overlapped and its event is waited on by the tray thread ([`crate::wait`]): nothing
//! runs until the plugin writes. Local clients only.
//!
//! Plugin → app:
//! ```text
//! {"type":"call","channel":"General","selfMute":false,"selfDeaf":false,
//!  "members":[{"id":"1","name":"Ana","avatar":"<base64 png>","speaking":false,"muted":false,"me":true}]}
//! {"type":"call","channel":null}                       (left the call)
//! {"type":"speaking","id":"1","speaking":true}
//! {"type":"selfMute","muted":true}
//! ```
//! App → plugin: `{"type":"toggleMute"}`, `{"type":"setMute","muted":false}`.

use std::sync::Arc;

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use windows::core::w;
use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_IO_PENDING, ERROR_PIPE_CONNECTED, HANDLE};
use windows::Win32::Storage::FileSystem::{ReadFile, WriteFile, FILE_FLAG_OVERLAPPED, PIPE_ACCESS_DUPLEX};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_WAIT,
};
use windows::Win32::System::Threading::CreateEventW;
use windows::Win32::System::IO::{CancelIo, GetOverlappedResult, OVERLAPPED};

use crate::hub::{Call, Event, Member, VoiceApp};

const NAME: &str = "Discord";
const BUFFER: usize = 64 * 1024;
/// A line longer than this (avatars are a few kB each) is a broken client, not a call.
const MAX_LINE: usize = 4 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum Message {
    #[serde(rename_all = "camelCase")]
    Call {
        channel: Option<String>,
        #[serde(default)]
        self_mute: bool,
        #[serde(default)]
        self_deaf: bool,
        #[serde(default)]
        members: Vec<WireMember>,
    },
    Speaking { id: String, speaking: bool },
    SelfMute { muted: bool },
    Hello {},
}

#[derive(Deserialize)]
struct WireMember {
    id: String,
    name: String,
    avatar: Option<String>,
    #[serde(default)]
    speaking: bool,
    #[serde(default)]
    muted: bool,
    #[serde(default)]
    me: bool,
}

/// The pipe and where its overlapped I/O stands. Lives on the tray thread.
pub struct Server {
    pipe: HANDLE,
    event: HANDLE,
    overlapped: Box<OVERLAPPED>,
    buffer: Box<[u8; BUFFER]>,
    pending: Vec<u8>,
    connected: bool,
    call: Option<Call>,
}

impl Server {
    pub fn new() -> Result<Self> {
        unsafe {
            let pipe = CreateNamedPipeW(
                w!(r"\\.\pipe\audio-tray"),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                BUFFER as u32,
                BUFFER as u32,
                0,
                None,
            );
            if pipe.is_invalid() {
                bail!("CreateNamedPipe: {:?}", GetLastError());
            }
            let event = CreateEventW(None, true, false, None).context("CreateEvent")?;
            let mut server = Self {
                pipe,
                event,
                overlapped: Box::new(OVERLAPPED { hEvent: event, ..Default::default() }),
                buffer: Box::new([0; BUFFER]),
                pending: Vec::new(),
                connected: false,
                call: None,
            };
            server.listen();
            Ok(server)
        }
    }

    pub fn event(&self) -> HANDLE {
        self.event
    }

    /// Wait for the plugin to connect.
    fn listen(&mut self) {
        self.reset();
        unsafe {
            match ConnectNamedPipe(self.pipe, Some(&mut *self.overlapped)) {
                Ok(()) => self.on_connected(),
                Err(_) if GetLastError() == ERROR_PIPE_CONNECTED => self.on_connected(),
                Err(_) if GetLastError() == ERROR_IO_PENDING => {}
                Err(e) => eprintln!("discord: cannot listen on the pipe ({e})"),
            }
        }
    }

    fn reset(&mut self) {
        let _ = unsafe { windows::Win32::System::Threading::ResetEvent(self.event) };
        *self.overlapped = OVERLAPPED { hEvent: self.event, ..Default::default() };
    }

    fn on_connected(&mut self) {
        println!("discord: plugin connected");
        self.connected = true;
        self.pending.clear();
        crate::hub::emit(Event::Integration { name: NAME, connected: true });
        self.read();
    }

    /// Start the next overlapped read; completions arrive through [`Server::signalled`].
    fn read(&mut self) {
        self.reset();
        unsafe {
            match ReadFile(self.pipe, Some(&mut self.buffer[..]), None, Some(&mut *self.overlapped)) {
                // Completed at once: the event is set too, so the wait loop delivers it.
                Ok(()) => {}
                Err(_) if GetLastError() == ERROR_IO_PENDING => {}
                Err(_) => self.disconnected(),
            }
        }
    }

    /// The event fired: a connection or a read completed.
    pub fn signalled(&mut self) {
        let mut moved = 0u32;
        let done = unsafe { GetOverlappedResult(self.pipe, &*self.overlapped, &mut moved, false) };
        if !self.connected {
            match done {
                Ok(()) => self.on_connected(),
                Err(_) => self.listen(),
            }
            return;
        }
        if done.is_err() {
            return self.disconnected();
        }
        self.pending.extend_from_slice(&self.buffer[..moved as usize]);
        while let Some(end) = self.pending.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=end).collect();
            if let Err(e) = self.line(&line) {
                eprintln!("discord: ignoring a line ({e:#})");
            }
        }
        if self.pending.len() > MAX_LINE {
            eprintln!("discord: a line over {MAX_LINE} bytes; dropping the client");
            return self.disconnected();
        }
        self.read();
    }

    fn disconnected(&mut self) {
        println!("discord: plugin went away");
        self.connected = false;
        unsafe {
            let _ = DisconnectNamedPipe(self.pipe);
        }
        if self.call.take().is_some() {
            crate::hub::emit(Event::Call { app: VoiceApp::Discord, call: None });
        }
        crate::hub::emit(Event::Integration { name: NAME, connected: false });
        self.listen();
    }

    fn line(&mut self, line: &[u8]) -> Result<()> {
        let text = std::str::from_utf8(line).context("not UTF-8")?.trim();
        if text.is_empty() {
            return Ok(());
        }
        match serde_json::from_str::<Message>(text).context("not a message")? {
            Message::Hello {} => {}
            Message::Call { channel: None, .. } => {
                self.call = None;
                crate::hub::emit(Event::Call { app: VoiceApp::Discord, call: None });
            }
            Message::Call { channel, self_mute, self_deaf, members } => {
                let old = self.call.take();
                let members = members.into_iter().map(|m| member(m, old.as_ref())).collect();
                let call = Call { channel, members, self_muted: self_mute, self_deafened: self_deaf };
                self.call = Some(call.clone());
                crate::hub::emit(Event::Call { app: VoiceApp::Discord, call: Some(call) });
            }
            Message::Speaking { id, speaking } => {
                if let Some(call) = self.call.as_mut() {
                    if let Some(m) = call.members.iter_mut().find(|m| m.id == id) {
                        if m.speaking != speaking {
                            m.speaking = speaking;
                            crate::hub::emit(Event::Call { app: VoiceApp::Discord, call: Some(call.clone()) });
                        }
                    }
                }
            }
            Message::SelfMute { muted } => {
                if let Some(call) = self.call.as_mut() {
                    call.self_muted = muted;
                }
                crate::hub::emit(Event::SelfMuted { app: VoiceApp::Discord, muted });
            }
        }
        Ok(())
    }

    /// Send a command to the plugin. Best-effort; small writes complete at once.
    pub fn send(&mut self, json: &str) {
        if !self.connected {
            eprintln!("discord: no plugin connected; {json} not sent");
            return;
        }
        let line = format!("{json}\n");
        unsafe {
            let Ok(event) = CreateEventW(None, true, false, None) else { return };
            let mut overlapped = OVERLAPPED { hEvent: event, ..Default::default() };
            let mut written = 0u32;
            let result = match WriteFile(self.pipe, Some(line.as_bytes()), None, Some(&mut overlapped)) {
                Ok(()) => Ok(()),
                Err(_) if GetLastError() == ERROR_IO_PENDING => GetOverlappedResult(self.pipe, &overlapped, &mut written, true),
                Err(e) => Err(e),
            };
            let _ = CloseHandle(event);
            if let Err(e) = result {
                eprintln!("discord: could not send a command ({e})");
            }
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        unsafe {
            let _ = CancelIo(self.pipe);
            let _ = DisconnectNamedPipe(self.pipe);
            let _ = CloseHandle(self.pipe);
            let _ = CloseHandle(self.event);
        }
    }
}

/// A member from the wire, keeping the previous avatar when the plugin sent none this time.
fn member(wire: WireMember, old: Option<&Call>) -> Member {
    let avatar = wire
        .avatar
        .as_deref()
        .and_then(base64_decode)
        .map(Arc::new)
        .or_else(|| old.and_then(|c| c.members.iter().find(|m| m.id == wire.id)).and_then(|m| m.avatar.clone()));
    Member { id: wire.id, name: wire.name, avatar, speaking: wire.speaking, muted: wire.muted, me: wire.me }
}

/// Standard base64 (with or without padding); `None` when it is not.
fn base64_decode(text: &str) -> Option<Vec<u8>> {
    fn value(c: u8) -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    }
    let bytes: Vec<u8> = text.bytes().filter(|&b| b != b'=').collect();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        let mut acc = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            acc |= value(c)? << (18 - 6 * i);
        }
        let n = chunk.len();
        if n < 2 {
            return None;
        }
        out.push((acc >> 16) as u8);
        if n > 2 {
            out.push((acc >> 8) as u8);
        }
        if n > 3 {
            out.push(acc as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips_with_and_without_padding() {
        assert_eq!(base64_decode("aGVsbG8=").as_deref(), Some(&b"hello"[..]));
        assert_eq!(base64_decode("aGVsbG8").as_deref(), Some(&b"hello"[..]));
        assert_eq!(base64_decode("aGk=").as_deref(), Some(&b"hi"[..]));
        assert_eq!(base64_decode("not base64!"), None);
    }

    #[test]
    fn messages_parse_from_the_plugins_wire_format() {
        let call: Message = serde_json::from_str(
            r#"{"type":"call","channel":"General","selfMute":true,"selfDeaf":false,"members":[{"id":"1","name":"Ana","avatar":null,"speaking":true,"muted":false,"me":true}]}"#,
        )
        .unwrap();
        match call {
            Message::Call { channel, self_mute, members, .. } => {
                assert_eq!(channel.as_deref(), Some("General"));
                assert!(self_mute);
                assert!(members[0].me && members[0].speaking);
            }
            _ => panic!("not a call"),
        }
        assert!(matches!(serde_json::from_str::<Message>(r#"{"type":"call","channel":null}"#).unwrap(), Message::Call { channel: None, .. }));
        assert!(matches!(serde_json::from_str::<Message>(r#"{"type":"selfMute","muted":false}"#).unwrap(), Message::SelfMute { muted: false }));
    }
}
