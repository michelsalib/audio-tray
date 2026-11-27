//! Clicks: the strip's gestures relayed by the TAP, the plain icon's, the middle clicks the mouse
//! hook takes, and the right-click menus.

use std::cell::Cell;
use std::time::{Duration, Instant};

use anyhow::Result;
use tray_icon::{MouseButton, MouseButtonState, TrayIconEvent};

use crate::audio::Flow;
use crate::config::{BellMiddleClick, Config, MiddleClick};
use crate::flyout;
use crate::taskbar::{self, Action, WM_TASKBAR_ACTION};

use super::messages::{pack_point, unpack_point, TRAY_CLICK_BATTERY_MENU, WM_TRAY_CLICK};
use super::{battery, open_flyout, post, quit, reschedule, with_tray, Tray, DRAINING};

thread_local! {
    /// The calendar's card to open as soon as the audio panel has closed.
    pub(super) static CALENDAR_NEXT: Cell<Option<crate::calendar::Panel>> = const { Cell::new(None) };
    /// When the clock or the bell last asked for a card, and which, to drop the same click's echo.
    static LAST_OPEN: Cell<Option<(Instant, crate::calendar::Panel)>> = const { Cell::new(None) };
}

/// Whether a click may act: not while the flyout is open or settling, since then it is the
/// click that dismissed it. Hides the readout either way — the click supersedes it.
fn click_allowed(what: &dyn std::fmt::Debug) -> bool {
    with_tray(|tray| {
        tray.osd.hide();
        let allowed = !tray.flyout_open && Instant::now() >= tray.reopen_guard;
        if !allowed {
            println!("tray: {what:?} ignored — that click dismissed the panel");
        }
        allowed
    })
    .unwrap_or(false)
}

/// A gesture on the injected strip, relayed by the TAP.
pub(super) fn on_action(action: Action) {
    let panel = match action {
        Action::OpenCalendar => Some(crate::calendar::Panel::Calendar),
        Action::OpenNotifications => Some(crate::calendar::Panel::Notifications),
        Action::OpenBatteries => Some(crate::calendar::Panel::Batteries),
        _ => None,
    };
    if let Some(panel) = panel {
        // One click can arrive twice (the mouse hook and the TAP's tap handler, for touch or pen):
        // opening toggles, so the second would close what the first opened.
        let now = Instant::now();
        let echo = LAST_OPEN.with(|last| last.replace(Some((now, panel)))).is_some_and(|(at, was)| was == panel && now - at < Duration::from_millis(300));
        if echo {
            return;
        }
        // The clock's click never reaches the audio panel (the hook takes it, or the shell's own
        // calendar would open), so it is not the click that closed it: the panel gives way and the
        // calendar opens once it has ([`open_flyout`]).
        if with_tray(|tray| tray.flyout_open).unwrap_or(false) {
            CALENDAR_NEXT.with(|next| next.set(Some(panel)));
            // The panel closes on losing the capture (same thread).
            let _ = unsafe { windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture() };
            return;
        }
        if let Some(config) = with_tray(|tray| {
            tray.osd.hide();
            tray.config.clone()
        }) {
            // Only a part that is Audio Tray's opens (a stale word, or a TAP from before the bell
            // stood alone, can name the other).
            let ours = match panel {
                crate::calendar::Panel::Calendar => config.calendar.enabled,
                crate::calendar::Panel::Notifications => config.calendar.bell,
                _ => true,
            };
            if ours {
                crate::calendar::open(&config, panel);
            } else {
                println!("tray: {panel:?} is Windows' own; left alone");
            }
        }
        return;
    }
    if !click_allowed(&action) {
        return;
    }
    // A segment's left click opens the panel (its middle click does what `[strip] middle_click`
    // says: [`on_middle_click`]).
    if matches!(action, Action::OpenPanel | Action::CycleOutput | Action::CycleInput) {
        // No anchor: the strip's rect is not ours to know; the flyout uses its default place.
        return open_flyout(None);
    }
    with_tray(|tray| {
        // Never propagated: COM against a session that can vanish mid-click must not end the loop.
        if let Err(e) = tray.taskbar_action(action) {
            eprintln!("taskbar: {action:?} failed ({e:#})");
        }
    });
}

impl Tray {
    /// Act on a gesture from the injected strip that is not a panel's (those are `on_action`'s): the
    /// music tile's transport.
    fn taskbar_action(&mut self, action: Action) -> Result<()> {
        use crate::music::smtc::Command;
        // Handed to the feed thread and not awaited: a slow media session must not stall this
        // thread. The strip catches up on the next poll.
        let command = match action {
            Action::MusicPrevious => Command::Previous,
            Action::MusicNext => Command::Next,
            Action::MusicPlayPause => Command::TogglePlayPause,
            _ => return Ok(()),
        };
        let Some(music) = self.music.as_ref() else {
            println!("taskbar: {action:?} ignored — the music half is not running");
            return Ok(());
        };
        music.command(command);
        Ok(())
    }
}

/// Whether a middle click on the bell is Audio Tray's: its notifications are on, and the click does
/// something (else it is Windows').
pub(super) fn bell_middle_taken(config: &Config) -> bool {
    crate::calendar::merges_notices(config) && config.calendar.bell_middle_click != BellMiddleClick::Nothing
}

/// A middle click on the bell (the hook's, [`super::WM_BELL_MIDDLE`]): Do not disturb on or off, or
/// every notification cleared, as `[calendar] bell_middle_click` says.
pub(super) fn on_bell_middle_click() {
    let Some(click) = with_tray(|tray| tray.config.calendar.bell_middle_click) else { return };
    match click {
        BellMiddleClick::DoNotDisturb => {
            let on = crate::calendar::focus::toggle();
            println!("calendar: Do not disturb {} by a middle click on the bell", if on { "on" } else { "off" });
            // The bell's cross and the panel's moon follow.
            crate::calendar::quiet_changed();
            reschedule();
        }
        BellMiddleClick::ClearAll => {
            crate::calendar::notifications::clear();
            if let Some(config) = with_tray(|tray| tray.config.clone()) {
                crate::calendar::notifications_changed(&config);
            }
            reschedule();
        }
        BellMiddleClick::Nothing => {}
    }
}

/// A middle click on the strip's output or input button (the hook's, [`super::WM_STRIP_MIDDLE`]):
/// mute it, or switch it to its next device, with or without a muted stop, as
/// `[strip] middle_click` says.
pub(super) fn on_middle_click(flow: Flow) {
    if !click_allowed(&"middle click") {
        return;
    }
    with_tray(|tray| {
        let done = match tray.config.strip.middle_click {
            MiddleClick::Mute => tray.toggle_endpoint_mute(flow),
            MiddleClick::Switch => tray.cycle(flow, false),
            MiddleClick::SwitchMute => tray.cycle(flow, true),
        };
        // Never propagated: COM against endpoints that can vanish mid-click must not end the loop.
        if let Err(e) = done {
            eprintln!("taskbar: middle click on {flow:?} failed ({e:#})");
            // The strip may be previewing a switch that failed: put the truth back.
            tray.strip = None;
            tray.refresh();
        }
    });
}

/// A click on the plain notification icon. Right click opens the menu ([`context_menu`]); left
/// click opens the flyout, unless a strip is up — the shell invokes the icon under the strip, so a
/// segment's click also arrives here, and the strip's own handler is the one that opens it.
pub(super) fn on_tray_click(button: usize, packed: isize) {
    if button == TRAY_CLICK_BATTERY_MENU {
        return battery::menu();
    }
    if button == 1 && taskbar::buttons_up() {
        println!("tray: left click on the strip — its own handler opens the panel");
        return;
    }
    if !click_allowed(&"tray click") {
        return;
    }
    let (cx, bottom) = unpack_point(packed);
    if button == 2 {
        // A right click is a menu, as on Windows' own buttons beside it (the clock, the bell).
        return context_menu();
    }
    open_flyout(Some(flyout::Anchor { cx, bottom }));
}

/// The strip's (and the plain icon's) right-click menu, at the pointer: Windows' sound settings,
/// Audio Tray's, and Quit (no longer in the panel's footer).
fn context_menu() {
    use crate::lang::tr;
    const SOUND: usize = 1;
    const SETTINGS: usize = 2;
    const QUIT: usize = 3;
    // In Windows' language, as the panels and Settings.
    let items = [
        (SOUND, tr("Sound settings", "Paramètres du son"), false),
        (SETTINGS, tr("Audio Tray settings", "Paramètres d’Audio Tray"), false),
        (QUIT, tr("Quit Audio Tray", "Quitter Audio Tray"), true),
    ];
    match popup_menu(&items) {
        Some(SOUND) => {
            crate::win::shell_open("ms-settings:sound", None);
        }
        Some(SETTINGS) => {
            post(crate::settings::WM_OPEN_SETTINGS, crate::settings::OPEN_TASKBAR, 0);
        }
        Some(QUIT) => quit(),
        _ => {}
    }
}

/// A menu of `items` (its id, its label, a separator before it) at the pointer, as Windows' own
/// buttons' right-click menus; the id chosen, if any. One at a time: a second right click while one
/// is up opens none.
pub(super) fn popup_menu(items: &[(usize, &str, bool)]) -> Option<usize> {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, PostMessageW, SetForegroundWindow, TrackPopupMenu, MF_SEPARATOR, MF_STRING,
        TPM_BOTTOMALIGN, TPM_CENTERALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_NULL,
    };
    thread_local! {
        /// A menu is up: a second right click meanwhile opens no second one.
        static MENU_OPEN: Cell<bool> = const { Cell::new(false) };
    }
    let owner = taskbar::receiver()?;
    if MENU_OPEN.with(|open| open.replace(true)) {
        return None;
    }
    let labels: Vec<HSTRING> = items.iter().map(|(_, label, _)| HSTRING::from(*label)).collect();
    let Ok(menu) = (unsafe { CreatePopupMenu() }) else {
        MENU_OPEN.with(|open| open.set(false));
        return None;
    };
    for ((id, _, separated), label) in items.iter().zip(&labels) {
        if *separated {
            let _ = unsafe { AppendMenuW(menu, MF_SEPARATOR, 0, None) };
        }
        let _ = unsafe { AppendMenuW(menu, MF_STRING, *id, PCWSTR(label.as_ptr())) };
    }
    let mut at = POINT::default();
    let _ = unsafe { GetCursorPos(&mut at) };
    // Without the foreground, the menu would not close on a click elsewhere.
    let _ = unsafe { SetForegroundWindow(owner) };
    // Its modal loop drains the queue as the panel's does: the strip, the banners and the
    // hub stay live while it is open.
    let was_draining = DRAINING.with(|draining| draining.replace(false));
    let chosen = unsafe { TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN | TPM_CENTERALIGN, at.x, at.y, None, owner, None) };
    DRAINING.with(|draining| draining.set(was_draining));
    // Windows' advice for a menu of a window in the background: a message after it, so the
    // next click opens it again rather than only dismissing it.
    let _ = unsafe { PostMessageW(Some(owner), WM_NULL, Default::default(), Default::default()) };
    let _ = unsafe { DestroyMenu(menu) };
    MENU_OPEN.with(|open| open.set(false));
    let chosen = chosen.0 as usize;
    (chosen != 0).then_some(chosen)
}

/// tray-icon's event handler (called on this thread, from its own window's procedure): turn an
/// icon click into a message, so it is handled — or dropped — like every other gesture.
pub(super) fn on_tray_event(event: TrayIconEvent) {
    // Audio Tray's own battery icon: its click opens the batteries, its right click its menu (from
    // the queue, as the audio icon's).
    if let TrayIconEvent::Click { id, button, button_state: MouseButtonState::Up, .. } = &event {
        if id.as_ref() == battery::ICON_ID {
            match button {
                MouseButton::Left => post(WM_TASKBAR_ACTION, tap_proto::ACTION_OPEN_BATTERIES, 0),
                MouseButton::Right => post(WM_TRAY_CLICK, TRAY_CLICK_BATTERY_MENU, 0),
                _ => false,
            };
            return;
        }
    }
    if let TrayIconEvent::Click { button, button_state: MouseButtonState::Up, rect, .. } = event {
        let code = match button {
            MouseButton::Left => 1,
            MouseButton::Right => 2,
            _ => return,
        };
        let cx = (rect.position.x + rect.size.width as f64 / 2.0) as i32;
        post(WM_TRAY_CLICK, code, pack_point(cx, rect.position.y as i32));
    }
}
