//! Audio backend abstraction.
//!
//! `Device` identity is the WASAPI endpoint ID string (`IMMDevice::GetId`), never the
//! friendly name — names collide, localize, and change; the ID is stable (plan §2.5).

pub mod battery;
pub mod mic;
pub mod notify;
pub mod switch;
pub mod wasapi;

/// Stable endpoint identity from `IMMDevice::GetId`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DeviceId(pub String);

/// Endpoint direction — playback (`eRender`) vs recording (`eCapture`). The flyout shows
/// both; the tray icon still reflects the default *output* only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Output,
    Input,
}

/// Endpoint form factor. This is a *hint only* — Windows cannot distinguish wireless
/// earbuds from headphones, so the per-device icon mapping in Settings is authoritative
/// (plan §2.2).
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
    /// `PKEY_Device_ContainerId` as an upper-case `{GUID}` string — groups all functions of
    /// one physical device, used to find its Bluetooth battery node (see [`battery`]).
    pub container_id: Option<String>,
}

/// `PKEY_Device_ContainerId` / `DEVPKEY_Device_ContainerId`, which the `windows` crate binds
/// under neither name.
///
/// Named once because it is read twice, through two different property APIs and as two
/// different key types: off the audio endpoint's property store ([`wasapi`]) and off the
/// PnP device node ([`battery`]). The whole point is that the two answers can be compared,
/// so a typo in either would not fail — it would silently stop matching.
pub(crate) const CONTAINER_ID_FMTID: u128 = 0x8C7E_D206_3F8A_4827_B3AB_AE9E_1FAE_FC6C;
pub(crate) const CONTAINER_ID_PID: u32 = 2;

// There is deliberately no `AudioBackend` trait. It had one implementor and two methods,
// both of which duplicated a `WasapiBackend` inherent method that takes the direction as
// an argument — so every call site had to import the trait to reach a worse version of a
// method it already had.
