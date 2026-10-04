//! `IMMNotificationClient` → wake the tray on endpoint changes (plan §8).
//!
//! Callbacks fire on a system-owned COM thread, so they don't touch the UI directly.
//! Instead each relevant change posts [`WM_AUDIO_REFRESH`] to the tray's message window
//! (coalesced: one switch fires a callback per role), which owns what the icon shows.

use anyhow::{Context, Result};
use windows::core::{implement, PCWSTR};
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Media::Audio::{
    EDataFlow, ERole, IMMDeviceEnumerator, IMMNotificationClient, IMMNotificationClient_Impl,
    MMDeviceEnumerator, DEVICE_STATE,
};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
use windows::Win32::UI::WindowsAndMessaging::WM_APP;

/// Posted to the tray thread when an endpoint change should trigger a refresh.
pub const WM_AUDIO_REFRESH: u32 = WM_APP + 1;

#[implement(IMMNotificationClient)]
struct NotifyClient;

impl NotifyClient {
    fn wake(&self) {
        crate::tray::AUDIO_CHANGED.post();
    }
}

#[allow(non_snake_case)]
impl IMMNotificationClient_Impl for NotifyClient_Impl {
    fn OnDeviceStateChanged(&self, _id: &PCWSTR, _state: DEVICE_STATE) -> windows::core::Result<()> {
        self.wake();
        Ok(())
    }
    fn OnDeviceAdded(&self, _id: &PCWSTR) -> windows::core::Result<()> {
        self.wake();
        Ok(())
    }
    fn OnDeviceRemoved(&self, _id: &PCWSTR) -> windows::core::Result<()> {
        self.wake();
        Ok(())
    }
    fn OnDefaultDeviceChanged(
        &self,
        _flow: EDataFlow,
        _role: ERole,
        _id: &PCWSTR,
    ) -> windows::core::Result<()> {
        self.wake();
        Ok(())
    }
    fn OnPropertyValueChanged(&self, _id: &PCWSTR, _key: &PROPERTYKEY) -> windows::core::Result<()> {
        // Ignore property churn (volume, etc.) to avoid refresh storms.
        Ok(())
    }
}

/// Keeps the callback registered for its lifetime; unregisters on drop.
pub struct Notifications {
    enumerator: IMMDeviceEnumerator,
    client: IMMNotificationClient,
}

impl Drop for Notifications {
    fn drop(&mut self) {
        unsafe {
            let _ = self
                .enumerator
                .UnregisterEndpointNotificationCallback(&self.client);
        }
    }
}

/// Register endpoint-change notifications that wake the tray via [`WM_AUDIO_REFRESH`].
/// COM must already be initialized on the calling thread.
pub fn register() -> Result<Notifications> {
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .context("create IMMDeviceEnumerator for notifications")?;
        let client: IMMNotificationClient = NotifyClient.into();
        enumerator
            .RegisterEndpointNotificationCallback(&client)
            .context("RegisterEndpointNotificationCallback")?;
        Ok(Notifications { enumerator, client })
    }
}
