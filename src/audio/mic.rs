//! "Is an app recording right now?" — the state behind the red dot on the mic icon.
//!
//! Read where Windows itself keeps it: the Capability Access Manager's consent store.
//! When an app opens the microphone the audio service stamps `LastUsedTimeStart` under
//! that app's key, and when it lets go it stamps `LastUsedTimeStop` — so a key with a
//! start and a **zero stop** is an app that still has the microphone open. That is the
//! same record the shell's own "microphone in use" indicator is driven from, which is
//! the point: our dot says what Windows says, for every app, whatever endpoint it
//! opened.
//!
//! Two roots, because the store is split by who is asking: `HKCU` carries the user's
//! own apps — packaged ones by package family name, desktop ones one level down under
//! `NonPackaged` — and `HKLM` the system and service side.
//!
//! A watcher thread blocks in `RegNotifyChangeKeyValue` on both roots and recomputes when
//! the store changes, then posts [`WM_MIC_CHANGED`] to whichever thread asked to hear about
//! it (the tray's loop, which owns the taskbar strip). Everyone else reads the answer from
//! [`in_use`] — a cached atomic, so the flyout can sample it on every frame for nothing.
//! The wait carries a [`RECHECK_MS`] ceiling as a safety net; the notification is what
//! makes it prompt, not what makes it correct.
//!
//! What this deliberately does *not* do is watch our own default input endpoint: an app
//! recording from some other microphone is still an app recording, and hiding that would
//! make the dot disagree with the system indicator sitting a few pixels away.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE, LPARAM, WPARAM};
use windows::Win32::System::Registry::{
    RegCloseKey, RegEnumKeyExW, RegGetValueW, RegNotifyChangeKeyValue, RegOpenKeyExW, HKEY,
    HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_NOTIFY, KEY_READ, REG_NOTIFY_CHANGE_LAST_SET,
    REG_NOTIFY_CHANGE_NAME, REG_SAM_FLAGS, RRF_RT_REG_QWORD,
};
use windows::Win32::System::Threading::{CreateEventW, WaitForMultipleObjects};
use windows::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_APP};

/// Posted to the thread registered with [`notify_thread`] when the answer to [`in_use`]
/// changes. A thread message with no window, like [`super::notify::WM_AUDIO_REFRESH`].
pub const WM_MIC_CHANGED: u32 = WM_APP + 3;

/// The consent store's microphone branch, under both `HKCU` and `HKLM`.
const CONSENT_STORE: PCWSTR =
    w!(r"SOFTWARE\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone");

/// The two roots the store is split across, in the order they are reported.
const ROOTS: [HKEY; 2] = [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE];

/// How far below a root the timestamps can sit: a packaged app is one level down
/// (`microphone\<family name>`), a desktop app two (`microphone\NonPackaged\<exe>`).
///
/// Three rather than two as slack for a nesting we have not seen, and a *budget* rather
/// than a guess: [`users`] runs on a caller's thread, so the walk has to be bounded.
const MAX_DEPTH: u32 = 3;

/// The cached answer, kept current by [`watch`].
static IN_USE: AtomicBool = AtomicBool::new(false);

/// Whether the watcher has been started, so [`in_use`] starts exactly one.
static WATCHING: AtomicBool = AtomicBool::new(false);

/// Thread to wake on a change; 0 until someone asks (the `--flyout` previews never do).
static NOTIFY_TID: AtomicU32 = AtomicU32::new(0);

/// Wake `thread_id` with [`WM_MIC_CHANGED`] whenever the answer changes.
///
/// Separate from [`in_use`] because the two have different owners: anything that paints
/// asks the question, while only the tray thread — the one holding the strip and the
/// notification icon — has a message loop to be told about it.
pub fn notify_thread(thread_id: u32) {
    NOTIFY_TID.store(thread_id, Ordering::SeqCst);
}

/// Whether any app has the microphone open right now.
///
/// An atomic load, so callers can ask per frame; the work happens on the watcher thread
/// instead. The first call starts that watcher and seeds the answer with one synchronous
/// sweep, so it is never wrong for the first frame drawn.
pub fn in_use() -> bool {
    if !WATCHING.swap(true, Ordering::SeqCst) {
        IN_USE.store(!users().is_empty(), Ordering::SeqCst);
        std::thread::spawn(watch);
    }
    IN_USE.load(Ordering::SeqCst)
}

/// The apps currently holding the microphone, named for a human — the exe path for a
/// desktop app, the package family name for a packaged one.
///
/// The live sweep behind [`in_use`], and on its own the answer to `--mic`. Sweeping is
/// cheap (a few dozen keys, two values each) but it is not free, which is why nothing on
/// a paint path calls it directly.
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
    // The root itself carries no timestamps (only the Allow/Deny `Value`), so testing it
    // is harmless — and testing every key is what keeps this indifferent to how deep a
    // given app's entry happens to sit.
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

/// Whether this key's timestamps say its app has the microphone open *now*: it started
/// using the microphone, and has not stopped **since**.
///
/// `stop < start` rather than `stop == 0`. Zeroing the stop is what Windows was seen doing
/// — and a fresh key has no stop at all, which reads as 0 — but that is not something to
/// rely on for every writer: an app whose previous session left a stop behind and whose new
/// session only stamps the start would then look idle for as long as it recorded. Comparing
/// the two timestamps is true in both spellings, and it is what the values mean anyway.
fn holding(key: &Key) -> bool {
    let start = qword(key, w!("LastUsedTimeStart")).unwrap_or(0);
    let stop = qword(key, w!("LastUsedTimeStop")).unwrap_or(0);
    start != 0 && stop < start
}

/// A consent-store key name as something worth printing: `NonPackaged` is an
/// implementation detail, and a desktop app's path is stored with `#` where its
/// separators were.
fn label(path: &str) -> String {
    path.trim_start_matches(r"NonPackaged\").replace('#', r"\")
}

/// The watcher thread: block on the store, recompute when it changes, announce a flip.
///
/// Runs for the life of the process. There is nothing to shut down — it holds two
/// registry handles and an event apiece, and the loop exits only if the store cannot be
/// opened at all.
fn watch() {
    // `KEY_NOTIFY` as well as `KEY_READ`: the same handle is both what we read through
    // and what the notification is registered on.
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
        // Armed *before* the sweep, so a change that lands while we are reading re-signals
        // rather than being missed — the same ordering the flyout's volume coalescing
        // uses. A root that refuses to arm is not fatal: the wait below falls back to a
        // slow poll, which is worse than free but still correct.
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

/// Ceiling on the wait, even with both roots armed — so a change we were somehow not
/// notified of costs a few seconds of a stale dot rather than a wrong one until the next
/// app records.
///
/// The notification is the mechanism and this is the safety net: a sweep is a few dozen
/// registry reads, so paying for one every few seconds in a background thread is cheaper
/// than being wrong. It exists because the dot is *the* signal that something is listening
/// — with Explorer's own indicator hidden, nothing else is going to correct us.
const RECHECK_MS: u32 = 5_000;

/// Fallback interval for a store that would not arm a notification at all. Faster than
/// [`RECHECK_MS`] because in that state the sweep is the only thing there is.
const POLL_MS: u32 = 2_000;

/// Ask for one notification on `key`, signalled through `event`. Returns whether it took.
///
/// Subtree, because the timestamps live in the app keys below the root rather than on it,
/// and `NAME` alongside `LAST_SET` because an app recording for the first time *creates*
/// its key rather than writing to one.
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

/// Tell the registered thread the answer changed. Best-effort: no registered thread (the
/// dev previews) or a queue that has gone away both mean there is nobody to tell.
fn announce() {
    let thread_id = NOTIFY_TID.load(Ordering::SeqCst);
    if thread_id == 0 {
        return;
    }
    unsafe {
        let _ = PostThreadMessageW(thread_id, WM_MIC_CHANGED, WPARAM(0), LPARAM(0));
    }
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
        // `ERROR_NO_MORE_ITEMS` is how the walk ends; anything else is a key we cannot
        // read, and stopping is the same answer either way.
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
