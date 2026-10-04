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
