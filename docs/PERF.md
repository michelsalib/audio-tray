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

All idle cost comes from the music feed's 1 s poll (`src/music/mod.rs`, `Music::serve`), which
runs even when no player exists.

### Music playing

Not measured yet.
