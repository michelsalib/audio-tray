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
const AUMID: &str = "MichelSalib.AudioTray";

/// Show a notification: a bold first line, then the body.
pub fn show(title: &str, body: &str) -> Result<()> {
    register();
    let xml = XmlDocument::new()?;
    xml.LoadXml(&HSTRING::from(format!(
        r#"<toast><visual><binding template="ToastGeneric"><text>{}</text><text>{}</text></binding></visual></toast>"#,
        escape(title),
        escape(body)
    )))
    .context("toast XML")?;
    let toast = ToastNotification::CreateToastNotification(&xml)?;
    ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(AUMID))?.Show(&toast).context("show the toast")?;
    Ok(())
}

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
