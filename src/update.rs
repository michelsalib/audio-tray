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
    self_update::backends::github::Update::configure()
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
        .context("downloading/applying update")
}
