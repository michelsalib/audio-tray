//! "Is an app recording right now?" — the state behind the red dot on the mic icon.
//!
//! Read from the Capability Access Manager's consent store (`HKCU` and `HKLM`), the record the
//! shell's own indicator uses, so it covers every app and every endpoint. A watcher thread
//! blocks in `RegNotifyChangeKeyValue`, recomputes, and posts [`WM_MIC_CHANGED`] to the tray;
//! [`in_use`] is a cached atomic, cheap enough to sample per frame.

use std::sync::atomic::{AtomicBool, Ordering};

use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Registry::{
    RegCloseKey, RegEnumKeyExW, RegGetValueW, RegNotifyChangeKeyValue, RegOpenKeyExW, HKEY,
    HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_NOTIFY, KEY_READ, REG_NOTIFY_CHANGE_LAST_SET,
    REG_NOTIFY_CHANGE_NAME, REG_SAM_FLAGS, RRF_RT_REG_QWORD,
};
use windows::Win32::System::Threading::{CreateEventW, WaitForMultipleObjects};
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
        std::thread::spawn(watch);
    }
    IN_USE.load(Ordering::SeqCst)
}

/// The apps holding the microphone (exe path or package family name). A live registry
/// sweep: keep it off paint paths and use [`in_use`] there.
pub fn users() -> Vec<String> {
    let mut found = Vec::new();
    for root in ROOTS {
        if let Some(key) = open(root, CONSENT_STORE, KEY_READ) {
            collect(&key, "", MAX_DEPTH, &mut found);
        }
    }
    found
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

    loop {
        // Arm before the sweep so a change during it re-signals. A root that will not arm
        // falls back to polling.
        let armed = keys
            .iter()
            .zip(&events)
            .filter(|(key, event)| arm(key, event))
            .count();

        let users = users();
        let now = !users.is_empty();
        if now != IN_USE.swap(now, Ordering::SeqCst) {
            if now {
                println!("mic: in use by {}", users.join(", "));
            } else {
                println!("mic: released");
            }
            announce();
        }

        let handles: Vec<HANDLE> = events.iter().map(|event| event.0).collect();
        let timeout = if armed > 0 { RECHECK_MS } else { POLL_MS };
        unsafe { WaitForMultipleObjects(&handles, false, timeout) };
    }
}

/// Ceiling on the wait even when armed: a safety net for a missed notification.
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
