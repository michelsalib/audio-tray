//! Windows' notifications, read through the documented notification listener (allowed for
//! unpackaged apps on Windows 11), with their pictures and buttons from Windows' store
//! ([`super::rich`]): all of them for the calendar's notification centre, and how many for the
//! clock, with opening one's app, dismissing one, an app's, or all.
//!
//! A thread of its own (the listener's calls block on async operations, which an STA must not do),
//! asleep until something happens: it reads when the listener says something changed, when the
//! store changes on disk (once per burst, and only if the store itself changed), when asked (the
//! calendar opening, a picture downloaded), every few seconds while the calendar is open, and once a
//! minute while any are waiting (a dismissal in Windows' own centre is not always written promptly).

use std::sync::Mutex;
use std::time::{Duration, Instant};

use windows::core::HSTRING;
use windows::Foundation::TypedEventHandler;
use windows::UI::Notifications::Management::{UserNotificationListener, UserNotificationListenerAccessStatus};
use windows::UI::Notifications::{KnownNotificationBindings, NotificationKinds};

/// One notification, as the calendar lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    pub id: u32,
    pub app: String,
    /// The app's id, to open it.
    pub aumid: String,
    pub title: String,
    pub body: String,
    /// When it arrived, in seconds since the epoch.
    pub at: i64,
    /// The app's logo (an image file's bytes), when it has one.
    pub logo: Option<std::sync::Arc<Vec<u8>>>,
    /// Its pictures, reply box and buttons, from Windows' store.
    pub rich: super::rich::Rich,
}

/// What the reader thread is asked to do.
enum Ask {
    Read,
    Remove(Vec<u32>),
    Clear,
    /// The calendar was turned off: the reader ends.
    Stop,
}

/// How often the list is read again while any are waiting: the store on disk does not always say
/// when one goes (a dismissal in Windows' own centre).
const RECHECK: Duration = Duration::from_secs(60);
/// …once Windows has been heard saying what changed: a slow safety net only.
const RECHECK_HEARD: Duration = Duration::from_secs(300);
/// Windows has sent a change event (it does not always, to an unpackaged app).
static HEARD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// …and while the calendar is open, so what it lists follows what arrives.
const RECHECK_OPEN: Duration = Duration::from_secs(3);
/// Windows writes its store in bursts: one read once it has settled.
const SETTLE: Duration = Duration::from_millis(300);
/// How long a notification removed here stays hidden from reads that may predate its removal.
const REMOVED_FOR: Duration = Duration::from_secs(10);

/// The calendar is open.
static WATCHING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The calendar opened (read often) or closed.
pub fn set_watching(on: bool) {
    WATCHING.store(on, std::sync::atomic::Ordering::SeqCst);
    if on {
        // Looked at: the clock's badge clears, as Windows' does.
        // Their ids only, taken first: [`unseen`] locks the seen list before this one.
        let ids: Vec<u32> = TOASTS.lock().unwrap_or_else(|e| e.into_inner()).iter().flatten().map(|t| t.id).collect();
        let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
        for id in ids {
            if !seen.contains(&id) {
                seen.push(id);
            }
        }
        drop(seen);
        held_changed();
        refresh();
    }
}

/// Notifications the user has had in front of them (the panel open), by id.
static SEEN: Mutex<Vec<u32>> = Mutex::new(Vec::new());

fn see(toasts: &[Toast]) {
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    for t in toasts {
        if !seen.contains(&t.id) {
            seen.push(t.id);
        }
    }
}

/// How many notifications came since the panel was last open: the clock's badge.
pub fn unseen() -> usize {
    let seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    TOASTS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map_or(0, |list| list.iter().filter(|t| !seen.contains(&t.id)).count())
}

/// Every notification, newest first, as last read; `None` before the first read or when the
/// listener is not allowed.
static TOASTS: Mutex<Option<Vec<Toast>>> = Mutex::new(None);
/// Asks waiting for the reader thread, which [`WAKE`] wakes.
static ASKS: Mutex<Vec<Ask>> = Mutex::new(Vec::new());
/// The reader thread's wake-up event (a raw handle), made once; 0 before.
static WAKE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
/// Notifications removed here, and when: a read already under way must not bring them back.
static REMOVED: Mutex<Vec<(u32, Instant)>> = Mutex::new(Vec::new());

/// Start reading (once; again if Windows refused the last time). The tray hears of each change as
/// [`super::WM_CALENDAR`] with [`super::NOTIFICATIONS`].
pub fn start() {
    use std::sync::atomic::Ordering::SeqCst;
    use windows::Win32::System::Threading::CreateEventW;
    if RUNNING.swap(true, SeqCst) {
        return;
    }
    // Made once and never closed: an [`ask`] racing the reader's end must not signal a handle
    // reused by something else.
    if WAKE.load(SeqCst) == 0 {
        let Ok(event) = (unsafe { CreateEventW(None, false, false, None) }) else {
            RUNNING.store(false, SeqCst);
            return;
        };
        WAKE.store(event.0 as usize, SeqCst);
    }
    let raw = WAKE.load(SeqCst);
    let spawned = std::thread::Builder::new().name("notifications".into()).spawn(move || {
        run(windows::Win32::Foundation::HANDLE(raw as *mut _));
        // Refused, or gone: a later start may try again.
        RUNNING.store(false, SeqCst);
    });
    if spawned.is_err() {
        RUNNING.store(false, SeqCst);
    }
}

/// The reader runs (or is starting).
static RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The notifications, newest first; empty when there are none or they cannot be read.
pub fn list() -> Vec<Toast> {
    TOASTS.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_default()
}

/// How many notifications are waiting.
pub fn count() -> usize {
    TOASTS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map_or(0, Vec::len)
}

fn ask(what: Ask) {
    let event = WAKE.load(std::sync::atomic::Ordering::SeqCst);
    if event == 0 || !RUNNING.load(std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    ASKS.lock().unwrap_or_else(|e| e.into_inner()).push(what);
    let _ = unsafe { windows::Win32::System::Threading::SetEvent(windows::Win32::Foundation::HANDLE(event as *mut _)) };
}

/// End the reader (the calendar was turned off): nothing wakes for notifications no one shows.
pub fn stop() {
    ask(Ask::Stop);
    *TOASTS.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Read the list again now (the calendar is opening, a picture has come).
pub fn refresh() {
    ask(Ask::Read);
}

pub fn remove(ids: Vec<u32>) {
    hide(&ids);
    ask(Ask::Remove(ids));
    held_changed();
}

/// Take notification `id` off the list while its action runs ([`super::rich::invoke`]): removed
/// from Windows once done ([`remove`]), back if not ([`failed`]).
pub fn hold(id: u32) {
    hide(&[id]);
    held_changed();
}

/// An action the app did not take: `id` comes back, with what was typed (`draft`), and the panel
/// says so ([`take_failure`]).
pub fn failed(id: u32, draft: Option<String>) {
    REMOVED.lock().unwrap_or_else(|e| e.into_inner()).retain(|(r, _)| *r != id);
    *FAILURE.lock().unwrap_or_else(|e| e.into_inner()) = Some((id, draft));
    refresh();
}

/// The last action not taken: its notification and what was typed, once.
pub fn take_failure() -> Option<(u32, Option<String>)> {
    FAILURE.lock().unwrap_or_else(|e| e.into_inner()).take()
}

static FAILURE: Mutex<Option<(u32, Option<String>)>> = Mutex::new(None);

pub fn clear() {
    let ids: Vec<u32> = list().iter().map(|t| t.id).collect();
    hide(&ids);
    ask(Ask::Clear);
    held_changed();
}

/// Take `ids` off the list now, and off reads for a while.
fn hide(ids: &[u32]) {
    let now = Instant::now();
    REMOVED.lock().unwrap_or_else(|e| e.into_inner()).extend(ids.iter().map(|&id| (id, now)));
    if let Some(list) = TOASTS.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        list.retain(|t| !ids.contains(&t.id));
    }
}

/// The list was changed here, ahead of Windows: the next read then matches it and says nothing,
/// so the clock's count is told now.
fn held_changed() {
    crate::tray::post(super::WM_CALENDAR, super::NOTIFICATIONS, 0);
}

/// Open the app a notification came from (for an app that takes no buttons from outside).
pub fn open_app(aumid: &str) -> bool {
    !aumid.is_empty() && crate::win::shell_open(&format!(r"shell:AppsFolder\{aumid}"), None)
}

/// The reader: it sleeps until asked, until the store changes on disk, or until a recheck is due,
/// and then reads once.
fn run(wake: windows::Win32::Foundation::HANDLE) {
    use windows::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows::Win32::System::Threading::{WaitForMultipleObjects, INFINITE};
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
    }
    let listener = match UserNotificationListener::Current() {
        Ok(listener) if listener.GetAccessStatus().ok() == Some(UserNotificationListenerAccessStatus::Allowed) => listener,
        _ => {
            eprintln!("notifications: Windows does not let Audio Tray read them");
            return;
        }
    };
    // The listener's own word on changes, where Windows sends it to an unpackaged app.
    let handler = TypedEventHandler::new(move |_, _| {
        HEARD.store(true, std::sync::atomic::Ordering::SeqCst);
        refresh();
        Ok(())
    });
    if listener.NotificationChanged(&handler).is_err() {
        println!("notifications: no change events here; following the store on disk");
    }
    let changes = watch_store();
    let mut known = Known::default();
    let mut stamp = store_stamp();
    let mut read_now = true;
    loop {
        if read_now {
            stamp = store_stamp();
            match read(&listener, &mut known) {
                Ok(toasts) => publish(toasts),
                Err(e) => eprintln!("notifications: could not read them ({e:#})"),
            }
        }
        let waiting = TOASTS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|l| !l.is_empty());
        let timeout = if WATCHING.load(std::sync::atomic::Ordering::SeqCst) {
            RECHECK_OPEN.as_millis() as u32
        } else if waiting {
            if HEARD.load(std::sync::atomic::Ordering::SeqCst) { RECHECK_HEARD.as_millis() as u32 } else { RECHECK.as_millis() as u32 }
        } else {
            INFINITE
        };
        let mut handles = vec![wake];
        if let Some(c) = &changes {
            handles.push(c.0);
        }
        let woke = unsafe { WaitForMultipleObjects(&handles, false, timeout) };
        if woke == WAIT_TIMEOUT {
            read_now = true;
        } else if woke == WAIT_OBJECT_0 {
            for asked in std::mem::take(&mut *ASKS.lock().unwrap_or_else(|e| e.into_inner())) {
                match asked {
                    Ask::Stop => return,
                    Ask::Read => {}
                    Ask::Remove(ids) => {
                        for id in ids {
                            if let Err(e) = listener.RemoveNotification(id) {
                                eprintln!("notifications: could not remove {id} ({e})");
                            }
                        }
                    }
                    Ask::Clear => {
                        // Windows can refuse the whole sweep; one by one then.
                        if let Err(e) = listener.ClearNotifications() {
                            println!("notifications: Windows would not clear them all ({e}); removing them one by one");
                            for &(id, _) in &known.toasts {
                                if let Err(e) = listener.RemoveNotification(id) {
                                    eprintln!("notifications: could not remove {id} ({e})");
                                }
                            }
                        }
                    }
                }
            }
            read_now = true;
        } else if woke.0 == WAIT_OBJECT_0.0 + 1 {
            if let Some(c) = &changes {
                c.rearm();
                std::thread::sleep(SETTLE);
                // What came meanwhile is the same burst.
                c.take();
            }
            // Reading the store touches its shared-memory file, which signals a change too: only
            // a change to the store itself is worth a read.
            read_now = store_stamp() != stamp;
        } else {
            // The watch failed: no busy loop.
            std::thread::sleep(RECHECK_OPEN);
            read_now = true;
        }
    }
}

/// Hold a fresh read (less what was removed here meanwhile), and tell the tray if it changed.
fn publish(mut toasts: Vec<Toast>) {
    {
        // Those gone are forgotten; while the panel is open, what arrives is seen.
        let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
        seen.retain(|id| toasts.iter().any(|t| t.id == *id));
    }
    if WATCHING.load(std::sync::atomic::Ordering::SeqCst) {
        see(&toasts);
    }
    // Held while the list is replaced: a removal in between ([`hide`]) would come back until the
    // next read.
    let mut removed = REMOVED.lock().unwrap_or_else(|e| e.into_inner());
    removed.retain(|(id, at)| at.elapsed() < REMOVED_FOR && toasts.iter().any(|t| t.id == *id));
    toasts.retain(|t| !removed.iter().any(|(id, _)| *id == t.id));
    let changed = {
        let mut held = TOASTS.lock().unwrap_or_else(|e| e.into_inner());
        let changed = held.as_ref() != Some(&toasts);
        if changed {
            println!("notifications: {} waiting", toasts.len());
            let old = held.as_deref().unwrap_or_default();
            for t in toasts.iter().filter(|t| !old.iter().any(|o| o.id == t.id)) {
                println!("notifications: + {} {} \"{}\" at {}", t.id, t.app, t.title, t.at);
            }
            for t in old.iter().filter(|o| !toasts.iter().any(|t| t.id == o.id)) {
                println!("notifications: - {} {} \"{}\" at {}", t.id, t.app, t.title, t.at);
            }
        }
        *held = Some(toasts);
        changed
    };
    if changed {
        crate::tray::post(super::WM_CALENDAR, super::NOTIFICATIONS, 0);
    }
}

/// What the reader knows of each listed notification, by id: what it says never changes, so it is
/// asked of Windows once.
#[derive(Default)]
struct Known {
    toasts: Vec<(u32, Detail)>,
}

struct Detail {
    toast: Toast,
    /// When it leaves Windows' centre, in seconds since the epoch.
    expires: Option<i64>,
    info: Option<windows::ApplicationModel::AppInfo>,
}

/// Every toast in Windows' notification centre, newest first.
fn read(listener: &UserNotificationListener, known: &mut Known) -> anyhow::Result<Vec<Toast>> {
    let all = listener.GetNotificationsAsync(NotificationKinds::Toast)?.get()?;
    let generic: HSTRING = KnownNotificationBindings::ToastGeneric()?;
    let now = super::time::now();
    let mut listed = Vec::new();
    for n in all {
        let id = n.Id().unwrap_or_default();
        if let Some(i) = known.toasts.iter().position(|(k, _)| *k == id) {
            listed.push(known.toasts.swap_remove(i));
            continue;
        }
        let notification = n.Notification().ok();
        let expires = notification.as_ref().and_then(|x| x.ExpirationTime().ok()).and_then(|t| t.Value().ok()).map(|t| super::time::from_filetime(t.UniversalTime));
        let info = n.AppInfo().ok();
        let app = info.as_ref().and_then(|a| a.DisplayInfo().ok()).and_then(|d| d.DisplayName().ok()).map(|s| s.to_string()).unwrap_or_default();
        let aumid = info.as_ref().and_then(|a| a.AppUserModelId().ok()).map(|s| s.to_string()).unwrap_or_default();
        let texts: Vec<String> = notification
            .and_then(|x| x.Visual().ok())
            .and_then(|v| v.GetBinding(&generic).ok())
            .and_then(|b| b.GetTextElements().ok())
            .map(|list| list.into_iter().filter_map(|t| t.Text().ok().map(|s| s.to_string())).collect())
            .unwrap_or_default();
        let at = n.CreationTime().map_or(0, |t| super::time::from_filetime(t.UniversalTime));
        let logo = logo_of(&app, &aumid, info.as_ref());
        let toast = Toast {
            id,
            app,
            aumid,
            title: texts.first().cloned().unwrap_or_default(),
            body: texts.get(1..).map(|rest| rest.join(" ")).unwrap_or_default(),
            at,
            logo,
            rich: Default::default(),
        };
        listed.push((id, Detail { toast, expires, info }));
    }
    // One past its expiry has left Windows' centre, whatever the listener still holds.
    listed.retain(|(_, d)| d.expires.is_none_or(|t| t > now));
    known.toasts = listed;
    let infos: Vec<(u32, String, Option<windows::ApplicationModel::AppInfo>)> = known.toasts.iter().map(|(id, d)| (*id, d.toast.aumid.clone(), d.info.clone())).collect();
    let mut toasts: Vec<Toast> = known
        .toasts
        .iter()
        .zip(super::rich::for_toasts(&infos))
        .map(|((_, d), rich)| Toast { rich, ..d.toast.clone() })
        .collect();
    toasts.sort_by_key(|t| std::cmp::Reverse(t.at));
    Ok(toasts)
}

/// Logos read so far, by app: each is read once.
static LOGOS: Mutex<Vec<(String, Option<Logo>)>> = Mutex::new(Vec::new());

/// An image file's bytes, shared.
type Logo = std::sync::Arc<Vec<u8>>;

/// An app's logo, as its notifications show it in Windows' centre (by its id, else its name: apps
/// without an id must not share one's logo).
fn logo_of(app: &str, aumid: &str, info: Option<&windows::ApplicationModel::AppInfo>) -> Option<std::sync::Arc<Vec<u8>>> {
    let key = if aumid.is_empty() { format!("name:{app}") } else { aumid.to_string() };
    if let Some((_, logo)) = LOGOS.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(a, _)| *a == key) {
        return logo.clone();
    }
    let read = || -> Option<Vec<u8>> {
        use windows::Storage::Streams::DataReader;
        let reference = info?.DisplayInfo().ok()?.GetLogo(windows::Foundation::Size { Width: 32.0, Height: 32.0 }).ok()?;
        let stream = reference.OpenReadAsync().ok()?.get().ok()?;
        let size = stream.Size().ok()?;
        if size == 0 || size > 1024 * 1024 {
            return None;
        }
        let reader = DataReader::CreateDataReader(&stream).ok()?;
        reader.LoadAsync(size as u32).ok()?.get().ok()?;
        let mut bytes = vec![0u8; size as usize];
        reader.ReadBytes(&mut bytes).ok()?;
        Some(bytes)
    };
    let logo = read().map(std::sync::Arc::new);
    // Kept when read: a failure (a moment's refusal) is tried again with the app's next notification.
    if logo.is_some() {
        LOGOS.lock().unwrap_or_else(|e| e.into_inner()).push((key, logo.clone()));
    }
    logo
}

/// The store's files as they stand (size and last write of the database and its log), to tell a
/// real change from the reader's own traces.
fn store_stamp() -> Vec<(u64, Option<std::time::SystemTime>)> {
    let Ok(folder) = std::env::var("LOCALAPPDATA") else { return Vec::new() };
    ["wpndatabase.db", "wpndatabase.db-wal"]
        .iter()
        .map(|name| {
            let meta = std::fs::metadata(format!(r"{folder}\Microsoft\Windows\Notifications\{name}")).ok();
            (meta.as_ref().map_or(0, |m| m.len()), meta.and_then(|m| m.modified().ok()))
        })
        .collect()
}

/// A change notification on the folder of Windows' notification store.
struct StoreWatch(windows::Win32::Foundation::HANDLE);

impl StoreWatch {
    /// Watch for the next change.
    fn rearm(&self) {
        let _ = unsafe { windows::Win32::Storage::FileSystem::FindNextChangeNotification(self.0) };
    }

    /// Take a change signalled since (and re-arm).
    fn take(&self) {
        use windows::Win32::System::Threading::WaitForSingleObject;
        if unsafe { WaitForSingleObject(self.0, 0) }.0 == 0 {
            self.rearm();
        }
    }
}

impl Drop for StoreWatch {
    fn drop(&mut self) {
        let _ = unsafe { windows::Win32::Storage::FileSystem::FindCloseChangeNotification(self.0) };
    }
}

fn watch_store() -> Option<StoreWatch> {
    use windows::Win32::Storage::FileSystem::{FindFirstChangeNotificationW, FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE};
    let folder = std::env::var("LOCALAPPDATA").ok()? + r"\Microsoft\Windows\Notifications";
    let wide = crate::win::wide(&folder);
    let handle = unsafe { FindFirstChangeNotificationW(windows::core::PCWSTR(wide.as_ptr()), false, FILE_NOTIFY_CHANGE_LAST_WRITE | FILE_NOTIFY_CHANGE_SIZE) }.ok()?;
    Some(StoreWatch(handle))
}

/// The notifications as the centre lists them: by app (the app with the newest first), newest
/// first within each.
pub fn grouped(toasts: &[Toast]) -> Vec<(String, Vec<&Toast>)> {
    let mut groups: Vec<(String, Vec<&Toast>)> = Vec::new();
    for toast in toasts {
        let key = if toast.aumid.is_empty() { &toast.app } else { &toast.aumid };
        match groups.iter_mut().find(|(k, _)| k == key) {
            Some((_, list)) => list.push(toast),
            None => groups.push((key.clone(), vec![toast])),
        }
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toast(id: u32, app: &str, at: i64) -> Toast {
        Toast { id, app: app.into(), aumid: format!("{app}.id"), title: String::new(), body: String::new(), at, logo: None, rich: Default::default() }
    }

    #[test]
    fn notifications_group_by_app_newest_app_first() {
        let list = vec![toast(1, "Outlook", 30), toast(2, "Teams", 20), toast(3, "Outlook", 10)];
        let groups = grouped(&list);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].0, "Outlook.id");
        assert_eq!(groups[0].1.iter().map(|t| t.id).collect::<Vec<_>>(), vec![1, 3]);
        assert_eq!(groups[1].1[0].id, 2);
    }
}
