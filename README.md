# Audio Tray

[![CI](https://github.com/michelsalib/audio-tray/actions/workflows/ci.yml/badge.svg)](https://github.com/michelsalib/audio-tray/actions/workflows/ci.yml)
[![Release](https://github.com/michelsalib/audio-tray/actions/workflows/release.yml/badge.svg)](https://github.com/michelsalib/audio-tray/releases/latest)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

A tiny Windows system-tray app for **viewing and switching your default audio output device** — without digging through Settings.

<p align="center">
  <img src="assets/app.ico" width="96" alt="Audio Tray icon">
</p>

## Features

- **Lives in the system tray** with an icon that reflects the current output device (speakers, headphones, headset, HDMI…), rendered from Segoe Fluent Icons and themed to your taskbar.
- **Left-click** → opens Windows' own audio-output flyout (the native "select output" picker).
- **Right-click** → a compact acrylic flyout to switch the default output device, choose a per-device icon, or quit.
- **Scroll** the mouse wheel over the tray icon to change the system volume.
- **Starts at sign-in** (optional, chosen at install time).
- **Auto-updates** itself from GitHub Releases.

## Install

**With winget** (once the package is published):

```powershell
winget install MichelSalib.AudioTray
```

**Or manually:** download the latest `AudioTray-<version>-Setup.exe` from the
[Releases page](https://github.com/michelsalib/audio-tray/releases/latest) and run it.
It installs per-user (no administrator rights required).

> The installer is currently unsigned, so Windows SmartScreen may show an
> "unknown publisher" prompt — choose **More info → Run anyway**.

**Requirements:** Windows 10 (1903+) or Windows 11, 64-bit.

## Usage

Once running, Audio Tray sits in the notification area:

| Action | Result |
|--------|--------|
| Left-click | Open the native Windows output-device flyout |
| Right-click | Open the Audio Tray flyout (switch device / pick icon / quit) |
| Scroll wheel | Adjust system volume |

### Command-line (dev/diagnostics)

```
audio-tray            run the tray (default)
audio-tray --list     print the current default + all active output devices
audio-tray --set <q>  switch default output to a device by name substring or id
audio-tray --update   check GitHub Releases and self-update now
```

Configuration (per-device icon overrides) is stored at
`%APPDATA%\AudioTray\config\config.toml`.

## Auto-update

Release builds check GitHub Releases on launch and, if a newer version exists,
download and replace the executable in place (per-user install → no admin
needed). The update is applied the **next** time the app starts, so the running
tray is never interrupted. Run `audio-tray --update` to check on demand.

## Build from source

Requires the Rust MSVC toolchain and the Windows SDK (for the resource compiler).

```powershell
rustup default stable-x86_64-pc-windows-msvc
cargo build --release
# → target\release\audio-tray.exe
```

## Releasing / maintaining

Publishing, the installer, winget submission, and the release workflow are
documented in [RELEASING.md](RELEASING.md).

## License

[MIT](LICENSE) © 2026 Michel Salib
