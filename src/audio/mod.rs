//! The audio backend. `Device` identity is the endpoint ID (`IMMDevice::GetId`), never the friendly name.

pub mod battery;
pub mod mic;
pub mod notify;
pub mod switch;
pub mod wasapi;

/// Stable endpoint identity from `IMMDevice::GetId`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DeviceId(pub String);

/// Endpoint direction: playback (`eRender`) or recording (`eCapture`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Output,
    Input,
}

/// Endpoint form factor, a hint only: the per-device icon in Settings is authoritative.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormFactor {
    Speakers,
    Headphones,
    Headset,
    Microphone,
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
    /// `PKEY_Device_ContainerId` as an upper-case `{GUID}`; finds the battery node ([`battery`]).
    pub container_id: Option<String>,
}

/// `PKEY_Device_ContainerId` (unbound in the `windows` crate), read by both [`wasapi`] and
/// [`battery`], whose readings must compare equal.
pub(crate) const CONTAINER_ID_FMTID: u128 = 0x8C7E_D206_3F8A_4827_B3AB_AE9E_1FAE_FC6C;
pub(crate) const CONTAINER_ID_PID: u32 = 2;
