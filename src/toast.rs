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

/// Show a notification: a bold first line, then the body.
pub fn show(title: &str, body: &str) -> Result<()> {
    show_with(title, body, None, false, "")
}

/// [`show`], with a web address its click and its one button (`label`) open, as a reminder
/// (`reminder`: it stays until dismissed), and with Windows' tag `tag` (empty: none): what it is,
/// for the banner that draws it (`audio-tray/battery/12`, see [`TAG`]). Windows replaces an earlier
/// notification of the same tag.
pub fn show_with(title: &str, body: &str, link: Option<(&str, &str)>, reminder: bool, tag: &str) -> Result<()> {
    register();
    let xml = XmlDocument::new()?;
    xml.LoadXml(&HSTRING::from(payload(title, body, link, reminder))).context("toast XML")?;
    let toast = ToastNotification::CreateToastNotification(&xml)?;
    if !tag.is_empty() {
        // Windows takes 64 characters at most.
        let tag: String = tag.chars().take(64).collect();
        toast.SetTag(&HSTRING::from(tag)).context("tag the toast")?;
    }
    ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(AUMID))?.Show(&toast).context("show the toast")?;
    Ok(())
}

/// Dev: a notification from its own XML, as an app would send it (a call's, a chat's).
#[cfg(feature = "dev")]
pub fn show_xml(xml: &str, tag: &str) -> Result<()> {
    register();
    let doc = XmlDocument::new()?;
    doc.LoadXml(&HSTRING::from(xml)).context("toast XML")?;
    let toast = ToastNotification::CreateToastNotification(&doc)?;
    if !tag.is_empty() {
        toast.SetTag(&HSTRING::from(tag))?;
    }
    ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(AUMID))?.Show(&toast).context("show the toast")?;
    Ok(())
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
