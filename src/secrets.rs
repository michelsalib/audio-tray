//! Secrets (the Discord application's secret, Discord's and Teams' tokens, the calendars' grants
//! and links) in Windows' Credential Manager: per user, encrypted at rest, and out of
//! `config.toml`, a file people paste into bug reports.
//!
//! A credential holds at most 2560 bytes; a longer secret (a Microsoft refresh token) is split
//! across `name`, `name#1`, `name#2`…, the first part saying how many there are.

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
/// The Google OAuth client's secret, for the calendar.
pub const CALENDAR_GOOGLE_SECRET: &str = "calendar-google-secret";
/// The signed-in calendar accounts (JSON: address, refresh token).
pub const CALENDAR_GOOGLE: &str = "calendar-google";
pub const CALENDAR_MICROSOFT: &str = "calendar-microsoft";
/// The calendar links (JSON: name and address of each).
pub const CALENDAR_LINKS: &str = "calendar-links";

/// What one credential's blob may hold (`CRED_MAX_CREDENTIAL_BLOB_SIZE`).
const BLOB_MAX: usize = 5 * 512;
/// The marker opening the first part of a split secret: `\u{1}parts=N\u{1}`.
const SPLIT: char = '\u{1}';

fn target(name: &str) -> Vec<u16> {
    crate::win::wide(&format!("AudioTray/{name}"))
}

fn read(name: &str) -> Option<String> {
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

fn write(name: &str, value: &str) {
    let mut target = target(name);
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
    if let Err(e) = unsafe { CredWriteW(&credential, 0) } {
        eprintln!("secrets: could not store {name} ({e})");
    }
}

fn delete(name: &str) {
    let target = target(name);
    let _ = unsafe { CredDeleteW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, None) };
}

fn part_name(name: &str, i: usize) -> String {
    format!("{name}#{i}")
}

/// How many parts the secret stored as `name` is in, and its first part's text.
fn parts(first: &str) -> (usize, &str) {
    let parsed = first
        .strip_prefix(SPLIT)
        .and_then(|rest| rest.split_once(SPLIT))
        .and_then(|(head, text)| Some((head.strip_prefix("parts=")?.parse().ok()?, text)));
    parsed.unwrap_or((1, first))
}

/// The secret stored as `name`, if any.
pub fn get(name: &str) -> Option<String> {
    let first = read(name)?;
    let (count, text) = parts(&first);
    let mut value = text.to_string();
    for i in 1..count {
        value.push_str(&read(&part_name(name, i))?);
    }
    Some(value)
}

/// Split `value` into blobs that each fit a credential, on character boundaries.
fn chunks(value: &str) -> Vec<&str> {
    // Room in the first for its marker.
    let mut out = Vec::new();
    let mut rest = value;
    while !rest.is_empty() {
        let room = if out.is_empty() { BLOB_MAX - 16 } else { BLOB_MAX };
        let mut at = rest.len().min(room);
        while !rest.is_char_boundary(at) {
            at -= 1;
        }
        let (head, tail) = rest.split_at(at);
        out.push(head);
        rest = tail;
    }
    out
}

/// Store `value` as `name`, or forget it (`None`). Best-effort: a failure is logged.
pub fn set(name: &str, value: Option<&str>) {
    let old_parts = read(name).map_or(0, |first| parts(&first).0);
    let Some(value) = value else {
        delete(name);
        for i in 1..old_parts {
            delete(&part_name(name, i));
        }
        return;
    };
    if get(name).as_deref() == Some(value) {
        return;
    }
    let pieces = chunks(value);
    if pieces.len() <= 1 && !value.starts_with(SPLIT) {
        write(name, value);
    } else {
        for (i, piece) in pieces.iter().enumerate().skip(1) {
            write(&part_name(name, i), piece);
        }
        write(name, &format!("{SPLIT}parts={}{SPLIT}{}", pieces.len(), pieces.first().copied().unwrap_or_default()));
    }
    for i in pieces.len().max(1)..old_parts {
        delete(&part_name(name, i));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_secrets_split_into_credential_sized_parts_and_short_ones_do_not() {
        assert_eq!(chunks("short"), vec!["short"]);
        let long = "é".repeat(3000); // 6000 bytes
        let pieces = chunks(&long);
        assert!(pieces.len() == 3 && pieces.iter().all(|p| p.len() <= BLOB_MAX));
        assert_eq!(pieces.concat(), long);
        assert_eq!(parts("\u{1}parts=3\u{1}abc"), (3, "abc"));
        assert_eq!(parts("plain"), (1, "plain"));
    }
}
