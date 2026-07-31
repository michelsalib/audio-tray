# Audio Tray

[![CI](https://github.com/michelsalib/audio-tray/actions/workflows/ci.yml/badge.svg)](https://github.com/michelsalib/audio-tray/actions/workflows/ci.yml)
[![Release](https://github.com/michelsalib/audio-tray/actions/workflows/release.yml/badge.svg)](https://github.com/michelsalib/audio-tray/releases/latest)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

A tiny Windows system-tray app for **controlling your audio without digging through Settings** — switch the default output/input device, set volume, and mute, all from one native-feeling flyout.

<p align="center">
  <img src="assets/app.ico" width="96" alt="Audio Tray icon">
</p>

## Features

- **Taskbar controls** — a pair of buttons drawn into the notification area, one for
  output and one for input, each showing that device's own icon. Windows' own volume
  icon is hidden, since the output button duplicates it. See
  [taskbar controls](#taskbar-controls).
- **Left-click** either button to switch that endpoint to the next device;
  **right-click** for a compact acrylic flyout that replaces the native sound flyout:
  set output **and** input volume, mute/unmute, switch the default output and input
  device, pick a per-device icon, and see the battery level of Bluetooth devices that
  report one.
- **Scroll** over a button — mouse wheel or touchpad — to change *that* endpoint's
  volume: the output button for playback, the input button for the microphone. A level
  bar appears next to the buttons while you scroll and fades away three seconds after
  the last change.
- **Falls back to a plain tray icon** if the taskbar controls cannot be drawn — an
  icon reflecting the current output device (speakers, headphones, headset, HDMI…),
  rendered from Segoe Fluent Icons and themed to your taskbar. Either button opens
  the flyout.
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

Once running, Audio Tray sits in the notification area as two buttons — output and
input:

| Action | Result |
|--------|--------|
| Left-click a button | Switch that endpoint to the next device |
| Right-click a button | Open the Audio Tray control flyout (volume, mute, switch output/input, pick icon, Sound settings, Quit) |
| Scroll a button (wheel or touchpad) | Adjust that endpoint's volume, 2% per notch, with a level bar beside the buttons |
| Scroll elsewhere on the taskbar | Adjust the output volume |

On the fallback single tray icon, either button opens the flyout.

### Command-line (dev/diagnostics)

```
audio-tray            run the tray (default)
audio-tray --list     print the current default + all active output devices
audio-tray --set <q>  switch default output to a device by name substring or id
audio-tray --update   check GitHub Releases and self-update now
audio-tray --taskbar-revert
                      put the taskbar back, without stopping the running tray
```

Configuration (per-device icon overrides) is stored at
`%APPDATA%\AudioTray\config\config.toml`.

## Taskbar controls

Audio Tray's notification-area presence is two buttons — output and input — drawn
directly into the taskbar. Left-click either one to cycle that endpoint to the next
device; right-click opens the flyout. Windows' own volume icon is hidden, because the
output button duplicates it.

It works by loading `audio_tray_tap.dll` into `explorer.exe` through the XAML
Diagnostics interface — the same mechanism TranslucentTB and Windhawk use. Some
things worth knowing:

- **The plain tray icon is never at risk.** It is registered unconditionally, the
  buttons are drawn on top of it, and every failure here leaves it as the whole of
  the UI — the app then behaves exactly as it did before the buttons existed.
- **Touchpad scrolling comes through the buttons.** Two-finger scroll never reaches a
  global mouse hook — Windows delivers it straight to the window under the pointer — so
  it is the injected buttons that pick it up. Without them, scroll-to-volume is
  wheel-only.
- **Everything is undone on the way out.** Quitting, being killed, or running
  `audio-tray --taskbar-revert` all restore the taskbar exactly as it was; so does
  Explorer restarting, which simply discards the DLL. Verified by pixel comparison
  against a pre-injection capture.
- **One consumer at a time.** If TranslucentTB, Windhawk or a similar taskbar tool
  is running, the injection may fail — they share the one diagnostics endpoint. You
  keep the plain tray icon.
- **It repairs itself by restarting Explorer.** Two situations need a fresh shell, and
  audio-tray handles both on startup without asking: an injection that fails (after a
  couple of retries, which is usually all a shell that is merely slow to be ready
  needs), and a TAP from an earlier audio-tray still loaded in Explorer — injecting
  alongside that one is what leaves you with a taskbar that looks untouched. At most
  one restart per run, so a machine where the injection can never succeed is not
  restarted round and round. `audio-tray --taskbar-restart` does the same by hand.
- **Your icon must be on the taskbar, not in the overflow.** Nothing is drawn if it
  is hidden behind the chevron.

The engineering notes, including the failure modes found along the way, are in
[crates/taskbar-tap/FINDINGS.md](crates/taskbar-tap/FINDINGS.md).

## Auto-update

Release builds check GitHub Releases on launch and, if a newer version exists,
download and replace the executable in place (per-user install → no admin
needed). The update is applied the **next** time the app starts, so the running
tray is never interrupted. Run `audio-tray --update` to check on demand.

Once one is downloaded, the flyout's footer offers a *Restart to update* button
(circular arrow) that relaunches into the new version straight away.

The update also carries a new taskbar component, which Explorer holds open and so
cannot simply be overwritten. Taking the update handles that too: the relaunched app
finds the old component still loaded, restarts Explorer, and installs the new one in
the moment nothing is holding it. Ignore the button and it all happens at your next
reboot instead — nothing is left half-applied either way.

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
