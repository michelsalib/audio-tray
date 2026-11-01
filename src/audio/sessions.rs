//! Per-app audio sessions on the default output: the mixer's rows, read and set through
//! `IAudioSessionManager2`, plus a [`Watch`] that wakes the tray when an app opens a session.
//! Expiry raises nothing without per-session callbacks, so readers re-list when they look.

use anyhow::{Context, Result};
use windows::core::{implement, Interface};
use windows::Win32::Media::Audio::{
    eConsole, eRender, AudioSessionStateExpired, IAudioSessionControl, IAudioSessionControl2,
    IAudioSessionManager2, IAudioSessionNotification, IAudioSessionNotification_Impl,
    IMMDeviceEnumerator, ISimpleAudioVolume, MMDeviceEnumerator,
};
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_ALL};


use crate::hub::AppSession;

/// Posted (coalesced) to the tray's message window when an app opened a session.
pub const WM_SESSIONS_CHANGED: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 35;

/// Coalesces [`WM_SESSIONS_CHANGED`].
pub(crate) static CHANGED: crate::tray::Wake = crate::tray::Wake::new(WM_SESSIONS_CHANGED);

fn manager() -> Result<IAudioSessionManager2> {
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).context("IMMDeviceEnumerator")?;
        let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole).context("default output")?;
        device.Activate(CLSCTX_ALL, None).context("activate IAudioSessionManager2")
    }
}

/// Every live session on the default output except the system sounds, in Windows' order.
pub fn list() -> Result<Vec<AppSession>> {
    let mut found = Vec::new();
    for control in controls()? {
        if let Some(session) = describe(&control) {
            // One row per app: a browser opens a session per tab.
            match found.iter_mut().find(|s: &&mut AppSession| s.app == session.app) {
                Some(row) => row.volume = row.volume.max(session.volume),
                None => found.push(session),
            }
        }
    }
    Ok(found)
}

/// Set the volume of every session of the app `app` (rows are per app, see [`list`]).
pub fn set_volume(app: &str, volume: f32) -> Result<()> {
    for_app(app, |simple| unsafe { simple.SetMasterVolume(volume.clamp(0.0, 1.0), std::ptr::null()) })
}

pub fn set_muted(app: &str, muted: bool) -> Result<()> {
    for_app(app, |simple| unsafe { simple.SetMute(muted, std::ptr::null()) })
}

fn for_app(app: &str, mut set: impl FnMut(&ISimpleAudioVolume) -> windows::core::Result<()>) -> Result<()> {
    for control in controls()? {
        if describe(&control).is_some_and(|s| s.app == app) {
            let simple: ISimpleAudioVolume = control.cast().context("ISimpleAudioVolume")?;
            set(&simple).context("setting a session")?;
        }
    }
    Ok(())
}

fn controls() -> Result<Vec<IAudioSessionControl2>> {
    let manager = manager()?;
    let sessions = unsafe { manager.GetSessionEnumerator() }.context("GetSessionEnumerator")?;
    let count = unsafe { sessions.GetCount() }.context("GetCount")?;
    let mut out = Vec::new();
    for i in 0..count {
        let Ok(control) = (unsafe { sessions.GetSession(i) }) else { continue };
        if let Ok(control) = control.cast::<IAudioSessionControl2>() {
            out.push(control);
        }
    }
    Ok(out)
}

fn describe(control: &IAudioSessionControl2) -> Option<AppSession> {
    unsafe {
        // S_OK means it is the system sounds session; S_FALSE that it is not.
        if control.IsSystemSoundsSession() == windows::Win32::Foundation::S_OK {
            return None;
        }
        if control.GetState().ok()? == AudioSessionStateExpired {
            return None;
        }
        let pid = control.GetProcessId().unwrap_or(0);
        let key = control
            .GetSessionInstanceIdentifier()
            .ok()
            .map(|p| {
                let s = p.to_string().unwrap_or_default();
                CoTaskMemFree(Some(p.0 as *const _));
                s
            })
            .unwrap_or_default();
        let display = control
            .GetDisplayName()
            .ok()
            .map(|p| {
                let s = p.to_string().unwrap_or_default();
                CoTaskMemFree(Some(p.0 as *const _));
                s
            })
            .filter(|s| !s.is_empty() && !s.starts_with('@'));
        let app = display.unwrap_or_else(|| crate::win::process_exe(pid).map(|p| crate::hub::names::app_name(&p)).unwrap_or_else(|| format!("pid {pid}")));
        let simple: ISimpleAudioVolume = control.cast().ok()?;
        let volume = simple.GetMasterVolume().unwrap_or(1.0);
        let muted = simple.GetMute().map(|m| m.as_bool()).unwrap_or(false);
        Some(AppSession { key, app, pid, volume, muted })
    }
}


#[implement(IAudioSessionNotification)]
struct Created;

#[allow(non_snake_case)]
impl IAudioSessionNotification_Impl for Created_Impl {
    fn OnSessionCreated(&self, _session: windows::core::Ref<'_, IAudioSessionControl>) -> windows::core::Result<()> {
        CHANGED.post();
        Ok(())
    }
}

/// Wakes the tray when an app opens a session on the default output; re-made when the default
/// changes. Unregisters on drop.
pub struct Watch {
    manager: IAudioSessionManager2,
    client: IAudioSessionNotification,
}

impl Watch {
    pub fn new() -> Result<Self> {
        let manager = manager()?;
        // Notifications only start once the session list has been read (documented quirk).
        let _ = unsafe { manager.GetSessionEnumerator() };
        let client: IAudioSessionNotification = Created.into();
        unsafe { manager.RegisterSessionNotification(&client) }.context("RegisterSessionNotification")?;
        Ok(Self { manager, client })
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        let _ = unsafe { self.manager.UnregisterSessionNotification(&self.client) };
    }
}
