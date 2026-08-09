//! In-app self-updater (the "auto-update" leg of the release setup).
//!
//! On tray launch we spawn a background thread that asks GitHub for the latest
//! release of `michelsalib/audio-tray`. If it is newer than the running build we
//! download the `audio-tray-x86_64-pc-windows-msvc.zip` asset and replace the
//! on-disk `audio-tray.exe` in place (per-user install → no admin needed). The
//! new version takes effect the next time the tray starts — we deliberately do
//! NOT kill the running tray out from under the user, so an autostart install
//! picks up the update at next sign-in.
//!
//! Two files ship together, and `self_update` only ever replaces one of them, so
//! `update_tap` handles `audio_tray_tap.dll` separately. Explorer normally has it
//! open — the taskbar strip is injected on every start — so the replacement is
//! usually handed to the OS for the next boot rather than copied into place.
//!
//! That second file can therefore fall behind the first, silently — it did, for two releases —
//! so [`repair_stale_tap`] checks on the way in and re-fetches a DLL that does not match this
//! exe. It is a separate mechanism from `update_tap` on purpose: `update_tap` runs in the
//! process being *replaced*, and can only ever be as correct as the build the user is leaving.
//!
//! Gated to release builds: `cargo run` / debug builds never self-replace, so
//! development is never disrupted. Force a check any time with
//! `audio-tray --update` (works in debug too).
//!
//! The version compared is `CARGO_PKG_VERSION`, so Cargo.toml's `version` must
//! match the release tag (CI enforces this — see .github/workflows/release.yml).

use std::sync::Mutex;

use anyhow::{Context, Result};

const REPO_OWNER: &str = "michelsalib";
const REPO_NAME: &str = "audio-tray";
const BIN_NAME: &str = "audio-tray";
/// Must match the asset name suffix produced by the release workflow.
const TARGET: &str = "x86_64-pc-windows-msvc";
/// The taskbar TAP, which travels with the exe. Must match `TAP_DLL` in
/// [`crate::taskbar`] and the name the release workflow puts in the zip.
const TAP_DLL: &str = "audio_tray_tap.dll";

/// Set to the new version string once a background update has been downloaded and applied
/// to the on-disk exe. The flyout reads this to offer a "restart to update" entry; the new
/// binary only takes effect once the process restarts.
static PENDING: Mutex<Option<String>> = Mutex::new(None);

/// Where a downloaded TAP waits when it could not be copied into place.
///
/// Deterministic rather than remembered in a global, because the process that *downloads* an
/// update is never the one that gets to install the DLL: taking the update relaunches
/// audio-tray, and it is the new process that meets the old TAP, restarts Explorer and so frees
/// the file (see `taskbar::apply_at_startup`). Keyed by version — and the version that matters
/// to a reader is the one it is running, which is exactly the version that was staged.
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

/// Spawn the background update check. Non-blocking; every error is swallowed
/// (logged to the attached console, if any) so a flaky network or GitHub outage
/// never affects the tray. No-op in debug builds.
pub fn spawn_background_check() {
    if cfg!(debug_assertions) {
        return;
    }
    std::thread::spawn(|| match check_and_apply(false) {
        Ok(self_update::Status::Updated(v)) => set_pending_version(v),
        // **Only with the exe settled.** An update that just landed has already put the *new*
        // DLL next to it, and this process is still the old version — so a comparison here
        // would read "stale", and the repair would fetch the version we are about to stop
        // running. A downgrade, on every launch that takes an update.
        Ok(self_update::Status::UpToDate(_)) => repair_stale_tap(false),
        Err(e) => eprintln!("audio-tray: background update check failed: {e:#}"),
    });
}

/// `audio_tray_tap.dll`, beside the running exe.
fn installed_tap() -> Result<std::path::PathBuf> {
    let exe = std::env::current_exe().context("locating the running exe")?;
    let dir = exe.parent().context("exe has no parent directory")?;
    Ok(dir.join(TAP_DLL))
}

/// The version stamped into the installed TAP, or `None` if it carries no version resource.
///
/// **`None` is a real answer, not just an error**: every DLL built before the stamp existed is
/// unstamped, and those are exactly the ones a repair is for.
///
/// Read from `VS_FIXEDFILEINFO` — the fixed block behind the `\` sub-block — rather than the
/// `FileVersion` string, which lives in a per-language string table and would mean first asking
/// `\VarFileInfo\Translation` which language to look under. The numbers are the same numbers and
/// need no such round trip. See `crates/taskbar-tap/build.rs` for the other end.
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
    // The pointer is *into* `block`, which outlives the read below — nothing is freed here.
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

/// Fetch the TAP that belongs with this exe, if the one on disk is not it.
///
/// **This is the fix for the fix.** `update_tap` runs in the process being replaced, so a bug in it
/// is only repaired one release *after* the repair ships — and until v0.10.1 it silently downloaded
/// the asset's JSON metadata instead of the asset, leaving a v0.10.0 exe beside a v0.8.0 DLL. This
/// side has the opposite property: it runs in the *new* build, so it can clean up after an old one.
///
/// Reported at every step and fatal at none. A stale TAP degrades rather than breaks — the
/// init-data protocol ignores keys it does not know — so this must never be the reason the tray
/// fails to come up.
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

    // Same stance as the background check: a debug build says what it found and touches nothing,
    // or `--update` in a dev tree would overwrite the TAP you are working on with a release one.
    if cfg!(debug_assertions) {
        println!("(debug build — leaving {TAP_DLL} alone)");
        return;
    }

    // **Already fetched, and waiting for the shell to let go of the file.** Explorer holds the DLL
    // for its own lifetime, so the placement usually cannot happen at the moment it is downloaded —
    // and without this, every launch until that reboot would download the same 2 MB again. Retrying
    // the *copy* is the cheap half, and it is the half that eventually succeeds.
    if staging_dir(running).join(TAP_DLL).is_file() {
        if !place_staged_tap() {
            println!("audio-tray: the v{running} {TAP_DLL} is staged — it lands on the next Explorer restart or boot.");
        }
        return;
    }

    match update_tap(running, verbose) {
        Ok(()) => match installed_tap_version() {
            Some(version) if version == running => println!("audio-tray: {TAP_DLL} is now v{version}."),
            // Explorer had the file, so the copy was turned into a boot rename. The staged DLL is
            // what the guard above will find next launch, which is why that path costs no download.
            _ if staging_dir(running).join(TAP_DLL).is_file() => println!(
                "audio-tray: the v{running} {TAP_DLL} is staged — it lands on the next Explorer restart or boot."
            ),
            // Copied, and *still* not the version we asked for: the release asset itself carries no
            // stamp. Nothing on this side can fix that, and it would put this check back where it
            // started on every launch — so it is the one outcome here worth shouting about, which
            // is the whole lesson of the bug this feature exists for.
            _ => eprintln!(
                "audio-tray: fetched the v{running} {TAP_DLL}, but it reports no version — that release's asset is unstamped"
            ),
        },
        Err(e) => eprintln!("audio-tray: could not repair {TAP_DLL} ({e:#})"),
    }
}

/// Print what the two halves are, for `--tap-version`.
///
/// The diagnostic this feature was missing: the skew it repairs was invisible for two releases, and
/// answering "which TAP is actually on disk?" meant comparing file dates by eye.
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
        self_update::Status::UpToDate(v) => {
            println!("Already up to date (v{v}).");
            // The exe is settled, so the DLL beside it can be held to the same version — see
            // [`repair_stale_tap`]. This is also the only way to drive that path by hand.
            repair_stale_tap(true);
        }
        self_update::Status::Updated(v) => {
            println!("Updated to v{v}. Restart audio-tray to run the new version.");
        }
    }
    Ok(())
}

fn check_and_apply(verbose: bool) -> Result<self_update::Status> {
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

    // `self_update` replaces exactly one file — the one named by `bin_name`. The
    // taskbar TAP is a second file that has to travel with the exe, so it gets its
    // own pass. Deliberately additive and never fatal: the exe has already been
    // replaced successfully by this point, and a stale DLL degrades rather than
    // breaks (the init-data protocol ignores unknown keys and defaults missing
    // ones), so failing here must not turn a good update into a bad one.
    if let self_update::Status::Updated(version) = &status {
        if let Err(e) = update_tap(version, verbose) {
            eprintln!("audio-tray: exe updated but the taskbar TAP did not ({e:#})");
        }
    }
    Ok(status)
}

/// Ships the new `audio_tray_tap.dll` alongside the freshly updated exe.
///
/// Explorer keeps the DLL loaded from the moment the strip is injected — and it
/// stays loaded even after a revert (see [`crate::taskbar`]) — so overwriting it
/// usually fails. That case is not an error: the replacement is handed to the OS
/// with `MOVEFILE_DELAY_UNTIL_REBOOT` and lands on the next boot, which is the same
/// mechanism the installer's `restartreplace` uses.
fn update_tap(version: &str, verbose: bool) -> Result<()> {
    use std::fs;

    let target = installed_tap()?;

    // Same asset the exe came from, fetched again — it is two small files, and this
    // only runs on the rare occasion an update was actually applied.
    let release = self_update::backends::github::ReleaseList::configure()
        .repo_owner(REPO_OWNER)
        .repo_name(REPO_NAME)
        .build()
        .context("configuring the release lookup")?
        .fetch()
        .context("listing releases")?
        .into_iter()
        .find(|release| release.version == version)
        .with_context(|| format!("release v{version} not found"))?;
    let asset = release
        .asset_for(TARGET, None)
        .with_context(|| format!("v{version} has no {TARGET} asset"))?;

    // A plain directory rather than a `TempDir`, because when the copy below is
    // blocked the file has to outlive this process — until the next boot, or until
    // some later run of audio-tray frees the DLL and calls [`place_staged_tap`]. A
    // self-deleting temp dir would take the pending replacement with it.
    let staging = staging_dir(version);
    fs::create_dir_all(&staging).context("creating a staging directory")?;

    let archive = staging.join(&asset.name);
    let mut file = fs::File::create(&archive).context("creating the download file")?;
    // **`download_url` is the GitHub *API* asset url, not the browser one** — `self_update`'s github
    // backend reads it from the asset's `url` key. Ask that endpoint for a file and it has to be told
    // so; without the header it answers with the asset's own JSON *metadata*, and 1.6 KB of
    // `{"url":…,"id":…}` lands on disk named `.zip`. `self_update` sets exactly this header on the
    // download it does itself, which is the whole reason the exe updated and the DLL silently did not.
    self_update::Download::from_url(&asset.download_url)
        .set_header(
            http::header::ACCEPT,
            http::HeaderValue::from_static("application/octet-stream"),
        )
        .show_progress(verbose)
        .download_to(&mut file)
        .context("downloading the release asset")?;
    drop(file);

    self_update::Extract::from_source(&archive)
        .archive(self_update::ArchiveKind::Zip)
        .extract_file(&staging, TAP_DLL)
        .with_context(|| format!("{TAP_DLL} is not in {}", asset.name))?;
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
        // Almost certainly ERROR_SHARING_VIOLATION: Explorer holds the DLL, which
        // is the normal state. Queue it for the next boot instead.
        Err(_) => schedule_replace_at_boot(&fresh, &target, verbose),
    }
}

/// Asks the OS to replace `target` with `fresh` during the next boot, before
/// anything can open either file.
///
/// `fresh` must outlive this process, which is why the staging directory is not a
/// self-deleting temporary — the pending rename is what finally consumes it.
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

/// Place a TAP replacement that is waiting for the next boot, now that its DLL has been freed.
/// Restarting Explorer is what frees it — see [`crate::taskbar::restart_explorer`], which calls
/// this in the gap between the old shell exiting and the new one starting.
///
/// Looks for a staging of *the running version*: an update is downloaded by the old build and
/// installed by the new one, so by the time anybody can place this DLL, `CARGO_PKG_VERSION` is
/// the version it belongs to.
///
/// Returns whether the new DLL is now in place. Best-effort in both directions — usually there
/// is nothing staged at all (no update, or one whose copy succeeded outright), and a copy that
/// fails changes nothing, since the boot-time rename scheduled alongside it still stands.
pub fn place_staged_tap() -> bool {
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
            // Now redundant, and its absence is what makes this idempotent: the pending boot
            // rename simply fails with nothing to move, and a later restart finds nothing to
            // place rather than recopying the same bytes on every one.
            let _ = std::fs::remove_dir_all(fresh.parent().unwrap_or(&fresh));
            true
        }
        Err(e) => {
            eprintln!("audio-tray: {TAP_DLL} is still held ({e}); it waits for the next boot");
            false
        }
    }
}
