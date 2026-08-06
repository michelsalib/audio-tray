//! System-tray icon, click handling, and the Win32 message loop that drives them.
//!
//! The tray owns no window of its own — `tray-icon` provides the icon and posts click
//! events through a global channel that we drain after each dispatched message. Right
//! click opens our acrylic control flyout (volume, mute, output/input switching,
//! per-device icons, and a footer strip — see [`crate::flyout`]); left click opens it
//! too, but only when there is no strip.
//!
//! Normally the icon is not what the user sees or clicks: [`crate::taskbar`] draws the
//! strip's two buttons over it on every start, and their gestures arrive as
//! [`crate::taskbar::WM_TASKBAR_ACTION`] instead (see [`handle_taskbar_action`]). The
//! icon clicks above are the path for when that injection is unavailable — and, since
//! the shell keeps invoking the icon under the strip, also a second delivery of the
//! strip's own clicks, which is why the left one defers to
//! [`crate::taskbar::strip_is_up`].
//!
//! Scrolling a button changes *that* endpoint's volume, and it arrives here by two
//! different roads, because no single one carries both pointing devices:
//!
//!   * **The wheel**, through the low-level mouse hook below ([`ScrollVolumeHook`]). It sees
//!     the pointer in screen coordinates, so which button it is over comes from the icon's
//!     own slot rect — see [`flow_at`].
//!   * **A precision touchpad**, through the TAP: two-finger scroll never enters a global
//!     mouse hook at all (Windows routes it straight to the hovered window), but it does
//!     reach the XAML element under the pointer, so the strip's own
//!     `PointerWheelChanged` handler posts it to us. That handler knows exactly which
//!     segment it belongs to.
//!
//! They cannot double up: the hook swallows the wheel event it acts on, so XAML never sees
//! that one. Both arrive as [`crate::taskbar::WM_TASKBAR_SCROLL`] and are coalesced by
//! [`drain_scrolls`], and the level they land on is shown beside the buttons by
//! [`crate::osd`].

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use anyhow::Result;
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetAncestor, GetClassNameW, GetMessageW, PeekMessageW,
    PostQuitMessage, PostThreadMessageW, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx,
    WindowFromPoint, GA_ROOT, HHOOK, MSG, MSLLHOOKSTRUCT, PM_REMOVE, WH_MOUSE_LL, WHEEL_DELTA,
    WM_MOUSEWHEEL, WM_TIMER,
};

use crate::audio::wasapi::WasapiBackend;
use crate::audio::{notify, AudioBackend, Flow};
use crate::config::Config;
use crate::flyout;
use crate::icons::{self, IconId};
use crate::osd::Osd;
use crate::taskbar::WM_TASKBAR_SCROLL;

/// Volume change per wheel notch — 2%, the same step Windows' own volume keys take, and the
/// same one the flyout uses when you scroll over its sliders. `pub(crate)` so `--vol up` is
/// a notch too, rather than its own idea of a step.
pub(crate) const SCROLL_STEP: f32 = 0.02;

/// Stable marker appended to the tray icon's tooltip.
///
/// The tooltip is also the icon's accessible name, and that is the only thing
/// the taskbar TAP can use to pick *our* icon out of the notification area.
/// It cannot match the tooltip itself, because [`refresh`] rewrites it to the
/// current device name — which changes with every switch and is localised. So
/// the tooltip carries this constant suffix and the TAP matches on it. Keep it
/// in step with the `tooltip=` value in [`crate::taskbar`].
pub const TRAY_MARKER: &str = "Audio Tray";

/// The tooltip shown for a given default-device name.
fn tooltip_for(device: &str) -> String {
    format!("{device} — {TRAY_MARKER}")
}

/// The tooltip the icon is *registered* with. **Do not change this string.**
///
/// Windows keys a tray icon's identity in
/// `HKCU\Control Panel\NotifyIconSettings` on the executable path plus this
/// initial tooltip, and "always show in the taskbar" is stored against that
/// identity. Change the string and every existing user's icon silently reverts to
/// the overflow flyout on upgrade — which also disables the taskbar strip
/// entirely, since [`crate::taskbar`] only decorates an icon that is actually on
/// the taskbar.
///
/// Found the hard way: briefly registering with [`TRAY_MARKER`] instead created a
/// second, unpromoted entry and dropped the icon into the overflow. The live
/// tooltip is set by [`refresh`] a moment later and carries the marker; only this
/// registration-time value has to stay frozen.
const INITIAL_TOOLTIP: &str = "Audio output";

/// Tray thread id, shared with the low-level mouse hook (which is a bare `fn`).
static TRAY_TID: AtomicU32 = AtomicU32::new(0);

/// Build the tray icon and run the message loop until the user quits.
pub fn run(backend: WasapiBackend) -> Result<()> {
    let mut config = Config::load();

    // This thread's id, needed before anything is drawn: the endpoint notifications and the
    // scroll hook below both post here, and so does the microphone watch — which is
    // registered *first* so that an app starting to record during startup is not missed
    // between the strip being drawn and the loop being ready to hear about it.
    let thread_id = unsafe { GetCurrentThreadId() };
    crate::audio::mic::notify_thread(thread_id);

    // Receives clicks from the injected strip. Created before the injection, so a
    // strip that comes up immediately already has somewhere to post to. Also how we
    // learn that Explorer restarted — see `create_receiver`.
    let _receiver = match crate::taskbar::create_receiver() {
        Ok(hwnd) => {
            println!("taskbar: click receiver window {:?}", hwnd.0);
            Some(hwnd)
        }
        Err(e) => {
            eprintln!("taskbar: click receiver unavailable ({e:#})");
            None
        }
    };

    // The taskbar strip. A failure here never blocks the tray — the plain icon
    // registered just below is the fallback, and the whole of the UI without it.
    //
    // Deliberately *before* the tray icon exists. Injecting afterwards looks
    // tidier — the TAP would find the icon on its first pass instead of waiting
    // for it — but it means the decoration happens inside the initial visual-tree
    // replay, while the shell is still building that icon's subtree, and
    // `put_Content` there simply never returns. Measured: the log stops at
    // "setting content on …" and the strip never appears. Letting the icon arrive
    // as a live delta afterwards is the ordering that works.
    crate::taskbar::apply_at_startup(strip_icons(&backend, &config));

    // At logon audio-tray can start ahead of `explorer.exe`, and registering the
    // icon then fails silently and permanently — hence the retry inside.
    let tray = build_tray(&backend, &config)?;

    // Register endpoint-change notifications that wake this thread's message loop.
    let _notifications = notify::register(thread_id)?;

    // Scroll over the taskbar/tray to change a device's volume.
    let _volume_hook = ScrollVolumeHook::install(thread_id);
    // The level bar that scrolling puts up beside the buttons. Its window is created on
    // first use, so a tray that is never scrolled never makes one.
    let mut osd = Osd::new();

    let devices = backend.enumerate().map(|d| d.len()).unwrap_or(0);
    // Which of the two click routings is live, since it depends on whether the
    // injection above took and that is the first thing to know when a gesture
    // does the wrong thing.
    let gestures = if crate::taskbar::strip_is_up() {
        "left click cycles a segment, right click opens the panel"
    } else {
        "no strip — either button opens the panel"
    };
    println!("tray: created ({devices} output device(s)); {gestures}.");

    // The YouTube Music half, on a thread of its own. It has to be a thread: every SMTC call blocks
    // on the async operation it returns, and *this* thread is an STA that owns windows, where that
    // deadlocks — see `music::on_mta_thread`. `None` means switched off in config, or SMTC would not
    // open; neither is a reason to take the audio half down.
    //
    // Held to the end of `run` on purpose: dropping the handle is what tears the feature down, and the
    // progress bar it puts on the player's taskbar button would otherwise outlive us.
    let music = crate::music::spawn(&config.music);
    if music.is_some() {
        println!("music: following YouTube Music");
    }

    let tray_rx = TrayIconEvent::receiver();
    // Gestures older than this are the flyout's own opening or dismissing click
    // arriving again — see [`settle_after_flyout`].
    let mut reopen_guard = Instant::now();
    let mut msg = MSG::default();
    unsafe {
        // GetMessageW returns >0 for a normal message, 0 for WM_QUIT, -1 on error.
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            // Endpoint-change wake-ups arrive as a thread message (no window); the
            // notification is the single source of truth for the icon (plan §8).
            if msg.message == notify::WM_AUDIO_REFRESH {
                // Coalesce a burst: one set_default fires a callback per role, so drain
                // any queued refresh messages and refresh only once.
                let mut extra = MSG::default();
                while PeekMessageW(
                    &mut extra,
                    None,
                    notify::WM_AUDIO_REFRESH,
                    notify::WM_AUDIO_REFRESH,
                    PM_REMOVE,
                )
                .as_bool()
                {}
                refresh(&backend, &tray, &config);
                continue;
            }
            // An app took the microphone, or let it go. Nothing else about the strip has
            // changed, so amend the dot in place rather than re-reading the devices — see
            // [`push_mic_state`]. Drained first for the same reason the refresh above is:
            // the state comes from the watcher's atomic, so only the last message says
            // anything the ones behind it do not.
            if msg.message == crate::audio::mic::WM_MIC_CHANGED {
                let mut extra = MSG::default();
                while PeekMessageW(
                    &mut extra,
                    None,
                    crate::audio::mic::WM_MIC_CHANGED,
                    crate::audio::mic::WM_MIC_CHANGED,
                    PM_REMOVE,
                )
                .as_bool()
                {}
                if !push_mic_state(crate::audio::mic::in_use()) {
                    refresh(&backend, &tray, &config);
                }
                continue;
            }
            // Scroll over a button (or anywhere on the tray, from the wheel hook) → nudge
            // that endpoint's volume and show the level beside the buttons.
            if msg.message == WM_TASKBAR_SCROLL {
                let flow = crate::taskbar::flow_from_code(msg.wParam.0);
                handle_scroll(&backend, &tray, &mut osd, flow, msg.lParam.0 as i32);
                continue;
            }
            // The readout's hold-then-fade. Its own window's timer, so it cannot be
            // confused with anyone else's.
            if msg.message == WM_TIMER && osd.owns(msg.hwnd) {
                osd.tick();
                continue;
            }
            // A click on the injected taskbar strip, relayed by the TAP running
            // inside Explorer. If there is no strip, this message simply never
            // arrives.
            if msg.message == crate::taskbar::WM_TASKBAR_ACTION {
                // Whatever the readout is showing is about to be stale — this click either
                // opens the panel over it or switches the endpoint it was reporting.
                osd.hide();
                if let Some(action) = crate::taskbar::Action::from_code(msg.wParam.0) {
                    // The click that dismissed the flyout lands on the strip like any
                    // other, and `Tapped` completes on release — after the flyout has
                    // already closed on the press. Acting on it would cycle or reopen
                    // on the click the user meant as "go away".
                    if Instant::now() < reopen_guard {
                        println!("taskbar: {action:?} ignored — that click dismissed the panel");
                        continue;
                    }
                    // Never propagated: this is COM against endpoints that can go
                    // away mid-click, and letting one failed gesture out of the
                    // loop would exit the app — the same trap `try_refresh`
                    // documents. A lost click is recoverable; a lost tray is not.
                    if let Err(e) = handle_taskbar_action(&backend, &mut config, &tray, action) {
                        eprintln!("taskbar: {action:?} failed ({e:#})");
                        // The strip may be previewing a switch that then failed, and
                        // the refresh that would have corrected it was skipped along
                        // with the rest. Put the truth back.
                        invalidate_strip();
                        refresh(&backend, &tray, &config);
                    }
                    // The panel has been and gone by the time that returns, so this
                    // is where a strip-opened flyout gets its settling — the branch
                    // below only covers one opened from the icon.
                    if action == crate::taskbar::Action::OpenPanel {
                        settle_after_flyout(&mut reopen_guard);
                    }
                }
                continue;
            }
            // Explorer restarted, so the taskbar is new and empty and the TAP
            // died with the old process. Relayed here by the receiver's window
            // procedure, which is the only place the shell's broadcast is
            // visible. `tray-icon` re-registers our plain notification icon on
            // the same signal, independently.
            if msg.message == crate::taskbar::WM_TASKBAR_RESTARTED {
                // Whatever the strip was showing died with the old Explorer, so
                // "unchanged since last time" no longer means "already on screen"
                // — the refresh below has to post again.
                invalidate_strip();
                crate::taskbar::apply_at_restart(strip_icons(&backend, &config));
                // The icon `tray-icon` just re-registered carries the defaults it
                // was built with, so put the current device's icon and tooltip
                // back on it — and this is also the retry for any refresh that
                // failed while the taskbar was away.
                refresh(&backend, &tray, &config);
                continue;
            }

            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);

            // Tray icon clicks open the panel, centred on the icon and just above it.
            while let Ok(ev) = tray_rx.try_recv() {
                if let TrayIconEvent::Click {
                    button, button_state: MouseButtonState::Up, rect, ..
                } = ev
                {
                    if Instant::now() < reopen_guard {
                        continue; // this is the click that just closed the flyout
                    }
                    // Right click always opens the panel. Left click only does so
                    // when there is no strip.
                    //
                    // The strip is drawn over this icon and the shell goes on
                    // invoking the icon underneath it, so a left click on a
                    // segment lands twice: once as the cycle, through
                    // `WM_TASKBAR_ACTION`, and once here. Acting on both made one
                    // click cycle the device *and* open the flyout. The panel
                    // belongs to the right click alone.
                    //
                    // A right click doubles the same way, and both deliveries mean
                    // "open the panel", so it needs no such test — the second one is
                    // absorbed by [`settle_after_flyout`] instead. Leaving it
                    // unconditional is also what keeps the panel reachable if the
                    // strip injected without managing to draw.
                    let opens_panel = match button {
                        MouseButton::Left if crate::taskbar::strip_is_up() => {
                            // The one visible sign this suppression happened, and
                            // the only way to tell it apart from a click the shell
                            // never delivered — worth a line, since a real click
                            // is the only thing that exercises this path.
                            println!("tray: left click on the strip — cycling, not opening");
                            false
                        }
                        MouseButton::Left => true,
                        MouseButton::Right => true,
                        _ => false,
                    };
                    if opens_panel {
                        let anchor = flyout::Anchor {
                            cx: (rect.position.x + rect.size.width as f64 / 2.0) as i32,
                            bottom: rect.position.y as i32,
                        };
                        osd.hide(); // the panel supersedes it, and covers where it sits
                        handle_flyout(&backend, &mut config, &tray, Some(anchor))?;
                        settle_after_flyout(&mut reopen_guard);
                    }
                }
            }
        }
    }
    Ok(())
}

/// Apply a scroll over the taskbar buttons: move that endpoint's volume, and show where it
/// landed beside the buttons.
///
/// `flow`/`delta` are the message that got us here; everything else already queued is folded
/// in with it — see [`drain_scrolls`].
fn handle_scroll(
    backend: &WasapiBackend,
    tray: &TrayIcon,
    osd: &mut Osd,
    flow: Flow,
    delta: i32,
) {
    // Asked for now rather than remembered from last time: the icon moves whenever the tray
    // is rearranged, and this is also what keeps the wheel hook's routing current.
    let slot = refresh_icon_rect(tray);
    for (code, notches) in drain_scrolls(flow, delta).into_iter().enumerate() {
        if notches == 0.0 {
            continue;
        }
        let flow = crate::taskbar::flow_from_code(code);
        // Never propagated, for the reason `handle_taskbar_action`'s caller documents: this
        // is COM against an endpoint that can be unplugged mid-gesture, and one failed
        // scroll must not take the tray down with it.
        match backend.nudge_volume(flow, notches * SCROLL_STEP) {
            Ok((level, muted)) => osd.show(flow, level, muted, slot),
            Err(e) => eprintln!("scroll: could not change the {flow:?} volume ({e:#})"),
        }
    }
}

/// Fold every queued scroll into one nudge per direction, returning the accumulated notches
/// indexed by [`crate::taskbar::flow_code`].
///
/// A precision touchpad reports a *stream* of sub-notch deltas — tens per gesture — and each
/// one taken on its own costs an endpoint `Activate` plus a get/set/get round trip. Handled
/// one at a time the volume would still be catching up long after the finger stopped.
/// Draining is self-throttling instead: whatever arrives while the audio work is in flight is
/// summed and applied in a single step, so the gesture stays live however fast the events
/// come. The wheel produces one message per notch and so drains nothing — it just takes this
/// same path.
fn drain_scrolls(flow: Flow, delta: i32) -> [f32; 2] {
    let mut totals = [0i32; 2];
    totals[crate::taskbar::flow_code(flow)] += delta;
    let mut extra = MSG::default();
    while unsafe {
        PeekMessageW(&mut extra, None, WM_TASKBAR_SCROLL, WM_TASKBAR_SCROLL, PM_REMOVE)
    }
    .as_bool()
    {
        let flow = crate::taskbar::flow_from_code(extra.wParam.0);
        totals[crate::taskbar::flow_code(flow)] += extra.lParam.0 as i32;
    }
    totals.map(|delta| delta as f32 / WHEEL_DELTA as f32)
}

/// The tray icon's slot on the taskbar, as the shell last reported it.
///
/// A static purely because of who needs it: [`flow_at`] tells the two buttons apart by which
/// half of the slot the pointer is on, and it is called from the mouse hook — a bare `fn`
/// with no tray icon in hand. Everything else asks [`refresh_icon_rect`] directly.
///
/// Only ever touched from the tray thread (the hook is called on the thread that installed
/// it), so the lock is never contended.
static ICON_RECT: std::sync::Mutex<Option<RECT>> = std::sync::Mutex::new(None);

fn icon_rect() -> Option<RECT> {
    match ICON_RECT.lock() {
        Ok(guard) => *guard,
        Err(poisoned) => *poisoned.into_inner(),
    }
}

/// Ask the shell where our notification icon is, remember it for [`flow_at`], and hand it
/// back to whoever is about to place the readout.
///
/// `Shell_NotifyIconGetRect` under the covers, so the answer is current rather than inferred:
/// the slot moves whenever tray icons are added, removed or reordered, and after every
/// Explorer restart. `None` while the icon is not on the taskbar at all — it has not been
/// registered yet, or it is in the overflow flyout — and the callers fall back rather than
/// fail.
fn refresh_icon_rect(tray: &TrayIcon) -> Option<RECT> {
    let rect = tray.rect()?;
    let (left, top) = (rect.position.x as i32, rect.position.y as i32);
    let slot = RECT {
        left,
        top,
        right: left + rect.size.width as i32,
        bottom: top + rect.size.height as i32,
    };
    let mut held = match ICON_RECT.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    // On change only: it decides where the readout appears and how the wheel is routed, and
    // this runs on every refresh — a line per refresh would drown the log it belongs in.
    if *held != Some(slot) {
        println!("tray: icon slot at {left},{top} {}x{}", rect.size.width, rect.size.height);
    }
    *held = Some(slot);
    Some(slot)
}

/// Settles the input that a just-closed flyout leaves behind, so the click that
/// opened or dismissed it cannot open it again.
///
/// Two separate leftovers, and the reopen-on-dismiss bug needed both handled:
///
///   * **Clicks queued while the flyout was up.** The gesture that opens the panel
///     is delivered twice — our handler *and* the shell invoking the notification
///     icon underneath the strip. The flyout is modal, so while it runs, that
///     second delivery is pumped by *its* loop into `tray-icon`'s channel and waits
///     there. Draining is the only fix available: it is already in the channel by
///     the time we get back, so no time-based guard can be armed early enough, and
///     it surfaces on whatever message happens to be dispatched next — which looked
///     exactly like the dismissing click reopening the panel, however long the
///     flyout had been open.
///   * **The dismissing click itself.** It has to reach the flyout (which holds
///     capture) to close it, and the shell reports it as a tray click too; on the
///     strip, `Tapped` completes on release, by which time the flyout has already
///     gone. Hence the short window, checked by both click paths.
///
/// Everything in the channel is stale by construction: nothing else could have put
/// a click there while the flyout owned the loop.
fn settle_after_flyout(reopen_guard: &mut Instant) {
    /// Long enough to cover the release half of the dismissing click, short enough
    /// that a deliberate second click never feels swallowed.
    const SETTLE: Duration = Duration::from_millis(350);

    let drained = TrayIconEvent::receiver().try_iter().count();
    if drained > 0 {
        println!("tray: dropped {drained} click(s) queued while the panel was open");
    }
    *reopen_guard = Instant::now() + SETTLE;
}

/// Act on a click from the injected taskbar strip.
///
/// Right click opens the full panel. Left click advances that endpoint one step
/// around a cycle whose stops are the active devices, each unmuted, plus a single
/// muted stop that sits between the last device and the wrap:
///
/// ```text
/// device 1 → device 2 → … → device N → device N, muted → device 1 → …
/// ```
///
/// Mute is a stop in the cycle rather than a gesture of its own because the strip
/// gives each endpoint exactly one left click, and it is what gives a one-device
/// machine somewhere to go — there the cycle is just mute and unmute, where it
/// used to be a silent no-op.
///
/// Every device stop is an unmuted one, in both directions:
///
///   * arriving at a device that was left muted (from the panel, a media key, or
///     another app) unmutes it. Without that the position in the cycle would be
///     ambiguous — "on device 2 and muted" is not one of the stops — and the click
///     would have landed on a device that stays silent.
///   * stepping off the muted stop takes the mute with it, so the cycle never
///     leaves a muted device behind. It used to: after wrapping, that device was
///     still muted, and choosing it again from the panel got silence.
fn handle_taskbar_action(
    backend: &WasapiBackend,
    config: &mut Config,
    tray: &TrayIcon,
    action: crate::taskbar::Action,
) -> Result<()> {
    use crate::audio::Flow;
    use crate::taskbar::Action;

    let flow = match action {
        Action::CycleOutput => Flow::Output,
        Action::CycleInput => Flow::Input,
        // No anchor: the strip is not the tray icon, so its rect is not ours to
        // know here. The flyout falls back to its default placement.
        Action::OpenPanel => return handle_flyout(backend, config, tray, None),
    };

    let devices = backend.enumerate_flow(flow)?;
    if devices.is_empty() {
        return Ok(()); // nothing plugged in for this direction
    }
    let current = backend.default_of(flow)?;
    let at = current
        .as_ref()
        .and_then(|id| devices.iter().position(|d| &d.id == id));
    let muted = current
        .as_ref()
        .and_then(|id| backend.is_muted(id).ok())
        .unwrap_or(false);

    // Which device the click lands on, or `None` for the muted stop.
    let next = match at {
        // On the muted stop: unmute and move on. The wrap is what normally
        // happens here, since the cycle only mutes on the last device — but a
        // mute that came from somewhere else is picked up wherever it left us.
        Some(i) if muted => Some((i + 1) % devices.len()),
        // Past the last device is the muted stop, on that same device.
        Some(i) if i + 1 == devices.len() => None,
        Some(i) => Some(i + 1),
        // Unknown current default: any switch satisfies "something else", so
        // start the cycle at the top rather than failing.
        None => Some(0),
    };

    // The strip can say where the click landed before any of the work below, which
    // is what the switch's latency was mostly made of. See [`preview_strip`].
    match next {
        Some(next) => preview_strip(flow, Some(icon_of(&devices[next], config)), false),
        None => preview_strip(flow, None, true),
    }

    match next {
        Some(next) => {
            let id = &devices[next].id;
            if current.as_ref() != Some(id) {
                backend.set_default_of(id)?;
            }
            // Unmute the device we are leaving *after* the switch: by then it is no
            // longer the default, so coming back off mute makes no sound.
            if muted {
                if let Some(left) = current.as_ref().filter(|left| *left != id) {
                    backend.set_muted(left, false)?;
                }
            }
            if backend.is_muted(id).unwrap_or(false) {
                backend.set_muted(id, false)?;
            }
        }
        // Muting the current default, in place — `current` is `Some` here, since
        // `at` only matches on a device we resolved from it.
        None => {
            if let Some(id) = &current {
                backend.set_muted(id, true)?;
            }
        }
    }

    // Brings the tray icon and the tooltip along, and confirms the preview above
    // against what actually happened — a no-op for the strip when they agree, which
    // is the normal case.
    //
    // Also the only thing that updates the strip after a *mute*: that raises no
    // endpoint-default notification, so nothing else would.
    refresh(backend, tray, config);
    Ok(())
}

/// Show the flyout and apply its outcome. Device switching
/// and volume/mute happen live inside the flyout; here we only persist icon changes and
/// honour a Quit. The tray icon is refreshed whenever the config changed (a per-device
/// icon may be the current default's).
fn handle_flyout(
    backend: &WasapiBackend,
    config: &mut Config,
    tray: &TrayIcon,
    anchor: Option<flyout::Anchor>,
) -> Result<()> {
    let outcome = flyout::show(backend, config, anchor);
    if outcome.config_changed {
        if let Err(e) = config.save() {
            eprintln!("save config failed: {e:#}");
        }
    }
    // The tray icon tracks the default output; switching it inside the flyout consumes the
    // endpoint-change notifications, so refresh here when the config or the default changed.
    if outcome.config_changed || outcome.output_changed {
        refresh(backend, tray, config);
    }
    // The flyout swallows [`crate::audio::mic::WM_MIC_CHANGED`] the same way, and worse: it
    // is a *thread* message, so the flyout's pump retrieves it, has no window to dispatch it
    // to, and drops it. An app that started or stopped recording while the panel was open
    // would leave the strip's dot wrong until the next thing to touch the strip. Reconciling
    // here costs an atomic read, and nothing at all if the answer has not moved — including
    // after the refresh just above, which already read the dot along with everything else.
    //
    // Only with a strip up: it is the one surface that has to be *told*. The flyout and the
    // readout paint from the same atomic every time they are drawn.
    if crate::taskbar::strip_is_up() && !push_mic_state(crate::audio::mic::in_use()) {
        refresh(backend, tray, config);
    }
    if outcome.restart {
        restart_app();
    }
    if outcome.quit {
        // Put the taskbar back before we go. The TAP also watches this process
        // and reverts when it sees it exit, which is what covers a kill or a
        // crash — but asking explicitly means a normal quit tidies up promptly
        // and predictably instead of racing our own teardown.
        crate::taskbar::revert();
        unsafe { PostQuitMessage(0) };
    }
    Ok(())
}

/// Relaunch the (already self-updated on disk) exe as a fresh process, then quit this one so
/// the newer build takes over. Best-effort: if the relaunch fails we stay running rather
/// than leaving the user with no tray.
///
/// Deliberately does *not* revert the taskbar strip: the replacement process
/// injects again and adopts the strip that is already there, so tearing it down
/// here would only make it flicker. The TAP recognises the handover by process
/// id and skips the revert its watcher would otherwise fire.
fn restart_app() {
    match std::env::current_exe() {
        Ok(exe) => match std::process::Command::new(exe).spawn() {
            Ok(_) => unsafe { PostQuitMessage(0) },
            Err(e) => eprintln!("restart: failed to relaunch: {e:#}"),
        },
        Err(e) => eprintln!("restart: current_exe() failed: {e:#}"),
    }
}

/// Registers the tray icon, retrying until the shell actually accepts it.
///
/// Two things make a plain "build it once" wrong at logon, when audio-tray can
/// start ahead of `explorer.exe`:
///
///   * `Shell_NotifyIcon`'s *add* failing is terminal, not transient. The re-add
///     path is driven by the `TaskbarCreated` broadcast, and by then that has
///     already been and gone — so the icon never appears at all for the life of
///     the process. This used to kill the app outright with `E_FAIL`; making the
///     failure non-fatal on its own just traded a dead app for a silent one.
///   * Waiting for `Shell_TrayWnd` to exist is not enough. Measured: 300ms after
///     killing Explorer the old window still answers `FindWindow`, so the wait
///     returns immediately and the add fails anyway. The only trustworthy signal
///     is the shell accepting a write.
///
/// Hence: build, then prove it took by setting a property. `TrayIconBuilder::build`
/// reports success even when the add did not land, so that second step is what
/// actually decides.
///
/// Retries **without a deadline**, backing off to a slow poll. Giving up would
/// mean exiting, which is the same outcome as the crash this exists to prevent —
/// an audio-tray with no icon has nothing to offer, so waiting for the shell is
/// strictly better than quitting on it. Observed failure is `ERROR_TIMEOUT`
/// (1460), and it can persist for minutes on a shell that has been restarted
/// repeatedly.
fn build_tray(backend: &WasapiBackend, config: &Config) -> Result<TrayIcon> {
    const FIRST_GAP: Duration = Duration::from_millis(500);
    const MAX_GAP: Duration = Duration::from_secs(5);
    /// How often to repeat the "still waiting" line, in attempts at `MAX_GAP`.
    const NAG_EVERY: u32 = 12;

    let initial_icon = Endpoint::read(backend, config, crate::audio::Flow::Output).icon;
    let mut gap = FIRST_GAP;
    for attempt in 1.. {
        // Clicks are handled via TrayIconEvent — we deliberately don't hand a
        // menu to tray-icon.
        let built = TrayIconBuilder::new()
            .with_tooltip(INITIAL_TOOLTIP)
            .with_icon(icon_image(initial_icon)?)
            .build();
        let failure = match built {
            Ok(tray) => match try_refresh(backend, &tray, config) {
                Ok(()) => {
                    if attempt > 1 {
                        println!("tray: registered on attempt {attempt}");
                    }
                    return Ok(tray);
                }
                Err(e) => e,
            },
            Err(e) => e.into(),
        };
        if attempt == 1 || attempt % NAG_EVERY == 0 {
            println!("tray: the shell is not taking icons yet, retrying… ({failure:#})");
        }
        std::thread::sleep(gap);
        gap = (gap * 2).min(MAX_GAP);
    }
    unreachable!("the retry loop only exits by returning")
}

/// Update the tray icon + tooltip, treating failure as non-fatal.
///
/// `Shell_NotifyIcon` fails whenever the taskbar is not accepting icons: at logon
/// before the shell is up, and again in the window while Explorer restarts.
/// Propagating that out of the message loop **killed the app** — measured, an
/// audio-tray started a few seconds too early exits with `E_FAIL` and the user is
/// left with no tray at all. It is a transient condition, and `TaskbarCreated`
/// is what puts the icon back.
fn refresh(backend: &WasapiBackend, tray: &TrayIcon, config: &Config) {
    let state = Current::read(backend, config);

    // The strip first, and the tray icon second, because the strip is drawn *over*
    // the icon: it is the surface the user is watching, and the icon underneath it
    // is not even visible while a strip is up. Doing it in the other order put a
    // DirectWrite glyph render (measured 40–85ms) in front of the thing that had to
    // change.
    //
    // Skipped entirely when it would change nothing. Not a micro-optimisation: our
    // own switch is followed by the endpoint-change notification for the same
    // switch, so a click produces two or three refreshes, and every restyle makes
    // the TAP tear the strip down and rebuild it. Only the first of them has
    // anything to say.
    push_strip(state.strip_icons());

    // Where the icon is now — so the *first* scroll of a session is routed and placed as
    // well as every later one, and so an Explorer restart (which re-registers the icon,
    // often somewhere else) is picked up without waiting for a gesture.
    refresh_icon_rect(tray);

    if let Err(e) = state.output.apply_to(tray) {
        eprintln!("tray: could not update the icon, will retry when the taskbar is back ({e:#})");
    }
}

/// Tell the strip what to draw, unless it is already drawing exactly that.
///
/// Returns whether the strip can be taken to be showing `icons` afterwards — either
/// because it already was, or because the restyle went out.
fn push_strip(icons: crate::taskbar::StripIcons) -> bool {
    let mut applied = match STRIP_APPLIED.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if *applied == Some(icons) {
        return true;
    }
    // Only recorded when the post actually went out, so a restyle dropped for want
    // of a control window is retried rather than assumed.
    if crate::taskbar::restyle(icons) {
        *applied = Some(icons);
        return true;
    }
    false
}

/// Push the current defaults to the strip, for the flyout — which changes devices
/// and mute live while it owns the message loop, so the tray's own [`refresh`]
/// cannot do it until the flyout closes.
pub(crate) fn restyle_strip(backend: &WasapiBackend, config: &Config) {
    push_strip(strip_icons(backend, config));
}

/// Put the recording dot on the input button, or take it off, without asking the
/// audio devices anything.
///
/// The answer to "is an app recording" arrives on its own schedule (an app opens or
/// closes a stream — see [`crate::audio::mic`]), and none of the rest of the strip
/// has changed with it. A full [`refresh`] would re-enumerate both directions and
/// re-read their mute state for a single bit, on the same critical path a click uses:
/// measured at 119–383ms. So this amends the state already on screen, exactly as
/// [`preview_strip`] does for a click.
///
/// Returns whether the amendment landed. `false` means there was nothing recorded to
/// amend, or the restyle never went out, and the caller has to fall back to a full
/// [`refresh`] — which reads the dot from [`Current::read`] like everything else.
///
/// That fallback is not a formality: the strip is normally drawn from the *injection's*
/// init data, and a restyle posted before the TAP has a control window is deliberately
/// dropped rather than remembered (see [`crate::taskbar::restyle`]). Nothing minded that
/// before, because the init data already carried the right glyphs — but recording state
/// can change after it was read, so this is the one thing that has to get through.
fn push_mic_state(recording: bool) -> bool {
    let Some(mut icons) = strip_applied() else {
        return false;
    };
    icons.input_recording = recording;
    push_strip(icons)
}

fn strip_applied() -> Option<crate::taskbar::StripIcons> {
    match STRIP_APPLIED.lock() {
        Ok(guard) => *guard,
        Err(poisoned) => *poisoned.into_inner(),
    }
}

/// Show the outcome of a click on the strip *before* doing the audio work that
/// realises it.
///
/// The work is the slow part and the glyph has nothing to learn from it: which
/// device is next is decided before a single COM call is made, and the switch
/// itself is three `SetDefaultEndpoint` calls (measured 25 + 66 + 64ms) plus up to
/// ~150ms of mute reads on the target. Posting afterwards meant the segment the
/// user just clicked sat unchanged for all of that, which is most of what made the
/// switch feel slow.
///
/// `None` for `icon` keeps the glyph and changes only the mute — the muted stop
/// stays on the same device, and the TAP picks the muted glyph itself.
///
/// A no-op until a refresh has recorded a state to amend: without one there is
/// nothing to say about the *other* segment, and the refresh at the end of the
/// click covers it.
fn preview_strip(flow: crate::audio::Flow, icon: Option<IconId>, muted: bool) {
    use crate::audio::Flow;

    let Some(mut icons) = strip_applied() else {
        return;
    };
    let glyph = icon.map(crate::taskbar::strip_glyph);
    match flow {
        Flow::Output => {
            icons.output = glyph.unwrap_or(icons.output);
            icons.output_muted = muted;
        }
        Flow::Input => {
            icons.input = glyph.unwrap_or(icons.input);
            icons.input_muted = muted;
        }
    }
    push_strip(icons);
}

/// The icon for a device: a per-device override from the config if there is one,
/// otherwise the form-factor default.
fn icon_of(device: &crate::audio::Device, config: &Config) -> IconId {
    config
        .icon_for(&device.id.0)
        .unwrap_or_else(|| icons::default_icon(device.form_factor, &device.friendly_name))
}

/// The strip state we last asked the TAP for, so an identical restyle can be
/// dropped before it costs a rebuild.
///
/// A static because [`refresh`] is reached from the flyout and the click handler as
/// well as the message loop, none of which own tray state — and it is only ever
/// touched from the tray thread. Deliberately *not* extended to the tray icon:
/// that write is how [`build_tray`] proves the shell accepted the icon at all, and
/// re-applying it is the retry when the taskbar comes back.
static STRIP_APPLIED: std::sync::Mutex<Option<crate::taskbar::StripIcons>> =
    std::sync::Mutex::new(None);

/// Forget what the strip is showing, so the next [`refresh`] posts a restyle even
/// if the devices have not changed.
///
/// For when the strip is gone rather than wrong: after an Explorer restart the new
/// TAP starts from the init data, and after a revert there is nothing there at all.
fn invalidate_strip() {
    match STRIP_APPLIED.lock() {
        Ok(mut guard) => *guard = None,
        Err(poisoned) => *poisoned.into_inner() = None,
    }
}

fn try_refresh(backend: &WasapiBackend, tray: &TrayIcon, config: &Config) -> Result<()> {
    Endpoint::read(backend, config, crate::audio::Flow::Output).apply_to(tray)
}

/// The current default of one flow, as both surfaces need it.
struct Endpoint {
    name: String,
    icon: IconId,
    muted: bool,
}

impl Endpoint {
    /// One `enumerate_flow`, one `default_of` and one `is_muted` — where working
    /// this out per surface cost three enumerations and six `default_of`s for the
    /// same answer (measured at 119–383ms per refresh, on the critical path of
    /// every click).
    fn read(backend: &WasapiBackend, config: &Config, flow: crate::audio::Flow) -> Self {
        let default = backend.default_of(flow).ok().flatten();
        let Some(id) = default else {
            return Self { name: "Audio output".to_string(), icon: IconId::Unknown, muted: false };
        };
        let muted = backend.is_muted(&id).unwrap_or(false);
        let device = backend
            .enumerate_flow(flow)
            .ok()
            .and_then(|devices| devices.into_iter().find(|d| d.id == id));
        match device {
            Some(d) => Self {
                icon: icon_of(&d, config),
                name: d.friendly_name,
                muted,
            },
            None => Self { name: "Audio output".to_string(), icon: IconId::Unknown, muted },
        }
    }

    /// Put this endpoint on the notification icon: its glyph and its tooltip.
    fn apply_to(&self, tray: &TrayIcon) -> Result<()> {
        tray.set_icon(Some(icon_image(self.icon)?))?;
        tray.set_tooltip(Some(&tooltip_for(&self.name)))?;
        println!("refresh: default \"{}\" -> icon {:?}", self.name, self.icon);
        Ok(())
    }
}

/// Both defaults at once — what the strip draws, and what the tray icon shows.
struct Current {
    output: Endpoint,
    input: Endpoint,
}

impl Current {
    fn read(backend: &WasapiBackend, config: &Config) -> Self {
        use crate::audio::Flow;
        Self {
            output: Endpoint::read(backend, config, Flow::Output),
            input: Endpoint::read(backend, config, Flow::Input),
        }
    }

    /// The glyphs the taskbar strip should draw.
    ///
    /// Resolved from the same [`Endpoint`]s the tray icon uses, so all three of
    /// strip, icon and flyout agree. They did not before: the strip drew fixed
    /// Volume and Microphone glyphs while the flyout showed the device's own icon,
    /// so the same speaker appeared as a laptop in one place and a speaker in the
    /// other.
    fn strip_icons(&self) -> crate::taskbar::StripIcons {
        crate::taskbar::StripIcons {
            output: crate::taskbar::strip_glyph(self.output.icon),
            input: crate::taskbar::strip_glyph(self.input.icon),
            output_muted: self.output.muted,
            input_muted: self.input.muted,
            // Not part of an `Endpoint`, deliberately: "something is recording" is a
            // property of the microphone capability rather than of any one endpoint, and
            // it is answered from a cached atomic rather than by asking COM.
            input_recording: crate::audio::mic::in_use(),
        }
    }
}

/// The glyphs for the current defaults, for the injection's init data.
pub(crate) fn strip_icons(backend: &WasapiBackend, config: &Config) -> crate::taskbar::StripIcons {
    Current::read(backend, config).strip_icons()
}

fn icon_image(id: IconId) -> Result<Icon> {
    // Match the taskbar's monochrome tray icons: white glyph on a dark taskbar,
    // near-black on a light one. Render at the exact small-icon size for crispness.
    let tint = if taskbar_is_light() { [0x20, 0x20, 0x20] } else { [0xff, 0xff, 0xff] };
    let size = small_icon_size();
    let (rgba, w, h) = id.render(size, tint)?;
    Ok(Icon::from_rgba(rgba, w, h)?)
}

/// The DPI-scaled small-icon size Windows wants for the notification area.
fn small_icon_size() -> u32 {
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSMICON};
    let px = unsafe { GetSystemMetrics(SM_CXSMICON) };
    if px <= 0 { 16 } else { px as u32 }
}

/// A low-level mouse hook that turns wheel-over-taskbar into a volume change. The hook
/// callback stays trivial (it just posts [`WM_TASKBAR_SCROLL`] to the tray loop) so it
/// never trips the OS low-level-hook timeout. Unhooks on drop.
struct ScrollVolumeHook(HHOOK);

impl ScrollVolumeHook {
    fn install(tray_thread: u32) -> Option<Self> {
        TRAY_TID.store(tray_thread, Ordering::SeqCst);
        // Only the physical wheel reaches a low-level mouse hook. Precision-touchpad
        // two-finger scroll is routed by Windows straight to the hovered window and never
        // enters this stream at all — that half of the gesture comes in from the TAP's own
        // `PointerWheelChanged` handler instead (see the module docs).
        unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), None, 0) }
            .ok()
            .map(ScrollVolumeHook)
    }
}

impl Drop for ScrollVolumeHook {
    fn drop(&mut self) {
        unsafe {
            let _ = UnhookWindowsHookEx(self.0);
        }
    }
}

unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 && wparam.0 as u32 == WM_MOUSEWHEEL {
        let info = &*(lparam.0 as *const MSLLHOOKSTRUCT);
        if point_over_tray(info.pt) {
            let delta = (info.mouseData >> 16) as i16;
            let tid = TRAY_TID.load(Ordering::SeqCst);
            if tid != 0 && delta != 0 {
                let flow = crate::taskbar::flow_code(flow_at(info.pt));
                let _ = PostThreadMessageW(
                    tid,
                    WM_TASKBAR_SCROLL,
                    WPARAM(flow),
                    LPARAM(delta as isize),
                );
            }
            // Swallowed so the shell doesn't also scroll — and so XAML never sees it, which
            // is what keeps the TAP's own wheel handler from acting on the same notch.
            return LRESULT(1);
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

/// Which endpoint a wheel at this screen point belongs to.
///
/// The two buttons split the icon's slot down the middle — the pill is centred in it and its
/// halves are equal, 32 epx each (see the TAP's `decorate` module) — so the slot's midpoint is
/// the whole test. Anywhere else on the taskbar keeps the wheel's original meaning, the output
/// volume; so does a wheel over the icon when there is no strip, where there are no halves to
/// tell apart.
fn flow_at(pt: POINT) -> Flow {
    if !crate::taskbar::strip_is_up() {
        return Flow::Output;
    }
    match icon_rect() {
        Some(slot)
            if pt.x >= slot.left && pt.x < slot.right && pt.y >= slot.top && pt.y < slot.bottom =>
        {
            if pt.x < (slot.left + slot.right) / 2 {
                Flow::Output
            } else {
                Flow::Input
            }
        }
        _ => Flow::Output,
    }
}

/// Is the screen point over the taskbar / notification area (incl. the Win11 tray
/// overflow flyout)?
unsafe fn point_over_tray(pt: POINT) -> bool {
    let hwnd = WindowFromPoint(pt);
    if hwnd.is_invalid() {
        return false;
    }
    let root = GetAncestor(hwnd, GA_ROOT);
    let cls = window_class(root);
    matches!(
        cls.as_str(),
        "Shell_TrayWnd"
            | "Shell_SecondaryTrayWnd"
            | "NotifyIconOverflowWindow"
            | "TopLevelWindowForOverflowXamlIsland"
            | "Xaml_WindowedPopupClass"
    )
}

unsafe fn window_class(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    let n = GetClassNameW(hwnd, &mut buf);
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

/// Whether the Windows taskbar uses the light theme (registry `SystemUsesLightTheme`).
fn taskbar_is_light() -> bool {
    use windows::core::w;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};

    let mut data: u32 = 0; // default: dark taskbar
    let mut size = std::mem::size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
            w!("SystemUsesLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut data as *mut u32 as *mut core::ffi::c_void),
            Some(&mut size),
        )
    };
    status.0 == 0 && data == 1
}
