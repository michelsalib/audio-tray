# Audio Tray

[![CI](https://github.com/michelsalib/audio-tray/actions/workflows/ci.yml/badge.svg)](https://github.com/michelsalib/audio-tray/actions/workflows/ci.yml)
[![Release](https://github.com/michelsalib/audio-tray/actions/workflows/release.yml/badge.svg)](https://github.com/michelsalib/audio-tray/releases/latest)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

A tiny Windows system-tray app for **controlling your audio without digging through Settings** — switch the default output/input device, set volume, and mute, all from one native-feeling flyout. It also turns the YouTube Music PWA's taskbar icon into a now-playing strip.

<p align="center">
  <img src="assets/app.ico" width="96" alt="Audio Tray icon">
</p>

## Features

- **Taskbar controls** — a pair of buttons drawn into the notification area, one for
  output and one for input, each showing that device's own icon. Windows' own volume and
  "microphone in use" icons are hidden, since the buttons say the same thing. See
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
- **A red dot on the mic** whenever an app is recording — on the input button, on the
  flyout's microphone glyph, and on the level bar. It follows the same record Windows'
  own "microphone in use" indicator does (which it replaces, see below), so it covers
  every app and every microphone, and it stays lit while an app holds the stream open
  even if you are muted.
- **YouTube Music in its own taskbar button** — the PWA's icon becomes a 162-epx strip
  showing the track, the artist and the song's position, with previous/play-pause/next a
  hover away on the preview's toolbar. It is the app's *own* button, so launching adds no
  second icon, minimising still goes there, and it still drags to reorder. See
  [the YouTube Music tile](#the-youtube-music-tile).
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
audio-tray --tap-version
                      which taskbar TAP is on disk — the exe and the DLL ship
                      together, and only the exe is self-updated
audio-tray --taskbar-revert
                      put the taskbar back, without stopping the running tray
audio-tray --music-probe
                      list every media session with its app id, and which one matched
audio-tray --music-timeline
                      sample the matched session's position over a few seconds
audio-tray --music-progress <percent|off>
                      set the progress bar on the player's window by hand
audio-tray --music-windows
                      survey the player's windows (pid, visibility, cloaking, rect)
audio-tray --music-thumbbar [playing|paused]
                      put the transport buttons on the player's hover preview by hand
```

Configuration is stored at `%APPDATA%\AudioTray\config\config.toml`: per-device icon
overrides, and the music tile —

```toml
[music]
enabled = true            # false turns the whole music half off, progress bar included
tile = "YouTube Music"    # the taskbar button to draw into; "" = feed only, no strip
# app_id = "..."          # pin the SMTC app id, if --music-probe shows a miss
```

## Taskbar controls

Audio Tray's notification-area presence is two buttons — output and input — drawn
directly into the taskbar. Left-click either one to cycle that endpoint to the next
device; right-click opens the flyout. Two icons of Windows' own are hidden because the
buttons say the same thing: the volume icon, which the output button duplicates, and the
"microphone in use" icon, which the input button's red recording dot replaces. Both come
back when the buttons do not — nothing of Windows' is taken away until the strip is
actually on screen, and everything is put back on the way out.

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

## The YouTube Music tile

The same injection that draws the audio buttons also replaces the YouTube Music PWA's
taskbar icon with a now-playing strip: the track title (scrolling when it does not fit)
and the artist under it, in 162 epx of taskbar. The transport controls are one hover
away, on the preview's own toolbar.

| Where | What |
|-------|------|
| The strip's body | The shell's own click — activates the player, or minimises it; drag still reorders the icon |
| Hovering it | Windows' normal window preview, with previous / play-pause / next added underneath |
| Across the strip | The song's position, on the progress line Windows draws for any app — accent while playing, yellow when paused |
| The running pill | Left where it means something: under the icon, not centred in a widened button |

It follows the media session, not YouTube Music's process, so it needs no extension, no
API key and no login. Things worth knowing:

- **It is invisible until it applies.** No YouTube Music session on the machine means no
  state published, no progress bar and no strip — a user who never opens the player sees
  no difference at all. That is why it is on by default.
- **The icon must be on the taskbar**, pinned or running, and not in the overflow.
- **The transport buttons are the shell's own.** They are added to the preview with
  `ITaskbarList3::ThumbBarAddButtons` — the same thumbnail toolbar iTunes and MPC-HC use —
  so the shell draws, themes and scales them. Windows sends the click to the *player's*
  window rather than to us, so the TAP takes it from the button element instead; the two
  halves meet at the button's tooltip text and the 10/11/12 wire codes.
- **The strip carries no tooltip, deliberately.** Declaring one makes XAML's tooltip
  service own hover for the tile, and Windows' window preview then never opens — which
  would take the transport buttons with it.
- **A play click with nothing playing raises the player** instead of synthesising a media
  key. A Chromium media session does not exist until media has played, and a media key
  goes to whichever app Windows thinks owns them — which on this machine paused MPC-HC.
- **The position is interpolated, not polled.** The player publishes a checkpoint with a
  timestamp rather than a running clock, so the bar advances locally between checkpoints;
  see FINDINGS.
- **The button is handed back on the way out.** Quitting, `--taskbar-revert`, or Explorer
  restarting all restore its width, icon and indicators, and a clean quit also clears the
  progress bar off the player's window. One thing does not survive that promise, by the
  shell's design rather than by choice: a thumbnail toolbar cannot be removed from a window
  at all once added — on shutdown its buttons are greyed out instead, which is the honest
  version of taking them away.

Turn it off, or point it at a different app, with the `[music]` section of the config
above.

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
