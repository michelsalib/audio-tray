//! Windows' notifications, read through the documented notification listener (allowed for
//! unpackaged apps on Windows 11): all of them for the calendar's notification centre, and how many
//! for the clock, with opening one's app, dismissing one, an app's, or all.
//!
//! A thread of its own (the listener's calls block on async operations, which an STA must not do).
//! It reads again when the listener says something changed, when the notification store changes
//! on disk, when the calendar opens, and once a minute while any are waiting (a dismissal in
//! Windows' own centre is not always written promptly, and the count must not go stale).

use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Mutex, OnceLock};
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
}

/// What the reader thread is asked to do.
enum Ask {
    Read,
    Remove(Vec<u32>),
    Clear,
}

/// How often the list is read again: the store on disk does not always say when it changes.
const RECHECK: Duration = Duration::from_secs(60);
/// …and while the calendar is open, so what it lists follows what arrives.
const RECHECK_OPEN: Duration = Duration::from_secs(3);

/// The calendar is open.
static WATCHING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The calendar opened (read often) or closed.
pub fn set_watching(on: bool) {
    WATCHING.store(on, std::sync::atomic::Ordering::SeqCst);
    if on {
        refresh();
    }
}

/// Every notification, newest first, as last read; `None` before the first read or when the
/// listener is not allowed.
static TOASTS: Mutex<Option<Vec<Toast>>> = Mutex::new(None);
static ASK: OnceLock<Sender<Ask>> = OnceLock::new();

/// Start reading (once). The tray hears of each change as [`super::WM_CALENDAR`] with
/// [`super::NOTIFICATIONS`].
pub fn start() {
    if ASK.get().is_some() {
        return;
    }
    let (send, asks) = channel();
    if ASK.set(send.clone()).is_err() {
        return;
    }
    let _ = std::thread::Builder::new().name("notifications".into()).spawn(move || run(asks, send));
}

/// The notifications, newest first; empty when there are none or they cannot be read.
pub fn list() -> Vec<Toast> {
    TOASTS.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_default()
}

/// How many notifications are waiting.
pub fn count() -> usize {
    TOASTS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map_or(0, Vec::len)
}

fn ask(what: Ask) {
    if let Some(ask) = ASK.get() {
        let _ = ask.send(what);
    }
}

/// Read the list again now (the calendar is opening).
pub fn refresh() {
    ask(Ask::Read);
}

pub fn remove(ids: Vec<u32>) {
    TOASTS.lock().unwrap_or_else(|e| e.into_inner()).iter_mut().for_each(|list| list.retain(|t| !ids.contains(&t.id)));
    ask(Ask::Remove(ids));
}

pub fn clear() {
    if let Some(list) = TOASTS.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        list.clear();
    }
    ask(Ask::Clear);
}

/// Open the app a notification came from (its own buttons belong to Windows' centre).
pub fn open_app(aumid: &str) -> bool {
    !aumid.is_empty() && crate::win::shell_open(&format!(r"shell:AppsFolder\{aumid}"), None)
}

fn run(asks: Receiver<Ask>, again: Sender<Ask>) {
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
        let _ = again.send(Ask::Read);
        Ok(())
    });
    if listener.NotificationChanged(&handler).is_err() {
        println!("notifications: no change events here; following the store on disk");
    }
    let changes = watch_store();
    loop {
        let mut last_read = Instant::now();
        match read(&listener) {
            Ok(toasts) => {
                let changed = {
                    let mut held = TOASTS.lock().unwrap_or_else(|e| e.into_inner());
                    let changed = held.as_ref() != Some(&toasts);
                    if changed {
                        println!("notifications: {} waiting", toasts.len());
                    }
                    *held = Some(toasts);
                    changed
                };
                if changed {
                    crate::tray::post(super::WM_CALENDAR, super::NOTIFICATIONS, 0);
                }
            }
            Err(e) => eprintln!("notifications: could not read them ({e:#})"),
        }
        // Until something changes, or the calendar asks for something.
        loop {
            match asks.recv_timeout(Duration::from_millis(500)) {
                Ok(Ask::Read) => break,
                Ok(Ask::Remove(ids)) => {
                    for id in ids {
                        let _ = listener.RemoveNotification(id);
                    }
                    break;
                }
                Ok(Ask::Clear) => {
                    let _ = listener.ClearNotifications();
                    break;
                }
                Err(RecvTimeoutError::Disconnected) => return,
                Err(RecvTimeoutError::Timeout) => {}
            }
            if changes.as_ref().is_some_and(|c| c.changed()) {
                // Windows writes in bursts: let it finish.
                std::thread::sleep(Duration::from_millis(300));
                if let Some(c) = &changes {
                    c.changed();
                }
                break;
            }
            let every = if WATCHING.load(std::sync::atomic::Ordering::SeqCst) { RECHECK_OPEN } else { RECHECK };
            if last_read.elapsed() >= every {
                last_read = Instant::now();
                break;
            }
        }
    }
}

/// Every toast in Windows' notification centre, newest first.
fn read(listener: &UserNotificationListener) -> anyhow::Result<Vec<Toast>> {
    let all = listener.GetNotificationsAsync(NotificationKinds::Toast)?.get()?;
    let generic: HSTRING = KnownNotificationBindings::ToastGeneric()?;
    let now = super::time::now();
    let mut toasts = Vec::new();
    for n in all {
        let notification = n.Notification().ok();
        // One past its expiry has left Windows' centre, whatever the listener still holds.
        let expired = notification
            .as_ref()
            .and_then(|x| x.ExpirationTime().ok())
            .and_then(|t| t.Value().ok())
            .is_some_and(|t| t.UniversalTime / 10_000_000 - 11_644_473_600 <= now);
        if expired {
            continue;
        }
        let info = n.AppInfo().ok();
        let app = info.as_ref().and_then(|a| a.DisplayInfo().ok()).and_then(|d| d.DisplayName().ok()).map(|s| s.to_string()).unwrap_or_default();
        let aumid = info.as_ref().and_then(|a| a.AppUserModelId().ok()).map(|s| s.to_string()).unwrap_or_default();
        let texts: Vec<String> = notification
            .and_then(|x| x.Visual().ok())
            .and_then(|v| v.GetBinding(&generic).ok())
            .and_then(|b| b.GetTextElements().ok())
            .map(|list| list.into_iter().filter_map(|t| t.Text().ok().map(|s| s.to_string())).collect())
            .unwrap_or_default();
        let at = n.CreationTime().map_or(0, |t| t.UniversalTime / 10_000_000 - 11_644_473_600);
        let logo = logo_of(&aumid, info.as_ref());
        toasts.push(Toast {
            id: n.Id().unwrap_or_default(),
            app,
            aumid,
            title: texts.first().cloned().unwrap_or_default(),
            body: texts.get(1..).map(|rest| rest.join(" ")).unwrap_or_default(),
            at,
            logo,
        });
    }
    toasts.sort_by_key(|t| std::cmp::Reverse(t.at));
    Ok(toasts)
}

/// Logos read so far, by app: each is read once.
static LOGOS: Mutex<Vec<(String, Option<Logo>)>> = Mutex::new(Vec::new());

/// An image file's bytes, shared.
type Logo = std::sync::Arc<Vec<u8>>;

/// An app's logo, as its notifications show it in Windows' centre.
fn logo_of(aumid: &str, info: Option<&windows::ApplicationModel::AppInfo>) -> Option<std::sync::Arc<Vec<u8>>> {
    if let Some((_, logo)) = LOGOS.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(a, _)| a == aumid) {
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
    LOGOS.lock().unwrap_or_else(|e| e.into_inner()).push((aumid.to_string(), logo.clone()));
    logo
}

/// A change notification on the folder of Windows' notification store.
struct StoreWatch(windows::Win32::Foundation::HANDLE);

impl StoreWatch {
    /// Whether the store changed since the last look (and re-arm).
    fn changed(&self) -> bool {
        use windows::Win32::Storage::FileSystem::FindNextChangeNotification;
        use windows::Win32::System::Threading::WaitForSingleObject;
        let signalled = unsafe { WaitForSingleObject(self.0, 0) }.0 == 0;
        if signalled {
            let _ = unsafe { FindNextChangeNotification(self.0) };
        }
        signalled
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
        Toast { id, app: app.into(), aumid: format!("{app}.id"), title: String::new(), body: String::new(), at, logo: None }
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
