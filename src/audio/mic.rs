//! "Is an app recording right now?" — the state behind the red dot on the mic icon.
//!
//! Read from the Capability Access Manager's consent store (`HKCU` and `HKLM`), the record the
//! shell's own indicator uses, and from the capture endpoints' active audio sessions: some Windows
//! builds stop writing the store (November 2026: Teams in a meeting, absent from it), and a
//! session is the capture itself. A watcher thread blocks in `RegNotifyChangeKeyValue` and on the
//! sessions' own events, recomputes, and posts [`WM_MIC_CHANGED`] to the tray; [`in_use`] is a
//! cached atomic, cheap enough to sample per frame. Who holds the microphone is also reported to
//! the hub as [`crate::hub::Event::MicUsers`].

use std::sync::atomic::{AtomicBool, Ordering};

use windows::core::{implement, w, Interface, PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Media::Audio::{
    eCapture, AudioSessionDisconnectReason, AudioSessionState, AudioSessionStateActive, AudioSessionStateExpired,
    IAudioSessionControl, IAudioSessionControl2, IAudioSessionEvents, IAudioSessionEvents_Impl, IAudioSessionManager2,
    IAudioSessionNotification, IAudioSessionNotification_Impl, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator,
    DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED};
use windows::Win32::System::Registry::{
    RegCloseKey, RegEnumKeyExW, RegGetValueW, RegNotifyChangeKeyValue, RegOpenKeyExW, HKEY,
    HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_NOTIFY, KEY_READ, REG_NOTIFY_CHANGE_LAST_SET,
    REG_NOTIFY_CHANGE_NAME, REG_SAM_FLAGS, RRF_RT_REG_QWORD,
};
use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForMultipleObjects};
use windows::Win32::UI::WindowsAndMessaging::WM_APP;

/// Posted (coalesced) to the tray's message window when the answer to [`in_use`] changes.
pub const WM_MIC_CHANGED: u32 = WM_APP + 3;

/// The consent store's microphone branch, under both `HKCU` and `HKLM`.
const CONSENT_STORE: PCWSTR =
    w!(r"SOFTWARE\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone");

/// The two roots the store is split across, in the order they are reported.
const ROOTS: [HKEY; 2] = [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE];

/// Walk depth below a root: packaged apps sit one level down, desktop apps two (`NonPackaged\<exe>`);
/// one level of slack, and bounded because [`users`] runs on the caller's thread.
const MAX_DEPTH: u32 = 3;

/// The cached answer, kept current by [`watch`].
static IN_USE: AtomicBool = AtomicBool::new(false);

/// Whether the watcher has been started, so [`in_use`] starts exactly one.
static WATCHING: AtomicBool = AtomicBool::new(false);

/// Whether any app has the microphone open right now. The first call starts the watcher
/// after one synchronous sweep; later calls are an atomic load.
pub fn in_use() -> bool {
    if !WATCHING.swap(true, Ordering::SeqCst) {
        IN_USE.store(!users().is_empty(), Ordering::SeqCst);
        let _ = std::thread::Builder::new().name("mic".into()).spawn(watch);
    }
    IN_USE.load(Ordering::SeqCst)
}

/// The apps holding the microphone (exe path or package family name). A live registry and
/// session sweep: keep it off paint paths and use [`in_use`] there. COM must be initialised.
pub fn users() -> Vec<String> {
    merge(store_users(), Capture::default().sweep())
}

/// The apps the consent store says hold the microphone.
fn store_users() -> Vec<String> {
    let mut found = Vec::new();
    for root in ROOTS {
        if let Some(key) = open(root, CONSENT_STORE, KEY_READ) {
            collect(&key, "", MAX_DEPTH, &mut found);
        }
    }
    found
}

/// The store's users, then the capturing exes it missed. One app can be in both, under its
/// package name in one and its exe in the other, so the app's name decides.
fn merge(mut store: Vec<String>, captured: Vec<String>) -> Vec<String> {
    use crate::hub::names::app_name;
    for exe in captured {
        let name = app_name(&exe);
        if !store.iter().any(|user| app_name(user) == name) {
            store.push(exe);
        }
    }
    store
}

/// Walk `key` and its subkeys, recording every app that has the microphone open.
fn collect(key: &Key, path: &str, depth: u32, found: &mut Vec<String>) {
    // Every key is tested (the root has no timestamps), so app entries can sit at any depth.
    if holding(key) {
        found.push(label(path));
    }
    if depth == 0 {
        return;
    }
    for name in subkeys(key) {
        let child_path = if path.is_empty() {
            name.clone()
        } else {
            format!(r"{path}\{name}")
        };
        // Bound to a local: a `PCWSTR` into a temporary would dangle before the call.
        let name_w = crate::win::wide(&name);
        if let Some(child) = open(key.0, PCWSTR(name_w.as_ptr()), KEY_READ) {
            collect(&child, &child_path, depth - 1, found);
        }
    }
}

/// Whether this key's app has the microphone open now. `stop < start`, not `stop == 0`:
/// a writer may leave the previous session's stop behind.
fn holding(key: &Key) -> bool {
    let start = qword(key, w!("LastUsedTimeStart")).unwrap_or(0);
    let stop = qword(key, w!("LastUsedTimeStop")).unwrap_or(0);
    start != 0 && stop < start
}

/// A consent-store key path for display: drops `NonPackaged\`, turns `#` back into `\`.
fn label(path: &str) -> String {
    path.trim_start_matches(r"NonPackaged\").replace('#', r"\")
}

/// The watcher thread: block on the store, recompute when it changes, announce a flip.
/// Runs for the life of the process.
fn watch() {
    // The capture sessions' COM objects live on this thread; their callbacks need no pump.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let keys: Vec<Key> = ROOTS
        .into_iter()
        .filter_map(|root| open(root, CONSENT_STORE, KEY_READ | KEY_NOTIFY))
        .collect();
    if keys.is_empty() {
        eprintln!("mic: no microphone consent store to watch — the recording dot stays off");
        return;
    }
    let events: Vec<Event> = keys.iter().filter_map(|_| Event::new()).collect();
    if events.len() != keys.len() {
        eprintln!("mic: could not create the watch events — the recording dot stays off");
        return;
    }

    // Set by the capture sessions' events: one opening, starting or stopping.
    let Some(sessions) = Event::new() else {
        eprintln!("mic: could not create the watch events — the recording dot stays off");
        return;
    };
    let mut capture = Capture { signal: Some(sessions.0 .0 as isize), ..Capture::default() };

    let mut reported: Option<Vec<String>> = None;
    loop {
        // Arm before the sweep so a change during it re-signals. A root that will not arm
        // falls back to polling.
        let armed = keys
            .iter()
            .zip(&events)
            .filter(|(key, event)| arm(key, event))
            .count();

        let users = merge(store_users(), capture.sweep());
        if reported.as_ref() != Some(&users) {
            crate::hub::emit(crate::hub::Event::MicUsers(users.clone()));
            reported = Some(users.clone());
        }
        let now = !users.is_empty();
        if now != IN_USE.swap(now, Ordering::SeqCst) {
            if now {
                println!("mic: in use by {}", users.join(", "));
            } else {
                println!("mic: released");
            }
            announce();
        }

        let handles: Vec<HANDLE> = events.iter().chain([&sessions]).map(|event| event.0).collect();
        let timeout = if armed > 0 { RECHECK_MS } else { POLL_MS };
        unsafe { WaitForMultipleObjects(&handles, false, timeout) };
    }
}

/// Ceiling on the wait even when armed: a safety net for a missed notification, and when a
/// microphone plugged in starts being watched.
const RECHECK_MS: u32 = 5_000;

/// Poll interval when no root would arm a notification.
const POLL_MS: u32 = 2_000;

/// Ask for one notification on `key`'s subtree, signalled through `event`; returns whether it
/// took. `NAME` too, because an app recording for the first time creates its key.
fn arm(key: &Key, event: &Event) -> bool {
    let status = unsafe {
        RegNotifyChangeKeyValue(
            key.0,
            true,
            REG_NOTIFY_CHANGE_NAME | REG_NOTIFY_CHANGE_LAST_SET,
            Some(event.0),
            true,
        )
    };
    status.is_ok()
}

/// Tell the tray the answer changed. A no-op without one (the dev previews).
fn announce() {
    crate::tray::MIC_CHANGED.post();
}

/// An open registry key, closed on drop.
struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

/// An auto-reset event, closed on drop — one per watched root.
struct Event(HANDLE);

impl Event {
    fn new() -> Option<Self> {
        unsafe { CreateEventW(None, false, false, None) }.ok().map(Event)
    }
}

impl Drop for Event {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// The capture endpoints' audio sessions. With a `signal`, it watches them: every endpoint's new
/// sessions and every session's state changes set that event. Without, a one-off look.
#[derive(Default)]
struct Capture {
    signal: Option<isize>,
    endpoints: Vec<Endpoint>,
}

/// One capture endpoint looked at: its session manager, the notification on it, its sessions.
struct Endpoint {
    id: String,
    manager: IAudioSessionManager2,
    created: Option<IAudioSessionNotification>,
    sessions: Vec<Session>,
}

/// One capture session watched for state changes.
struct Session {
    key: String,
    control: IAudioSessionControl,
    events: IAudioSessionEvents,
}

impl Capture {
    /// The exes with an active capture session, Audio Tray left out; the endpoints and sessions
    /// watched are brought up to date on the way.
    fn sweep(&mut self) -> Vec<String> {
        let devices = capture_endpoints();
        self.endpoints.retain(|e| devices.iter().any(|(id, _)| *id == e.id));
        for (id, device) in devices {
            if self.endpoints.iter().any(|e| e.id == id) {
                continue;
            }
            let Ok(manager) = (unsafe { device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) }) else { continue };
            let created = self.signal.and_then(|signal| {
                // Notifications only start once the session list has been read (documented quirk).
                let _ = unsafe { manager.GetSessionEnumerator() };
                let created: IAudioSessionNotification = Created(signal).into();
                unsafe { manager.RegisterSessionNotification(&created) }.ok().map(|()| created)
            });
            self.endpoints.push(Endpoint { id, manager, created, sessions: Vec::new() });
        }
        let mut pids = Vec::new();
        for endpoint in &mut self.endpoints {
            let Ok(list) = (unsafe { endpoint.manager.GetSessionEnumerator() }) else { continue };
            let mut live = Vec::new();
            for i in 0..unsafe { list.GetCount() }.unwrap_or(0) {
                let Ok(control) = (unsafe { list.GetSession(i) }) else { continue };
                let Ok(control2) = control.cast::<IAudioSessionControl2>() else { continue };
                let state = unsafe { control.GetState() }.unwrap_or(AudioSessionStateExpired);
                if state == AudioSessionStateExpired {
                    continue;
                }
                if state == AudioSessionStateActive {
                    pids.push(unsafe { control2.GetProcessId() }.unwrap_or(0));
                }
                let Some(signal) = self.signal else { continue };
                let key = unsafe { control2.GetSessionInstanceIdentifier() }
                    .map(|p| unsafe {
                        let s = p.to_string().unwrap_or_default();
                        CoTaskMemFree(Some(p.0 as *const _));
                        s
                    })
                    .unwrap_or_default();
                if !endpoint.sessions.iter().any(|s| s.key == key) {
                    let events: IAudioSessionEvents = Changed(signal).into();
                    if unsafe { control.RegisterAudioSessionNotification(&events) }.is_ok() {
                        endpoint.sessions.push(Session { key: key.clone(), control, events });
                    }
                }
                live.push(key);
            }
            endpoint.sessions.retain(|s| live.contains(&s.key));
        }
        let own = std::process::id();
        pids.sort_unstable();
        pids.dedup();
        pids.into_iter().filter(|&pid| pid != own).filter_map(crate::win::process_exe).collect()
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        if let Some(created) = &self.created {
            let _ = unsafe { self.manager.UnregisterSessionNotification(created) };
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = unsafe { self.control.UnregisterAudioSessionNotification(&self.events) };
    }
}

/// The active capture endpoints, with their ids.
fn capture_endpoints() -> Vec<(String, IMMDevice)> {
    let mut found = Vec::new();
    unsafe {
        let Ok(enumerator) = CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL) else { return found };
        let Ok(devices) = enumerator.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE) else { return found };
        for i in 0..devices.GetCount().unwrap_or(0) {
            let Ok(device) = devices.Item(i) else { continue };
            let Ok(id) = device.GetId() else { continue };
            let text = id.to_string().unwrap_or_default();
            CoTaskMemFree(Some(id.0 as *const _));
            found.push((text, device));
        }
    }
    found
}

/// Wake the watcher (an event handle kept as an `isize`, which COM callbacks may carry).
fn wake(event: isize) {
    let _ = unsafe { SetEvent(HANDLE(event as *mut core::ffi::c_void)) };
}

/// A session opened on a capture endpoint.
#[implement(IAudioSessionNotification)]
struct Created(isize);

#[allow(non_snake_case)]
impl IAudioSessionNotification_Impl for Created_Impl {
    fn OnSessionCreated(&self, _session: windows::core::Ref<'_, IAudioSessionControl>) -> windows::core::Result<()> {
        wake(self.0);
        Ok(())
    }
}

/// A capture session started, stopped or went away; its volume and name are not our business.
#[implement(IAudioSessionEvents)]
struct Changed(isize);

#[allow(non_snake_case)]
impl IAudioSessionEvents_Impl for Changed_Impl {
    fn OnDisplayNameChanged(&self, _: &PCWSTR, _: *const windows::core::GUID) -> windows::core::Result<()> {
        Ok(())
    }
    fn OnIconPathChanged(&self, _: &PCWSTR, _: *const windows::core::GUID) -> windows::core::Result<()> {
        Ok(())
    }
    fn OnSimpleVolumeChanged(&self, _: f32, _: windows::core::BOOL, _: *const windows::core::GUID) -> windows::core::Result<()> {
        Ok(())
    }
    fn OnChannelVolumeChanged(&self, _: u32, _: *const f32, _: u32, _: *const windows::core::GUID) -> windows::core::Result<()> {
        Ok(())
    }
    fn OnGroupingParamChanged(&self, _: *const windows::core::GUID, _: *const windows::core::GUID) -> windows::core::Result<()> {
        Ok(())
    }
    fn OnStateChanged(&self, _: AudioSessionState) -> windows::core::Result<()> {
        wake(self.0);
        Ok(())
    }
    fn OnSessionDisconnected(&self, _: AudioSessionDisconnectReason) -> windows::core::Result<()> {
        wake(self.0);
        Ok(())
    }
}

fn open(root: HKEY, path: PCWSTR, access: REG_SAM_FLAGS) -> Option<Key> {
    let mut key = HKEY::default();
    let status = unsafe { RegOpenKeyExW(root, path, None, access, &mut key) };
    status.is_ok().then_some(Key(key))
}

/// Names of `key`'s immediate subkeys.
fn subkeys(key: &Key) -> Vec<String> {
    /// The registry caps a key name at 255 characters, so nothing can overflow this.
    const MAX_NAME: usize = 256;

    let mut names = Vec::new();
    for index in 0u32.. {
        let mut buf = [0u16; MAX_NAME];
        let mut len = buf.len() as u32;
        let status = unsafe {
            RegEnumKeyExW(
                key.0,
                index,
                Some(PWSTR(buf.as_mut_ptr())),
                &mut len,
                None,
                None,
                None,
                None,
            )
        };
        // `ERROR_NO_MORE_ITEMS` ends the walk; any other error stops it too.
        if status.is_err() {
            break;
        }
        names.push(String::from_utf16_lossy(&buf[..len as usize]));
    }
    names
}

/// Reads a `REG_QWORD` from `key`, or `None` if it is absent or another type.
fn qword(key: &Key, value: PCWSTR) -> Option<u64> {
    let mut data = 0u64;
    let mut size = std::mem::size_of::<u64>() as u32;
    let status = unsafe {
        RegGetValueW(
            key.0,
            PCWSTR::null(),
            value,
            RRF_RT_REG_QWORD,
            None,
            Some(&mut data as *mut u64 as *mut core::ffi::c_void),
            Some(&mut size),
        )
    };
    status.is_ok().then_some(data)
}

#[cfg(test)]
mod tests {
    use super::merge;

    #[test]
    fn a_capture_the_store_missed_is_added_once() {
        let teams = r"C:\Program Files\WindowsApps\MSTeams_26304.1201.5310.7985_x64__8wekyb3d8bbwe\ms-teams.exe";
        assert_eq!(merge(vec![], vec![teams.into()]), vec![teams.to_string()]);
        let packaged = "MSTeams_8wekyb3d8bbwe".to_string();
        assert_eq!(merge(vec![packaged.clone()], vec![teams.into()]), vec![packaged], "one Teams, not two");
    }
}
