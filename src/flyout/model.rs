//! The flyout's *display model*: plain data describing what the panel shows — the output
//! and input groups, their devices, the current level/mute/peak of each default endpoint,
//! the active screen, and the outcome flags accumulated while the flyout is open.
//!
//! It deliberately holds no Win32/COM handles (those — the live volume watches and peak
//! meters — stay on the controller), so the model is pure data: cheap to build, easy to
//! reason about, and constructible in the layout unit tests.

use crate::audio::wasapi::WasapiBackend;
use crate::audio::{DeviceId, Flow};
use crate::config::Config;
use crate::icons::{self, IconId};

use super::layout::View;
use super::Trigger;

pub(super) struct DeviceRow {
    pub id: DeviceId,
    pub label: String,
    pub icon: IconId,
    pub selected: bool,
    pub battery: Option<u8>, // Bluetooth battery 0..=100, if the device reports one
}

pub(super) struct Group {
    pub flow: Flow,
    pub title: &'static str,
    pub default_id: Option<DeviceId>,
    pub level: f32, // 0.0..=1.0 of the default endpoint
    pub muted: bool,
    pub peak: f32, // smoothed live peak level 0.0..=1.0 of the default endpoint (activity glow)
    pub devices: Vec<DeviceRow>,
}

/// Everything the flyout paints, plus the outcome flags the caller reads back when it
/// closes. No live audio handles — see the module docs.
pub(super) struct Model {
    pub trigger: Trigger,
    pub groups: Vec<Group>,
    pub view: View, // which screen is shown (main panel / an icon picker)
    pub update: Option<String>, // staged update's version, if any → bottom "restart to update" row
    // outcome, accumulated while the flyout is open
    pub config_changed: bool,
    pub output_changed: bool,
    pub quit: bool,
    pub restart: bool,
}

impl Model {
    pub(super) fn new(trigger: Trigger, groups: Vec<Group>, update: Option<String>) -> Self {
        Model {
            trigger,
            groups,
            view: View::Main,
            update,
            config_changed: false,
            output_changed: false,
            quit: false,
            restart: false,
        }
    }

    /// The label for the restart-to-update banner, if an update is staged.
    pub(super) fn update_label(&self) -> Option<String> {
        self.update.as_ref().map(|v| format!("Restart to update to v{v}"))
    }
}

/// Read the current output + input state into display groups. Groups with no devices are
/// omitted (e.g. a machine with no microphone shows no Input section).
pub(super) fn build_groups(backend: &WasapiBackend, config: &Config) -> Vec<Group> {
    // Bluetooth battery levels keyed by ContainerId — enumerated once for all devices.
    let batteries = crate::audio::battery::levels();
    let battery_of = |container: &Option<String>| -> Option<u8> {
        let c = container.as_ref()?;
        batteries
            .iter()
            .find(|(id, _)| id.eq_ignore_ascii_case(c))
            .map(|(_, pct)| *pct)
    };

    let mut groups = Vec::new();
    for (flow, title) in [(Flow::Output, "Output"), (Flow::Input, "Input")] {
        let devices = backend.enumerate_flow(flow).unwrap_or_default();
        if devices.is_empty() {
            continue;
        }
        let default_id = backend.default_of(flow).ok().flatten();
        let (level, muted) = match &default_id {
            Some(id) => (
                backend.volume_of(id).unwrap_or(0.0),
                backend.is_muted(id).unwrap_or(false),
            ),
            None => (0.0, false),
        };
        let rows = devices
            .into_iter()
            .map(|d| {
                let icon = config
                    .icon_for(&d.id.0)
                    .unwrap_or_else(|| icons::default_icon(d.form_factor, &d.friendly_name));
                let selected = default_id.as_ref() == Some(&d.id);
                let battery = battery_of(&d.container_id);
                DeviceRow { id: d.id, label: d.friendly_name, icon, selected, battery }
            })
            .collect();
        groups.push(Group { flow, title, default_id, level, muted, peak: 0.0, devices: rows });
    }
    groups
}
