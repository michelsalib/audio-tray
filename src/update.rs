//! In-app self-updater (the "auto-update" leg of the release setup).
//!
//! A background thread replaces the on-disk exe from the latest GitHub release; it takes effect on
//! the next start (the running tray is never killed). `self_update` replaces only the exe, so
//! `update_tap` fetches `audio_tray_tap.dll` too, staging it for the next Explorer restart or boot
//! when Explorer holds it. [`repair_stale_tap`] runs in the *new* build and re-fetches a DLL that
//! does not match this exe. Release builds only (`--update` checks in debug but never replaces the
//! TAP). Compares `CARGO_PKG_VERSION`, which CI holds equal to the release tag.

use std::sync::Mutex;

use anyhow::{Context, Result};

const REPO_OWNER: &str = "michelsalib";
const REPO_NAME: &str = "audio-tray";
const BIN_NAME: &str = "audio-tray";
/// Must match the asset name suffix produced by the release workflow.
const TARGET: &str = "x86_64-pc-windows-msvc";
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
            println!("audio-tray: the v{running} {TAP_DLL} is staged — it lands on the next Explorer restart or boot.");
        }
        return;
    }

    match update_tap(running, verbose) {
        Ok(()) => match installed_tap_version() {
            Some(version) if version == running => println!("audio-tray: {TAP_DLL} is now v{version}."),
            // Explorer held the file: staged for a boot rename; the guard above finds it next launch.
            _ if staging_dir(running).join(TAP_DLL).is_file() => println!(
                "audio-tray: the v{running} {TAP_DLL} is staged — it lands on the next Explorer restart or boot."
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

/// Fetch `audio_tray_tap.dll` for `version` and copy it beside the exe. Explorer usually holds it
/// (never unloaded, even after a revert), so that is not an error: it is left staged with a
/// `MOVEFILE_DELAY_UNTIL_REBOOT` rename.
fn update_tap(version: &str, verbose: bool) -> Result<()> {
    use std::fs;

    let target = installed_tap()?;

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

    // Not a `TempDir`: a blocked copy must outlive this process (boot rename / `place_staged_tap`).
    let staging = staging_dir(version);
    fs::create_dir_all(&staging).context("creating a staging directory")?;

    let archive = staging.join(asset.name());
    let mut file = fs::File::create(&archive).context("creating the download file")?;
    // `download_url` is the GitHub API asset url: without this header it returns the JSON metadata.
    self_update::Download::from_url(asset.download_url())
        .request_header(self_update::http::header::ACCEPT, "application/octet-stream")
        .show_download_progress(verbose)
        .download_to(&mut file)
        .context("downloading the release asset")?;
    drop(file);

    self_update::Extract::from_source(&archive)
        .archive(self_update::ArchiveKind::Zip)
        .extract_file(&staging, TAP_DLL)
        .with_context(|| format!("{TAP_DLL} is not in {}", asset.name()))?;
    let fresh = staging.join(TAP_DLL);
    let _ = fs::remove_file(&archive);

    match fs::copy(&fresh, &target) {
        Ok(_) => {
            if verbose {
                println!("Updated {TAP_DLL}.");
            }
            let _ = fs::remove_dir_all(&staging);
            Ok(())
        }
        // Normally a sharing violation (Explorer holds the DLL): queue for the next boot.
        Err(_) => schedule_replace_at_boot(&fresh, &target, verbose),
    }
}

/// Asks the OS to replace `target` with `fresh` during the next boot; `fresh` must stay on disk.
fn schedule_replace_at_boot(
    fresh: &std::path::Path,
    target: &std::path::Path,
    verbose: bool,
) -> Result<()> {
    use windows::core::HSTRING;
    use windows::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_DELAY_UNTIL_REBOOT, MOVEFILE_REPLACE_EXISTING,
    };

    let from = HSTRING::from(fresh.as_os_str());
    let to = HSTRING::from(target.as_os_str());
    unsafe {
        MoveFileExW(
            &from,
            &to,
            MOVEFILE_DELAY_UNTIL_REBOOT | MOVEFILE_REPLACE_EXISTING,
        )
    }
    .with_context(|| format!("scheduling {TAP_DLL} for replacement at next boot"))?;

    if verbose {
        println!("{TAP_DLL} is in use by Explorer; it will be replaced on the next restart.");
    }
    Ok(())
}

/// Copy the staged TAP for the running version into place; called by
/// [`crate::taskbar::restart_explorer`] while no Explorer holds the DLL. Returns whether it landed;
/// on failure the boot-time rename still stands.
pub fn place_staged_tap() -> bool {
    // A debug build must not drop a staged release TAP over the dev one.
    if cfg!(debug_assertions) {
        return false;
    }
    let fresh = staging_dir(self_update::cargo_crate_version!()).join(TAP_DLL);
    if !fresh.is_file() {
        return false;
    }
    let Ok(target) = installed_tap() else {
        return false;
    };
    match std::fs::copy(&fresh, &target) {
        Ok(_) => {
            println!("audio-tray: placed the pending {TAP_DLL} — no reboot needed.");
            // Removing the staging makes this idempotent; the boot rename then finds nothing.
            let _ = std::fs::remove_dir_all(fresh.parent().unwrap_or(&fresh));
            true
        }
        Err(e) => {
            eprintln!("audio-tray: {TAP_DLL} is still held ({e}); it waits for the next boot");
            false
        }
    }
}
