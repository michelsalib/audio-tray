//! Optional per-app integrations. Each reports through [`crate::hub`] and takes commands back
//! (mute toggles); each can be off without touching the rest.

use crate::hub::VoiceApp;

/// Toggle the user's mute in a voice app.
pub fn toggle_mute(app: VoiceApp) {
    eprintln!("integrations: {} is not connected; cannot toggle its mute", app.label());
}

/// Set the user's mute in a voice app.
pub fn set_muted(app: VoiceApp, muted: bool) {
    eprintln!("integrations: {} is not connected; cannot set its mute to {muted}", app.label());
}
