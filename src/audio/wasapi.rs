//! `windows`-crate implementation of [`AudioBackend`] read paths: enumerate active
//! render endpoints, read the current default, and read per-device properties
//! (friendly name, form factor). Switching lives in [`super::switch`] (plan §6).

use anyhow::{Context, Result};
use windows::core::PWSTR;
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{
    eCommunications, eConsole, eMultimedia, eRender, ERole, IMMDevice, IMMDeviceEnumerator,
    MMDeviceEnumerator, PKEY_AudioEndpoint_FormFactor, DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::StructuredStorage::{PropVariantToStringAlloc, PropVariantToUInt32};
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_ALL, STGM_READ};

use super::{AudioBackend, Device, DeviceId, FormFactor};

/// Reads the WASAPI render-endpoint state. COM must already be initialized on the
/// calling thread (see `main`).
pub struct WasapiBackend {
    enumerator: IMMDeviceEnumerator,
}

impl WasapiBackend {
    pub fn new() -> Result<Self> {
        let enumerator: IMMDeviceEnumerator =
            unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
                .context("create IMMDeviceEnumerator")?;
        Ok(Self { enumerator })
    }

    /// Default render endpoint for a specific role. Returns `Ok(None)` when no default
    /// is set for that role (`GetDefaultAudioEndpoint` fails with e.g. E_NOTFOUND).
    pub fn default_for_role(&self, role: ERole) -> Result<Option<DeviceId>> {
        unsafe {
            match self.enumerator.GetDefaultAudioEndpoint(eRender, role) {
                Ok(device) => Ok(Some(DeviceId(take_pwstr(device.GetId().context("GetId")?)?))),
                Err(_) => Ok(None),
            }
        }
    }

    /// Nudge the current default endpoint's master volume one step up/down (uses the
    /// endpoint's own step increment, matching the volume keys).
    pub fn step_volume(&self, up: bool) -> Result<()> {
        unsafe {
            let device = self
                .enumerator
                .GetDefaultAudioEndpoint(eRender, eConsole)
                .context("default endpoint for volume")?;
            let volume: IAudioEndpointVolume = device
                .Activate(CLSCTX_ALL, None)
                .context("activate IAudioEndpointVolume")?;
            if up {
                volume.VolumeStepUp(std::ptr::null())
            } else {
                volume.VolumeStepDown(std::ptr::null())
            }
            .context("volume step")?;
        }
        Ok(())
    }

    /// Master volume of the current default endpoint, 0.0..=1.0.
    pub fn master_volume(&self) -> Result<f32> {
        unsafe {
            let device = self
                .enumerator
                .GetDefaultAudioEndpoint(eRender, eConsole)
                .context("default endpoint for volume")?;
            let volume: IAudioEndpointVolume = device
                .Activate(CLSCTX_ALL, None)
                .context("activate IAudioEndpointVolume")?;
            volume.GetMasterVolumeLevelScalar().context("get master volume")
        }
    }

    /// The default for each of the three roles Windows tracks independently.
    pub fn defaults_by_role(&self) -> [(&'static str, Result<Option<DeviceId>>); 3] {
        [
            ("eConsole", self.default_for_role(eConsole)),
            ("eMultimedia", self.default_for_role(eMultimedia)),
            ("eCommunications", self.default_for_role(eCommunications)),
        ]
    }

    /// Read the friendly name + form factor of an already-resolved endpoint.
    fn describe(&self, device: &IMMDevice) -> Result<Device> {
        unsafe {
            let id = DeviceId(take_pwstr(device.GetId().context("GetId")?)?);
            let store = device
                .OpenPropertyStore(STGM_READ)
                .context("OpenPropertyStore")?;

            let friendly_name = {
                let pv = store.GetValue(&PKEY_Device_FriendlyName).context("friendly name")?;
                let s = PropVariantToStringAlloc(&pv).ok().and_then(|p| {
                    let out = p.to_string().ok();
                    CoTaskMemFree(Some(p.0 as *const _));
                    out
                });
                s.unwrap_or_else(|| "(unknown)".to_string())
            };

            let form_factor = read_form_factor(&store);

            Ok(Device { id, friendly_name, form_factor })
        }
    }
}

impl AudioBackend for WasapiBackend {
    fn enumerate(&self) -> Result<Vec<Device>> {
        unsafe {
            let collection = self
                .enumerator
                .EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)
                .context("EnumAudioEndpoints")?;
            let count = collection.GetCount().context("GetCount")?;
            let mut devices = Vec::with_capacity(count as usize);
            for i in 0..count {
                let device = collection.Item(i).with_context(|| format!("Item({i})"))?;
                devices.push(self.describe(&device)?);
            }
            Ok(devices)
        }
    }

    fn current_default(&self) -> Result<DeviceId> {
        self.default_for_role(eConsole)?
            .context("no default render endpoint for eConsole role")
    }

    fn set_default(&self, id: &DeviceId) -> Result<()> {
        super::switch::set_default(id)
    }
}

/// Read `PKEY_AudioEndpoint_FormFactor` and map to our [`FormFactor`]. Any failure or
/// unrecognized value collapses to [`FormFactor::Unknown`] — it's only a hint (plan §2.2).
fn read_form_factor(store: &windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore) -> FormFactor {
    unsafe {
        let Ok(pv) = store.GetValue(&PKEY_AudioEndpoint_FormFactor) else {
            return FormFactor::Unknown;
        };
        match PropVariantToUInt32(&pv) {
            // Values from the Win32 `EndpointFormFactor` enum (stable ABI).
            Ok(1) => FormFactor::Speakers,
            Ok(3) => FormFactor::Headphones,
            Ok(5) => FormFactor::Headset,
            Ok(8) => FormFactor::Spdif,
            Ok(9) => FormFactor::DigitalDisplay,
            _ => FormFactor::Unknown,
        }
    }
}

/// Take ownership of a COM-allocated wide string: decode to `String`, then free with
/// `CoTaskMemFree` (required for `IMMDevice::GetId` and `PropVariantToStringAlloc`).
unsafe fn take_pwstr(p: PWSTR) -> Result<String> {
    if p.is_null() {
        return Ok(String::new());
    }
    let s = p.to_string().context("utf16 decode")?;
    CoTaskMemFree(Some(p.0 as *const _));
    Ok(s)
}
