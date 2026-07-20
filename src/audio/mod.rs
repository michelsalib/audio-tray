//! Audio backend abstraction.
//!
//! `Device` identity is the WASAPI endpoint ID string (`IMMDevice::GetId`), never the
//! friendly name — names collide, localize, and change; the ID is stable (plan §2.5).

pub mod notify;
pub mod switch;
pub mod wasapi;

/// Stable endpoint identity from `IMMDevice::GetId`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DeviceId(pub String);

/// Endpoint form factor. This is a *hint only* — Windows cannot distinguish wireless
/// earbuds from headphones, so the per-device icon mapping in Settings is authoritative
/// (plan §2.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormFactor {
    Speakers,
    Headphones,
    Headset,
    Spdif,
    DigitalDisplay,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct Device {
    pub id: DeviceId,
    /// `PKEY_Device_FriendlyName`.
    pub friendly_name: String,
    /// Hint only — see [`FormFactor`].
    pub form_factor: FormFactor,
}

pub trait AudioBackend {
    /// Active render (output) endpoints only.
    fn enumerate(&self) -> anyhow::Result<Vec<Device>>;
    /// Current default render endpoint (`eConsole` role).
    fn current_default(&self) -> anyhow::Result<DeviceId>;
    /// Set the default render endpoint across all three roles (plan §2.3).
    fn set_default(&self, id: &DeviceId) -> anyhow::Result<()>;
}
