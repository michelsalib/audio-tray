//! A picture for a desktop player's local file. MPC-HC publishes the file's name as its session's
//! title and no cover; its history has the file's path, and the shell has the file's thumbnail (a
//! frame of the video, as Explorer shows it).

use std::cell::RefCell;
use std::path::PathBuf;

use windows::core::PCWSTR;
use windows::Win32::System::Registry::{
    RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, KEY_READ, RRF_RT_REG_SZ,
};

/// The thumbnail's size, in pixels: the largest the surfaces draw a cover, at high DPI.
const THUMBNAIL_PX: u32 = 256;
/// Answers kept, a missing thumbnail among them: no file is looked for twice per reading.
const KEPT: usize = 16;

thread_local! {
    static SEEN: RefCell<Vec<(String, Option<Vec<u8>>)>> = const { RefCell::new(Vec::new()) };
}

/// The thumbnail of the file a player's session titled `title` plays, as PNG, when the player is
/// one this knows and the shell has a thumbnail for the file.
pub fn thumbnail(app_id: &str, title: &str) -> Option<Vec<u8>> {
    if !crate::integrations::mpc::is_mpc(app_id) || title.is_empty() {
        return None;
    }
    if let Some(known) = SEEN.with(|seen| seen.borrow().iter().find(|(t, _)| t == title).map(|(_, png)| png.clone())) {
        return known;
    }
    let png = mpc_file(title).and_then(|path| crate::image::file_thumbnail_png(&path, THUMBNAIL_PX).ok());
    SEEN.with(|seen| {
        let mut seen = seen.borrow_mut();
        if seen.len() >= KEPT {
            seen.remove(0);
        }
        seen.push((title.to_string(), png.clone()));
    });
    png
}

/// The file named `name` in MPC-HC's history, the most recently opened when several match.
fn mpc_file(name: &str) -> Option<PathBuf> {
    let root = windows::core::w!(r"Software\MPC-HC\MPC-HC\MediaHistory");
    let mut key = HKEY::default();
    if unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, root, None, KEY_READ, &mut key) }.is_err() {
        return None;
    }
    let mut best: Option<(String, PathBuf)> = None;
    for index in 0.. {
        let mut buf = [0u16; 256];
        let mut len = buf.len() as u32;
        if unsafe { RegEnumKeyExW(key, index, Some(windows::core::PWSTR(buf.as_mut_ptr())), &mut len, None, None, None, None) }.is_err() {
            break;
        }
        let entry = PCWSTR(buf.as_ptr());
        let Some(file) = string(key, entry, windows::core::w!("Filename")) else { continue };
        let path = PathBuf::from(&file);
        if path.file_name().is_none_or(|f| f.to_string_lossy() != name) {
            continue;
        }
        // ISO 8601: newer sorts later.
        let opened = string(key, entry, windows::core::w!("LastOpened")).unwrap_or_default();
        if best.as_ref().is_none_or(|(at, _)| opened > *at) {
            best = Some((opened, path));
        }
    }
    let _ = unsafe { RegCloseKey(key) };
    best.map(|(_, path)| path).filter(|path| path.exists())
}

/// A string value of subkey `sub` of `key`.
fn string(key: HKEY, sub: PCWSTR, name: PCWSTR) -> Option<String> {
    let mut buf = vec![0u16; 1024];
    let mut size = (buf.len() * 2) as u32;
    let read = unsafe { RegGetValueW(key, sub, name, RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as *mut _), Some(&mut size)) };
    if read.is_err() {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..len]))
}

#[cfg(test)]
mod tests {
    use crate::integrations::mpc::is_mpc;

    #[test]
    fn only_mpc_sessions_are_looked_up() {
        assert!(is_mpc("mpc-hc64.exe"));
        assert!(is_mpc("MPC-BE64.EXE"));
        assert!(!is_mpc("Spotify.exe"));
    }
}
