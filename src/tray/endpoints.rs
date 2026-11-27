//! The default endpoints: read once per refresh into an [`Endpoints`], which the hub, the strip,
//! the notification icon and the mute watches all take theirs from; and what the strip's middle
//! click does to them (mute, or switch to the next device), shown on the strip before the audio work.

use anyhow::Result;

use crate::audio::wasapi::{VolumeWatch, WasapiBackend};
use crate::audio::{mic, DeviceId, Flow};
use crate::config::Config;
use crate::hub::{Choice, Endpoint, Endpoints};
use crate::icons::IconId;
use crate::taskbar::{self, StripIcons};

use super::messages::WM_ENDPOINT_VOLUME;
use super::{Tray, AUDIO_CHANGED};

/// The name the notification icon gives when there is no default output to name.
const NO_OUTPUT: &str = "Audio output";

/// One flow's default (`None` when there is none, or it is not among the active devices) and its
/// active devices, in Windows' order: one `enumerate_flow`, one `default_of` and one `is_muted`.
fn read_flow(backend: &WasapiBackend, config: &Config, flow: Flow) -> (Option<Endpoint>, Vec<Choice>) {
    let devices = backend.enumerate_flow(flow).unwrap_or_default();
    let choices = devices
        .iter()
        .map(|d| Choice { id: d.id.clone(), name: d.friendly_name.clone(), icon: config.icon_of(d) })
        .collect();
    let default = backend.default_of(flow).ok().flatten().and_then(|id| {
        let device = devices.iter().find(|d| d.id == id)?;
        let muted = backend.is_muted(&id).unwrap_or(false);
        Some(Endpoint { icon: config.icon_of(device), name: device.friendly_name.clone(), muted, id })
    });
    (default, choices)
}

/// The defaults and every active device: what the hub is told, and what the strip and the icon show.
pub(crate) fn read(backend: &WasapiBackend, config: &Config) -> Endpoints {
    let (output, outputs) = read_flow(backend, config, Flow::Output);
    let (input, inputs) = read_flow(backend, config, Flow::Input);
    Endpoints { output, input, outputs, inputs }
}

/// The default output alone, for the notification icon before the first refresh.
pub(super) fn default_output(backend: &WasapiBackend, config: &Config) -> Option<Endpoint> {
    read_flow(backend, config, Flow::Output).0
}

/// The default output as the notification icon shows it: its name and its icon.
pub(super) fn output_face(output: Option<&Endpoint>) -> (&str, IconId) {
    output.map_or((NO_OUTPUT, IconId::Unknown), |e| (e.name.as_str(), e.icon))
}

/// What the strip draws for these defaults.
pub(super) fn strip_icons(endpoints: &Endpoints) -> StripIcons {
    let glyph = |e: Option<&Endpoint>| taskbar::strip_glyph(e.map_or(IconId::Unknown, |e| e.icon));
    let muted = |e: Option<&Endpoint>| e.is_some_and(|e| e.muted);
    StripIcons {
        output: glyph(endpoints.output.as_ref()),
        input: glyph(endpoints.input.as_ref()),
        output_muted: muted(endpoints.output.as_ref()),
        input_muted: muted(endpoints.input.as_ref()),
        // A property of the microphone, not of an endpoint; a cached atomic, not COM.
        input_recording: mic::in_use(),
    }
}

/// [`strip_icons`] for the defaults as they are now.
pub(super) fn current_strip_icons(backend: &WasapiBackend, config: &Config) -> StripIcons {
    strip_icons(&read(backend, config))
}

/// A volume/mute subscription on a default endpoint, with the mute last seen.
pub(super) struct MuteWatch {
    id: DeviceId,
    watch: VolumeWatch,
    muted: bool,
}

impl Tray {
    /// Watch the defaults' volume and mute, keeping existing subscriptions whose endpoint is still
    /// the default. A failure leaves that flow unwatched (the next refresh retries).
    pub(super) fn rearm_mute_watches(&mut self, endpoints: &Endpoints) {
        let receiver = taskbar::receiver().map_or(0, |hwnd| hwnd.0 as isize);
        if receiver == 0 {
            return;
        }
        let defaults: Vec<DeviceId> = [&endpoints.output, &endpoints.input].into_iter().flatten().map(|e| e.id.clone()).collect();
        self.mute_watches.retain(|w| defaults.contains(&w.id));
        for id in defaults {
            if self.mute_watches.iter().any(|w| w.id == id) {
                continue;
            }
            match self.backend.watch_volume(&id, receiver, WM_ENDPOINT_VOLUME, self.mute_pending.clone()) {
                Ok(watch) => {
                    let muted = watch.read().is_some_and(|(_, muted)| muted);
                    self.mute_watches.push(MuteWatch { id, watch, muted });
                }
                Err(e) => eprintln!("tray: cannot watch the volume of {} ({e:#})", id.0),
            }
        }
    }

    /// After a volume notification: refresh only if a watched mute flipped. Volume-only changes
    /// cost one read each and nothing else.
    pub(super) fn check_mutes(&mut self) {
        self.mute_pending.store(false, std::sync::atomic::Ordering::SeqCst);
        let mut flipped = false;
        for w in &mut self.mute_watches {
            if let Some((_, muted)) = w.watch.read() {
                flipped |= muted != w.muted;
                w.muted = muted;
            }
        }
        if flipped {
            AUDIO_CHANGED.post();
        }
    }

    /// Tell the strip what to draw unless it already is. Returns whether it can be taken to show
    /// `icons` now; a restyle that could not be posted (no control window yet) is not remembered.
    pub(super) fn push_strip(&mut self, icons: StripIcons) -> bool {
        if self.strip == Some(icons) {
            return true;
        }
        if taskbar::restyle(icons) {
            self.strip = Some(icons);
            return true;
        }
        false
    }

    /// Put the recording dot on (or off) without re-reading the devices. `false`: nothing recorded
    /// to amend, or the restyle did not go out — the caller falls back to a full refresh.
    pub(super) fn push_mic_state(&mut self, recording: bool) -> bool {
        let Some(mut icons) = self.strip else {
            return false;
        };
        icons.input_recording = recording;
        self.push_strip(icons)
    }

    /// Show where a strip click lands before doing the (slow) audio work behind it. `None` for
    /// `icon` keeps the glyph and changes only the mute. A no-op until a refresh recorded a state.
    fn preview_strip(&mut self, flow: Flow, icon: Option<IconId>, muted: bool) {
        let Some(mut icons) = self.strip else {
            return;
        };
        let glyph = icon.map(taskbar::strip_glyph);
        match flow {
            Flow::Output => {
                icons.output = glyph.unwrap_or(icons.output);
                icons.output_muted = muted;
            }
            Flow::Input => {
                icons.input = glyph.unwrap_or(icons.input);
                icons.input_muted = muted;
            }
        }
        self.push_strip(icons);
    }

    /// Mute or unmute `flow`'s default device: a segment's middle click under
    /// [`crate::config::MiddleClick::Mute`]. Like the cycle, the strip shows it before the audio work.
    pub(super) fn toggle_endpoint_mute(&mut self, flow: Flow) -> Result<()> {
        let Some(id) = self.backend.default_of(flow)? else { return Ok(()) };
        let muted = !self.backend.is_muted(&id)?;
        self.preview_strip(flow, None, muted);
        self.backend.set_muted(&id, muted)?;
        // A mute raises no endpoint notification: this is the strip's update.
        self.refresh();
        Ok(())
    }

    /// Switch `flow` to its next device (a segment's middle click), around a cycle of the active
    /// devices, each unmuted, plus (`mute_stop`) one muted stop on the last device before the wrap:
    ///
    /// ```text
    /// device 1 → device 2 → … → device N → device N, muted → device 1 → …
    /// ```
    ///
    /// Without `mute_stop`, device N wraps to device 1. Every device stop is unmuted: arriving on a
    /// muted device unmutes it, and stepping off the muted stop takes the mute with it, so the cycle
    /// never leaves a muted device behind.
    pub(super) fn cycle(&mut self, flow: Flow, mute_stop: bool) -> Result<()> {
        let devices = self.backend.enumerate_flow(flow)?;
        if devices.is_empty() {
            return Ok(());
        }
        let current = self.backend.default_of(flow)?;
        let at = current.as_ref().and_then(|id| devices.iter().position(|d| &d.id == id));
        let muted = current.as_ref().and_then(|id| self.backend.is_muted(id).ok()).unwrap_or(false);

        // Which device the click lands on, or `None` for the muted stop.
        let next = match at {
            Some(i) if muted => Some((i + 1) % devices.len()),
            Some(i) if i + 1 == devices.len() && mute_stop => None,
            Some(i) => Some((i + 1) % devices.len()),
            None => Some(0),
        };

        match next {
            Some(next) => self.preview_strip(flow, Some(self.config.icon_of(&devices[next])), false),
            None => self.preview_strip(flow, None, true),
        }

        match next {
            Some(next) => {
                let id = &devices[next].id;
                if current.as_ref() != Some(id) {
                    self.backend.set_default_of(id)?;
                }
                // Unmute the device we left *after* the switch, so it makes no sound.
                if muted {
                    if let Some(left) = current.as_ref().filter(|left| *left != id) {
                        self.backend.set_muted(left, false)?;
                    }
                }
                if self.backend.is_muted(id).unwrap_or(false) {
                    self.backend.set_muted(id, false)?;
                }
            }
            None => {
                if let Some(id) = &current {
                    self.backend.set_muted(id, true)?;
                }
            }
        }

        // Confirms the preview, and is the only strip update after a mute (which raises no
        // endpoint notification).
        self.refresh();
        Ok(())
    }
}

/// Push the current defaults to the strip, for the flyout's live changes. A no-op outside the
/// tray (the `--flyout` preview).
pub(crate) fn restyle_strip(backend: &WasapiBackend, config: &Config) {
    let icons = current_strip_icons(backend, config);
    super::with_tray(|tray| tray.push_strip(icons));
}
