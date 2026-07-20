//! Embeds the Windows application icon (`assets/app.ico`) and version/metadata
//! into the executable, so audio-tray shows a proper icon and file properties in
//! Explorer, the taskbar, the Inno installer, and its shortcuts. Windows-only;
//! a no-op on other targets.

fn main() {
    println!("cargo:rerun-if-changed=assets/app.ico");
    println!("cargo:rerun-if-changed=build.rs");

    if std::env::var("CARGO_CFG_WINDOWS").is_ok() {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/app.ico");
        res.set("FileDescription", "Audio Tray");
        res.set("ProductName", "Audio Tray");
        res.set("OriginalFilename", "audio-tray.exe");
        res.set("LegalCopyright", "Copyright (c) 2026 Michel Salib");
        // Fail loudly: the whole point of this build script is the icon/metadata,
        // and both dev and CI (windows-latest) have the resource compiler.
        res.compile().expect("failed to embed Windows resources (need the MSVC/SDK resource compiler)");
    }
}
