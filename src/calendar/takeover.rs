//! Audio Tray showing every notification itself (Settings' "Audio Tray shows every notification"):
//! Windows' own banners are turned off, app by app, the way its Settings page does it (each app's
//! `ShowBanner` under `HKCU\…\Notifications\Settings\<app>`), so that only Audio Tray's show. The
//! notifications themselves still reach Windows' centre, where Audio Tray reads them.
//!
//! What each app had before is written down first (`banners.txt`, next to the config) and given
//! back when this is turned off, when the app quits or Windows signs out, when it is uninstalled
//! (`--give-back-banners`), and at the next start after a crash: Windows' banners must never stay
//! off without Audio Tray there to show its own.
//!
//! The user stays in charge of each app in Windows' Settings: an app whose banners they turn back
//! on there is left to Windows (its banners are Windows', not Audio Tray's), and one they keep out
//! of the notification centre gets its banners back (Audio Tray could not show it otherwise).
//!
//! An app Windows has never heard from has no key yet: its first notification shows Windows'
//! banner, then its key is taken at the next read.

use std::sync::Mutex;
use super::lock;

use windows::core::PCWSTR;
use windows::Win32::System::Registry::{RegCloseKey, RegDeleteKeyValueW, RegOpenKeyExW, RegSetKeyValueW, HKEY, HKEY_CURRENT_USER, KEY_READ, REG_DWORD};

const SETTINGS: &str = r"Software\Microsoft\Windows\CurrentVersion\Notifications\Settings";
const SHOW_BANNER: &str = "ShowBanner";
/// An app keeping its notifications out of Windows' centre: Audio Tray could not show them, so its
/// banners stay Windows'.
const SHOW_IN_CENTER: &str = "ShowInActionCenter";

/// One app Audio Tray has dealt with.
#[derive(Clone, Debug, PartialEq)]
struct Kept {
    app: String,
    /// Its banner setting before Audio Tray took it: `None`, never set (Windows' default, on).
    was: Option<u32>,
    /// Left to Windows: the user turned its banners back on in Windows' Settings. Not taken again,
    /// and Audio Tray shows none of its own for it.
    left: bool,
}

/// What `banners.txt` holds, read once (`None` before).
static KEPT: Mutex<Option<Vec<Kept>>> = Mutex::new(None);

/// Windows' banners are Audio Tray's to replace (`on`) or Windows' again. Cheap enough to call on
/// every change of the list: one walk of the apps' keys, and only what changed is written.
pub fn apply(on: bool) {
    if on {
        take();
    } else {
        give_back();
    }
}

/// The written-down apps, read from the file at the first call.
fn kept() -> std::sync::MutexGuard<'static, Option<Vec<Kept>>> {
    let mut kept = kept_lock();
    if kept.is_none() {
        *kept = Some(load());
    }
    kept
}

/// [`KEPT`] as it is (not read from the file).
fn kept_lock() -> std::sync::MutexGuard<'static, Option<Vec<Kept>>> {
    lock(&KEPT)
}

/// A walk of every app's key is owed whatever the watch says (the last could not note its work).
static WALK_OWED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether app `aumid`'s notifications get Audio Tray's banner: not when the user had turned its
/// banners off in Windows before (kept as they were), nor when they left it to Windows, nor when
/// Windows keeps it out of its centre.
pub fn wants_banner(aumid: &str) -> bool {
    let key = format!(r"{SETTINGS}\{aumid}");
    if dword(&key, SHOW_IN_CENTER) == Some(0) {
        return false;
    }
    let found = kept().as_ref().and_then(|k| k.iter().find(|k| k.app == aumid).cloned());
    match found {
        Some(kept) => !kept.left && kept.was != Some(0),
        // Not taken (yet): banners on unless turned off.
        None => dword(&key, SHOW_BANNER) != Some(0),
    }
}

/// Whether Windows shows its own banner for a notification from `aumid` that just came: its app
/// (or, for a browser, its site) has banners on that Audio Tray has not taken. Windows makes an
/// app's settings at its first notification, so its first banner is Windows' (taken at once
/// after): Audio Tray does not show a second. A browser notifies under its own id (`…!App`) while
/// Windows keeps a setting per site (`…!https://site/`): the app's package (before `!`) is matched.
pub fn windows_shows(aumid: &str) -> bool {
    let package = |id: &str| id.split('!').next().unwrap_or(id).to_ascii_lowercase();
    let wanted = package(aumid);
    let taken: Vec<String> = kept().as_ref().map(|k| k.iter().filter(|k| !k.left).map(|k| k.app.clone()).collect()).unwrap_or_default();
    apps().into_iter().any(|app| {
        let key = format!(r"{SETTINGS}\{app}");
        package(&app) == wanted && !taken.contains(&app) && dword(&key, SHOW_BANNER) != Some(0) && dword(&key, SHOW_IN_CENTER) != Some(0)
    })
}

/// What [`take`] does with one app, from what its key says now.
#[derive(Debug, PartialEq)]
enum Step {
    /// Nothing to do.
    Leave,
    /// Its banners off, what it had written down first.
    Take { was: Option<u32> },
    /// Kept out of the centre now: its banners back as they were, and forgotten.
    GiveBack { was: Option<u32> },
    /// Its banners turned back on by the user: left to Windows.
    LeaveToWindows,
}

/// What to do with an app whose key says `banner` and `in_center`, given what was written down.
fn step(kept: Option<&Kept>, banner: Option<u32>, in_center: Option<u32>) -> Step {
    let out_of_center = in_center == Some(0);
    match kept {
        // Off already, by the user: theirs, left alone (and never turned on later).
        None if banner == Some(0) || out_of_center => Step::Leave,
        None => Step::Take { was: banner },
        Some(k) if k.left => Step::Leave,
        Some(k) if out_of_center => Step::GiveBack { was: k.was },
        Some(k) if k.was == Some(0) => Step::Leave,
        // Audio Tray wrote 0: anything else is the user's doing, in Windows' Settings.
        Some(_) if banner != Some(0) => Step::LeaveToWindows,
        Some(_) => Step::Leave,
    }
}

/// Windows' notification settings changed since last asked (an app's first notification made its
/// key, a switch was flipped in Settings, Audio Tray wrote): only then is a walk of every app's
/// key worth it. The first ask is always yes. Asked on the tray thread only (the watch is the
/// calling thread's).
fn settings_changed() -> bool {
    use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
    use windows::Win32::System::Registry::{RegNotifyChangeKeyValue, KEY_NOTIFY, REG_NOTIFY_CHANGE_LAST_SET, REG_NOTIFY_CHANGE_NAME};
    use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
    /// The watched key and its event, as raw values (made once, kept for the process).
    static WATCH: Mutex<Option<(isize, isize)>> = Mutex::new(None);
    let mut watch = lock(&WATCH);
    let arm = |key: isize, event: isize| unsafe {
        RegNotifyChangeKeyValue(HKEY(key as *mut _), true, REG_NOTIFY_CHANGE_NAME | REG_NOTIFY_CHANGE_LAST_SET, Some(HANDLE(event as *mut _)), true).is_ok()
    };
    match *watch {
        Some((key, event)) => {
            if unsafe { WaitForSingleObject(HANDLE(event as *mut _), 0) } != WAIT_OBJECT_0 {
                return false;
            }
            // Armed again before the walk: what this walk writes is seen next time (once).
            if !arm(key, event) {
                *watch = None;
            }
            true
        }
        None => {
            let path = crate::win::wide(SETTINGS);
            let mut key = HKEY::default();
            if unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(path.as_ptr()), None, KEY_NOTIFY | KEY_READ, &mut key) }.is_err() {
                return true;
            }
            let Ok(event) = (unsafe { CreateEventW(None, false, false, None) }) else {
                let _ = unsafe { RegCloseKey(key) };
                return true;
            };
            if arm(key.0 as isize, event.0 as isize) {
                *watch = Some((key.0 as isize, event.0 as isize));
            }
            true
        }
    }
}

/// Every app's banner off (what it had written down first), and what the user changed in
/// Windows' Settings since followed: walked only when those settings changed.
fn take() {
    // Both asked: the watch is re-armed by its ask even when a walk is owed anyway.
    let changed = settings_changed();
    if !WALK_OWED.swap(false, std::sync::atomic::Ordering::Relaxed) && !changed {
        return;
    }
    let mut guard = kept();
    let kept = guard.get_or_insert_with(Vec::new);
    let mut plan = Vec::new();
    for app in apps() {
        let key = format!(r"{SETTINGS}\{app}");
        let was = kept.iter().find(|k| k.app == app);
        let s = step(was, dword(&key, SHOW_BANNER), dword(&key, SHOW_IN_CENTER));
        if s != Step::Leave {
            plan.push((app, s));
        }
    }
    if plan.is_empty() {
        return;
    }
    for (app, s) in &plan {
        match s {
            Step::Take { was } => kept.push(Kept { app: app.clone(), was: *was, left: false }),
            Step::GiveBack { .. } => kept.retain(|k| k.app != *app),
            Step::LeaveToWindows => {
                if let Some(k) = kept.iter_mut().find(|k| k.app == *app) {
                    k.left = true;
                }
            }
            Step::Leave => {}
        }
    }
    // Written down before anything changes, once: a crash between the two must not lose it.
    if let Err(e) = save(kept) {
        eprintln!("notifications: could not note Windows' banner settings, leaving them ({e:#})");
        *guard = None;
        // Nothing written, so the watch hears nothing: the next read tries again.
        WALK_OWED.store(true, std::sync::atomic::Ordering::Relaxed);
        return;
    }
    let mut taken = 0;
    // Given back in writing only: one Windows refused stays written down, for the next walk.
    let mut not_given = Vec::new();
    for (app, s) in &plan {
        let key = format!(r"{SETTINGS}\{app}");
        match s {
            Step::Take { .. } => {
                if set_dword(&key, SHOW_BANNER, 0) {
                    taken += 1;
                }
            }
            Step::GiveBack { was } => {
                let given = match was {
                    Some(value) => set_dword(&key, SHOW_BANNER, *value),
                    None => delete(&key, SHOW_BANNER),
                };
                if given {
                    println!("notifications: {app} is kept out of Windows' centre: its banners are Windows' again");
                } else {
                    not_given.push(Kept { app: app.clone(), was: *was, left: false });
                }
            }
            Step::LeaveToWindows => println!("notifications: {app}'s banners were turned back on in Windows: left to Windows"),
            Step::Leave => {}
        }
    }
    if !not_given.is_empty() {
        kept.extend(not_given);
        if let Err(e) = save(kept) {
            eprintln!("notifications: could not note the apps whose banners were not given back ({e:#})");
        }
        WALK_OWED.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    if taken > 0 {
        println!("notifications: Windows' banners off for {} apps ({taken} new)", kept.iter().filter(|k| !k.left).count());
    }
}

/// Every app's banner as it was (an app's key gone since, uninstalled, is not made again).
fn give_back() {
    let mut guard = kept();
    let kept = guard.take().unwrap_or_default();
    *guard = Some(Vec::new());
    drop(guard);
    if kept.is_empty() {
        return;
    }
    let mut given = 0;
    // Those that could not be written stay written down, for the next try (the next give-back,
    // the next start): forgetting them would leave their banners off for good.
    let mut failed = Vec::new();
    for k in kept.iter().filter(|k| !k.left) {
        let key = format!(r"{SETTINGS}\{}", k.app);
        if !key_exists(&key) {
            continue;
        }
        let done = match k.was {
            Some(value) => set_dword(&key, SHOW_BANNER, value),
            None => delete(&key, SHOW_BANNER),
        };
        if done {
            given += 1;
        } else {
            failed.push(k.clone());
        }
    }
    if failed.is_empty() {
        if let Ok(path) = file() {
            let _ = std::fs::remove_file(path);
        }
    } else {
        eprintln!("notifications: {} apps' banners could not be given back; kept for the next try", failed.len());
        if let Err(e) = save(&failed) {
            eprintln!("notifications: could not note them either ({e:#})");
        }
        *kept_lock() = Some(failed);
    }
    println!("notifications: Windows' banners given back to {given} apps");
}

/// Give Windows its banners back from the file alone, Audio Tray not running (the uninstaller's
/// `--give-back-banners`).
pub fn give_back_now() {
    give_back();
}

/// The apps Windows keeps notification settings for.
fn apps() -> Vec<String> {
    crate::win::subkeys(HKEY_CURRENT_USER, SETTINGS)
}

fn key_exists(key: &str) -> bool {
    let wide = crate::win::wide(key);
    let mut handle = HKEY::default();
    let opened = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(wide.as_ptr()), None, KEY_READ, &mut handle) }.is_ok();
    if opened {
        let _ = unsafe { RegCloseKey(handle) };
    }
    opened
}

fn dword(key: &str, name: &str) -> Option<u32> {
    let (key, name) = (crate::win::wide(key), crate::win::wide(name));
    crate::win::hkcu_dword(PCWSTR(key.as_ptr()), PCWSTR(name.as_ptr()))
}

/// Whether it was written.
fn set_dword(key: &str, name: &str, value: u32) -> bool {
    let (k, n) = (crate::win::wide(key), crate::win::wide(name));
    let set = unsafe { RegSetKeyValueW(HKEY_CURRENT_USER, PCWSTR(k.as_ptr()), PCWSTR(n.as_ptr()), REG_DWORD.0, Some(&value as *const u32 as *const _), 4) };
    if set.is_err() {
        eprintln!("notifications: could not set {name} for {key} ({set:?})");
    }
    set.is_ok()
}

/// Whether it is gone (or was never there).
fn delete(key: &str, name: &str) -> bool {
    let (k, n) = (crate::win::wide(key), crate::win::wide(name));
    let deleted = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, PCWSTR(k.as_ptr()), PCWSTR(n.as_ptr())) };
    let gone = deleted.is_ok() || deleted == windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
    if !gone {
        eprintln!("notifications: could not delete {name} for {key} ({deleted:?})");
    }
    gone
}

/// Whether Windows' "Allow notifications to play sounds" is on (absent: on, its default).
pub fn sounds_allowed() -> bool {
    dword(SETTINGS, "NOC_GLOBAL_SETTING_ALLOW_NOTIFICATION_SOUND") != Some(0)
}

/// Where what each app had is written down.
fn file() -> anyhow::Result<std::path::PathBuf> {
    let config = crate::config::Config::path()?;
    Ok(config.parent().ok_or_else(|| anyhow::anyhow!("the config has no folder"))?.join("banners.txt"))
}

fn load() -> Vec<Kept> {
    file().ok().and_then(|p| std::fs::read_to_string(p).ok()).map(|text| parse(&text)).unwrap_or_default()
}

fn save(kept: &[Kept]) -> anyhow::Result<()> {
    let path = file()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if kept.is_empty() {
        let _ = std::fs::remove_file(path);
        return Ok(());
    }
    std::fs::write(path, render(kept))?;
    Ok(())
}

/// One app a line: its value before (`-`: none; `w` before it: left to Windows), a tab, its name
/// (which may hold anything but a line break or a tab).
fn render(kept: &[Kept]) -> String {
    kept.iter()
        .map(|k| format!("{}{}\t{}\n", if k.left { "w" } else { "" }, k.was.map_or_else(|| "-".to_string(), |v| v.to_string()), k.app))
        .collect()
}

fn parse(text: &str) -> Vec<Kept> {
    text.lines()
        .filter_map(|line| {
            let (was, app) = line.split_once('\t')?;
            let (left, was) = match was.strip_prefix('w') {
                Some(rest) => (true, rest),
                None => (false, was),
            };
            let was = if was == "-" { None } else { Some(was.parse().ok()?) };
            (!app.is_empty()).then(|| Kept { app: app.to_string(), was, left })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_each_app_had_reads_back_as_written() {
        let kept = vec![
            Kept { app: "Microsoft.MicrosoftEdge.Stable_8wekyb3d8bbwe!https://claude.ai/".into(), was: None, left: false },
            Kept { app: "Microsoft.YourPhone_8wekyb3d8bbwe!FileTransfer_S23 Ultra".into(), was: Some(1), left: true },
        ];
        assert_eq!(parse(&render(&kept)), kept);
        // A damaged line is skipped, not the file.
        assert_eq!(parse("x\tA\n-\tB\n"), vec![Kept { app: "B".into(), was: None, left: false }]);
    }

    #[test]
    fn the_user_stays_in_charge_of_each_app_in_windows_settings() {
        let taken = Kept { app: "A".into(), was: None, left: false };
        // New: taken, what it had written down; off already by the user, or out of the centre: left.
        assert_eq!(step(None, None, None), Step::Take { was: None });
        assert_eq!(step(None, Some(1), None), Step::Take { was: Some(1) });
        assert_eq!(step(None, Some(0), None), Step::Leave);
        assert_eq!(step(None, None, Some(0)), Step::Leave);
        // Taken and still off: nothing to do.
        assert_eq!(step(Some(&taken), Some(0), None), Step::Leave);
        // Kept out of the centre since: its banners back, or it would show nowhere.
        assert_eq!(step(Some(&taken), Some(0), Some(0)), Step::GiveBack { was: None });
        // Its banners turned back on in Windows (a 1, or the value deleted): Windows' from now on.
        assert_eq!(step(Some(&taken), Some(1), None), Step::LeaveToWindows);
        assert_eq!(step(Some(&taken), None, None), Step::LeaveToWindows);
        let left = Kept { left: true, ..taken };
        assert_eq!(step(Some(&left), Some(1), None), Step::Leave);
    }
}
