# Audio Tray — redesign brief

Nov 1, 2026 · Michel. Source: the "Audiotray UX redesign" doc on claude.ai. Progress is tracked in
the phase checklists below.

## Context and goal

Turn Audio Tray from a device switcher into a passive audio awareness tool: you see what is playing,
who is talking and whether your mic is live without opening anything.

- **Today:** Rust, Windows-only. Two buttons are injected into the taskbar through XAML Diagnostics
  in `explorer.exe` (`crates/taskbar-tap`), plus a YouTube Music tile, and a custom-drawn flyout
  (`src/flyout`) that lists devices.
- **Target:** a floating left-edge pill as the passive surface, the flyout kept for control, a plain
  tray icon as the entry point. Explorer injection becomes optional.
- **Keep:** everything under `src/audio` and `src/music`, the updater, the installer, the OSD.

## Hard constraints

CPU efficiency is the top priority: when nothing changes, the process does not wake up.

1. **Event-driven only.** No fixed polling loops. Allowed exceptions: the existing 5 s mic safety
   recheck, peak meters while the flyout is open (33 ms), and the mic peak at about 10 Hz only while
   in a call and muted.
2. **Compositor-driven animation.** Draw the pill with
   [DirectComposition](https://learn.microsoft.com/en-us/windows/win32/directcomp/directcomposition-portal).
   Fades, slide-outs and the progress ring are DWM animations set once per state change, not
   per-frame repaints.
3. **No global mouse hook for hover.** Use a 2 px non-transparent edge strip window with
   [`TrackMouseEvent`](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-trackmouseevent).
4. **Few threads, one message loop.** Wait with
   [`MsgWaitForMultipleObjects`](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-msgwaitformultipleobjects)
   on pipe and socket handles. No async runtime unless networking truly needs it.
5. **Stay native Rust.** No WebView, no Tauri, no .NET. Use
   [windows-rs](https://github.com/microsoft/windows-rs).
6. **Never steal focus.** Pill windows use `WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST` and
   are shown with `SW_SHOWNOACTIVATE`.
7. **Explorer injection is opt-in.** The app must be fully usable without `taskbar-tap`.

**Budget to verify:** Process Explorer's CPU Cycles Delta reads 0 at idle; a
[WPR](https://learn.microsoft.com/en-us/windows-hardware/test/wpt/windows-performance-recorder)
trace shows no wakeups beyond the mic safety recheck. Measurements live in [PERF.md](PERF.md).

## Architecture

Six sources, one core, four surfaces. Sources only emit events and surfaces only render core state.
The core also sends commands back to integrations, such as Discord or Teams mute toggles. Each
integration can be disabled without touching the rest.

## Edge pill UX

The pill is discreet by default and only gets louder when something needs you. It floats on the left
edge, vertically centered, reserves no screen space (not an AppBar), and hides while a fullscreen app
runs
([`SHQueryUserNotificationState`](https://learn.microsoft.com/en-us/windows/win32/api/shellapi/nf-shellapi-shqueryusernotificationstate)),
except for red alerts.

| Level | State | Look | Duration | Click-through |
| --- | --- | --- | --- | --- |
| Quiet | Idle | 6 px grey sliver | Until something happens | Yes |
| Quiet | Music playing | Translucent 44 px pill, album art in a progress ring | While playing | Yes |
| Quiet | Hover on music | Card with title, artist, prev/pause/next | Until pointer leaves | No |
| Normal | In a call | Opaque pill, faces stacked and dimmed, red dot on mic | While mic is live | No |
| Normal | Someone talking | Their face lit green, name tag beside it | While they speak | No |
| Warn (amber) | Talking while muted | Amber pill, "You're muted in [app] · click to unmute" | Until you stop or unmute | No |
| Warn (amber) | Audio moved | "Now on Speakers · headphones disconnected" | 5 s | No |
| Alert (red) | App starts recording outside a known call | "[App] is listening · click to mute" | Until acknowledged | No |

Rules:

- Warnings use color plus icon plus text, never color alone.
- Live streams show "LIVE" instead of the progress ring when the session has no usable end time.
- The flyout stays for control: device rows with the device list collapsed, Now Playing, per-app
  mixer, voice panel.
- Mockups: [Audio Tray canvas](https://claude.ai/artifact/LArE6TGsZN7DbQM7TDLt3k), section "Floating
  edge pill".

## Sources

A generic Windows layer covers every app; per-app integrations are optional add-ons that degrade
gracefully when absent.

| Source | Provides | Mechanism | Limits |
| --- | --- | --- | --- |
| Media (any app) | Title, artist, art, playback, timeline | [SMTC](https://learn.microsoft.com/en-us/uwp/api/windows.media.control.globalsystemmediatransportcontrolssessionmanager) events: `SessionsChanged`, `PlaybackInfoChanged`, `MediaPropertiesChanged`, `TimelinePropertiesChanged`; interpolate position locally | Plain browser tabs report a bare browser id; metadata quality depends on the site |
| Mic usage (any app) | Which apps hold the mic | Existing `src/audio/mic.rs` (consent store + `RegNotifyChangeKeyValue`) | None new |
| Per-app volume | Volume, mute, peak per app | [`IAudioSessionManager2`](https://learn.microsoft.com/en-us/windows/win32/api/audiopolicy/nn-audiopolicy-iaudiosessionmanager2) | Per-app device routing is undocumented (`IAudioPolicyConfigFactory`, as in [EarTrumpet](https://github.com/File-New-Project/EarTrumpet)) |
| Discord | Channel, members, who is talking, self mute and deafen, mute toggle | Vencord user plugin; its `native.ts` ([docs](https://docs.vencord.dev/plugins/native/)) writes JSON events to a named pipe hosted by Audio Tray, e.g. `\\.\pipe\audio-tray` | Uses internal Discord stores (can break on updates); client mods violate Discord's terms. Fallback: [Discord RPC](https://docs.discord.com/developers/topics/rpc), limited to the app owner and up to 50 testers |
| Teams | In meeting, self mute, mute toggle | Local third-party app API, enabled by the user in Settings > Privacy > Manage API ([Microsoft support](https://support.microsoft.com/office/connect-to-third-party-devices-in-microsoft-teams-aabca9f2-47bb-407f-9f9b-81a104a883d6)); WebSocket on `localhost:8124` | Admin can disable it; wire format only [community-documented](https://lostdomain.notion.site/Microsoft-Teams-WebSocket-API-5c042838bc3e4731bdfe679e864ab52a); no participants or active speaker |
| Twitch | Followed channel went live | [EventSub WebSocket](https://dev.twitch.tv/docs/eventsub/handling-websocket-events/) `stream.online`, or [Get Followed Streams](https://dev.twitch.tv/docs/api/reference/#get-followed-streams) every few minutes | Needs a Twitch app registration and `user:read:follows`; check EventSub subscription limits |

All sources feed one internal event type so the UI never depends on which integration produced a
fact (for example, `SelfMuted { app, muted }` comes from Discord or Teams alike).

## Phased plan

Ship in five phases; each must pass its checks before the next starts. Make small commits and keep
the app working at every step.

**Phase 0: measure the baseline**

- [x] Record idle CPU cycles and wakeups per second, with nothing playing (`scripts\idle-perf.ps1`;
      WPR needs elevation)
- [x] Same, with music playing
- [x] Write the numbers into `docs/PERF.md`

**Phase 1: event-driven music**

- [x] Replace the 1 s poll loop in `src/music/mod.rs` with SMTC event subscriptions
- [x] Interpolate position from timeline checkpoints
- [x] Check: zero wakeups from the music thread when nothing plays (0 with no session and when
      paused); track changes appear in under 500 ms (1–12 ms after SMTC publishes them; the player
      itself takes ~1 s to republish its session on a skip, see PERF.md)

**Phase 2: source abstraction**

- [x] Define one internal event enum (media, mic users, app sessions, voice state, self mute):
      `src/hub`
- [x] Wrap existing audio, mic and SMTC code as sources emitting those events
- [x] Check: existing taskbar buttons and flyout behave exactly as before (their code paths are
      unchanged; the hub is fed alongside them)

**Phase 3: edge pill**

- [x] DirectComposition window, edge hit strip, quiet and normal states for music and mic:
      `src/pill`
- [x] Warn and alert states, fullscreen suppression (an appbar registered only for
      `ABN_FULLSCREENAPP`, no space reserved)
- [x] Check: idle budget met (see PERF.md); pill never takes focus (`NOACTIVATE` plus
      `MA_NOACTIVATE`); text and icons pass 4.5:1 contrast (white on the dark pill, near-black on
      amber 10.9:1, white on red 5.9:1, white on the notice purple 6.5:1)

**Phase 4: integrations**

- [x] Named-pipe server plus Vencord user plugin (TypeScript) for Discord: `src/integrations/discord.rs`,
      `integrations/vencord`. Tested against a scripted pipe client, not against Discord itself.
- [x] Teams local API client: `src/integrations/teams.rs`, off by default. Not tested against
      Teams (it needs the API turned on and a pairing click in Teams).
- [x] Twitch went-live notices, off by default: `src/integrations/twitch.rs`. Not tested (needs
      a Twitch application and token).
- [x] Check: each integration can be disabled, and the app works without it

**Phase 5: flyout refresh**

- [x] Collapsed device lists, Now Playing card, per-app mixer, voice panel

## Decisions on the open questions

- **Monitor:** the primary one. It is where the taskbar's tray lives, and the pill never moves
  under the user. Revisit if it should follow the cursor.
- **Live end times:** a session without an end time shows "LIVE" instead of the ring; not yet
  checked against a live Twitch or YouTube stream.
- **Teams format:** unverified; the client follows the community-documented protocol 2.0.0 and
  stays off until enabled.
- **Taskbar buttons:** kept, as an opt-in mode (`[strip] enabled = true`); `taskbar-tap` stays.
  With them off, a strip left by an earlier run is reverted at start-up.

## References

- [Repository](https://github.com/michelsalib/audio-tray)
- [Mockup canvas](https://claude.ai/artifact/LArE6TGsZN7DbQM7TDLt3k)
- [DirectComposition](https://learn.microsoft.com/en-us/windows/win32/directcomp/directcomposition-portal)
- [SMTC session events](https://learn.microsoft.com/en-us/uwp/api/windows.media.control.globalsystemmediatransportcontrolssession)
- [`IAudioMeterInformation`](https://learn.microsoft.com/en-us/windows/win32/api/endpointvolume/nn-endpointvolume-iaudiometerinformation)
- [Extended window styles](https://learn.microsoft.com/en-us/windows/win32/winmsgs/extended-window-styles)
- [Vencord native code](https://docs.vencord.dev/plugins/native/)
- [Discord RPC](https://docs.discord.com/developers/topics/rpc)
