//! Discord, through a Vencord user plugin (`integrations/vencord/audioTray`). Audio Tray hosts
//! `\\.\pipe\audio-tray`; the plugin's native half connects and writes one JSON object per line
//! describing the voice call, and reads commands (mute toggles) back.
//!
//!
//! The pipe is a [`super::pipe::Server`]: nothing runs until the plugin writes.
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

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::hub::{Call, Event, Member, VoiceApp};

pub const PIPE: &str = "audio-tray";
/// Its own name in the hub: Discord's RPC client reports as "Discord", and Settings reads both.
pub const NAME: &str = "Vencord";
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

/// The Discord side of the pipe: the call as the plugin last described it.
#[derive(Default)]
pub struct Discord {
    call: Option<Call>,
}

impl super::pipe::Handler for Discord {
    fn connected(&mut self) {
        println!("discord: plugin connected");
        crate::hub::emit(Event::Integration { name: NAME, connected: true });
    }

    fn disconnected(&mut self) {
        println!("discord: plugin went away");
        if self.call.take().is_some() && !super::discord_rpc::connected() {
            crate::hub::emit(Event::Call { app: VoiceApp::Discord, call: None });
        }
        crate::hub::emit(Event::Integration { name: NAME, connected: false });
    }

    fn line(&mut self, text: &str) -> Result<()> {
        match serde_json::from_str::<Message>(text).context("not a message")? {
            Message::Hello {} => {}
            // The RPC client, when connected, owns Discord's call: two sources overwrote each other.
            _ if super::discord_rpc::connected() => {}
            Message::Call { channel: None, .. } => {
                self.call = None;
                crate::hub::emit(Event::Call { app: VoiceApp::Discord, call: None });
            }
            Message::Call { channel, self_mute, self_deaf, members } => {
                let old = self.call.take();
                let members = members.into_iter().map(|m| member(m, old.as_ref())).collect();
                let call = Call { channel, members, self_muted: self_mute, self_deafened: self_deaf, icon: None };
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
pub(super) fn base64_decode(text: &str) -> Option<Vec<u8>> {
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
