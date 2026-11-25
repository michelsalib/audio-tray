//! Windows notifications (toasts), for the warnings worth seeing outside the app: a battery going
//! low.
//!
//! An unpackaged app shows toasts under an AppUserModelID it registers itself, per user, under
//! `HKCU\Software\Classes\AppUserModelId\<id>`: the name and icon the notification carries. No
//! Start menu shortcut is needed.

use anyhow::{Context, Result};
use windows::core::{HSTRING, PCWSTR};
use windows::Data::Xml::Dom::XmlDocument;
use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};
use windows::Win32::System::Registry::{RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ};

/// The id the notifications show under.
pub(crate) const AUMID: &str = "MichelSalib.AudioTray";

/// Dev: a plain notification, a bold first line, then the body.
#[cfg(feature = "dev")]
pub fn show(title: &str, body: &str) -> Result<()> {
    show_with(title, body, None, false, "")
}

/// A notification: a bold first line, then the body, with a web address its click and its one
/// button (`label`) open, as a reminder (`reminder`: it stays until dismissed), and with Windows'
/// tag `tag` (empty: none): what it is, for the banner that draws it (`audio-tray/battery/3f2a`,
/// see [`TAG`]). Windows replaces an earlier notification of the same tag.
pub fn show_with(title: &str, body: &str, link: Option<(&str, &str)>, reminder: bool, tag: &str) -> Result<()> {
    show_payload(&payload(title, body, link, reminder), tag)
}

/// Show toast XML `xml` under Audio Tray's id, tagged `tag` (empty: none).
fn show_payload(xml: &str, tag: &str) -> Result<()> {
    register();
    let doc = XmlDocument::new()?;
    doc.LoadXml(&HSTRING::from(xml)).context("toast XML")?;
    let toast = ToastNotification::CreateToastNotification(&doc)?;
    if !tag.is_empty() {
        // Windows takes 64 characters at most.
        let tag: String = tag.chars().take(64).collect();
        toast.SetTag(&HSTRING::from(tag)).context("tag the toast")?;
    }
    ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(AUMID))?.Show(&toast).context("show the toast")?;
    Ok(())
}

/// [`show_with`] for `o`, its failure logged.
fn show_outgoing(o: &Outgoing) {
    let link = o.link.as_ref().map(|(label, url)| (label.as_str(), url.as_str()));
    if let Err(e) = show_with(&o.title, &o.body, link, o.reminder, &o.tag) {
        eprintln!("notifications: could not show Audio Tray's own ({e:#})");
    }
}

/// A notification to send: [`show_with`]'s arguments, owned.
pub struct Outgoing {
    pub title: String,
    pub body: String,
    /// Its button's label and the web address it (and a click) opens.
    pub link: Option<(String, String)>,
    pub reminder: bool,
    pub tag: String,
}

/// [`show_with`] on a thread of its own (one, long-lived): Windows' notification platform can be
/// slow to answer, and the tray thread must not wait for it. Failures are logged.
pub fn send(outgoing: Outgoing) {
    use std::sync::mpsc::{channel, Sender};
    static WORKER: std::sync::Mutex<Option<Sender<Outgoing>>> = std::sync::Mutex::new(None);
    let mut worker = WORKER.lock().unwrap_or_else(|e| e.into_inner());
    if worker.is_none() {
        let (send, receive) = channel::<Outgoing>();
        let spawned = std::thread::Builder::new().name("toasts".into()).spawn(move || {
            use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
            let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
            while let Ok(o) = receive.recv() {
                show_outgoing(&o);
            }
        });
        if spawned.is_ok() {
            *worker = Some(send);
        }
    }
    let o = match worker.as_ref() {
        Some(w) => match w.send(outgoing) {
            Ok(()) => return,
            Err(std::sync::mpsc::SendError(o)) => o,
        },
        None => outgoing,
    };
    // The thread is gone (or never started): sent from here instead.
    *worker = None;
    drop(worker);
    show_outgoing(&o);
}

/// Dev: a notification from its own XML, as an app would send it (a call's, a chat's).
#[cfg(feature = "dev")]
pub fn show_xml(xml: &str, tag: &str) -> Result<()> {
    show_payload(xml, tag)
}

/// What Audio Tray's own notifications' tags start with: `audio-tray/<kind>/<detail>`.
pub(crate) const TAG: &str = "audio-tray/";

/// Name the id, once per run: the app's name, and its icon, written next to the config (the
/// registry wants a file).
fn register() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let key = format!(r"Software\Classes\AppUserModelId\{AUMID}");
        set(&key, "DisplayName", "Audio Tray");
        let icon = crate::config::Config::path().ok().and_then(|p| p.parent().map(|d| d.join("notification.ico")));
        if let Some(icon) = icon {
            if std::fs::write(&icon, include_bytes!("../assets/app.ico")).is_ok() {
                set(&key, "IconUri", &icon.to_string_lossy());
            }
        }
    });
}

fn set(key: &str, name: &str, value: &str) {
    let (key, name, value) = (crate::win::wide(key), crate::win::wide(name), crate::win::wide(value));
    let status = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            PCWSTR(key.as_ptr()),
            PCWSTR(name.as_ptr()),
            REG_SZ.0,
            Some(value.as_ptr() as *const core::ffi::c_void),
            (value.len() * 2) as u32,
        )
    };
    if status.is_err() {
        eprintln!("toast: could not register the app's name ({status:?})");
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// The toast's XML. A reminder needs a button (Windows' rule): without a link it is a plain toast.
fn payload(title: &str, body: &str, link: Option<(&str, &str)>, reminder: bool) -> String {
    let text = format!(r#"<visual><binding template="ToastGeneric"><text>{}</text><text>{}</text></binding></visual>"#, escape(title), escape(body));
    match link {
        Some((label, url)) => {
            let scenario = if reminder { r#" scenario="reminder""# } else { "" };
            format!(
                r#"<toast activationType="protocol" launch="{url}"{scenario}>{text}<actions><action content="{label}" activationType="protocol" arguments="{url}"/></actions></toast>"#,
                url = escape(url),
                label = escape(label),
            )
        }
        None => format!("<toast>{text}</toast>"),
    }
}

#[cfg(test)]
mod tests {
    use super::payload;

    #[test]
    fn a_reminder_carries_its_link_as_its_click_and_its_button() {
        let xml = payload("Sync", "In 5 min", Some(("Join", "https://meet.google.com/a?b=1&c=2")), true);
        assert!(xml.starts_with(r#"<toast activationType="protocol" launch="https://meet.google.com/a?b=1&amp;c=2" scenario="reminder">"#), "{xml}");
        assert!(xml.contains(r#"<action content="Join" activationType="protocol" arguments="https://meet.google.com/a?b=1&amp;c=2"/>"#));
        assert_eq!(payload("A", "B", None, true), r#"<toast><visual><binding template="ToastGeneric"><text>A</text><text>B</text></binding></visual></toast>"#);
    }
}
