//! Secrets (the Discord application's secret, Discord's and Teams' tokens) in Windows' Credential
//! Manager: per user, encrypted at rest, and out of `config.toml`, a file people paste into bug
//! reports.

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Security::Credentials::{
    CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_FLAGS, CRED_PERSIST_LOCAL_MACHINE,
    CRED_TYPE_GENERIC,
};

/// The Discord application's client secret.
pub const DISCORD_SECRET: &str = "discord-client-secret";
/// Discord's OAuth token (JSON: access, refresh, expiry).
pub const DISCORD_TOKEN: &str = "discord-token";
/// The token Teams hands over when Audio Tray is allowed.
pub const TEAMS_TOKEN: &str = "teams-token";

fn target(name: &str) -> Vec<u16> {
    crate::win::wide(&format!("AudioTray/{name}"))
}

/// The secret stored as `name`, if any.
pub fn get(name: &str) -> Option<String> {
    let target = target(name);
    let mut credential: *mut CREDENTIALW = std::ptr::null_mut();
    unsafe {
        CredReadW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, None, &mut credential).ok()?;
        let blob = std::slice::from_raw_parts((*credential).CredentialBlob, (*credential).CredentialBlobSize as usize);
        let value = String::from_utf8(blob.to_vec()).ok();
        CredFree(credential as *const core::ffi::c_void);
        value
    }
}

/// Store `value` as `name`, or forget it (`None`). Best-effort: a failure is logged.
pub fn set(name: &str, value: Option<&str>) {
    let mut target = target(name);
    unsafe {
        let Some(value) = value else {
            let _ = CredDeleteW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, None);
            return;
        };
        if get(name).as_deref() == Some(value) {
            return;
        }
        let mut user = crate::win::wide("Audio Tray");
        let credential = CREDENTIALW {
            Flags: CRED_FLAGS(0),
            Type: CRED_TYPE_GENERIC,
            TargetName: PWSTR(target.as_mut_ptr()),
            CredentialBlobSize: value.len() as u32,
            CredentialBlob: value.as_ptr() as *mut u8,
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            UserName: PWSTR(user.as_mut_ptr()),
            ..Default::default()
        };
        if let Err(e) = CredWriteW(&credential, 0) {
            eprintln!("secrets: could not store {name} ({e})");
        }
    }
}
