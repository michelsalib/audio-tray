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
        Ok(self_update::Status::UpToDate(_)) => {}
        Err(e) => eprintln!("audio-tray: background update check failed: {e:#}"),
    });
}

/// Run an update check synchronously, printing progress. Backs the `--update`
/// command. Returns Ok whether or not an update was applied.
pub fn run_manual() -> Result<()> {
    println!("audio-tray v{}", self_update::cargo_crate_version!());
    println!("Checking github.com/{REPO_OWNER}/{REPO_NAME} for a newer release...");
    match check_and_apply(true)? {
        self_update::Status::UpToDate(v) => println!("Already up to date (v{v})."),
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

    let exe = std::env::current_exe().context("locating the running exe")?;
    let dir = exe.parent().context("exe has no parent directory")?;
    let target = dir.join(TAP_DLL);

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
    // blocked the file has to survive this process until the next boot — a
    // self-deleting temp dir would take the pending replacement with it.
    let staging = std::env::temp_dir().join(format!("audio-tray-tap-{version}"));
    fs::create_dir_all(&staging).context("creating a staging directory")?;

    let archive = staging.join(&asset.name);
    let mut file = fs::File::create(&archive).context("creating the download file")?;
    // No `Accept` header needed: `download_url` is the direct browser download.
    self_update::Download::from_url(&asset.download_url)
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
