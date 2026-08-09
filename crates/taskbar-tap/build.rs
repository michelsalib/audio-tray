//! Stamps **the app's** version into `audio_tray_tap.dll`.
//!
//! The exe and this DLL ship together and must stay in lockstep, but only one of them is replaced
//! by `self_update`, so they can silently drift apart — which they did, for two releases (see
//! `update::repair_stale_tap`). The repair needs to answer "which version is the DLL on disk?", and
//! a version resource is the only answer that cannot itself go stale: it travels *inside* the file,
//! so the installer and the self-updater get it right by construction and there is no marker file
//! to land out of step with what it describes.
//!
//! **The version comes from the workspace root, not from this crate.** This package is pinned at
//! `0.0.0` — `cargo release` skips it (`publish = false`), so its own `CARGO_PKG_VERSION` is not a
//! version at all. The number that means something is the one in the root manifest, which is what
//! the tag, the exe and the release assets all carry.

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    let root = workspace_manifest();
    println!("cargo:rerun-if-changed={}", root.display());

    if std::env::var("CARGO_CFG_WINDOWS").is_err() {
        return;
    }

    let version = app_version(&root);
    let (major, minor, patch) = triple(&version);

    let mut res = winresource::WindowsResource::new();
    res.set("FileDescription", "Audio Tray taskbar TAP");
    res.set("ProductName", "Audio Tray");
    res.set("OriginalFilename", "audio_tray_tap.dll");
    res.set("LegalCopyright", "Copyright (c) 2026 Michel Salib");
    // Both halves, because both are read: the string is what a human sees in Explorer's
    // properties, and the packed number is what `update::installed_tap_version` queries out of
    // `VS_FIXEDFILEINFO` — no string table, no code-page dance.
    res.set("FileVersion", &version);
    res.set("ProductVersion", &version);
    let packed = u64::from(major) << 48 | u64::from(minor) << 32 | u64::from(patch) << 16;
    res.set_version_info(winresource::VersionInfo::FILEVERSION, packed);
    res.set_version_info(winresource::VersionInfo::PRODUCTVERSION, packed);

    // Fail loudly, like the root build script: an unstamped DLL reads as "stale" forever, so a
    // resource compiler that quietly did nothing would cost every user a download per launch.
    res.compile()
        .expect("failed to embed Windows resources (need the MSVC/SDK resource compiler)");
}

/// The root `Cargo.toml`, from this crate's own manifest directory rather than a path relative to
/// the working directory — a build script's cwd is its manifest dir today, and that is not a
/// promise worth relying on.
fn workspace_manifest() -> PathBuf {
    let here = PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("cargo always sets CARGO_MANIFEST_DIR"),
    );
    here.join("..").join("..").join("Cargo.toml")
}

/// `version` from the root manifest's `[package]` section.
///
/// Scanned rather than parsed with `toml`, to keep a build script free of dependencies — but
/// scanned *within the section*, because `[dependencies]` is full of `version =` lines and the
/// first one in the file is only the right answer by luck of the current ordering.
///
/// Every failure here panics. A wrong version is worse than no build: it would stamp a DLL that
/// disagrees with the exe it ships beside, which is the exact condition this whole mechanism
/// exists to detect.
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

/// `"0.10.1"` -> `(0, 10, 1)`. Anything that is not three numbers is a mis-parse, not a variant to
/// tolerate — see [`app_version`].
fn triple(version: &str) -> (u16, u16, u16) {
    let mut parts = version.split('.').map(|part| {
        part.parse::<u16>()
            .unwrap_or_else(|e| panic!("version {version:?} is not numeric: {e}"))
    });
    let mut next = || parts.next().unwrap_or_else(|| panic!("version {version:?} is not x.y.z"));
    (next(), next(), next())
}
