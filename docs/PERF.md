# Idle performance

The redesign's budget (see [REDESIGN.md](REDESIGN.md)): when nothing changes, the process does not
wake up. The only allowed periodic work is the 5 s microphone safety recheck, peak meters while the
flyout is open, and the muted-in-call mic peak.

Measured with `scripts\idle-perf.ps1` (no elevation needed): process CPU cycles, and context
switches per named thread, the same counters as Process Explorer's Cycles Delta and Context Switch
Delta. A context switch is a wakeup. WPR traces are still worth taking for the final check.

## Baseline: v0.12.3, before the redesign (2026-11-01)

Installed release build, laptop on mains, 30 s window, taskbar strip on.

### Nothing playing (no SMTC session at all)

| Thread | Wakeups/s | Cycles/s |
| --- | --- | --- |
| `music` | 2.53 | 1.15 M |
| unnamed (WinRT pool, serving the music thread's async calls) | 0.73 | 1.26 M |
| `main` | 0 | 0 |
| `wheel-hook` | 0 | 0 |
| **Process** | **3.27** | **2.41 M** |

The music feed's 1 s poll (`src/music/mod.rs`, `Music::serve`) runs even when no player exists.
The unnamed thread is the microphone watcher (`src/audio/mic.rs`), not the WinRT pool: see below.

### Music playing (YouTube Music PWA, strip up)

| Thread | Wakeups/s | Cycles/s |
| --- | --- | --- |
| `main` (applies the progress bar) | 4.30 | 1.31 M |
| `music` | 2.67 | 0.74 M |
| unnamed, two threads | 2.63 | 1.72 M |
| **Process** | **9.60** | **3.91 M** |

## Phase 1: event-driven music (2026-11-01)

SMTC events wake the feed; while a track plays it wakes only when the taskbar bar is due to move
one step (200 steps, about a pixel of the ~150 epx bar), and interpolates the position locally.
The tray keeps the player's window between steps and skips `SetProgressState` when the playing
state is unchanged. Release build, 30 s windows.

| State | `music` wakeups/s | `main` wakeups/s | Process wakeups/s | Process cycles/s |
| --- | --- | --- | --- | --- |
| No session (standalone `--music-events`) | 0 | 0 | 0.03 | 0.08 M |
| Playing | 1.37 | 5.73 | 7.73 | 2.70 M |
| Paused, steady state | 0 | 0 | 0.65 | 0.83 M |

Event latency, `--music-events`: an SMTC change reaches a fresh reading in 1–12 ms. A skip takes
about 1.1 s from click to new title, all of it Chromium dropping its SMTC session on Next and
creating a new one ~1 s later; the old poll added up to another second on top. The cover arrives
on a later `MediaPropertiesChanged`, which now marks the cached cover stale.

YouTube Music publishes a timeline checkpoint once per state change and never updates it while
playing ("updated 19 s ago"), so interpolation is required, not an optimisation.

## Open: the costs that remain

- **Playing: `main` wakes about four times per bar step.** It is the cross-process
  `SetProgressValue` into Explorer. Fewer steps would mean a coarser bar; the edge pill's progress
  ring (a DirectComposition animation) will not need steps at all.
- **The microphone watcher's 5 s safety recheck** (the brief's allowed exception). An earlier
  reading here blamed spurious registry notifications; logging the wait showed the loop only
  ever wakes on its 5 s timeout. The "wakeups" are context switches inside each sweep of the
  consent store (about eight per sweep, ~3.4 M cycles in release), which is now the whole of
  the idle cost. Making the sweep cheaper (re-reading only the keys that hold the mic) is the
  next lever if that matters.

## After the redesign (2026-11-01)

Release build, strip off (the new default), pill on, Discord pipe listening, 60 s window,
nothing playing and no call:

| Thread | Wakeups/s | Cycles/s |
| --- | --- | --- |
| `mic` (5 s safety recheck) | 0.67 | 0.69 M |
| WinRT and COM pool | 0.08 | 0.02 M |
| `main` (tray, pill, pipe) | 0 | 0 |
| `music` | 0 | 0 |
| GPU driver threads (the pill's D3D device) | 0 | 0 |
| **Process** | **0.75** | **0.80 M** |

Down from 3.27 wakeups/s and 2.41 M cycles/s before the redesign. While a track plays, `main`
and `music` wake only for SMTC events (about 0.3/s each, measured on a debug build): the pill's
progress ring is a DirectComposition animation, and with the strip off nothing steps the
taskbar progress bar.
