//! Windows' notifications, read through the documented notification listener (allowed for
//! unpackaged apps on Windows 11): how many there are for the clock, and the newest few for the
//! calendar, with dismissing one and Clear all.
//!
//! A thread of its own (the listener's calls block on async operations, which an STA must not do)
//! that sleeps until the notification store changes on disk: Windows writes
//! `%LOCALAPPDATA%\Microsoft\Windows\Notifications\wpndatabase.db*` on every arrival and dismissal,
//! so nothing polls.

use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use windows::core::HSTRING;
use windows::UI::Notifications::Management::{UserNotificationListener, UserNotificationListenerAccessStatus};
use windows::UI::Notifications::{KnownNotificationBindings, NotificationKinds};

/// One notification, as the calendar lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    pub id: u32,
    pub app: String,
    pub title: String,
    pub body: String,
    /// When it arrived, in seconds since the epoch.
    pub at: i64,
}

/// What the reader thread is asked to do.
enum Ask {
    Remove(u32),
    Clear,
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
    if ASK.set(send).is_err() {
        return;
    }
    let _ = std::thread::Builder::new().name("notifications".into()).spawn(move || run(asks));
}

/// The notifications, newest first; empty when there are none or they cannot be read.
pub fn list() -> Vec<Toast> {
    TOASTS.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_default()
}

/// How many notifications are waiting.
pub fn count() -> usize {
    TOASTS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map_or(0, Vec::len)
}

pub fn remove(id: u32) {
    if let Some(ask) = ASK.get() {
        let _ = ask.send(Ask::Remove(id));
    }
}

pub fn clear() {
    if let Some(ask) = ASK.get() {
        let _ = ask.send(Ask::Clear);
    }
}

fn run(asks: Receiver<Ask>) {
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
    let changes = watch_store();
    loop {
        match read(&listener) {
            Ok(toasts) => {
                let changed = {
                    let mut held = TOASTS.lock().unwrap_or_else(|e| e.into_inner());
                    let changed = held.as_ref() != Some(&toasts);
                    *held = Some(toasts);
                    changed
                };
                if changed {
                    crate::tray::post(super::WM_CALENDAR, super::NOTIFICATIONS, 0);
                }
            }
            Err(e) => eprintln!("notifications: could not read them ({e:#})"),
        }
        // Until the store changes, or the calendar asks for something.
        loop {
            match asks.recv_timeout(Duration::from_millis(500)) {
                Ok(Ask::Remove(id)) => {
                    let _ = listener.RemoveNotification(id);
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
        }
    }
}

/// Every toast in Windows' notification centre, newest first.
fn read(listener: &UserNotificationListener) -> anyhow::Result<Vec<Toast>> {
    let all = listener.GetNotificationsAsync(NotificationKinds::Toast)?.get()?;
    let generic: HSTRING = KnownNotificationBindings::ToastGeneric()?;
    let mut toasts = Vec::new();
    for n in all {
        let app = n.AppInfo().and_then(|a| a.DisplayInfo()).and_then(|d| d.DisplayName()).map(|s| s.to_string()).unwrap_or_default();
        let texts: Vec<String> = n
            .Notification()
            .and_then(|x| x.Visual())
            .and_then(|v| v.GetBinding(&generic))
            .and_then(|b| b.GetTextElements())
            .map(|list| list.into_iter().filter_map(|t| t.Text().ok().map(|s| s.to_string())).collect())
            .unwrap_or_default();
        let at = n.CreationTime().map_or(0, |t| t.UniversalTime / 10_000_000 - 11_644_473_600);
        toasts.push(Toast {
            id: n.Id().unwrap_or_default(),
            app,
            title: texts.first().cloned().unwrap_or_default(),
            body: texts.get(1..).map(|rest| rest.join(" ")).unwrap_or_default(),
            at,
        });
    }
    toasts.sort_by_key(|t| std::cmp::Reverse(t.at));
    Ok(toasts)
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
