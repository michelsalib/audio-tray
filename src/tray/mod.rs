//! System-tray icon, click handling, and the Win32 message loop that drives them.
//!
//! The tray owns no window of its own — `tray-icon` provides the icon and posts click
//! events through a global channel that we drain after each dispatched message. Left-click
//! opens our acrylic control flyout (volume, mute, output/input switching, per-device
//! icons — see [`crate::flyout`]); right-click opens the tiny quick menu (Sound settings +
//! Quit).

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
use crate::flyout::{self, Trigger};
use crate::icons::{self, IconId};

/// Posted (from the mouse hook) when the user scrolls over the taskbar/tray; wParam is
/// 1 for volume up, 0 for down.
const WM_VOLUME_STEP: u32 = WM_APP + 3;

/// Tray thread id, shared with the low-level mouse hook (which is a bare `fn`).
static TRAY_TID: AtomicU32 = AtomicU32::new(0);

/// Build the tray icon and run the message loop until the user quits.
pub fn run(backend: WasapiBackend) -> Result<()> {
    let mut config = Config::load();

    let (_, initial_icon) = resolve_current(&backend, &config);
    // Left-click opens our acrylic control flyout; right-click opens the quick menu
    // (handled via TrayIconEvent — we deliberately don't hand a menu to tray-icon).
    let tray = TrayIconBuilder::new()
        .with_tooltip("Audio output")
        .with_icon(icon_image(initial_icon)?)
        .build()?;

    refresh(&backend, &tray, &config)?;

    // Register endpoint-change notifications that wake this thread's message loop.
    let thread_id = unsafe { GetCurrentThreadId() };
    let _notifications = notify::register(thread_id)?;

    // Scroll over the taskbar/tray to change the default device's volume.
    let _volume_hook = ScrollVolumeHook::install(thread_id);

    let devices = backend.enumerate().map(|d| d.len()).unwrap_or(0);
    println!("tray: created ({devices} output device(s)); left = panel, right = menu.");

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
                refresh(&backend, &tray, &config)?;
                continue;
            }
            // Scroll-over-tray → nudge the default device's volume.
            if msg.message == WM_VOLUME_STEP {
                let _ = backend.step_volume(msg.wParam.0 != 0);
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);

            // Tray icon clicks: left → control panel; right → quick menu. Both centre on
            // the icon, opening just above it.
            while let Ok(ev) = tray_rx.try_recv() {
                if let TrayIconEvent::Click {
                    button, button_state: MouseButtonState::Up, rect, ..
                } = ev
                {
                    if Instant::now() < reopen_guard {
                        continue; // this is the click that just closed the flyout
                    }
                    let trigger = match button {
                        MouseButton::Left => Some(Trigger::LeftClick),
                        MouseButton::Right => Some(Trigger::RightClick),
                        _ => None,
                    };
                    if let Some(trigger) = trigger {
                        let anchor = flyout::Anchor {
                            cx: (rect.position.x + rect.size.width as f64 / 2.0) as i32,
                            bottom: rect.position.y as i32,
                        };
                        handle_flyout(&backend, &mut config, &tray, Some(anchor), trigger)?;
                        reopen_guard = Instant::now() + Duration::from_millis(350);
                    }
                }
            }
        }
    }
    Ok(())
}

/// Show the flyout (control panel or quick menu) and apply its outcome. Device switching
/// and volume/mute happen live inside the flyout; here we only persist icon changes and
/// honour a Quit. The tray icon is refreshed whenever the config changed (a per-device
/// icon may be the current default's).
fn handle_flyout(
    backend: &WasapiBackend,
    config: &mut Config,
    tray: &TrayIcon,
    anchor: Option<flyout::Anchor>,
    trigger: Trigger,
) -> Result<()> {
    let outcome = flyout::show(backend, config, anchor, trigger);
    if outcome.config_changed {
        if let Err(e) = config.save() {
            eprintln!("save config failed: {e:#}");
        }
    }
    // The tray icon tracks the default output; switching it inside the flyout consumes the
    // endpoint-change notifications, so refresh here when the config or the default changed.
    if outcome.config_changed || outcome.output_changed {
        refresh(backend, tray, config)?;
    }
    if outcome.restart {
        restart_app();
    }
    if outcome.quit {
        unsafe { PostQuitMessage(0) };
    }
    Ok(())
}

/// Relaunch the (already self-updated on disk) exe as a fresh process, then quit this one so
/// the newer build takes over. Best-effort: if the relaunch fails we stay running rather
/// than leaving the user with no tray.
fn restart_app() {
    match std::env::current_exe() {
        Ok(exe) => match std::process::Command::new(exe).spawn() {
            Ok(_) => unsafe { PostQuitMessage(0) },
            Err(e) => eprintln!("restart: failed to relaunch: {e:#}"),
        },
        Err(e) => eprintln!("restart: current_exe() failed: {e:#}"),
    }
}

/// Update the tray icon + tooltip to reflect the current default device.
fn refresh(backend: &impl AudioBackend, tray: &TrayIcon, config: &Config) -> Result<()> {
    let (name, icon_id) = resolve_current(backend, config);
    tray.set_icon(Some(icon_image(icon_id)?))?;
    tray.set_tooltip(Some(&name))?;
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
