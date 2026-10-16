//! In-app self-updater (the "auto-update" leg of the release setup).
//!
//! A background thread replaces the on-disk exe from the latest GitHub release; it takes effect on
//! the next start (the running tray is never killed). `self_update` replaces only the exe, so
//! `update_tap` fetches `audio_tray_tap.dll` too, moving a copy Explorer holds aside (or, failing
//! that, staging it for the next Explorer restart). [`repair_stale_tap`] runs in the *new* build
//! and re-fetches a DLL that does not match this exe. Release builds only (`--update` checks in
//! debug but never replaces the TAP). Compares `CARGO_PKG_VERSION`, which CI holds equal to the
//! release tag.
//!
//! Every download is checked against the release's [`SUMS_ASSET`] before anything is replaced; a
//! release without one, or a file that does not match it, is refused.

use std::sync::Mutex;

use anyhow::{bail, Context, Result};

const REPO_OWNER: &str = "michelsalib";
const REPO_NAME: &str = "audio-tray";
const BIN_NAME: &str = "audio-tray";
/// Must match the asset name suffix produced by the release workflow.
const TARGET: &str = "x86_64-pc-windows-msvc";
/// The release asset listing a SHA-256 for every other asset (`<hex>  <name>` lines). Must match
/// the name the release workflow publishes; its name must not contain [`TARGET`], or
/// `asset_for` could pick it as the update archive.
const SUMS_ASSET: &str = "SHA256SUMS";
use tap_proto::TAP_DLL;

/// The version a background update applied to the on-disk exe; the flyout offers a restart.
static PENDING: Mutex<Option<String>> = Mutex::new(None);

/// Where a downloaded TAP waits when it could not be copied into place. Keyed by version, so the
/// new build (which places it after restarting Explorer) finds it by its own version.
fn staging_dir(version: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("audio-tray-tap-{version}"))
}

/// The version an applied-but-not-yet-running update will upgrade to, if any.
pub fn pending_version() -> Option<String> {
    PENDING.lock().ok().and_then(|g| g.clone())
}

/// Record that version `v` has been staged on disk (called by the background check on a
/// successful update, and by the `--flyout update` dev preview to fake one).
pub fn set_pending_version(v: impl Into<String>) {
    if let Ok(mut g) = PENDING.lock() {
        *g = Some(v.into());
    }
}

/// Spawn the background update check. Errors are only logged. No-op in debug builds.
pub fn spawn_background_check() {
    if cfg!(debug_assertions) {
        return;
    }
    std::thread::spawn(|| match check_and_apply(false) {
        Ok(self_update::VersionStatus::Updated(v)) => set_pending_version(v),
        // Only when up to date: after an update the new DLL is beside an old exe, and repairing
        // would downgrade it.
        Ok(self_update::VersionStatus::UpToDate(_)) => repair_stale_tap(false),
        Ok(_) => {}
        Err(e) => eprintln!("audio-tray: background update check failed: {e:#}"),
    });
}

/// `audio_tray_tap.dll`, beside the running exe.
fn installed_tap() -> Result<std::path::PathBuf> {
    let exe = std::env::current_exe().context("locating the running exe")?;
    let dir = exe.parent().context("exe has no parent directory")?;
    Ok(dir.join(TAP_DLL))
}

/// The version stamped into the installed TAP (`VS_FIXEDFILEINFO`, written by
/// `crates/taskbar-tap/build.rs`), or `None` if unstamped — older DLLs, the ones a repair is for.
fn installed_tap_version() -> Option<String> {
    use windows::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW, VS_FIXEDFILEINFO,
    };
    use windows_core::HSTRING;

    let path = HSTRING::from(installed_tap().ok()?.as_os_str());
    let size = unsafe { GetFileVersionInfoSizeW(&path, None) };
    if size == 0 {
        return None;
    }
    let mut block = vec![0u8; size as usize];
    unsafe { GetFileVersionInfoW(&path, None, size, block.as_mut_ptr().cast()) }.ok()?;

    let mut info: *mut core::ffi::c_void = core::ptr::null_mut();
    let mut len = 0u32;
    let ok = unsafe {
        VerQueryValueW(
            block.as_ptr().cast(),
            &HSTRING::from("\\"),
            &mut info,
            &mut len,
        )
    };
    // Borrowed pointer into `block`; nothing to free.
    if !ok.as_bool() || info.is_null() || (len as usize) < size_of::<VS_FIXEDFILEINFO>() {
        return None;
    }
    let fixed = unsafe { *info.cast::<VS_FIXEDFILEINFO>() };
    Some(format!(
        "{}.{}.{}",
        fixed.dwFileVersionMS >> 16,
        fixed.dwFileVersionMS & 0xFFFF,
        fixed.dwFileVersionLS >> 16
    ))
}

/// Fetch the TAP that belongs with this exe, if the one on disk is not it. Runs in the new build,
/// so it can fix what an old build's `update_tap` got wrong. Logged, never fatal.
pub fn repair_stale_tap(verbose: bool) {
    let running = self_update::cargo_crate_version!();
    if !cfg!(debug_assertions) {
        remove_set_aside_taps();
    }
    match installed_tap_version() {
        Some(version) if version == running => {
            if verbose {
                println!("{TAP_DLL} is v{version} — in step with the exe.");
            }
            return;
        }
        Some(version) => {
            eprintln!("audio-tray: {TAP_DLL} is v{version} but the exe is v{running}");
        }
        None => eprintln!("audio-tray: {TAP_DLL} carries no version — it predates the stamp"),
    }

    // A debug build must not overwrite the dev TAP with a release one.
    if cfg!(debug_assertions) {
        println!("(debug build — leaving {TAP_DLL} alone)");
        return;
    }

    // Already staged and waiting for Explorer to release the file: retry the copy, don't re-download.
    if staging_dir(running).join(TAP_DLL).is_file() {
        if !place_staged_tap() {
            println!("audio-tray: the v{running} {TAP_DLL} is staged — it lands on the next Explorer restart.");
        }
        return;
    }

    match update_tap(running, verbose) {
        Ok(()) => match installed_tap_version() {
            Some(version) if version == running => println!("audio-tray: {TAP_DLL} is now v{version}."),
            // Neither copied nor set aside: staged; the guard above finds it next launch.
            _ if staging_dir(running).join(TAP_DLL).is_file() => println!(
                "audio-tray: the v{running} {TAP_DLL} is staged — it lands on the next Explorer restart."
            ),
            // Copied yet unstamped: the release asset itself is wrong, and this repeats every launch.
            _ => eprintln!(
                "audio-tray: fetched the v{running} {TAP_DLL}, but it reports no version — that release's asset is unstamped"
            ),
        },
        Err(e) => eprintln!("audio-tray: could not repair {TAP_DLL} ({e:#})"),
    }
}

/// Print the exe and installed TAP versions, for `--tap-version`.
pub fn report_tap_version() {
    println!("audio-tray v{}", self_update::cargo_crate_version!());
    match installed_tap() {
        Ok(path) => println!("{}", path.display()),
        Err(e) => println!("(cannot locate {TAP_DLL}: {e:#})"),
    }
    match installed_tap_version() {
        Some(version) => println!("{TAP_DLL} v{version}"),
        None => println!("{TAP_DLL} carries no version resource — it predates the stamp, or is not there"),
    }
}

/// Run an update check synchronously, printing progress. Backs the `--update`
/// command. Returns Ok whether or not an update was applied.
pub fn run_manual() -> Result<()> {
    println!("audio-tray v{}", self_update::cargo_crate_version!());
    println!("Checking github.com/{REPO_OWNER}/{REPO_NAME} for a newer release...");
    match check_and_apply(true)? {
        self_update::VersionStatus::UpToDate(v) => {
            println!("Already up to date (v{v}).");
            // Exe settled: hold the DLL to the same version.
            repair_stale_tap(true);
        }
        self_update::VersionStatus::Updated(v) => {
            println!("Updated to v{v}. Restart audio-tray to run the new version.");
        }
        status => println!("Update check: {status}."),
    }
    Ok(())
}

fn check_and_apply(verbose: bool) -> Result<self_update::VersionStatus> {
    let status = self_update::backends::github::Update::configure()
        .repo_owner(REPO_OWNER)
        .repo_name(REPO_NAME)
        .bin_name(BIN_NAME)
        .target(TARGET)
        .current_version(self_update::cargo_crate_version!())
        // GUI/background process: never block on a stdin confirmation prompt.
        .no_confirm(true)
        // Fetched and checked before the exe is touched; a missing entry or mismatch fails the update.
        .checksum_from_asset(SUMS_ASSET)
        .show_download_progress(verbose)
        .show_output(verbose)
        .build()
        .context("configuring self-updater")?
        .update()
        .context("downloading/applying update")?;

    // `self_update` replaced only the exe; the TAP gets its own pass. Never fatal: a stale DLL
    // degrades (the init-data protocol tolerates unknown and missing keys).
    if let self_update::VersionStatus::Updated(version) = &status {
        if let Err(e) = update_tap(version, verbose) {
            eprintln!("audio-tray: exe updated but the taskbar TAP did not ({e:#})");
        }
    }
    Ok(status)
}


/// Fetch `audio_tray_tap.dll` for `version`, verified against the release's [`SUMS_ASSET`], and
/// put it beside the exe (see [`install_tap`]). Nothing is replaced unless the archive verifies.
fn update_tap(version: &str, verbose: bool) -> Result<()> {
    use std::fs;

    let target = installed_tap()?;
    remove_set_aside_taps();

    // Same asset the exe came from, fetched again.
    let release = self_update::backends::github::ReleaseList::configure()
        .repo_owner(REPO_OWNER)
        .repo_name(REPO_NAME)
        .build()
        .context("configuring the release lookup")?
        .fetch()
        .context("listing releases")?
        .into_vec()
        .into_iter()
        .find(|release| release.version() == version)
        .with_context(|| format!("release v{version} not found"))?;
    let asset = release
        .asset_for(TARGET, None)
        .with_context(|| format!("v{version} has no {TARGET} asset"))?;
    let sums_asset = release
        .assets()
        .iter()
        .find(|a| a.name() == SUMS_ASSET)
        .with_context(|| format!("v{version} publishes no {SUMS_ASSET}; refusing an unverifiable {TAP_DLL}"))?;

    // Resolved before the archive download, so an unverifiable release costs one small request.
    let mut sums = Vec::new();
    download_asset(sums_asset, &mut sums, false).with_context(|| format!("downloading {SUMS_ASSET}"))?;
    let sums = String::from_utf8(sums).with_context(|| format!("{SUMS_ASSET} is not UTF-8 text"))?;
    let expected = expected_sha256(&sums, asset.name())?;

    // Not a `TempDir`: a blocked copy must outlive this process (`place_staged_tap`).
    let staging = staging_dir(version);
    fs::create_dir_all(&staging).context("creating a staging directory")?;

    let archive = staging.join(asset.name());
    let file = fs::File::create(&archive).context("creating the download file")?;
    download_asset(&asset, file, verbose).context("downloading the release asset")?;

    if let Err(e) = verify_sha256(&archive, &expected) {
        let _ = fs::remove_dir_all(&staging);
        return Err(e.context(format!("refusing {}", asset.name())));
    }

    self_update::Extract::from_source(&archive)
        .archive(self_update::ArchiveKind::Zip)
        .extract_file(&staging, TAP_DLL)
        .with_context(|| format!("{TAP_DLL} is not in {}", asset.name()))?;
    let fresh = staging.join(TAP_DLL);
    let _ = fs::remove_file(&archive);

    if install_tap(&fresh, &target) {
        if verbose {
            println!("Updated {TAP_DLL}.");
        }
        let _ = fs::remove_dir_all(&staging);
    } else if verbose {
        println!("{TAP_DLL} is in use by Explorer; it lands on the next Explorer restart.");
    }
    Ok(())
}

/// Write release asset `asset` to `dest`. `download_url` is the GitHub API asset url: without the
/// `Accept` header it returns the asset's JSON metadata instead of its bytes.
fn download_asset(asset: &self_update::ReleaseAsset, dest: impl std::io::Write, verbose: bool) -> Result<()> {
    self_update::Download::from_url(asset.download_url())
        .request_header(self_update::http::header::ACCEPT, "application/octet-stream")
        .show_download_progress(verbose)
        .download_to(dest)?;
    Ok(())
}

/// The lowercase hex SHA-256 that `sums` (a [`SUMS_ASSET`]) lists for `asset`. Errors when the
/// entry is missing or is not a SHA-256: an asset that cannot be verified is not installed.
fn expected_sha256(sums: &str, asset: &str) -> Result<String> {
    match self_update::Checksum::from_sums_file(sums, asset) {
        Ok(self_update::Checksum::Sha256(hex)) => Ok(hex.trim().to_ascii_lowercase()),
        Ok(_) => bail!("{SUMS_ASSET} lists {asset} with a digest that is not SHA-256"),
        Err(e) => Err(anyhow::Error::new(e).context(format!("{SUMS_ASSET} has no usable entry for {asset}"))),
    }
}

/// Lowercase hex SHA-256 of the file at `path`.
fn sha256_file(path: &std::path::Path) -> Result<String> {
    use sha2::Digest;
    use std::io::Read;

    let mut file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut hasher = sha2::Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).with_context(|| format!("reading {}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Ok when the file at `path` hashes to `expected` (lowercase hex SHA-256).
fn verify_sha256(path: &std::path::Path, expected: &str) -> Result<()> {
    let actual = sha256_file(path)?;
    if actual != expected {
        bail!("SHA-256 mismatch for {}: {SUMS_ASSET} says {expected}, the download is {actual}", path.display());
    }
    Ok(())
}

/// How many set-aside names [`install_tap`] tries; each Explorer generation pins at most one.
const SET_ASIDE_SLOTS: u32 = 4;

/// `audio_tray_tap.dll.old<n>`: where [`install_tap`] moves a TAP that Explorer holds.
fn set_aside_path(target: &std::path::Path, slot: u32) -> std::path::PathBuf {
    let mut name = target.as_os_str().to_owned();
    name.push(format!(".old{slot}"));
    name.into()
}

/// Put `fresh` at `target`, without elevation. A DLL Explorer has loaded cannot be overwritten or
/// deleted but can be renamed, so a held `target` is moved to a [`set_aside_path`] first; Explorer
/// keeps running the old image until it restarts (a same-protocol handover adopts the old TAP, so
/// the new one loads with the next Explorer; a protocol change asks to restart it). Returns false, leaving `fresh` where it is for [`place_staged_tap`], when
/// neither works; `target` is then unchanged.
fn install_tap(fresh: &std::path::Path, target: &std::path::Path) -> bool {
    use std::fs;

    if fs::copy(fresh, target).is_ok() {
        return true;
    }
    for slot in 0..SET_ASIDE_SLOTS {
        let aside = set_aside_path(target, slot);
        // A leftover from an earlier generation; still held when this fails, so try the next slot.
        if aside.exists() && fs::remove_file(&aside).is_err() {
            continue;
        }
        if fs::rename(target, &aside).is_err() {
            break;
        }
        if fs::copy(fresh, target).is_ok() {
            return true;
        }
        // Never leave the install without a TAP.
        let _ = fs::remove_file(target);
        let _ = fs::rename(&aside, target);
        break;
    }
    eprintln!("audio-tray: {TAP_DLL} is held and could not be set aside; staged at {}", fresh.display());
    false
}

/// Delete the TAPs [`install_tap`] set aside. One Explorer still holds fails quietly and is retried
/// on a later call.
fn remove_set_aside_taps() {
    let Ok(target) = installed_tap() else {
        return;
    };
    for slot in 0..SET_ASIDE_SLOTS {
        let _ = std::fs::remove_file(set_aside_path(&target, slot));
    }
}

/// Place the staged TAP for the running version; called by [`crate::taskbar::restart_explorer`]
/// while no Explorer holds the DLL. Returns whether it landed.
pub fn place_staged_tap() -> bool {
    // A debug build must not drop a staged release TAP over the dev one.
    if cfg!(debug_assertions) {
        return false;
    }
    remove_set_aside_taps();
    let fresh = staging_dir(self_update::cargo_crate_version!()).join(TAP_DLL);
    if !fresh.is_file() {
        return false;
    }
    let Ok(target) = installed_tap() else {
        return false;
    };
    if install_tap(&fresh, &target) {
        println!("audio-tray: placed the pending {TAP_DLL}.");
        // Removing the staging makes this idempotent.
        let _ = std::fs::remove_dir_all(fresh.parent().unwrap_or(&fresh));
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ZIP: &str = "audio-tray-x86_64-pc-windows-msvc.zip";
    const HELLO_SHA256: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";

    #[test]
    fn expected_sha256_finds_the_asset_line() {
        let sums = format!(
            "0000000000000000000000000000000000000000000000000000000000000000  AudioTray-1.2.3-Setup.exe\r\n\
             {}  {ZIP}\r\n",
            HELLO_SHA256.to_ascii_uppercase()
        );
        assert_eq!(expected_sha256(&sums, ZIP).unwrap(), HELLO_SHA256);
    }

    #[test]
    fn expected_sha256_refuses_a_missing_entry() {
        let sums = format!("{HELLO_SHA256}  AudioTray-1.2.3-Setup.exe\n");
        assert!(expected_sha256(&sums, ZIP).is_err());
        assert!(expected_sha256("", ZIP).is_err());
    }

    #[test]
    fn expected_sha256_refuses_a_non_sha256_digest() {
        let sums = format!("{}  {ZIP}\n", "ab".repeat(64));
        assert!(expected_sha256(&sums, ZIP).is_err());
        let sums = format!("{}  {ZIP}\n", "ab".repeat(20));
        assert!(expected_sha256(&sums, ZIP).is_err());
    }

    #[test]
    fn verify_sha256_accepts_a_match_and_refuses_a_mismatch() {
        let dir = std::env::temp_dir().join(format!("audio-tray-sha-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("artifact");
        std::fs::write(&file, b"hello").unwrap();

        assert_eq!(sha256_file(&file).unwrap(), HELLO_SHA256);
        assert!(verify_sha256(&file, HELLO_SHA256).is_ok());
        std::fs::write(&file, b"hellO").unwrap();
        assert!(verify_sha256(&file, HELLO_SHA256).is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn set_aside_path_appends_the_slot() {
        let target = std::path::Path::new(r"C:\Apps\AudioTray\audio_tray_tap.dll");
        assert_eq!(set_aside_path(target, 2), std::path::Path::new(r"C:\Apps\AudioTray\audio_tray_tap.dll.old2"));
    }
}
