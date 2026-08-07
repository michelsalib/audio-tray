# XAML Diagnostics TAP from Rust — M1/M2 spike results

Measured on Windows 11 Pro 26200, `windows`/`windows-core` 0.61, rustc 1.97.

**Verdict: the mechanism works from Rust. The taskbar's XAML tree is fully
readable, and the elements the plan wants to decorate are located.**

## The one thing that makes or breaks it

**You must implement `IVisualTreeServiceCallback2`, not just
`IVisualTreeServiceCallback`.**

This is the whole ballgame, and it is not documented anywhere. With only the v1
callback, `AdviseVisualTreeChange` delivers exactly two elements — `PopupRoot`
and `FullWindowMediaRoot`, both with `NumChildren == 0` — and then nothing.
Every enumeration route out of those two roots is a dead end, and it looks for
all the world like Explorer is refusing to cooperate.

Add the v2 callback (IID `BAD9EB88-AE77-4397-B948-5FA2DB0A19EA`, one extra
method, `OnElementStateChanged`) and the same call replays the entire live tree:
**280 elements**, rooted at `DesktopWindowXamlSource`, including the whole
taskbar and notification area. Nothing else changed.

This is why the known-good C++ TAPs work: they all implement v2.

Practical note for windows-core: declare *only* v2 in `#[implement]`. Its vtable
already contains v1's slot and the generated `matches` answers `QueryInterface`
for the v1 IID, so declaring both builds two vtables and makes QI ambiguous.

## The tree, as it actually is on 26200

The plan's reference tree (`SystemTray.IconView` under
`SystemTray.OmniButton#ControlCenterButton`) does not match this build. What is
actually there:

```
SystemTray.SystemTrayFrame
  Grid#SystemTrayFrameGrid
    SystemTray.Stack#NotifyIconStack
      Grid#Content
        SystemTray.StackListView#IconStack
          ItemsPresenter > StackPanel > ContentPresenter
            SystemTray.ChevronIconView              <-- the overflow chevron
              Grid#ContainerGrid
                Border#BackgroundBorder
                ContentPresenter#ContentPresenter
                  Grid#ContentGrid
                    SystemTray.TextIconContent
                      Grid#ContainerGrid
                        SystemTray.AdaptiveTextBlock#Underlay      > TextBlock#InnerTextBlock
                        SystemTray.AdaptiveTextBlock#Base          > TextBlock#InnerTextBlock
                        SystemTray.AdaptiveTextBlock#AccentOverlay > TextBlock#InnerTextBlock
    SystemTray.NotificationAreaIcons#NotificationAreaIcons
      ItemsPresenter > StackPanel > ContentPresenter
        SystemTray.NotifyIconView#NotifyItemIcon    <-- one per tray icon
          Grid#ContainerGrid
            Border#BackgroundBorder
            ContentPresenter#ContentPresenter
              Grid#ContentGrid
                SystemTray.ImageIconContent
                  Grid#ContainerGrid
                    Image                            <-- the 16x16 icon bitmap
            Grid
              Rectangle#LeftDropInsertionMarker
              Rectangle#RightDropInsertionMarker
```

Three things worth noting for M3:

* The type is **`SystemTray.NotifyIconView`**, named `NotifyItemIcon` — not
  `SystemTray.IconView`.
* **`SystemTray.ChevronIconView` already exists** and is exactly the affordance
  the plan wants to build. Its structure (`ContentGrid` → `TextIconContent` →
  three layered `AdaptiveTextBlock`s for underlay/base/accent) is a ready-made
  template for a glyph that themes itself correctly.
* Our icon's content is `ImageIconContent` → `Image`. A chevron would go
  alongside it inside `Grid#ContentGrid`, or by swapping `ContentGrid`'s content
  for a `StackPanel`.

## Identifying *our* icon (rest of M2)

Not yet done, but the shape of the answer changed. The plan assumed tooltip or
`NotifyIconViewModel` matching, which needs `IFrameworkElement::get_DataContext`
(vtable slot 31) plus bindings for the view model — a chunk of hand-rolled WinRT.

There is a cheaper route the live feed makes available: audio-tray registers its
`Shell_NotifyIcon` *after* the TAP is attached, so the TAP observes the exact
`SystemTray.NotifyIconView` `Add` event caused by our own registration. That
identifies our element with no property inspection at all. Worth trying before
building the view-model bindings.

## M3 is DONE — the chevron renders in the live taskbar

Following the Windhawk route (below), the icon now draws as a device glyph plus
an up-chevron, inside Explorer, themed by the shell:

```
decorating icon 0x156579e0 (tooltip "") via presenter 0x155845c8
XamlReader.Load ok -> Windows.UI.Xaml.Controls.StackPanel
MUTATION SUCCEEDED — chevron content set on 0x155845c8
```

Verified visually in the notification area. The working sequence is:

1. Implement **`IVisualTreeServiceCallback2`** (see below — this is the linchpin).
2. Call `AdviseVisualTreeChange` **from a fresh thread**, per Windhawk's hang
   warning. The full tree replays.
3. In `OnVisualTreeChange` — the XAML UI thread, and the only place WinRT calls
   succeed — wait for a `ContentPresenter#ContentPresenter` whose grandparent is a
   `SystemTray.NotifyIconView`.
4. `XamlReader.Load(markup)` to build a `StackPanel` of two `FontIcon`s.
5. `IContentPresenter::put_Content(element)`.

Identification is via `AutomationProperties.GetName` on the `NotifyIconView`,
which carries the tooltip text. The injector passes the target tooltip as
`InitializeXamlDiagnosticsEx`'s initialization data, readable in the TAP through
`IXamlDiagnostics::GetInitializationData`; empty means "the first icon", which is
how the spike demos itself.

### The agreed design

Layout is the "even triad": output glyph, input glyph, chevron. Output and input
each cycle on click, through their active devices and then a muted stop — so a
machine with one device still has a cycle, mute and unmute (this supersedes the
earlier "mute only when there is no alternative device"). The chevron opens the
full flyout.

Geometry is "V2 — shell-matched", chosen from mockups because it mirrors the
Control Center button's metrics and so reads as a peer of that control. All values
are effective pixels; XAML applies the per-monitor scale itself.

| Token | epx | |
| --- | --- | --- |
| `PILL_H` | 32 | **load-bearing** — see below |
| `PILL_PAD_X` | 8 | |
| `PILL_RADIUS` | 6 | |
| `SEGMENT_W` | 24 | output / input hit width |
| `GLYPH_PX` | 16 | |
| `CHEVRON_W` | 20 | |
| `CHEVRON_PX` | 12 | |
| `DIVIDER_H` | 16 | |
| `DIVIDER_M` | 3 | |
| | **91 × 32** | total |

**`PILL_H` fixes a real defect.** Without an explicit height the `Border`
shrink-wraps the `FontIcon` layout boxes — and Segoe Fluent glyph ink overshoots
those boxes, so the microphone's stand was being clipped by the pill's bottom
edge. Only visible at magnification, which is why it shipped.

**Fill is the accent at 50% alpha** (`PILL_ALPHA = 0x80`). A fully opaque accent
block is brighter than anything Windows puts in a taskbar; at half alpha the pill
sits at the same weight as the Control Center button. Applied as alpha rather than
a pre-blended colour, so the taskbar's real backdrop shows through.

**Foreground contrast was also wrong.** `on_accent` now composites the accent over
the taskbar ground *before* judging luminance, and the dark-foreground threshold
moved from `0.45` to `0.32` (`DARK_FG_ABOVE`). Measured against Windows: on accent
`#D88DE1` (luminance 0.39) Quick Settings draws dark glyphs, where the old
threshold picked white.

Muted swaps the glyph (`E74F` output, `EC54` mic) *and* applies a warm tint. The
glyph swap is the primary signal; the tint is a single constant to drop if it
reads as an error rather than a mode.

**Hover and press are specified but not built.** White overlay at 14% / 24%,
clipped to the segment. They need per-segment `PointerEntered` / `PointerPressed`
handlers — a WinRT delegate plus a hit-testable background per icon — which the
zone-maths click routing does not provide.

### Init-data protocol

The injector passes a `key=value;` payload as `InitializeXamlDiagnosticsEx`'s
initialization data, which the TAP reads via `GetInitializationData`:

```
tooltip=Audio Tray;out=E7F6;in=E720;outmuted=0;inmuted=1
```

`tooltip` selects which tray icon to decorate (empty = the first found); `out` /
`in` are Segoe Fluent codepoints in hex. Unknown keys are ignored, so the payload
can grow without breaking an older TAP. This is a snapshot taken at injection
time — keeping it live is M4's job.

Injector flags mirror it: `--tooltip`, `--out`, `--in`, `--muted-out`, `--muted-in`.

### Still to do: click routing

Nothing routes clicks yet. The plan is zone maths — keep the existing
`Shell_NotifyIcon` click, take the cursor X against the icon's rect, and decide
which third was hit. That needs no new bindings. Per-segment *hover* would need
real `PointerPressed` handlers and a WinRT delegate, which is only worth paying
for if the missing hover turns out to bother in daily use.

## Repositioning the strip — DONE, verified on screen

The strip now sits **between the keyboard-layout indicator and wifi**, which is
where it was asked to go.

Reparenting is impossible (`Panel.Children` is refused, below), but the tray's
sections are children of one `Grid#SystemTrayFrameGrid` laid out by the
`Grid.Column` attached property, and *that* is writable. So the move is a column
shift, not a reparent.

Columns as found on 26200:

| col | section |
| --- | --- |
| 0 | `SystemTray.Stack#NotifyIconStack` (the `^` overflow button) |
| 1 | `SystemTray.NotificationAreaIcons` ← **ours lives here** |
| 2 | `SystemTray.Stack#MainStack` (keyboard layout) |
| 3 | `SystemTray.Stack#NonActivatableStack` |
| 4 | `SystemTray.Stack#SecondaryClockStack` |
| 5 | `SystemTray.OmniButton#ControlCenterButton` (wifi / volume / battery) |
| 6 | `SystemTray.OmniButton#NotificationCenterButton` |
| 7 | `SystemTray.Stack#ShowDesktopStack` |

`NotificationAreaIcons` goes to `ControlCenterButton - 1`, and everything it
steps over shifts one column left — which preserves their relative order rather
than blindly swapping two sections.

Reading the column needs `IGridStatics::GetColumn`, and it takes an
**`IFrameworkElement*`**. Handing it the `IInspectable` from
`GetIInspectableFromHandle` calls through the wrong vtable and **hangs the
shell** rather than returning an error — QI first.

Two caveats that are properties of the approach, not bugs:

- This moves the **whole notification-area section**, so every notify icon
  travels with ours. There is no per-icon alternative while reparenting is
  refused.
- Like hiding the system volume icon, it is a shell-wide edit with **no undo**
  short of restarting Explorer — the TAP pins itself.

## Registering the tray icon (audio-tray's side, but it decides whether any of this runs)

Two traps, both of which silently disable the whole feature — the TAP only
decorates an icon that is actually **on** the taskbar, so an icon in the overflow
flyout means no strip and, correctly, no edits to the shell either.

**The registration tooltip is the icon's identity.** Windows keys
`HKCU\Control Panel\NotifyIconSettings` on executable path + `InitialTooltip`,
and "always show in the taskbar" is stored against that. Registering with a
different string creates a *second*, unpromoted entry and drops the icon into the
overflow on upgrade:

```
Exe : …\audio-tray.exe   Tip : Audio output   IsPromoted : 1     <- the user's setting
Exe : …\audio-tray.exe   Tip : Audio Tray     IsPromoted :       <- what a renamed tooltip creates
```

Hence `tray::INITIAL_TOOLTIP`, frozen. The live tooltip (which carries
`TRAY_MARKER` for the TAP to match on) is set a moment later and is free to
change.

**`Shell_NotifyIcon`'s add failing is terminal.** It returns `ERROR_TIMEOUT`
(1460) whenever the shell is busy or not yet up — common at logon, and easy to
provoke by restarting Explorer. There is no second chance: the re-add path is the
`TaskbarCreated` broadcast, which has already been and gone. This used to kill
audio-tray outright with `E_FAIL`; making the failure non-fatal on its own only
traded a dead app for a silent one. `build_tray` retries, and proves the add
landed by setting a property afterwards — `TrayIconBuilder::build` reports
success even when it did not.

Waiting for `Shell_TrayWnd` to exist is *not* a usable readiness signal: 300ms
after killing Explorer the old window still answers `FindWindow`. And the retry
must be unbounded: giving up means exiting, which is the same outcome as the
crash it exists to prevent.

One consequence of freezing the registration tooltip: for a moment the icon's
accessible name is *only* `INITIAL_TOOLTIP`, before `refresh` writes the real one.
The TAP's substring match fails during that window —

```
try_decorate: icon 0x11204f60 named "Audio output" is not "Audio Tray"
```

— which the 3s sweep then covers. Without the sweep this would be a race that
sometimes left the strip undrawn.

## A freshly killed Explorer is not a usable test bed

Repeatedly force-restarting Explorer leaves its tray wedged in a way that looks
exactly like a bug in this code: `Shell_NotifyIcon` returns `ERROR_TIMEOUT` for
minutes, and `put_Content` against a tray element never returns, so decoration
stops dead at "setting content on …" while Explorer carries on repainting.

The tell that it is environmental: **the same TAP binary** produced a working
strip and a hung one in consecutive runs. Give the shell 45s to settle after a
restart and it works every time. Do not chase a decoration hang without checking
that first — several hours went into exactly that.

## Identifying *our* tray icon

Two bugs stacked here, and the first hid the second.

1. **`AutomationProperties::GetName` takes an `IDependencyObject*`.** Passing the
   `IInspectable` from `GetIInspectableFromHandle` calls the wrong vtable and
   returns an **empty string for every icon** — the same mistake that made
   `GetColumn` hang. With every manual test using an empty `--tooltip` ("decorate
   the first icon found"), the filter path was never exercised, so this only
   surfaced once the app started passing a real tooltip: the strip silently never
   appeared, and the gate then correctly suppressed everything else.

2. **A tray icon's accessible name is its tooltip, and ours changes.**
   `tray::refresh` rewrites the tooltip to the current default device's name on
   every switch, and that name is localised. Matching it exactly can never hold.

So the tooltip carries a stable suffix — `tray::TRAY_MARKER` (`"Audio Tray"`) —
and the TAP matches it as a **substring**. Observed accessible name:

```
"Audio Tray Haut-parleur (3- Realtek(R) Audio) — Audio Tray"
```

(Windows prepends its own app identifier, hence the marker appearing twice.)

There is deliberately **no fallback to "first icon found"** when the match fails:
that silently replaces another application's tray icon with our strip. If our
icon cannot be identified the TAP draws nothing, which the gate below makes a
clean no-op.

## Only touch the shell's UI once ours is up

Hiding Windows' volume icon and moving the notification area are only defensible
as *part of* replacing them. Done alone they take controls away and put nothing
back — which is exactly what shipped for one round: with the tooltip filter
broken, the strip never drew while the volume icon was still removed and the tray
still reordered.

Both edits are now gated on `strip_placed()` (decoration actually succeeded).
Finding the volume slot stays ungated and separate from collapsing it: the glyph
is announced during the replay, long before our icon is decorated, so gating the
*search* would mean never finding it even when the strip does appear.

## Turning it off — revert in place, never unload

Unloading the DLL cannot do the undo, and does not need to.

It cannot, for three independent reasons: `DLL_PROCESS_DETACH` runs under the
loader lock (no COM, no waiting on another thread); `FreeLibrary` runs on the
caller's thread while XAML objects are thread-affine; and diagnostics still holds
a reference to our callback, so unpinning races an in-flight
`OnVisualTreeChange` — a use-after-free in the shell, to reclaim a page of
memory.

It does not need to, because everything we change is an ordinary property write.
Record the previous value before each one (`restore.rs`) and play them back:

* the presenter's previous `Content` — an owned reference, kept so the shell's
  own visual cannot be collected while it is out of the tree. **Last-wins**: the
  shell data-binds this property and can rebuild it, so the thing to put back at
  the end is the newest shell visual, not the first one we displaced.
* every tray section's `Grid.Column`. **First-wins.**
* the volume slot's `Visibility`, `Width`, `MinWidth`. **First-wins**, and it
  matters most here: the collapse is re-applied ~24× to cover a layout race, and
  any read after the first would record *our own* zero width as the original.
  `Width` reads back as `NaN` when unset — that NaN is what has to go back;
  writing `0.0` leaves the element permanently zero-width, which looks exactly
  like still being hidden.

Handles die. A revert routinely finds the presenter gone, because killing
audio-tray destroys its notify icon first. That is success, not failure — hence
`Restored::{Done, Gone, Failed}` rather than a bool, so the log stops reading
like something broke.

### Getting "revert now" onto the XAML thread

A hidden window created **on the visual-tree callback thread**. Explorer already
pumps messages there, so a cross-process `PostMessage` is delivered by that pump
onto the one thread that may touch XAML. No dispatcher — `GetDispatcher` points
at a different island and posting through it returns `RPC_E_WRONG_THREAD`.

Measured: control window created on thread 39444, `WM_TAP_REVERT` received on
thread 39444. Creating it in `SetSite` instead would bind it to the injector's
marshalling thread and deliver every revert to a thread that cannot act on it.

Four triggers, because there are four ways for the strip to stop being wanted:

| event | mechanism |
| --- | --- |
| user turns it off | audio-tray posts `WM_TAP_REVERT` |
| audio-tray quits | same, before `PostQuitMessage` |
| audio-tray killed/crashes | TAP waits on the owner pid from the init data |
| Explorer restarts | nothing to revert; re-inject into the new one |

The owner watch is one blocking `WaitForSingleObject` on a process handle — no
polling, fires the moment the process dies however it dies. It re-checks
`OWNER_PID` before acting: a self-update spawns the replacement *before* the old
process exits, and reverting then would dismantle the new owner's strip.

A revert can also arrive before the control window exists (audio-tray failing
during startup, having already injected). That is deferred to a flag the next
callback consumes, rather than dropped.

### Re-enabling without an Explorer restart

Injecting again does not reuse the existing TAP — diagnostics builds a second one
and advises it too. Without a guard both would mutate the tray and both would
feed the one global visual tree. A `GENERATION` counter, bumped on every
`SetSite` and every stand-down, decides which instance acts; the others return
immediately *after* `tree::record` — recording is deliberately ungated, see above.

Standing down also clears `DECORATED`, `VOLUME_SLOT`, `WIRED`, and the "have we
done X yet" flags — that is what makes the toggle a real toggle. Verified: after
a revert, re-injecting restores a strip pixel-identical to the first one, no
Explorer restart.

The init data is re-read on every injection, so `TARGET_TOOLTIP` and `STRIP` are
mutexes, not `OnceLock`s — a once-only cell would pin the first run's settings
for the life of the Explorer process and silently ignore a changed accent.

### `TaskbarCreated` is *sent*, not posted

Explorer delivers this broadcast straight to the window procedure; it never
enters the message queue, so a `GetMessage` loop cannot see it. This is easy to
get backwards: a hand-rolled `PostMessage(HWND_BROADCAST, …)` shows up in the
loop immediately, while three real Explorer restarts produced nothing at all.
The receiver's `WndProc` re-posts it to itself, which is what puts it in the
queue where the loop can act with the config in scope.

Also: broadcasts reach *every* top-level window a thread owns, so the app sees
one per window (ours and `tray-icon`'s). Acting on the message directly injects
twice.

## Decoration must trigger on the icon *and* the presenter

`try_decorate` needs a `SystemTray.NotifyIconView` **and** its `ContentPresenter`
descendant. XAML announces children before parents, so triggering only on
`ContentPresenter` adds means that on any run where the `NotifyIconView` is the
last event of its subtree, the re-scan never happens and the strip never appears.

This was the cause of every intermittent "injection reported success, no strip"
in testing — including one run that left the volume icon hidden and the tray
reordered with nothing drawn in their place. Trigger on both.

Corollary for diagnosis: early returns must say why. They used to be silent,
which made this undiagnosable from the log. Log on *change* of reason, not a
first-N cap — a cap spends itself on the replay burst and then hides the reason
that mattered.

### The presenter must be found live, not in the recorded tree

Restarting audio-tray inside one Explorer session used to leave no strip at all:

```
try_decorate: no icon+presenter pair yet; icons+kids: []
try_decorate: no icon+presenter pair yet; icons+kids: [("0x143e7b00", 0)]   <- forever
```

The new `SystemTray.NotifyIconView` is announced, but **nothing under it ever
is** — no further event, even after 25s. XAML reuses the child elements it
already announced for the previous icon and silently re-parents them, so from our
side the new icon has no children and the pair can never be formed.

Fixed by asking XAML instead of our own bookkeeping:
`decorate::descendant_presenter` walks down from the icon handle with
`VisualTreeHelper.GetChildrenCount`/`GetChild`. That is also immune to the
announce-children-before-parents ordering the recorded scan had to work around.
Icons still come from the recorded tree — those are reliably announced.

(Gating `tree::record` on the generation made this worse and was fixed too, but
it was never the cause: the events genuinely never arrive.)

## Ordering: inject *before* the tray icon exists

Registering the tray icon first, so the TAP finds it on its first pass, used to
deadlock the shell reliably: the first decoration then landed inside the initial
replay burst. That is the same bug as the section below, and it is fixed there —
the ordering is no longer load-bearing.

`crate::tray::run` keeps it anyway, as belt and braces and because it costs
nothing: an icon arriving as a live delta after the replay is the easy case.

## Which island owns the tray

Explorer runs several XAML islands and calls back on more than one thread —
measured, `OnVisualTreeChange` arriving on 9804 and 20008 in one session. Triggers
matching on element *type* or *name* ("a `ContentPresenter` was added") therefore
fire in every island, so type filtering alone does not tell you where you are.

`lifecycle::on_tray_thread` tracks the thread of the most recent `SystemTray.*`
event and gates the work on it. Worth keeping, but be clear about its status: the
theory it was written for — that a call from the wrong island is what hangs — was
**disproven** (see below), and it has never been shown to prevent anything. It is
a defensive measure, not a fix.

Do not repeat the mistake of reading a stall at `setting content …` as evidence of
wrong-thread dispatch; log the thread id alongside and compare it to
`tray island is thread N` before believing that story.

### Resolved: never mutate from inside the event stream

**`put_Content` against a tray element while `AdviseVisualTreeChange` is streaming
does not return.** The UI thread is inside a marshalled call and wedges there with
the entire taskbar — CPU flat at 0.0s, clock stopped, recoverable only by
restarting Explorer.

Decoration used to run inline from `OnVisualTreeChange`, which is *always* inside
the stream. Whether it wedged came down to timing: if the tray icon happened to
arrive after the replay finished it worked, and if it arrived mid-replay it did
not. That is the coin flip that had the same binary behaving differently on
consecutive runs — and why injecting *after* the icon already existed reproduced
it every time, which was the clue that mattered.

So **`OnVisualTreeChange` touches no XAML at all.** It records the tree, and that
is the whole of its job. Every mutation — decorate, collapse the volume slot,
reorder the sections, attach the pointer handlers — moved into `sweep()`, the
control window's `WM_TIMER`, which bails unless the stream has been silent for
`QUIET_BEFORE_MUTATING` (400ms). `tree::quiet_for` reuses the `last_event` the dump
watchdog already tracked.

A `WM_TIMER` can itself be dispatched mid-burst, because an STA thread pumps while
a call is outstanding — so the timer is only the *driver* and the quiet check is
the guard.

Two steps are identified by the *handle on an event* rather than by the recorded
tree — the volume glyph's `TextBlock`, and our own segment `Grid`s — so deferring
them would lose the handle. They are queued (`PENDING_GLYPHS`, `PENDING_SEGMENTS`)
and drained by the sweep. Pushed only from the tray island's thread, so the handles
are always that island's to use.

Order within a sweep matters: find the volume slot (recording only), then decorate,
then collapse and reorder — both of those are gated on the strip actually being on
screen, which is what stops us removing Windows' controls and putting nothing back.

The sweep paces itself: `SWEEP_FAST_MS` (1s) while there is work, `SWEEP_IDLE_MS`
(4s) once the strip is placed, reordered *and* wired. The wiring term matters —
our segments are announced after `put_Content` returns rather than during it, so
they always land on the following tick, and without it the pace dropped first and
hover arrived an idle interval late.

Verified after the change, all five lifecycle paths, pixel-compared against a
pre-injection baseline:

| case | result |
| --- | --- |
| apply | strip up, segments wired |
| revert (`--taskbar-revert`) | exact revert (differs from baseline only by our own icon, which is still registered) |
| re-inject, same Explorer | 0 px from the first applied state |
| owner killed | 0 px from baseline |
| Explorer restart | re-injected, 0 px from before the restart |

Injection now succeeds with a 15–20s settle where 90s had been failing.

### Three theories that were wrong on the way

Kept because each looked convincing and cost hours:

1. **Wrong XAML island.** Disproven directly: `setting content … from thread 41704`
   against `tray island is thread 41704`.
2. **First-caller-wins thread pinning.** The reasoning — that the replay arrives on
   a marshalling thread, so pinning during it locks onto a thread that cannot act
   — is sound, and `HasThreadAccess` does read false there. Switching to last-wins
   changed nothing, because the thread was never the problem.
3. **Sweep-timer re-entrancy.** `BusyGuard` already prevented it, and the wedge
   happened on the *first* decoration, before any sweep had run.

The common mistake in all three: reasoning about *which thread* when the variable
that mattered was *when*.

### How the wedge presents, for next time

* **A hard block, not a spin and not a panic.** Explorer's CPU flat at **0.0s over
  70 seconds**, the taskbar clock **pixel-identical** across that window, no
  `panicked` line in the log. The taskbar is unusable until Explorer is restarted.
* The log stops between `XamlReader.Load ok` and the mutation result:

  ```
  XamlReader.Load ok -> Windows.UI.Xaml.Controls.Border
  setting content on 0x141da918 from thread 41704…     <- last line, forever
  ```

* Before the cause was understood it looked *nondeterministic* — the same binary
  producing a working strip and a wedge on consecutive runs — which sent the
  investigation after environmental explanations and thread identity. It was
  neither.

Two liveness checks worth reusing: sample Explorer's `CPU` over a window, and
compare two captures of the clock region **more than 60 seconds apart** (a shorter
window need not cross a minute boundary, and then a frozen clock and a healthy one
look identical).

## A periodic sweep, and why it needs a re-entrancy guard

Event-driven re-apply is not enough on its own: the shell data-binds the
presenter's `Content` and can overwrite our strip, and the re-apply only runs when
another tray mutation happens to arrive. If the tree goes quiet the strip stays
gone — with the volume icon still hidden and the tray still reordered, which is
the failure mode the `strip_placed()` gate exists to prevent, reached from the
other direction.

So the control window carries a 3s `WM_TIMER` that re-runs `try_decorate`.

Two things this needs:

* **`ACTIVE`**, separate from `GENERATION`. `GENERATION` answers "which instance
  may act"; the sweep has no instance to compare against, and without a second
  flag it cheerfully re-applies the strip seconds after the user turned the
  feature off. Verified: after a stand-down the taskbar stays reverted through
  5+ ticks, pixel-identical to clean.
* **A re-entrancy guard.** An STA thread pumps messages while an outgoing COM
  call is in flight, so `WM_TIMER` can be dispatched *inside* `put_Content`, on
  the same thread, mid-decoration. `BusyGuard` is claimed for the whole of the
  callback's XAML work and by the sweep; whoever loses simply skips.

## The hover treatment, and the alignment that broke it

Agreed shape: the hovered half **fills its half of the pill** — flush on all four
sides, sharing the pill's radius on its outer corners, square where it meets the
other half. Getting there ruled out three things worth not re-proposing:

* **A white wash.** It bleaches a saturated fill instead of lighting it. On accent
  `#D88DE1` the pill sits at `127,102,147`; white at 0.16 gives `148,127,164` —
  lighter but with the red-to-blue spread cut from 45 to 16, so it reads as grey.
  The plate is the *accent itself* at 0.30 instead: `154,114,170`, brighter and
  more saturated, and correct for any accent the user picks.
* **An inset on all four sides.** Puts the plate's rounded corners ~3px inside the
  pill's own, and two nested curves that close together read as a smeared double
  edge at taskbar scale.
* **A gap down the middle.** Reads as a crack splitting the pill in two.

The plate also cannot be wider than the segment it lives in, which is why the
segments are each half the pill (32 epx) with no padding on the Border. At 8 + 24
+ 24 + 8 the plate was stuck at 24 wide against 26 tall — permanently taller than
wide.

**The one that took three rounds**: the strip's `StackPanel` had
`VerticalAlignment="Center"`. That makes it size to its content, so the segment
`Grid`s were only as tall as a 16px glyph (~21 epx) inside a 32 epx pill — and a
hover plate can never exceed the Grid it lives in. No margin or radius change
could have fixed it. Stretching (the default) is load-bearing; the glyphs stay
centred by their own alignment.

### Explorer's own hover plate, and the one lever over it

Separately from our segment hover, the shell draws its own highlight on the
notification-icon *slot*, behind whatever we put in it. It is reachable — it is
`Border#BackgroundBorder` inside the icon's `ContainerGrid`, and we already mutate
shell properties elsewhere — but restyling it is a bad trade: every tray icon gets
that same plate, so making ours behave differently is what would look wrong next to
its neighbours, not the plate itself.

The indirect lever is `PILL_MARGIN`, and the numbers are what make it obvious:

```
slot 68 x 48 epx      pill 64 x 32 epx      surround 2 at the ends, 8 top and bottom
```

The slot is **48 epx tall** against a 32 epx pill, so the pill is centred with 8 epx
of slack — and a margin cannot do anything with slack that is already there. Going
from `Margin="2,0"` to `"2"` therefore changed **nothing**: horizontally the slot is
exactly pill-plus-margin, vertically the margin vanishes into the slack. Matching
the vertical figure (`PILL_MARGIN = 8`) is what evens it out, at a cost of 16 epx of
notification-area width. The alternative — `PILL_H = 44`, squeezing the slack out
instead — is free in width but makes the pill taller than the shell's own icons.

Measure the pill, never the presenter: the presenter fills the slot by definition,
so slot-minus-presenter is always zero. `decorate::content_size` reads the
presenter's *content*, which is the only handle-free way to get at our own Border.
`scratchpad/pill-metrics.ps1` cross-checks the same thing from a screenshot.

Cautionary note on mockups: the comparison sheets in `scratchpad` composite plate
geometry supplied *by hand* onto a real capture. They are pixel-accurate about
colour and position but say nothing about whether the markup produces that
geometry — and for three rounds they didn't, because they drew the plate at the
full pill height while the code was producing 21 epx. Verify a mockup against the
running strip before trusting it.

## Icons the font does not have

The strip draws the *current devices'* icons, resolved through the same path as the
flyout and the tray icon, so all three agree. Two of them — `WirelessEarbuds` and
`RoundEarbuds` — have no Segoe Fluent glyph and the tray hand-draws them from
circles and capsules. `IconId::glyph` returns the headphone codepoint for those as a
deliberate fallback, which in the strip would quietly show the wrong icon.

The strip carries a **Plane 15 private-use codepoint** for each instead (U+F0001,
U+F0002), and the TAP redraws the shape as XAML `Ellipse`/`Rectangle` in a `Canvas`.
Worth knowing why that route:

* **Not BMP private use.** Segoe Fluent occupies much of it itself (roughly
  U+E700..U+F8B3), so a BMP codepoint could collide with a real glyph.
* **A codepoint rather than markup or a bitmap.** Both channels the app has —
  the init-data string and the two-parameter `WM_TAP_RESTYLE` — already carry a
  codepoint, so nothing new is needed. Passing XAML would have meant `WM_COPYDATA`;
  passing a rendered bitmap would have meant a file on disk and an `Image` source.
* Vectors also follow the accent-derived foreground and stay crisp at any scale,
  which a bitmap would not.

The coordinates are duplicated between the app's rasteriser and the TAP, and that is
accepted: one is a signed-distance field, the other XAML shapes, so there is no
shared code to have. `VECTOR_STROKE` is `2 × OUTLINE_HW` and has to stay in step.

**Fit the ink to the box.** The normalised coordinates only span ~0.70 of it, which
is invisible in the tray where an icon is never seen beside another — but in the
strip they sit next to a font glyph whose ink fills the full 16 epx. Measured before
fitting: 11.3 epx tall against the microphone's 16.0, reading as a smaller, weaker
icon; after, 14.7. The scale is uniform about the centre and the **stroke is not
scaled** — it is a weight, and it should stay matched to the font's.

Shapes cannot inherit `Foreground` the way a `FontIcon` can, so `Stroke` is always
explicit. With a pill there is always a colour (`on_accent` picks black or white);
the bare-glyph mode falls back to white rather than guessing the taskbar's brush.

## Making the strip interactive — hover, left click, right click

All three work. Handlers are Rust objects living in `explorer.exe`, attached from
the visual-tree callback: our injected elements are announced back to us like any
others, so the segments are found by `x:Name` in the recorded tree.

Delegates (`TappedEventHandler`, `RightTappedEventHandler`, `PointerEventHandler`)
derive from **`IUnknown`, not `IInspectable`** — `Invoke` is slot 3 with no
`GetIids`/`GetRuntimeClassName`/`GetTrustLevel` ahead of it.

Three things that each cost a debugging round:

1. **`x:Name` needs its namespace declared.** `XamlReader.Load` parses the markup
   standalone, so the root must carry
   `xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"`. Without it the whole
   parse fails with `0x802B000A` and the strip silently never appears.

2. **`Background="Transparent"` is load-bearing.** A `null` background is not
   hit-testable in XAML, so a segment without one lets the pointer fall through
   to the pill and neither hover nor click can tell the halves apart.

3. **One click reaches a handler twice.** Measured: same `sender`, the *same
   event args object*, same thread, one registration — so it is one event
   delivered twice, not two events. Left unhandled it cycles the device two
   steps per click. Coalescing is keyed on the identity of the event args
   (a redelivery carries the same args object; a genuine second click carries a
   new one) with a 500 ms bound only to guard against COM recycling the address.
   The underlying cause in the taskbar's input hosting was not identified.

4. **The shell still invokes the icon under the strip.** Replacing the
   `ContentPresenter`'s content does not take the notify icon out of the shell's
   own input path, so one click on a segment is delivered twice at *two levels*:
   once to our XAML handler, and once to audio-tray as an ordinary
   `Shell_NotifyIcon` click. That is invisible while both mean the same thing, and
   it showed up the moment they diverged — left click began cycling the device
   *and* opening the flyout. Fixed on the audio-tray side (`taskbar::strip_is_up`
   gates the icon's left click) rather than by marking the routed args handled:
   the shell's handler sits on an ancestor and may well be watching pointer
   events rather than `Tapped`, and with clicks unsynthesisable there is no way to
   measure which. A right click doubles the same way but harmlessly — both
   deliveries mean "open the panel", and the reopen guard absorbs the second.

Hover is a pre-built white plate at `Opacity="0"` that handlers raise to `0.16`,
so nothing has to construct a brush inside Explorer. Verified by measuring mean
luminance per segment band: hovering the output half read 119.4 against 113.6 for
its twin, and vice versa when hovering the input half.

### Talking to audio-tray

The TAP decides nothing — it posts the gesture and audio-tray resolves what the
next device is. Transport is `PostMessage` (never `SendMessage`: a wedged
audio-tray must not be able to block Explorer's UI thread) to a hidden window
audio-tray registers.

**`FindWindow` could not locate that window.** As a message-only
(`HWND_MESSAGE`) window it is invisible to `FindWindow` by design, and
`FindWindowEx(HWND_MESSAGE, …)` did not find it across processes either. Even
recreated as a hidden top-level window, `FindWindow`/`FindWindowEx` returned
nothing while `EnumWindows` listed it plainly. So the lookup walks top-level
windows and compares the class name; it only runs on a click.

## Hiding Explorer's own volume icon — two traps

Matching it is easy: it is a glyph `TextBlock`, so it is found by **codepoint**
(`E992`–`E995`, `E74F`, `E198`) rather than by a translated name. Its slot is
another matter.

**Trap 1 — `Visibility` does not free the slot.** Inside `ControlCenterButton`
each icon sits in a generated container:

```
StackPanel
  ContentPresenter → SystemTray.IconView#SystemTrayIcon   ← wifi
  ContentPresenter → SystemTray.IconView#SystemTrayIcon   ← volume
  ContentPresenter → SystemTray.IconView#SystemTrayIcon   ← battery
```

Collapsing the `IconView` hides the glyph and leaves a hole. Collapsing the
`ContentPresenter` around it does too. Measured from screenshots: wifi stayed at
x 482..528 and battery at 626..684, **not one pixel of movement**, with a 97px
hole where the 25px spacing should be. `put_Width(0)` + `put_MinWidth(0)` on the
container is what actually closes it, and both levels get it for good measure.

**Trap 2 — it is a race with layout, and the failure is silent.** The same code
worked or didn't depending on when the collapse landed:

| `ActualWidth` when collapsed | result |
| --- | --- |
| `0` (layout had not run yet) | glyph hidden, **97px hole remains** |
| `24` (layout had run) | glyph hidden, slot closed, 25px spacing |

So the collapse must be **re-applied after layout has measured the item**. There
is no clean signal to stop on: a collapsed element goes on reporting its last
arranged `ActualWidth` — still `24` long after the slot has visibly closed — so
success cannot be read back off the element. The code therefore re-applies on
each mutation while `ActualWidth > 0`, bounded at 24 attempts.

Verified by three consecutive Explorer-restart-and-inject cycles producing
pixel-identical trays (gap 25px, matching the original wifi→volume spacing).

## Clicks cannot be synthesised onto the taskbar

Pointer *movement* injected with `SendInput` works fine — the hover plate lights
up, so the XAML island is receiving our input. Pointer *clicks* do not produce a
`Tapped`/`RightTapped` on the strip.

The decisive check is to aim the same synthetic click at the **plain** tray icon,
with the strip reverted: the flyout does not open either. So this is not our
event wiring — synthetic mouse clicks do not drive the Win11 taskbar's input path
at all. Real clicks work; they were verified by hand.

Consequence for testing: anything behind a click on the taskbar — left-click
cycling, right-click opening the panel — cannot be driven from a script here.
`audio-tray --taskbar-revert` exists so at least the revert path is reachable
without the UI, and `audio-tray --taskbar-click <out|in|panel>` posts the gesture
straight to the receiver window, which covers everything from `WM_TASKBAR_ACTION`
inward (the cycle, the mute stop, the restyle, the redraw) but *not* the shell's
own delivery of the click. Making real clicks work would mean
`CreateSyntheticPointerDevice` + `InjectSyntheticPointerInput` rather than
`SendInput`, which produces a real pointer frame; untried.

## Why the switch felt slow — 2.3s, and where it went

Measured end to end by screenshotting the pill in a loop around a click and
hashing each frame: **2343ms** from click to the glyph changing. Four costs, none
of them the audio stack:

| | |
| --- | --- |
| the redraw waited for the next sweep tick | up to 1000ms, ×2 |
| `restyle` cleared `DECORATED` → full rebuild, per message | ~2 rebuilds/click |
| audio-tray refreshed 2–3× per click (its own switch, then the endpoint-change notification for that same switch) | 164–252ms each |
| each refresh worked the same answer out three times over — 3 endpoint enumerations, 6 `default_of`, 2 `is_muted` | 119–383ms of it |

Fixes: the restyle handler now sweeps *inline* (safe, because `sweep` is what
carries the "tree has been quiet" check — if it declines, the timer still
retries); a restyle equal to the current `StripState` is dropped before it costs a
rebuild; `SWEEP_FAST_MS` went 1000 → 250 so a declined sweep retries promptly;
and on the audio-tray side one `Current::read` feeds both surfaces, identical
restyles are suppressed, and the strip is posted *before* the tray icon's
DirectWrite render.

That got it to 252–620ms, at which point the remaining cost was the switch
itself — `CoCreateInstance` for `IPolicyConfig` is free (0.07–0.9ms) but the three
`SetDefaultEndpoint` roles are 25 + 66 + 64ms, plus up to ~150ms of
`IAudioEndpointVolume` activations for the mute reads. The glyph does not depend
on any of it, so audio-tray now posts the outcome first and does the work after:
**50–200ms**, typical case 50–65ms.

The doubled-click workaround in `interact::already_seen` is therefore still
unverified against current code. It was measured once (same sender, same args
object, same thread, one registration) and coalescing on args identity fixed it;
whether the underlying double-delivery still happens is unknown.

## The music tile — a second strip, on an app's own button

Grafted in from a separate spike (`media-tray`), which existed to answer whether YouTube
Music could be drawn into the taskbar at all. It ends up in *this* TAP for one reason,
which is not tidiness: **XAML Diagnostics takes one consumer per endpoint.** A second
process injecting into the same Explorer gets `S_OK` and draws nothing — so two taskbar
features have to be one DLL, or one of them does not work. Verified in a single session,
one shell, one injection:

```text
init data = "…;hidevolume=1;hidemic=1;tile=YouTube Music;pid=6436"
MUTATION SUCCEEDED — chevron content set on 0x14e4e148     <- the audio strip
music: strip placed on 0xd9c8208 — "" / "" [Stopped]       <- the music tile
music: previous / play/pause / next wired
```

The tile lives in [src/music/](src/music/) and shares nothing with the audio strip but the
plumbing: the tree recorder, the mutation gate, the sweep, and `winrt.rs`.

### A `TaskListButton` is not the notification area, in three ways that each cost a fix

Everything this TAP knew was about `NotifyIconView`, where the slot sizes itself to its
content and a `ContentPresenter` accepts arbitrary XAML. A task button gives neither:

| | notification area | task button |
| --- | --- | --- |
| where content goes | `ContentPresenter.Content` | **`Border#BackgroundElement`.Child** — and *not* the unnamed `Border` before it in the panel, which draws behind the background |
| width | follows the content | forced: `button → panel → Border`, every level, **re-applied every sweep** because the shell puts `Width=44` back |
| the shell's own parts | none | `RunningIndicator` and `ProgressIndicator`, centred in a button that is now 244 epx wide |

Slot overhead is **4 epx**, not the 71 measured for the Widgets host and wrongly
generalised: a button's natural chain is `44 → 44 → 40`, so asking for 80 spent 76 on
nothing that paints — hover firing before the pointer reached the strip, and taskbar width
that pushed the shell into its compressed layout early. It cannot be 0 either: a flat
`240,240,240` puts the plate's rounded right corner on the parent's boundary and it is
shaved square. 4 is what the corner costs, which is why it is `Host::SLOT_OVERHEAD` per
host rather than a constant.

**The two indicators are moved, not hidden.** Collapsing `RunningIndicator` costs the only
cue that the app is open — the strip cannot supply it, since "closed" and "open, nothing
playing" both read as an empty strip. Centred in 244 epx it sits under the title text and
reads as a stray dot. Two writes put it where it means something:

```text
HorizontalAlignment = Left           the template centres it
Margin.Left = icon_centre - w/2      icon_centre = pad + cover/2 = 16 epx at the 240 strip
```

`w` is read back (`ActualWidth`, measured 6 epx → margin 13) rather than assumed, because
the shell grows the pill when the window is in the foreground. `ProgressIndicator` gets the
same treatment plus the icon's 28 epx width — at a 244-epx button it otherwise stretches
the whole strip. `Margin` needed two more `IFrameworkElement` placeholders
(VerticalAlignment g/p sit between it and `HorizontalAlignment`), counted against the SDK
header like every other slot here.

### The position goes on the shell's own progress bar, on another app's window

`ITaskbarList3::SetProgressState` + `SetProgressValue` **work cross-process**, on a window
we do not own. Nothing documents that case — it is normally an app reporting itself — so it
was measured: 40 % from a cold `--music-progress 40`, then against a live session, bar at
24 % with the session at 66.5 s of 275.7 s (24.1 %). Going through the shell buys the
colour (accent playing, **yellow paused**), the rounded ends, the track, the animation and
the user's theme, for no code.

Two things it costs. The fraction is quantised to 200 steps and sent only when the step or
the play state changes, because this is a cross-process COM call on a 1 s poll — half a
percent is seven times finer than the 28 epx it paints in. And it **must be cleared on
quit**, next to the notify-icon removal: a progress bar we put on somebody else's window
outlives us, and one frozen mid-track is a bug the user cannot attribute to audio-tray.

### The player publishes a checkpoint, not a clock

`GetTimelineProperties` on the YouTube Music session is not empty — worth checking rather
than assuming, since Chromium historically published nothing there. Measured over 6 s of
playback, the position moved **1.2 s**: it is republished on play, pause, seek and track
change, then sits still, and `LastUpdatedTime` is the timestamp of that checkpoint. A
paused read was correct at 158.3 s while the reads before it had been stale for seconds.

So the bar draws

```text
shown = position + (now − last_updated)   while Playing, clamped to end
shown = position                          otherwise
```

which is local arithmetic on a poll the feed already runs — a smooth bar for one
`GetTimelineProperties` per poll and no extra cross-process reads.

### Two text defects, both from numbers never read back

`report_widen` now logs our own elements in **both** dimensions, on a later tick (they have
no measured size on the tick that places them):

```text
before   Title 103.9 x 18.6      after   Title 119.1 x 17.0
         Artist 47.5 x 14.6              Artist 47.5 x 14.6
```

* **Clipped descenders.** 18.6 + 14.6 = 33.2 epx of stacked text in a 30-epx column with a
  `Clip` to match, so the tails of `p` and `y` went. The button gives no more than 32, so
  the last 1.6 comes out of the leading: `LineHeight="17"` *with*
  `LineStackingStrategy="BlockLineHeight"`, without which `LineHeight` is a minimum and
  nothing moves.
* **Dead space before the transport glyphs.** The epx-per-character figures were eyeballed
  at 7.43/5.78 and were 14 % too wide — a 16-character window rendered 103.9 epx of a 120
  epx column. Measured off rendered text: **6.49 and 5.28**, giving 18 and 23 characters at
  the 240 strip, filling the column to 119.1 of 120. An average will overflow on unusually
  wide characters; that is what the `Clip` is for, and overflowing by a character beats dead
  space on every normal title.

### Three behaviours that only using it could find

* **Suppressing `PointerPressed` costs drag-to-reorder.** The shell's press is where the
  drag gesture begins, so marking it handled removes reordering entirely — invisible in
  every log, visible only as a gesture that never starts. On the Widgets host the
  suppression must stay (an unhandled press opens the board); on an app's own button it
  protects nothing, because that button's click already *is* what we were reimplementing.
  `shell_owns_the_button()` gates the body's handlers, and the native path is strictly
  better: the shell activates or minimises with its own animation, and no foreground rights
  are needed at all. The three glyphs keep their suppression — pressing play must not also
  activate the app — so a drag starts on the body, 200 of the 240 epx.
* **A play click with no session must not synthesise a media key.** Reported from use:
  pressing play on the YouTube Music strip paused **MPC-HC**. A Chromium session does not
  exist until media has played, and Windows hands a media key to whichever app owns them.
  Checking for a foreign session first does not fix it (measured: *no* sessions reported
  while MPC-HC was running and responding to keys). The fallback raises the player instead
  — deterministic, always about the right app, and it puts the app's own play button under
  the cursor.
* **Raising a window: every call reports success and only one works.** From a process that
  has not just been interacted with — and we are structurally that process, since the click
  lands on Explorer's thread and reaches us as a posted message — Windows refuses the
  foreground change silently. `GetForegroundWindow` afterwards is the only honest test:

  | rung | result, from a process with no rights |
  | --- | --- |
  | `ShowWindow(SW_RESTORE)` if minimised | needed, needs no rights, does not foreground |
  | `SetForegroundWindow` | **refused** (returns success) |
  | `AttachThreadInput` to the foreground thread, then ask again | **works** |
  | `SwitchToThisWindow` | not reached; undocumented, increasingly ignored on Win11 |

  Verified end to end with the foreground read back from a *third* process. The proper
  handoff is done too, on the Explorer side where the click happens: `ipc::send` calls
  `AllowSetForegroundWindow(pid)` before posting, and **only** for that action — rights
  handed out that nothing spends are how a click steals focus by surprise.

### The app side: an MTA thread, because the STA deadlocks

Not a design choice. audio-tray's main thread is an STA (it owns windows), and every SMTC
call returns an `IAsyncOperation` this code blocks on — which on an STA that pumps messages
deadlocks. The first `--music-probe` hung with no output at all. The feed therefore owns
its own MTA thread (`music::on_mta_thread`), paces its own poll, and talks to the tray by
channel, so a wedged session cannot stall the audio half either.

Two smaller decisions worth recording:

* The tile's name comes through the existing init data as `tile=<app name>`, matched as a
  **substring** of the button's `AutomationProperties.Name` — the shell's name carries a
  localised suffix (`"YouTube Music épinglé"` here). A miss logs every button it saw, once,
  because those names are documented nowhere and change with the display language.
* Transport clicks are wire codes **10/11/12**, deliberately far from the audio strip's
  1/2/3: an off-by-one that cycled an audio device on a play click is exactly the kind of
  bug a shared wire invites.

### The transport controls moved to the hover preview — what that cost to learn

The three glyphs used to sit on the strip, which worked but spent 78 epx of taskbar on
controls that are only wanted occasionally. They are now on the preview's **thumbnail
toolbar**, and the strip is 162 epx instead of 240.

**`ThumbBarAddButtons` works cross-process**, like `SetProgressValue` before it: the shell
drew our three buttons under YouTube Music's preview, themed and DPI-scaled, from a call
made in audio-tray against a window Chromium owns. Nothing documents that case.

**But the click does not come back.** `THBN_CLICKED` arrives as a `WM_COMMAND` sent to the
window the buttons were registered against — the player's — which has never heard of them.
Measured exactly as predicted: the buttons drew and did nothing. What rescues it is that
the shell's buttons are ordinary XAML in Explorer, where this DLL already lives:

```text
Microsoft.UI.Xaml.Controls.ItemsRepeater#ThumbBarRepeater
  Taskbar.ThumbBarButton#ThumbBarButton
```

So the shell draws and the TAP listens, with a `Tapped` handler attached exactly as the
strip's own segments get one. Two traps on the way:

* **The tile's own tooltip suppressed the preview entirely.** `ToolTipService.ToolTip` on
  the strip made XAML's tooltip service own hover for that subtree, and the shell's
  `Taskbar.FlyoutFrame` never opened — the tile was the one taskbar button with no window
  preview, and nothing in any log said so. It presented as "the thumbnail toolbar does not
  work"; it was the preview never appearing. Remove the tooltip and both come back.
* **Buttons cannot be identified by position.** Changing the play glyph to a pause glyph
  means `ThumbBarUpdateButtons`, and the shell rebuilds that button — a new element,
  announced *after* the other two, with the handler still on the old one. Indexing a
  sequence-ordered list therefore put previous and next at 0 and 1 and the live play/pause
  off the end at 3, so the only button whose glyph changes was the only one that went dead
  after a single press. They are matched on `AutomationProperties.Name` instead, which
  carries the `szTip` audio-tray set — a contract between the halves, like the wire codes.

### Taking over the hover flyout — built, and abandoned

Drawing our own now-playing card into the preview (cover, title, artist, controls) works and
looks good, and it is still the wrong answer. Two measurements say why:

**There is exactly one `ContentPresenter#HoverFlyoutContent`, shared by every taskbar
button.** Its animation suggests one per hover; it is not:

```text
music: flyout 0x1349c888 ours=true  (1 recorded with that name)   <- our tile
music: preview card placed on 0x1349c888
music: flyout 0x1349c888 ours=false (1 recorded with that name)   <- VS Code, same handle
```

The shell shows a different app by *updating* the `TaskItemThumbnailList` in that presenter.
Replace the content and there is nothing left for it to update — so the now-playing card
appeared on **every** app's preview.

**Handing it back on the next sweep does not rescue it.** Ownership can only be re-checked
when the timer next runs, so a foreign preview shows our card until it does and ours shows
the shell's thumbnail until it does: a visible flip-flop in both directions. There is no
event to hang the work on instead, because `OnVisualTreeChange` may not mutate XAML.

A third cost, worth recording because it was the first symptom and looked like something
else: opening a flyout is a *sustained* burst of tree events, so `QUIET_BEFORE_MUTATING`
(400 ms) is not reached until the burst ends. That showed up as the shell's own thumbnail
sitting there for seconds before the card replaced it, and no amount of sweep-pacing fixed
it — the gate, not the timer, was the wall.

### Seen live

The last mile is done: the strip has been watched through track changes with the title,
artist, cover and progress line all on screen, the hover preview opens with the three
transport buttons under it, and they have been clicked by hand and drive the session.

One thing still cannot be driven from a script here, and it is the same limit the audio
strip has: **synthetic clicks do not reach the taskbar.** Synthetic *movement* does —
`SendInput` with relative nudges after a `SetCursorPos` opens the preview reliably, which is
how the flyout was measured at all — but `SetCursorPos` alone does not (a teleport never
starts the hover-intent timer), and no injected click produces a `Tapped`. So the buttons
drawing and wiring is verifiable from a script; the buttons *working* is not.

## The routes that do NOT work — three refused

Reading the tree is fully solved. **Changing it is not.** Three independent
routes were tried against real, live taskbar elements; all fail:

| Route | Result |
| --- | --- |
| `IVisualTreeService::CreateInstance` | `E_NOTIMPL` (0x80004001) |
| `IVisualTreeService::GetPropertyIndex` → `SetProperty` | `E_INVALIDARG`, so no property index can ever be obtained |
| WinRT `Panel.Children.Append` | `0x800F1000` (facility 0x0F = XAML) |

The last one is the informative failure. On the *same* object, in the *same*
call sequence, `IPanel::get_Children` and `IVector::get_Size` both return `S_OK`
with a correct count — so the object is live, the interface bindings are right,
and the thread is right. Only the write is refused. It fails identically on a
template-generated `Grid#ContentGrid` and on the island's own non-template root
`Grid`, so this is not about `ControlTemplate`-owned collections.

### Threading, settled

`SetSite` does **not** run on the XAML UI thread — WinRT calls from it return
`RPC_E_WRONG_THREAD` (0x8001010E). `OnVisualTreeChange` **does**: the identical
calls succeed there. Any real work has to happen on the callback. (Earlier
comments in this spike asserting the opposite about `SetSite` were wrong.)

Two refinements, both measured:

- The TAP must be `Agile = false` (`#[implement(…, Agile = false)]`). Left agile,
  COM delivered `OnVisualTreeChange` on **two different arbitrary threads**;
  opting out settles it onto one. This is the same `winrt::non_agile` Windhawk
  declares.
- **`IXamlDiagnostics::GetDispatcher` is a trap.** Explorer hosts several XAML
  islands. `GetDispatcher` hands back *one* island's `CoreDispatcher`, and it is
  not the tray's. Consequences, all confirmed by log:
  - `HasThreadAccess` on that dispatcher is `false` while on the callback
    thread — which reads like "you are on the wrong thread" but only means "you
    are not on *that island's* thread". The callback thread is the tray's.
  - Marshalling work to it with `RunAsync` genuinely lands there
    (`HasThreadAccess == true` inside the closure) and then every call against a
    tray element fails `RPC_E_WRONG_THREAD`.
  - Tray elements return a **null** `CoreDispatcher` from
    `DependencyObject::get_Dispatcher` (S_OK, null pointer), so there is no
    per-element queue to post to either.

  The rule that follows: **do tray work inline on the callback thread.** Do not
  dispatch.

### Resolved: how Windhawk actually does it

Read from `windows-11-taskbar-styler.wh.cpp` (781 KB, downloaded and grepped).
**Nobody appends to `Panel.Children`** — the styler never touches it either. The
real route is `DependencyObject::SetValue`:

1. The callback is `winrt::implements<VisualTreeWatcher, IVisualTreeServiceCallback2, winrt::non_agile>`
   — v2, and explicitly **non-agile** so COM keeps it on its creating apartment.
2. `AdviseVisualTreeChange` is called **from a freshly created thread**, not the
   `SetSite` thread, with this comment:
   > `// Calling AdviseVisualTreeChange from the current thread causes the app to`
   > `// hang in Advising::RunOnUIThread sometimes. Creating a new thread and`
   > `// calling it from there fixes it.`
3. In `OnVisualTreeChange`: `GetIInspectableFromHandle` → `DependencyObject`.
4. Property *names* become real `DependencyProperty` objects by a neat trick —
   build a XAML string
   `<ResourceDictionary><Style TargetType="…"><Setter Property="…" Value="…"/></Style></ResourceDictionary>`,
   run it through `Markup::XamlReader::Load`, and read `Setter.Property` /
   `Setter.Value` back off the parsed `Style`. No metadata API needed.
5. Apply with `elementDo.SetValue(property, value)` (or `ClearValue`), directly on
   the callback thread. Only one special case defers via
   `elementDo.Dispatcher().TryRunAsync(CoreDispatcherPriority::High, …)`.

Collection mutation is not universally blocked — the styler successfully appends
to `MergedDictionaries()` and to cloned `ColumnDefinitions`/`RowDefinitions`. It
is specifically `UIElementCollection` (`Panel.Children`) that refuses, which is
consistent with the `0x800F1000` measured above.

### The worked example we needed

The styler ships a built-in theme rule that does almost exactly what this plan
wants — replace a specific tray icon's content with a Segoe Fluent glyph:

```
SystemTray.NotifyIconView#NotifyItemIcon[AutomationProperties.Name=Bluetooth Devices]
  > Grid#ContainerGrid > ContentPresenter#ContentPresenter
    Content:=<FontIcon FontFamily="Segoe Fluent Icons" Glyph="&#xE702;" FontSize="16"/>
    Foreground:=<SolidColorBrush Color="{ThemeResource Accent1}" />
    Canvas.ZIndex=-1
```

Three things this settles:

* **Insertion point** is `ContentPresenter#ContentPresenter`'s `Content`
  property — one level *above* the `Grid#ContentGrid` this spike was trying to
  append into. Set `Content`, don't add children.
* **Identifying our icon (the rest of M2) is `AutomationProperties.Name`**, which
  carries the tooltip text. No `NotifyIconViewModel` bindings required — this is
  much cheaper than the plan assumed.
* Attached properties (`Canvas.ZIndex`) and enum values (`Visibility=1` =
  Collapsed, used on the inner `Image` in the sibling rule) both work through the
  same path.

For audio-tray specifically this is better than the original design: since we
already choose our own glyph, `Content` can be set to a horizontal `StackPanel`
holding a device `FontIcon` plus a chevron `FontIcon`, replacing the shell's
`Image` outright — and Segoe Fluent glyphs rendered by XAML come out crisp
without our DirectWrite path.

### Revised M3

1. Implement the XamlReader `Style`/`Setter` trick to resolve
   `DependencyProperty` + parsed value.
2. Bind `IDependencyObject::SetValue` (already declared in
   [src/winrt.rs](src/winrt.rs), slot 4).
3. Match our `NotifyIconView` on `AutomationProperties.Name`.
4. Set `Content` on its `ContentPresenter`.
5. Move `AdviseVisualTreeChange` onto its own thread, per the hang warning.

## Corrections to the plan

**windows-rs does not cover `xamlom`.** No `Win32_UI_Xaml_Diagnostics` feature in
`windows` 0.61 or 0.62; neither ships *any* `Xaml` feature. Not fatal —
`windows_core`'s `#[interface]` handles hand-rolled COM fine and `xamlOM.h` ships
in the SDK. [src/xamlom.rs](src/xamlom.rs) transcribes it, ~230 lines, no C++.

Traps worth knowing:

* `#[interface]` names `IUnknown_Vtbl` unqualified, so it must be imported, and
  trait methods need an explicit `pub`.
* By-value `BSTR` in-params must be raw `*mut u16`, never `windows_core::BSTR` —
  the latter's `Drop` calls `SysFreeString` on a string Explorer still owns.
* windows-rs has no `IInspectable_Impl`, so `#[interface]` can't inherit from
  `IInspectable`. For call-only WinRT interfaces, parent on `IUnknown` and spell
  out `IInspectable`'s three slots — the vtable is identical.

**`IVisualTreeService`'s property/collection API does not work here.**
`GetPropertyIndex` → `E_INVALIDARG` for every property name on every handle;
`GetCollectionCount` → `E_NOTFOUND`; `IXamlDiagnostics::HitTest` →
`E_INVALIDARG`. These are considered error codes, not crashes — the vtables are
correct. Don't build on them. ([src/walk.rs](src/walk.rs) keeps the probe that
proves this.)

`GetIInspectableFromHandle` works and returns live XAML objects, and
`RoGetActivationFactory` for `VisualTreeHelper` works inside Explorer
([src/xamltree.rs](src/xamltree.rs)) — but with the v2 callback in place the feed
already delivers the whole tree, so neither is needed for enumeration. They stay
relevant for M3, where we need real `UIElement`s to mutate.

## Stability

No Explorer crash across ~12 inject/restart cycles, including several runs with
deliberately wrong assumptions. Injection needs no admin rights and no COM
registration.

## Running it

```powershell
cargo build
./target/debug/xaml-tap-inject.exe --wait 10
```

Logs to `%TEMP%\xaml-tap.log`. The TAP pins itself (`DllCanUnloadNow` returns
`S_FALSE`), so **restart Explorer between iterations** or the rebuild fails with
a locked `xaml_tap.dll`.
