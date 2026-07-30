//! System-tray icon, click handling, and the Win32 message loop that drives them.
//!
//! The tray owns no window of its own — `tray-icon` provides the icon and posts click
//! events through a global channel that we drain after each dispatched message. Either
//! button opens our acrylic control flyout (volume, mute, output/input switching,
//! per-device icons, and a More section — see [`crate::flyout`]).

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use anyhow::Result;
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetAncestor, GetClassNameW, GetMessageW, PeekMessageW,
    PostQuitMessage, PostThreadMessageW, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx,
    WindowFromPoint, GA_ROOT, HHOOK, MSG, MSLLHOOKSTRUCT, PM_REMOVE, WH_MOUSE_LL, WM_APP,
    WM_MOUSEWHEEL,
};

use crate::audio::wasapi::WasapiBackend;
use crate::audio::{notify, AudioBackend};
use crate::config::Config;
use crate::flyout;
use crate::icons::{self, IconId};

/// Posted (from the mouse hook) when the user scrolls over the taskbar/tray; wParam is
/// 1 for volume up, 0 for down.
const WM_VOLUME_STEP: u32 = WM_APP + 3;

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

    // Receives clicks from the injected strip. Created unconditionally and
    // cheaply: it costs one message-only window, and having it always present
    // means enabling the feature needs no restart to become clickable.
    // Also how we learn that Explorer restarted — see `create_receiver`.
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

    // Opt-in and off by default; a failure here never blocks the tray.
    //
    // Deliberately *before* the tray icon exists. Injecting afterwards looks
    // tidier — the TAP would find the icon on its first pass instead of waiting
    // for it — but it means the decoration happens inside the initial visual-tree
    // replay, while the shell is still building that icon's subtree, and
    // `put_Content` there simply never returns. Measured: the log stops at
    // "setting content on …" and the strip never appears. Letting the icon arrive
    // as a live delta afterwards is the ordering that works.
    crate::taskbar::apply_at_startup(config.taskbar.enabled);

    // At logon audio-tray can start ahead of `explorer.exe`, and registering the
    // icon then fails silently and permanently — hence the retry inside.
    let tray = build_tray(&backend, &config)?;

    // Register endpoint-change notifications that wake this thread's message loop.
    let thread_id = unsafe { GetCurrentThreadId() };
    let _notifications = notify::register(thread_id)?;

    // Scroll over the taskbar/tray to change the default device's volume.
    let _volume_hook = ScrollVolumeHook::install(thread_id);

    let devices = backend.enumerate().map(|d| d.len()).unwrap_or(0);
    println!("tray: created ({devices} output device(s)); either button opens the panel.");

    let tray_rx = TrayIconEvent::receiver();
    // The click that dismisses an open flyout (it has mouse capture) is also reported by
    // the shell as a fresh tray click. This guard ignores tray clicks for a brief window
    // after a flyout closes, so a second click on the icon reads as "close", not "close
    // then immediately reopen".
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
            // Scroll-over-tray → nudge the default device's volume.
            if msg.message == WM_VOLUME_STEP {
                let _ = backend.step_volume(msg.wParam.0 != 0);
                continue;
            }
            // A click on the injected taskbar strip, relayed by the TAP running
            // inside Explorer. Only ever additive — if the feature is off, this
            // message simply never arrives.
            if msg.message == crate::taskbar::WM_TASKBAR_ACTION {
                if let Some(action) = crate::taskbar::Action::from_code(msg.wParam.0) {
                    handle_taskbar_action(&backend, &mut config, &tray, action)?;
                }
                continue;
            }
            // Explorer restarted, so the taskbar is new and empty and the TAP
            // died with the old process. Relayed here by the receiver's window
            // procedure, which is the only place the shell's broadcast is
            // visible. `tray-icon` re-registers our plain notification icon on
            // the same signal, independently.
            if msg.message == crate::taskbar::WM_TASKBAR_RESTARTED {
                crate::taskbar::apply_at_restart(config.taskbar.enabled);
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
                    // Both buttons open the same panel; the strip's own segments
                    // are what differentiate a left click, once zone routing lands.
                    if matches!(button, MouseButton::Left | MouseButton::Right) {
                        let anchor = flyout::Anchor {
                            cx: (rect.position.x + rect.size.width as f64 / 2.0) as i32,
                            bottom: rect.position.y as i32,
                        };
                        handle_flyout(&backend, &mut config, &tray, Some(anchor))?;
                        reopen_guard = Instant::now() + Duration::from_millis(350);
                    }
                }
            }
        }
    }
    Ok(())
}

/// Act on a click from the injected taskbar strip.
///
/// Left click cycles that endpoint to the next active device; right click opens
/// the full panel. Cycling wraps, and a single-device system is a deliberate
/// no-op rather than an error — there is nothing to switch to.
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
    if devices.len() < 2 {
        return Ok(());
    }
    let current = backend.default_of(flow)?;
    // Unknown current default starts the cycle at the first device rather than
    // failing — the user asked for "something else", and any switch satisfies it.
    let next = current
        .and_then(|id| devices.iter().position(|d| d.id == id))
        .map_or(0, |at| (at + 1) % devices.len());
    backend.set_default_of(&devices[next].id)?;
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
    // Opt-in Explorer integration. Purely additive: the tray icon above is already
    // registered and keeps working whatever happens here.
    if let Some(enabled) = outcome.taskbar_toggled {
        if enabled {
            match crate::taskbar::enable() {
                Ok(()) => eprintln!("taskbar: controls enabled"),
                Err(e) => eprintln!("taskbar: could not enable ({e:#})"),
            }
        } else {
            eprintln!("taskbar: {}", crate::taskbar::disable());
        }
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
fn build_tray(backend: &impl AudioBackend, config: &Config) -> Result<TrayIcon> {
    const FIRST_GAP: Duration = Duration::from_millis(500);
    const MAX_GAP: Duration = Duration::from_secs(5);
    /// How often to repeat the "still waiting" line, in attempts at `MAX_GAP`.
    const NAG_EVERY: u32 = 12;

    let (_, initial_icon) = resolve_current(backend, config);
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
fn refresh(backend: &impl AudioBackend, tray: &TrayIcon, config: &Config) {
    if let Err(e) = try_refresh(backend, tray, config) {
        eprintln!("tray: could not update the icon, will retry when the taskbar is back ({e:#})");
    }
}

fn try_refresh(backend: &impl AudioBackend, tray: &TrayIcon, config: &Config) -> Result<()> {
    let (name, icon_id) = resolve_current(backend, config);
    tray.set_icon(Some(icon_image(icon_id)?))?;
    tray.set_tooltip(Some(&tooltip_for(&name)))?;
    println!("refresh: default \"{name}\" -> icon {icon_id:?}");
    Ok(())
}

/// Resolve the current default device to its display name and the icon to show for it:
/// a per-device config override wins, otherwise `default_icon` picks a starting glyph.
fn resolve_current(backend: &impl AudioBackend, config: &Config) -> (String, IconId) {
    let device = backend
        .current_default()
        .ok()
        .and_then(|cur| backend.enumerate().ok()?.into_iter().find(|d| d.id == cur));
    match device {
        Some(d) => {
            let icon = config
                .icon_for(&d.id.0)
                .unwrap_or_else(|| icons::default_icon(d.form_factor, &d.friendly_name));
            (d.friendly_name, icon)
        }
        None => ("Audio output".to_string(), IconId::Unknown),
    }
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

/// A low-level mouse hook that turns wheel-over-taskbar into a volume step. The hook
/// callback stays trivial (it just posts [`WM_VOLUME_STEP`] to the tray loop) so it
/// never trips the OS low-level-hook timeout. Unhooks on drop.
struct ScrollVolumeHook(HHOOK);

impl ScrollVolumeHook {
    fn install(tray_thread: u32) -> Option<Self> {
        TRAY_TID.store(tray_thread, Ordering::SeqCst);
        // Note: only physical mouse-wheel scroll reaches a low-level mouse hook.
        // Precision-touchpad two-finger scroll is routed by Windows' Direct Manipulation
        // straight to the hovered window and never enters this stream — touchpad users
        // use the volume slider in the native sound flyout (left-click) instead.
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
                let up: usize = if delta > 0 { 1 } else { 0 };
                let _ = PostThreadMessageW(tid, WM_VOLUME_STEP, WPARAM(up), LPARAM(0));
            }
            return LRESULT(1); // swallow so the shell doesn't also scroll
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
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
