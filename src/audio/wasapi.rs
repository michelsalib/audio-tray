//! `windows`-crate implementation of [`AudioBackend`] read paths: enumerate active
//! render endpoints, read the current default, and read per-device properties
//! (friendly name, form factor). Switching lives in [`super::switch`] (plan §6).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use windows::core::{implement, PCWSTR, PWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Media::Audio::Endpoints::{
    IAudioEndpointVolume, IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl,
    IAudioMeterInformation,
};
use windows::Win32::Media::Audio::{
    eCapture, eCommunications, eConsole, eMultimedia, eRender, EDataFlow, ERole, IAudioCaptureClient,
    IAudioClient, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator, AUDCLNT_SHAREMODE_SHARED,
    AUDIO_VOLUME_NOTIFICATION_DATA, PKEY_AudioEndpoint_FormFactor, DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::StructuredStorage::{PropVariantToStringAlloc, PropVariantToUInt32};
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_ALL, STGM_READ};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use super::{AudioBackend, Device, DeviceId, Flow, FormFactor};

/// Posts `msg` to `hwnd` whenever an endpoint's volume or mute changes. The callback fires
/// on a WASAPI-owned thread, so it does nothing but post — the UI thread re-reads and
/// repaints. `hwnd` is stored as an `isize` to keep the COM object thread-agnostic.
///
/// `pending` coalesces: at most one message is queued at a time. Posted messages outrank
/// input in `GetMessage`, so an unthrottled storm (e.g. a microphone's automatic gain
/// control, which fires constantly) would starve the flyout of clicks and it could never
/// be dismissed. The UI clears the flag when it handles the message.
#[implement(IAudioEndpointVolumeCallback)]
struct VolCallback {
    hwnd: isize,
    msg: u32,
    pending: Arc<AtomicBool>,
}

#[allow(non_snake_case)]
impl IAudioEndpointVolumeCallback_Impl for VolCallback_Impl {
    fn OnNotify(&self, _data: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> windows::core::Result<()> {
        if !self.pending.swap(true, Ordering::SeqCst) {
            unsafe {
                let _ = PostMessageW(
                    Some(HWND(self.hwnd as *mut core::ffi::c_void)),
                    self.msg,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
        }
        Ok(())
    }
}

/// A live volume/mute-change subscription for one endpoint; unregisters on drop. Holds the
/// endpoint interface so the UI can re-read from it without re-activating COM.
pub struct VolumeWatch {
    endpoint: IAudioEndpointVolume,
    callback: IAudioEndpointVolumeCallback,
}

impl VolumeWatch {
    /// Current (volume 0.0..=1.0, muted) of the watched endpoint.
    pub fn read(&self) -> Option<(f32, bool)> {
        unsafe {
            let v = self.endpoint.GetMasterVolumeLevelScalar().ok()?;
            let m = self.endpoint.GetMute().ok()?.as_bool();
            Some((v, m))
        }
    }
}

impl Drop for VolumeWatch {
    fn drop(&mut self) {
        unsafe {
            let _ = self.endpoint.UnregisterControlChangeNotify(&self.callback);
        }
    }
}

/// A live peak-level meter for one endpoint, polled on a timer to drive the slider's
/// activity glow. Render and capture need different plumbing: a render endpoint's meter is
/// always live, but a capture endpoint's meter only reports while *something* is capturing,
/// so we hold a silent capture stream open for the duration (see [`CaptureMeter`]).
pub enum Meter {
    Render(RenderMeter),
    Capture(CaptureMeter),
}

impl Meter {
    /// Current peak sample value, 0.0..=1.0 (0 when silent).
    pub fn peak(&self) -> f32 {
        match self {
            Meter::Render(m) => m.peak(),
            Meter::Capture(m) => m.peak(),
        }
    }
}

/// Peak meter for a render endpoint (activated `IAudioMeterInformation`). The endpoint
/// meter aggregates every stream on the device, so it reflects whatever is playing without
/// us opening a stream of our own.
pub struct RenderMeter {
    meter: IAudioMeterInformation,
}

impl RenderMeter {
    fn peak(&self) -> f32 {
        unsafe { self.meter.GetPeakValue() }.unwrap_or(0.0).clamp(0.0, 1.0)
    }
}

/// Peak meter for a capture endpoint. A capture endpoint's `IAudioMeterInformation` is
/// dormant unless a capture stream is running (the same reason Windows' own mic level bar
/// only moves while the Sound page is open), so we open a silent shared-mode capture stream
/// and keep it running. Each poll drains and discards the queued frames (so the buffer
/// keeps flowing) and reads the endpoint peak. The stream stops on drop; while it lives,
/// Windows shows its "microphone in use" indicator, exactly as the Settings meter does.
pub struct CaptureMeter {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    meter: IAudioMeterInformation,
}

impl CaptureMeter {
    fn new(device: &IMMDevice) -> Result<Self> {
        unsafe {
            let client: IAudioClient =
                device.Activate(CLSCTX_ALL, None).context("activate IAudioClient")?;
            let fmt = client.GetMixFormat().context("GetMixFormat")?;
            // 200 ms shared-mode buffer; we drain it ~30x/s so it never overflows.
            let init = client.Initialize(AUDCLNT_SHAREMODE_SHARED, 0, 2_000_000, 0, fmt, None);
            CoTaskMemFree(Some(fmt as *const _));
            init.context("IAudioClient::Initialize (capture)")?;
            let capture: IAudioCaptureClient =
                client.GetService().context("GetService IAudioCaptureClient")?;
            let meter: IAudioMeterInformation =
                device.Activate(CLSCTX_ALL, None).context("activate IAudioMeterInformation")?;
            client.Start().context("IAudioClient::Start")?;
            Ok(CaptureMeter { client, capture, meter })
        }
    }

    fn peak(&self) -> f32 {
        unsafe {
            // Drain and discard queued packets so the capture buffer keeps flowing and the
            // meter stays current.
            while let Ok(frames) = self.capture.GetNextPacketSize() {
                if frames == 0 {
                    break;
                }
                let mut data = std::ptr::null_mut();
                let mut n = 0u32;
                let mut flags = 0u32;
                if self.capture.GetBuffer(&mut data, &mut n, &mut flags, None, None).is_err() {
                    break;
                }
                let _ = self.capture.ReleaseBuffer(n);
            }
            self.meter.GetPeakValue().unwrap_or(0.0).clamp(0.0, 1.0)
        }
    }
}

impl Drop for CaptureMeter {
    fn drop(&mut self) {
        unsafe {
            let _ = self.client.Stop();
        }
    }
}

impl Flow {
    fn data_flow(self) -> EDataFlow {
        match self {
            Flow::Output => eRender,
            Flow::Input => eCapture,
        }
    }
}

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

    /// Active endpoints for a direction (output = render, input = capture).
    pub fn enumerate_flow(&self, flow: Flow) -> Result<Vec<Device>> {
        unsafe {
            let collection = self
                .enumerator
                .EnumAudioEndpoints(flow.data_flow(), DEVICE_STATE_ACTIVE)
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

    /// Current default endpoint (eConsole role) for a direction, if one is set.
    pub fn default_of(&self, flow: Flow) -> Result<Option<DeviceId>> {
        unsafe {
            match self.enumerator.GetDefaultAudioEndpoint(flow.data_flow(), eConsole) {
                Ok(device) => Ok(Some(DeviceId(take_pwstr(device.GetId().context("GetId")?)?))),
                Err(_) => Ok(None),
            }
        }
    }

    /// Make an endpoint (any direction) the default across all three roles. The id itself
    /// encodes the direction, so `IPolicyConfig` handles capture devices the same way.
    pub fn set_default_of(&self, id: &DeviceId) -> Result<()> {
        super::switch::set_default(id)
    }

    /// Resolve an endpoint by its id string.
    fn device_by_id(&self, id: &DeviceId) -> Result<IMMDevice> {
        let wide: Vec<u16> = id.0.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe { self.enumerator.GetDevice(PCWSTR(wide.as_ptr())) }
            .with_context(|| format!("GetDevice({})", id.0))
    }

    /// Activate the volume/mute control for a specific endpoint.
    fn endpoint_volume(&self, id: &DeviceId) -> Result<IAudioEndpointVolume> {
        let device = self.device_by_id(id)?;
        unsafe { device.Activate(CLSCTX_ALL, None) }.context("activate IAudioEndpointVolume")
    }

    /// Master volume of a specific endpoint, 0.0..=1.0.
    pub fn volume_of(&self, id: &DeviceId) -> Result<f32> {
        unsafe { self.endpoint_volume(id)?.GetMasterVolumeLevelScalar() }.context("get volume")
    }

    /// Set the master volume of a specific endpoint (clamped to 0.0..=1.0).
    pub fn set_volume_of(&self, id: &DeviceId, level: f32) -> Result<()> {
        let level = level.clamp(0.0, 1.0);
        unsafe { self.endpoint_volume(id)?.SetMasterVolumeLevelScalar(level, std::ptr::null()) }
            .context("set volume")
    }

    /// Whether a specific endpoint is muted.
    pub fn is_muted(&self, id: &DeviceId) -> Result<bool> {
        Ok(unsafe { self.endpoint_volume(id)?.GetMute() }.context("get mute")?.as_bool())
    }

    /// Mute or unmute a specific endpoint.
    pub fn set_muted(&self, id: &DeviceId, muted: bool) -> Result<()> {
        unsafe { self.endpoint_volume(id)?.SetMute(muted, std::ptr::null()) }.context("set mute")
    }

    /// Open a live peak-level meter for a specific endpoint (for the activity glow). Output
    /// uses the always-live endpoint meter; input opens a silent capture stream so its
    /// otherwise-dormant meter reports — see [`Meter`].
    pub fn meter_for(&self, id: &DeviceId, flow: Flow) -> Result<Meter> {
        let device = self.device_by_id(id)?;
        match flow {
            Flow::Output => {
                let meter: IAudioMeterInformation = unsafe { device.Activate(CLSCTX_ALL, None) }
                    .context("activate IAudioMeterInformation")?;
                Ok(Meter::Render(RenderMeter { meter }))
            }
            Flow::Input => Ok(Meter::Capture(CaptureMeter::new(&device)?)),
        }
    }

    /// Subscribe to volume/mute changes on `id` from any source (media keys, other apps,
    /// us). `msg` is posted to `hwnd` on each change; the returned [`VolumeWatch`] keeps the
    /// subscription alive and unregisters when dropped.
    pub fn watch_volume(
        &self,
        id: &DeviceId,
        hwnd: isize,
        msg: u32,
        pending: Arc<AtomicBool>,
    ) -> Result<VolumeWatch> {
        let endpoint = self.endpoint_volume(id)?;
        let callback: IAudioEndpointVolumeCallback = VolCallback { hwnd, msg, pending }.into();
        unsafe { endpoint.RegisterControlChangeNotify(&callback) }
            .context("RegisterControlChangeNotify")?;
        Ok(VolumeWatch { endpoint, callback })
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

            // ContainerId groups all functions of the physical device; used to find its
            // Bluetooth battery node. Stored as a CLSID property → `{GUID}` string.
            let container_id = {
                use windows::Win32::Foundation::PROPERTYKEY;
                let key = PROPERTYKEY {
                    fmtid: windows::core::GUID::from_u128(0x8C7E_D206_3F8A_4827_B3AB_AE9E_1FAE_FC6C),
                    pid: 2,
                };
                store
                    .GetValue(&key)
                    .ok()
                    .and_then(|pv| {
                        PropVariantToStringAlloc(&pv).ok().and_then(|p| {
                            let out = p.to_string().ok();
                            CoTaskMemFree(Some(p.0 as *const _));
                            out
                        })
                    })
                    .filter(|s| !s.is_empty())
            };

            Ok(Device { id, friendly_name, form_factor, container_id })
        }
    }
}

impl AudioBackend for WasapiBackend {
    fn enumerate(&self) -> Result<Vec<Device>> {
        self.enumerate_flow(Flow::Output)
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
            Ok(4) => FormFactor::Microphone,
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
