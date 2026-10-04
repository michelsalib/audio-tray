# audio-tray: app-side engineering notes

The narrative behind decisions in `src/` — measurements, dead ends, and why the code looks the way
it does. The TAP's equivalent is [`crates/taskbar-tap/FINDINGS.md`](../crates/taskbar-tap/FINDINGS.md).
Code comments state contracts; this file keeps the history.

## Tray icon

### The registration tooltip is frozen (`tray::INITIAL_TOOLTIP`)

Windows keys a notification icon's identity in `HKCU\Control Panel\NotifyIconSettings` on the
executable path plus the tooltip it was *registered* with, and stores "always show on the taskbar"
against that identity. Briefly registering with the `Audio Tray` marker instead created a second,
unpromoted entry and dropped every user's icon into the overflow flyout — which also disables the
taskbar strip, since the TAP only decorates an icon that is on the taskbar. The live tooltip (which
carries the marker) is set a moment later by `refresh`; only the registration-time value is frozen.

## Message handling (`src/tray`)

### One message window instead of thread messages

Producers (endpoint notifications, the microphone watcher, the wheel hook) used to
`PostThreadMessageW` to the tray thread, and the TAP's messages were picked out of the tray's
`GetMessage` loop by `msg.message ==` checks. While the flyout's modal pump ran, thread messages
were retrieved by *its* `GetMessage`, had no window to dispatch to, and were dropped: device
changes, microphone changes, taskbar scrolls and Explorer restarts were all lost while the panel
was open, and the tray had to re-sync on close. Everything now posts to the receiver window, whose
procedure queues and drains, so any pump that dispatches delivers them.

## Moved out of the code

### src/music/session.rs — shape of a PWA's SMTC app id

Measured on Windows 11 26200 with a Chromium app-mode window standing in for an installed PWA, the
SMTC User Model ID reads `MSEdge.localhost_/.edgeprofile.Default`: browser, then origin, then profile.
An installed PWA therefore carries its origin (`music.youtube.com`) verbatim, which is what makes a
certain match possible; a plain tab reports only the bare browser name. The exact shape varies across
Chromium versions and channels, which is why the match is a case-insensitive substring.

### src/music/progress.rs — an Explorer restart silently drops the progress bar

A taskbar progress bar is state the shell holds against a window, so a new Explorer starts with none.
If the app keeps its "last posted value" cache across the restart, it suppresses every later post as
unchanged and the bar never comes back, with nothing logged. `Progress::taskbar_restarted` clears the
cache for this reason (the thumbnail toolbar had the same defect).

### src/music/player.rs — launching and raising the player

- Activating a PWA by AUMID is **not idempotent**: each `ActivateApplication` starts a fresh YouTube
  Music window instead of activating the running one (four activations left four cascaded windows).
  So `activate_player` raises an existing window first and launches only when there is none.
- `IApplicationActivationManager::ActivateApplication` returns an HRESULT and the pid it started
  (measured: `Started(14812)`, window 3 s later), whereas `ShellExecuteW` on `shell:AppsFolder\<aumid>`
  only says the shell accepted the request. A long hunt for a launch that "silently did nothing" was a
  working launch with a broken measurement; only the pid could tell the two apart.
- Opening `https://music.youtube.com` as a fallback goes to the default browser and opens a tab in
  Edge's last-used profile rather than the installed PWA. That is why the player's packaged AUMID is
  remembered on disk (`%LOCALAPPDATA%\audio-tray\player-aumid.txt`): the SMTC session, and so the id,
  exists only while something plays.
- `Command::new("explorer.exe")` with a URL reports success when the process starts even if Explorer
  ignores the argument; `ShellExecuteW` returns <= 32 on failure, so it is used for the URL fallback.
- A visible-but-cloaked window (another virtual desktop, or Edge holding a PWA window it is not
  showing) passes the "first visible title match" test and `raise` reports success while nothing
  appears; `--music-windows` prints the cloak state to tell that apart from "no window".
- A window with no AppUserModelID answers `GetValue(PKEY_AppUserModel_ID)` with an empty VT_EMPTY
  variant, not an error, so emptiness is the "no identity" signal.
- Every foreground call returns success while doing nothing; the strip once logged `Activate -> Raised`
  six times against a player that never came forward. `GetForegroundWindow` afterwards is the only
  honest test (see also FINDINGS.md, 'Three behaviours that only using it could find').

### src/music/smtc.rs — what SMTC publishes from Chromium

Measured on Windows 11 26200:
- Cover art is only published for artwork the browser actually fetched. A `data:` URL in
  `MediaMetadata.artwork` gives a session with no thumbnail at all; an `http(s)` URL gives one. This is
  decided in Chromium, so a missing cover is a normal state. YouTube Music serves real URLs.
- The thumbnail is re-encoded and downscaled by Chromium: a 256x256 PNG came back as 544 bytes. It is
  art to draw small.
- SMTC's `Closed`, `Opened` and `Changing` statuses all mean "a session exists but says nothing useful
  yet"; folding them into one `Unknown` keeps the UI from flickering through them on a track change.
- `TryPlayAsync` against an already-playing session returns true and does nothing: the bool means "the
  app accepted the command", not "the state changed".
- `GetPlaybackInfo` is local and cheap; `TryGetMediaPropertiesAsync` is an async round-trip to the
  owning app (a wedged player shows up as a slow read), and the thumbnail needs a stream open plus decode.

### src/flyout/layout.rs: why the footer has no "Restart Explorer" button

A Restart Explorer button was added to the flyout footer (v0.7.0, commit 7013677) and later removed.
The strip needing a fresh Explorer is a condition the app can detect and repair itself
(`taskbar::apply_at_startup`), and an unlabelled glyph only helps a user who already knows that
restarting the shell is the answer to a taskbar that looks wrong. `audio-tray --taskbar-restart`
remains as the manual escape hatch. `footer_buttons`' doc comment points here.
