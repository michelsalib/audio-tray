//! `IMMNotificationClient` → wake the tray's message loop on endpoint changes (plan §8).
//!
//! Callbacks fire on a system-owned COM thread, so they don't touch the UI directly.
//! Instead each relevant change posts a [`WM_AUDIO_REFRESH`] thread-message to the tray
//! thread, which owns the single source of truth for what the icon/menu should show.

use anyhow::{Context, Result};
use windows::core::{implement, PCWSTR};
use windows::Win32::Foundation::{LPARAM, PROPERTYKEY, WPARAM};
use windows::Win32::Media::Audio::{
    EDataFlow, ERole, IMMDeviceEnumerator, IMMNotificationClient, IMMNotificationClient_Impl,
    MMDeviceEnumerator, DEVICE_STATE,
};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
use windows::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_APP};

/// Posted to the tray thread when an endpoint change should trigger a refresh.
pub const WM_AUDIO_REFRESH: u32 = WM_APP + 1;

#[implement(IMMNotificationClient)]
struct NotifyClient {
    thread_id: u32,
}

impl NotifyClient {
    fn wake(&self) {
        // Best-effort: a failure means the target queue is gone (we're shutting down).
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_AUDIO_REFRESH, WPARAM(0), LPARAM(0));
        }
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

/// Register endpoint-change notifications that wake `thread_id` via [`WM_AUDIO_REFRESH`].
/// COM must already be initialized on the calling thread.
pub fn register(thread_id: u32) -> Result<Notifications> {
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .context("create IMMDeviceEnumerator for notifications")?;
        let client: IMMNotificationClient = NotifyClient { thread_id }.into();
        enumerator
            .RegisterEndpointNotificationCallback(&client)
            .context("RegisterEndpointNotificationCallback")?;
        Ok(Notifications { enumerator, client })
    }
}
