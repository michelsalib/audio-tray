//! Stamps **the app's** version into `audio_tray_tap.dll` as a version resource, which
//! `update::repair_stale_tap` reads to detect a DLL out of step with the exe.
//!
//! **The version comes from the workspace root manifest**: this crate is pinned at `0.0.0`, so its
//! own `CARGO_PKG_VERSION` means nothing.

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    let root = workspace_manifest();
    println!("cargo:rerun-if-changed={}", root.display());

    let version = app_version(&root);
    // Compiled in too: `hand_over` (lib.rs) accepts a new owner only from the same exe version.
    println!("cargo:rustc-env=AUDIO_TRAY_VERSION={version}");

    if std::env::var("CARGO_CFG_WINDOWS").is_err() {
        return;
    }

    let (major, minor, patch) = triple(&version);

    let mut res = winresource::WindowsResource::new();
    res.set("FileDescription", "Audio Tray taskbar TAP");
    res.set("ProductName", "Audio Tray");
    res.set("OriginalFilename", "audio_tray_tap.dll");
    res.set("LegalCopyright", "Copyright (c) 2026 Michel Salib");
    // Both forms: the string for humans, the packed number for `update::installed_tap_version`
    // (read from `VS_FIXEDFILEINFO`).
    res.set("FileVersion", &version);
    res.set("ProductVersion", &version);
    let packed = u64::from(major) << 48 | u64::from(minor) << 32 | u64::from(patch) << 16;
    res.set_version_info(winresource::VersionInfo::FILEVERSION, packed);
    res.set_version_info(winresource::VersionInfo::PRODUCTVERSION, packed);

    // Fail loudly: an unstamped DLL reads as stale forever (a download per launch).
    res.compile()
        .expect("failed to embed Windows resources (need the MSVC/SDK resource compiler)");
}

/// The root `Cargo.toml`, located from `CARGO_MANIFEST_DIR` (not the cwd).
fn workspace_manifest() -> PathBuf {
    let here = PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("cargo always sets CARGO_MANIFEST_DIR"),
    );
    here.join("..").join("..").join("Cargo.toml")
}

/// `version` from the root manifest's `[package]` section (scanned within the section only;
/// `[dependencies]` has `version =` lines too). Panics on failure: a wrong version is worse than
/// no build.
fn app_version(manifest: &std::path::Path) -> String {
    let text = std::fs::read_to_string(manifest)
        .unwrap_or_else(|e| panic!("reading {}: {e}", manifest.display()));

    let mut in_package = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
            continue;
        }
        if !in_package {
            continue;
        }
        if let Some(value) = line.strip_prefix("version") {
            if let Some(value) = value.trim_start().strip_prefix('=') {
                return value.trim().trim_matches('"').to_string();
            }
        }
    }
    panic!("no [package] version in {}", manifest.display());
}

/// `"0.10.1"` -> `(0, 10, 1)`; anything else panics.
fn triple(version: &str) -> (u16, u16, u16) {
    let mut parts = version.split('.').map(|part| {
        part.parse::<u16>()
            .unwrap_or_else(|e| panic!("version {version:?} is not numeric: {e}"))
    });
    let mut next = || parts.next().unwrap_or_else(|| panic!("version {version:?} is not x.y.z"));
    (next(), next(), next())
}
