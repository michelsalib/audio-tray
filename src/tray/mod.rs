//! The tray: notification icon, the hidden message window every app message goes to, and the
//! state behind them.
//!
//! **One window, one procedure.** The TAP, the endpoint and microphone watchers, the music feed,
//! the wheel hook and tray-icon's click handler all post to the receiver window
//! ([`crate::taskbar::create_receiver`]); [`wndproc`] queues each message and [`drain`] handles
//! them, so any pump that dispatches (this loop or the flyout's modal one) delivers them.
//!
//! **Re-entrancy.** An STA pumps during outgoing COM calls, so a message can arrive while a
//! handler is running. Handlers never run nested: a nested arrival only queues, and the outer
//! [`drain`] picks it up. The flyout is the one deliberate exception ([`open_flyout`]): no state
//! is borrowed across its modal loop, so draining is re-enabled for it, and [`next_message`] holds
//! back what has to wait for it to close.
//!
//! Left click on a strip segment opens the flyout ([`clicks::on_action`]), as does a right click or
//! either click on the plain icon when there is no strip; a middle click mutes that endpoint or
//! switches it to its next device, as `[strip] middle_click` says ([`clicks::on_middle_click`]).
//! Scrolling a button changes that endpoint's volume: the wheel through [`hook`], a precision
//! touchpad through the TAP, both as [`WM_TASKBAR_SCROLL`]; the level is shown by [`crate::osd`].
//!
//! The rest is split by subject: [`messages`] (ids, timers), [`clicks`], [`endpoints`] (the
//! defaults, the strip, the mute watches), [`icon`] (the notification icon), [`battery`] (the
//! batteries' icon) and [`hook`] (the mouse and keyboard hooks).

mod clicks;
pub(crate) mod endpoints;
mod hook;
mod icon;
mod messages;
pub(crate) mod battery;

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::atomic::Ordering::SeqCst;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64};
use std::time::{Duration, Instant};

use anyhow::Result;
use tray_icon::{TrayIcon, TrayIconEvent};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{DefWindowProcW, PostMessageW, PostQuitMessage, WHEEL_DELTA, WM_TIMER};

use crate::audio::wasapi::WasapiBackend;
use crate::audio::{mic, notify, sessions, Flow};
use crate::config::Config;
use crate::flyout;
use crate::osd::Osd;
use crate::taskbar::{self, Action, StripIcons, WM_MUSIC_PROGRESS, WM_TASKBAR_ACTION, WM_TASKBAR_RESTARTED, WM_TASKBAR_SCROLL};

use messages::{
    is_ours, kill_timer, set_timer, BATTERY_RETRY_TIMER, CALENDAR_TIMER, PREWARM_TIMER, WM_APPLY_SCROLL, WM_BATTERY_SYNC, WM_BELL_MIDDLE, WM_DISPLAY_CHANGED,
    WM_ENDPOINT_VOLUME, WM_ICON_CHECK, WM_ICON_RECT_CHECK, WM_POWER_CHANGED, WM_STRIP_MIDDLE, WM_TRAY_CLICK,
};

pub use icon::TRAY_MARKER;
pub(crate) use endpoints::restyle_strip;
pub(crate) use messages::WM_BANNER_LISTENING;

/// Volume change per wheel notch — 2%, the step Windows' own volume keys take.
pub(crate) const SCROLL_STEP: f32 = 0.02;

/// How long a burst of notifications has to settle before the closed panel is drawn ahead.
const PREWARM_AFTER_MS: u32 = 2000;

/// After the flyout closes, ignore clicks for this long: the dismissing click's release half
/// arrives as a strip `Tapped` or tray-icon click after the panel has gone.
const SETTLE: Duration = Duration::from_millis(350);

/// The message window, for producers on other threads.
static WINDOW: AtomicIsize = AtomicIsize::new(0);

/// The bell's middle on the screen, when measured (a banner going to the list flies there).
pub(crate) fn bell_point() -> Option<windows::Win32::Foundation::POINT> {
    hook::bell_point()
}

/// Post to the tray's message window from any thread. `false` when there is no tray (the dev CLI
/// modes) or the post failed.
pub(crate) fn post(message: u32, wparam: usize, lparam: isize) -> bool {
    let raw = WINDOW.load(SeqCst);
    raw != 0
        && unsafe { PostMessageW(Some(HWND(raw as *mut _)), message, WPARAM(wparam), LPARAM(lparam)) }.is_ok()
}

/// [`post`] after `delay`, from a short-lived thread (the tray thread must keep pumping).
fn post_after(message: u32, wparam: usize, delay: Duration) {
    std::thread::spawn(move || {
        std::thread::sleep(delay);
        post(message, wparam, 0);
    });
}

/// A "the world changed, go and look" message, coalesced: at most one is queued at a time, and
/// the handler re-arms it before reading the state, so the last change is never missed.
pub(crate) struct Wake {
    message: u32,
    queued: AtomicBool,
}

impl Wake {
    pub(crate) const fn new(message: u32) -> Self {
        Self { message, queued: AtomicBool::new(false) }
    }

    /// Ask for one handling of this message; free when one is already queued.
    pub(crate) fn post(&self) {
        self.post_with(|message| post(message, 0, 0));
    }

    fn post_with(&self, send: impl FnOnce(u32) -> bool) {
        if !self.queued.swap(true, SeqCst) && !send(self.message) {
            self.queued.store(false, SeqCst);
        }
    }

    /// Called by the handler, before it reads the state.
    pub(crate) fn take(&self) {
        self.queued.store(false, SeqCst);
    }
}

/// An endpoint changed (one switch fires a callback per role: three per switch).
pub(crate) static AUDIO_CHANGED: Wake = Wake::new(notify::WM_AUDIO_REFRESH);
/// An app took or released the microphone.
pub(crate) static MIC_CHANGED: Wake = Wake::new(mic::WM_MIC_CHANGED);

/// Scroll deltas folded per direction until [`WM_APPLY_SCROLL`] applies them. A touchpad posts
/// tens of sub-notch deltas per gesture; one COM round each would fall behind the finger, so
/// whatever arrives while a nudge is in flight is summed into the next one.
#[derive(Default)]
struct ScrollTotals {
    totals: [i32; 2],
    queued: bool,
}

impl ScrollTotals {
    /// Fold one delta in; `true` when the caller must post [`WM_APPLY_SCROLL`].
    fn add(&mut self, flow_code: usize, delta: i32) -> bool {
        self.totals[flow_code.min(1)] += delta;
        !std::mem::replace(&mut self.queued, true)
    }

    /// The notches per [`crate::taskbar::flow_code`], resetting the batch.
    fn take(&mut self) -> [f32; 2] {
        self.queued = false;
        std::mem::take(&mut self.totals).map(|delta| delta as f32 / WHEEL_DELTA as f32)
    }
}

/// Everything the tray thread owns.
struct Tray {
    backend: Rc<WasapiBackend>,
    config: Config,
    icon: TrayIcon,
    osd: Osd,
    /// What the strip was last asked to show, so an identical restyle (which costs the TAP a
    /// rebuild) is dropped. `None` = unknown, post the next one.
    strip: Option<StripIcons>,
    /// Dropping it tears the music feature down; see [`run`]'s teardown.
    music: Option<crate::music::Handle>,
    reopen_guard: Instant,
    flyout_open: bool,
    scroll: ScrollTotals,
    /// Volume/mute subscriptions on the current default output and input, with the mute last
    /// seen: a mute toggled outside the app (a media key) must reach the icon and the strip.
    /// Re-armed by [`Tray::refresh`] when a default changes.
    mute_watches: Vec<endpoints::MuteWatch>,
    /// Coalesces [`WM_ENDPOINT_VOLUME`]: a slider drag or mic auto-gain fires tens per second.
    mute_pending: std::sync::Arc<AtomicBool>,
    /// What every source has reported, folded; the surfaces render it.
    hub: crate::hub::State,
    /// Session-created notifications on the default output, and which output that is.
    sessions: Option<(crate::audio::DeviceId, sessions::Watch)>,
    /// The per-keyboard layout switcher, while `[keyboards] enabled`.
    keyboards: Option<crate::keyboards::Switcher>,
    /// The battery source, while `[batteries] enabled`.
    batteries: Option<crate::batteries::Watch>,
    /// The default output's name, the tooltip's first line.
    tooltip_device: String,
    /// The batteries' icon on the taskbar: Windows' own, or Audio Tray's.
    battery: battery::BatteryIcons,
}

thread_local! {
    static TRAY: RefCell<Option<Tray>> = const { RefCell::new(None) };
    static INBOX: RefCell<VecDeque<Queued>> = const { RefCell::new(VecDeque::new()) };
    static DRAINING: Cell<bool> = const { Cell::new(false) };
}

#[derive(Clone, Copy)]
struct Queued {
    message: u32,
    wparam: usize,
    lparam: isize,
}

/// Run `f` on the tray state; `None` outside [`run`]. Borrows are only taken by handlers, which
/// never nest (see the module docs), so this does not contend.
fn with_tray<R>(f: impl FnOnce(&mut Tray) -> R) -> Option<R> {
    TRAY.with(|cell| cell.try_borrow_mut().ok().and_then(|mut tray| tray.as_mut().map(f)))
}

/// Build the tray icon and run the message loop until the user quits; `settings` opens the settings
/// window once it is up.
pub fn run(backend: WasapiBackend, settings: bool) -> Result<()> {
    let mut config = Config::load();
    crate::config::set_motion(config.motion);
    // The pill where it was put on the screens plugged in now; a config from before places were
    // remembered keeps its place for them.
    let displays = crate::win::displays();
    let first = config.pill.places.is_empty();
    if first {
        config.pill.remember(&displays);
    }
    if config.pill.recall(&displays) || first {
        if let Err(e) = config.save() {
            eprintln!("pill: could not save its place ({e:#})");
        }
    }

    // First: everything below posts here, and the TAP needs a receiver before the strip exists.
    let window = taskbar::create_receiver(Some(wndproc))?;
    WINDOW.store(window.0 as isize, SeqCst);
    println!("taskbar: click receiver window {:?}", window.0);
    mic::in_use(); // start the watcher, so a recording that starts during startup is heard

    // Before the tray icon exists: decorating inside the initial visual-tree replay, while the
    // shell is still building the icon's subtree, hangs `put_Content` (see FINDINGS.md).
    // A TAP staged by an update that could not land it (the old one was loaded) is placed now:
    // the set-aside rename works on a loaded DLL, so this needs no Explorer restart.
    crate::update::place_staged_tap();
    if config.injects() {
        taskbar::apply_at_startup(endpoints::current_strip_icons(&backend, &config), &config);
    } else {
        // Opt-in: a strip left by an earlier run (or another build) comes off.
        taskbar::revert(0);
    }

    // At logon audio-tray can start ahead of `explorer.exe`; `build_tray` retries until it takes.
    let icon = icon::build_tray(&backend, &config)?;
    TrayIconEvent::set_event_handler(Some(clicks::on_tray_event));
    let _notifications = notify::register()?;
    let _hooks = hook::Hooks::spawn();

    let devices = backend.enumerate_flow(Flow::Output).map(|d| d.len()).unwrap_or(0);
    let gestures = if taskbar::buttons_up() {
        "left click opens the panel, middle click mutes or switches, right click opens the menu"
    } else {
        "no strip — left click opens the panel, right click the menu"
    };
    println!("tray: created ({devices} output device(s)); {gestures}.");

    // The passive surface; the app works without it.
    if let Err(e) = crate::pill::create(&config.pill) {
        eprintln!("pill: unavailable ({e:#})");
    }

    // On a thread of its own: every SMTC call blocks on an async operation, which deadlocks this
    // STA (see `music::on_mta_thread`). `None` = off in config, or SMTC would not open.
    let music = crate::music::spawn(&config.music, config.tile_shown());
    if music.is_some() {
        println!("music: following YouTube Music");
    }

    let keyboards = config.keyboards.enabled.then(|| crate::keyboards::Switcher::spawn(&config.keyboards)).flatten();
    let batteries = if config.batteries.enabled { crate::batteries::Watch::spawn() } else { None };
    let mut hub = crate::hub::State::default();
    hub.set_low_battery_at(config.batteries.warn_at());

    crate::calendar::focus::recover();
    follow_config(None, &config);
    if crate::calendar::takes_clock_button(&config) {
        // The first open finds its fonts loaded and its pictures decoded.
        prewarm_later(10_000);
    }
    // Windows says when the charge, the power source or the energy saver changes.
    crate::batteries::watch_power(window);
    let integrations = config.clone();

    TRAY.with(|cell| {
        *cell.borrow_mut() = Some(Tray {
            backend: Rc::new(backend),
            config,
            icon,
            osd: Osd::new(Some(window)),
            strip: None,
            music,
            reopen_guard: Instant::now(),
            flyout_open: false,
            scroll: ScrollTotals::default(),
            mute_watches: Vec::new(),
            mute_pending: std::sync::Arc::new(AtomicBool::new(false)),
            hub,
            sessions: None,
            keyboards,
            batteries,
            tooltip_device: String::new(),
            battery: battery::BatteryIcons::default(),
        })
    });
    with_tray(Tray::refresh);
    post(WM_BATTERY_SYNC, 0, 0);
    reschedule();
    if settings {
        post(crate::settings::WM_OPEN_SETTINGS, 0, 0);
    }

    // The integrations' handles are waited on beside the message queue (see `wait`).
    crate::integrations::start(&integrations);
    crate::wait::run();
    crate::integrations::stop();

    crate::calendar::close();
    // Windows' banners back: Audio Tray is no longer there to show its own.
    crate::calendar::takeover::apply(false);
    // Do not disturb Audio Tray set ends with it.
    crate::calendar::focus::release();
    crate::settings::destroy();
    crate::pill::destroy();
    // The progress bar sits on *another app's* button, so it has to come off here, while this
    // thread still pumps; dropping the music handle below is what stops the feed re-adding it.
    taskbar::clear_player_progress();
    WINDOW.store(0, SeqCst);
    drop(TRAY.with(|cell| cell.borrow_mut().take()));
    Ok(())
}

/// What follows the config outside the tray's own state: the hooks (which clicks and shortcuts are
/// Audio Tray's), the notifications' reader and the focus watch, Windows' bell and banners. At
/// startup (`before` is `None`) everything is set; after Settings, what only needs doing on a
/// change (showing the bell, taking the banners) is done only when it changed.
fn follow_config(before: Option<&Config>, now: &Config) {
    let clock_button = crate::calendar::takes_clock_button(now);
    if clock_button {
        crate::calendar::notifications::start();
        crate::calendar::focus::watch();
    } else if before.is_some() {
        crate::calendar::notifications::stop();
    }
    let calendar_shortcut = clock_button && now.calendar.shortcut;
    crate::calendar::set_shortcut(calendar_shortcut);
    hook::set_clock_taken(now.calendar.enabled, now.calendar.bell);
    hook::set_bell_middle_taken(clicks::bell_middle_taken(now));
    hook::set_battery_taken(now.uses_windows_battery_icon());
    use crate::calendar::{handed_over, merges_notices};
    if merges_notices(now) && !before.is_some_and(merges_notices) {
        crate::calendar::show_bell();
    }
    // Windows' banners taken (or given back after a crash), as Settings says.
    if before.is_none_or(|before| handed_over(before) != handed_over(now)) {
        crate::calendar::follow_takeover(now);
    }
    hook::AUDIO_SHORTCUT.store(now.strip.shortcut, SeqCst);
    hook::want_keys(calendar_shortcut || now.strip.shortcut);
}

/// The message window's procedure: app messages are queued and drained, the rest is default.
unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // The shell *sends* `TaskbarCreated`; answer at once and act on it from the queue.
    if msg == taskbar::taskbar_created_message() {
        let _ = unsafe { PostMessageW(Some(hwnd), WM_TASKBAR_RESTARTED, WPARAM(0), LPARAM(0)) };
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }
    // A plain store, read when the flyout opens: nothing to queue.
    if msg == tap_proto::WM_TASKBAR_STATUS {
        taskbar::set_status(wparam.0);
        // The TAP reported in: what Windows' battery icon says can be told now.
        let _ = unsafe { PostMessageW(Some(hwnd), WM_BATTERY_SYNC, WPARAM(0), LPARAM(0)) };
        return LRESULT(0);
    }
    // A plain store too, read by the mouse hook on its own thread.
    if msg == tap_proto::WM_TASKBAR_CLOCK_HOVER {
        hook::set_clock_hover(wparam.0);
        if matches!(wparam.0, tap_proto::HOVER_OVER | tap_proto::HOVER_BELL) {
            // Where it is, for a click with no hover told (after Windows' right-click menu).
            hook::measure_parts();
        }
        return LRESULT(0);
    }
    if msg == tap_proto::WM_TASKBAR_CLOCK_RECT {
        hook::set_part_rect(wparam.0, lparam.0);
        return LRESULT(0);
    }
    // A plain store, read by the mouse hook.
    if msg == tap_proto::WM_TASKBAR_BATTERY_HOVER {
        let over = wparam.0 == tap_proto::BATTERY_HOVER_OVER;
        hook::set_battery_hover(over);
        if over {
            // Where it is, so a press beside it is never taken.
            hook::measure_parts();
        }
        return LRESULT(0);
    }
    // Sent by Windows: answered at once, acted on from the queue.
    if msg == crate::batteries::WM_POWERBROADCAST {
        if wparam.0 == crate::batteries::PBT_POWERSETTINGCHANGE {
            let _ = unsafe { PostMessageW(Some(hwnd), WM_POWER_CHANGED, WPARAM(0), LPARAM(0)) };
        }
        return LRESULT(1);
    }
    // Windows is signing out or shutting down: the process may end as soon as this returns, so
    // Windows' banners are given back now (taken again at the next start).
    if msg == windows::Win32::UI::WindowsAndMessaging::WM_ENDSESSION && wparam.0 != 0 {
        crate::calendar::takeover::apply(false);
        return LRESULT(0);
    }
    // The screen changed (its resolution, its scale, the taskbar's room): where the clock and the
    // bell were measured is no longer where they are, and the banners move.
    {
        use windows::Win32::UI::WindowsAndMessaging::{SPI_SETWORKAREA, WM_DISPLAYCHANGE, WM_DPICHANGED, WM_SETTINGCHANGE};
        if msg == WM_DISPLAYCHANGE || msg == WM_DPICHANGED || (msg == WM_SETTINGCHANGE && wparam.0 == SPI_SETWORKAREA.0 as usize) {
            hook::set_part_rect(tap_proto::PART_FORGET, 0);
            let _ = unsafe { PostMessageW(Some(hwnd), WM_DISPLAY_CHANGED, WPARAM(0), LPARAM(0)) };
            return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
        }
    }
    if !is_ours(msg, wparam.0) {
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }
    INBOX.with(|inbox| inbox.borrow_mut().push_back(Queued { message: msg, wparam: wparam.0, lparam: lparam.0 }));
    // A panic must not unwind into user32.
    if std::panic::catch_unwind(drain).is_err() {
        eprintln!("tray: a message handler panicked");
        DRAINING.with(|draining| draining.set(false));
    }
    LRESULT(0)
}

/// Handle queued messages, oldest first, unless a handler further up the stack already is.
fn drain() {
    if DRAINING.with(|draining| draining.replace(true)) {
        return;
    }
    while let Some(next) = next_message() {
        handle(next);
    }
    DRAINING.with(|draining| draining.set(false));
}

/// Whether a message has to wait for the flyout to close: a refresh would race the flyout's own
/// copy of the config, and re-injecting after an Explorer restart is best done with it gone.
fn waits_for_flyout(message: u32) -> bool {
    matches!(message, notify::WM_AUDIO_REFRESH | WM_TASKBAR_RESTARTED | crate::settings::WM_SETTINGS)
}

/// The oldest queued message that may run now.
fn next_message() -> Option<Queued> {
    let flyout_open = with_tray(|tray| tray.flyout_open).unwrap_or(false);
    INBOX.with(|inbox| {
        let mut inbox = inbox.borrow_mut();
        let at = inbox.iter().position(|queued| !(flyout_open && waits_for_flyout(queued.message)))?;
        inbox.remove(at)
    })
}

fn handle(msg: Queued) {
    match msg.message {
        notify::WM_AUDIO_REFRESH => {
            AUDIO_CHANGED.take();
            with_tray(Tray::refresh);
        }
        WM_ENDPOINT_VOLUME => {
            with_tray(Tray::check_mutes);
        }
        crate::hub::WM_HUB_EVENTS => {
            with_tray(Tray::fold_events);
        }
        crate::pill::WM_PILL_ACTION => {
            with_tray(Tray::pill_actions);
        }
        crate::settings::WM_SETTINGS => match msg.wparam {
            crate::settings::QUIT => quit(),
            crate::settings::RESTART => {
                with_tray(|tray| restart_app(&tray.backend, &tray.config));
            }
            _ => {
                with_tray(Tray::adopt_settings);
            }
        },
        crate::settings::WM_OPEN_SETTINGS => {
            open_settings();
            crate::settings::show_page(msg.wparam);
        }
        WM_DISPLAY_CHANGED => crate::calendar::display_changed(),
        WM_BANNER_LISTENING => {
            with_tray(|tray| {
                if msg.wparam == 1 {
                    tray.set_microphone_muted(true);
                }
                // Said: the alert goes (until another app starts listening).
                tray.hub.acknowledge_listening();
                tray.hub_changed();
            });
        }
        crate::calendar::WM_CALENDAR if msg.wparam == crate::calendar::CLOSE => {
            let (x, y) = messages::unpack_point(msg.lparam);
            println!("calendar: closed by a click outside it, at {x},{y}");
            crate::calendar::close()
        }
        crate::calendar::WM_CALENDAR if msg.wparam == crate::calendar::QUIET => {
            // The bell's cross and the panel's moon follow.
            crate::calendar::quiet_changed();
            reschedule();
        }
        crate::calendar::WM_CALENDAR if msg.wparam == crate::calendar::NOTIFICATIONS => {
            if let Some(config) = with_tray(|tray| tray.config.clone()) {
                crate::calendar::notifications_changed(&config);
            }
            // New pictures decoded before the panel opens, once the burst has settled.
            prewarm_later(PREWARM_AFTER_MS);
            reschedule();
        }
        crate::calendar::WM_CALENDAR => {
            crate::calendar::changed();
            reschedule();
        }
        WM_TIMER if msg.wparam == CALENDAR_TIMER => reschedule(),
        WM_POWER_CHANGED => {
            // Whether this PC has a battery can come late after sign-in: which battery icon is the
            // batteries' home changes with it (Windows' takes the TAP's init data).
            let presence_changed = crate::batteries::refresh_presence();
            crate::calendar::power_changed();
            with_tray(|tray| {
                if presence_changed {
                    println!("batteries: this PC's battery is now {:?}", crate::batteries::presence());
                    tray.apply_taskbar();
                    hook::set_battery_taken(tray.config.uses_windows_battery_icon());
                    tray.apply_tooltip();
                }
                tray.battery.sync(&tray.config, &tray.hub.batteries);
            });
        }
        WM_BATTERY_SYNC => {
            with_tray(|tray| tray.battery.sync(&tray.config, &tray.hub.batteries));
        }
        WM_TIMER if msg.wparam == BATTERY_RETRY_TIMER => {
            kill_timer(BATTERY_RETRY_TIMER);
            with_tray(|tray| tray.battery.sync(&tray.config, &tray.hub.batteries));
        }
        WM_TIMER if msg.wparam == PREWARM_TIMER => {
            kill_timer(PREWARM_TIMER);
            if let Some(config) = with_tray(|tray| tray.config.clone()).filter(crate::calendar::takes_clock_button) {
                crate::calendar::prewarm(&config);
            }
        }
        sessions::WM_SESSIONS_CHANGED => {
            sessions::CHANGED.take();
            with_tray(Tray::report_sessions);
        }
        WM_ICON_CHECK => {
            with_tray(|tray| tray.ensure_icon(msg.wparam));
        }
        mic::WM_MIC_CHANGED => {
            MIC_CHANGED.take();
            // Amend only the dot; nothing recorded to amend means a full refresh.
            if with_tray(|tray| tray.push_mic_state(mic::in_use())) == Some(false) {
                AUDIO_CHANGED.post();
            }
        }
        // Applied here because `ITaskbarList3` is apartment-threaded and this is the STA.
        WM_MUSIC_PROGRESS => taskbar::apply_progress(msg.wparam, msg.lparam != 0),
        WM_TASKBAR_SCROLL => {
            // The hook marks a wheel's; the TAP's, and the hook's fine steps, are a touchpad's.
            let touchpad = msg.wparam & taskbar::SCROLL_FROM_WHEEL == 0;
            let flow = msg.wparam & !taskbar::SCROLL_FROM_WHEEL;
            let delta = crate::win::volume_delta(msg.lparam as i32, touchpad);
            if with_tray(|tray| tray.scroll.add(flow, delta)) == Some(true) {
                post(WM_APPLY_SCROLL, 0, 0);
            }
        }
        WM_APPLY_SCROLL => {
            with_tray(Tray::apply_scroll);
        }
        WM_ICON_RECT_CHECK => {
            with_tray(|tray| tray.refresh_icon_rect());
            hook::rect_checked();
        }
        // The only timer left: the scroll readout's (`osd::TIMER_ID`).
        WM_TIMER => {
            with_tray(|tray| tray.osd.tick());
        }
        WM_TASKBAR_ACTION => {
            if let Some(action) = Action::from_code(msg.wparam) {
                clicks::on_action(action);
            }
        }
        WM_TRAY_CLICK => clicks::on_tray_click(msg.wparam, msg.lparam),
        WM_STRIP_MIDDLE => clicks::on_middle_click(taskbar::flow_from_code(msg.wparam)),
        WM_BELL_MIDDLE => clicks::on_bell_middle_click(),
        WM_TASKBAR_RESTARTED => {
            // The clock the TAP said the pointer was over is gone with the old shell.
            hook::forget_clock();
            with_tray(Tray::taskbar_restarted);
        }
        _ => {}
    }
}

/// Show the flyout and apply its outcome.
///
/// No tray state is borrowed while it runs: it gets its own handle on the backend and a copy of
/// the config, adopted back when it closes. Draining is re-enabled for its modal loop, so the
/// microphone dot, scrolls and the progress bar stay live; refreshes wait for it (see
/// [`waits_for_flyout`]) and clicks are dropped (`clicks::click_allowed`).
fn open_flyout(anchor: Option<flyout::Anchor>) {
    // One flyout at a time: the calendar goes before the panel takes the mouse.
    crate::calendar::close();
    let opened = with_tray(|tray| {
        if tray.flyout_open {
            return None;
        }
        tray.flyout_open = true;
        tray.osd.hide(); // the panel supersedes it, and covers where it sits
        Some((Rc::clone(&tray.backend), tray.config.clone()))
    })
    .flatten();
    let Some((backend, mut config)) = opened else {
        return;
    };

    let was_draining = DRAINING.with(|draining| draining.replace(false));
    // No tooltip of the strip's (or the clock's) over the panel while it is open.
    taskbar::notify_panel(tap_proto::OPEN_AUDIO);
    let outcome = flyout::show(&backend, &mut config, anchor);
    taskbar::notify_panel(tap_proto::OPEN_NONE);
    DRAINING.with(|draining| draining.set(was_draining));

    with_tray(|tray| {
        tray.flyout_open = false;
        tray.reopen_guard = Instant::now() + SETTLE;
        // Teams may have handed over its token while the panel was open: the tray's copy has it.
        config.teams.token = tray.config.teams.token.take();
        tray.config = config;
        if outcome.restart {
            restart_app(&tray.backend, &tray.config);
        }
    });
    if outcome.settings {
        // The audio panel's gear: the Taskbar page, its buttons'.
        open_settings();
        crate::settings::show_page(crate::settings::OPEN_TASKBAR);
    }
    // The clock (or the bell) was clicked while the panel was open: the card it asked for.
    if let Some(panel) = clicks::CALENDAR_NEXT.with(|next| next.take()) {
        if let Some(config) = with_tray(|tray| tray.config.clone()) {
            crate::calendar::open(&config, panel);
        }
    }
}

/// Draw the closed panel ahead in `ms` (a call meanwhile pushes it back).
fn prewarm_later(ms: u32) {
    set_timer(PREWARM_TIMER, ms);
}

/// Have the calendar look again now (the focus timer started, a setting changed).
pub(crate) fn wake_calendar() {
    post(WM_TIMER, CALENDAR_TIMER, 0);
}

/// Let the calendar do what is due, and set its timer for the next moment.
fn reschedule() {
    let Some(delay) = with_tray(|tray| crate::calendar::schedule::tick(&tray.config)) else { return };
    set_timer(CALENDAR_TIMER, delay.as_millis().min(u32::MAX as u128) as u32);
}

/// Put the taskbar back and end the message loop. The TAP's owner watch would revert it too, but
/// asking means a normal quit tidies up promptly instead of racing our own teardown.
fn quit() {
    taskbar::revert(std::process::id());
    unsafe { PostQuitMessage(0) };
}

/// Open the settings window on the tray's config (or bring it to the front).
fn open_settings() {
    if let Some(config) = with_tray(|tray| {
        tray.osd.hide();
        tray.config.clone()
    }) {
        crate::settings::open(&config);
    }
}

impl Tray {
    /// Update the strip, the icon slot and the tray icon from the current defaults, read once.
    ///
    /// Strip first: it is drawn over the icon, so it is what the user is watching.
    fn refresh(&mut self) {
        let endpoints = endpoints::read(&self.backend, &self.config);
        self.rearm_mute_watches(&endpoints);
        self.push_strip(endpoints::strip_icons(&endpoints));
        self.refresh_icon_rect();
        self.show_output(endpoints.output.as_ref());
        self.report_endpoints(endpoints);
        crate::settings::devices_changed();
    }

    /// Take the settings window's config: save it, then bring along only what changed.
    fn adopt_settings(&mut self) {
        let Some(mut config) = crate::settings::take_pending() else {
            return;
        };
        // Teams hands its token to the tray, never to the window.
        config.teams.token = self.config.teams.token.clone();
        // The tray keeps the pill's places; the window's copy may be older.
        config.pill.places = self.config.pill.places.clone();
        let place = |c: &Config| (c.pill.edge, c.pill.align, c.pill.monitor.clone());
        if place(&config) != place(&self.config) {
            config.pill.remember(&crate::win::displays());
        }
        let before = std::mem::replace(&mut self.config, config);
        if let Err(e) = self.config.save() {
            eprintln!("save config failed: {e:#}");
        }
        let c = &self.config;
        let connectors = |c: &Config| (c.mpc.enabled, c.browser.enabled, c.browser.every_site, c.browser.sites.clone(), c.discord.enabled, c.teams.enabled, c.discord.client_id.clone(), c.discord.client_secret.clone());
        if connectors(&before) != connectors(c) {
            crate::integrations::reconfigure(&before, c);
        }
        crate::pill::configure(&c.pill);
        if before.motion != c.motion {
            crate::config::set_motion(c.motion);
            crate::pill::motion_changed();
        }
        // The strip and the tile need a fresh injection.
        let drawn = |c: &Config| {
            let s = &c.strip;
            (s.enabled, s.next_to_wifi, s.hide_windows_volume, s.hide_windows_mic, s.hide_windows_language, c.music.enabled, c.music.tile_style, crate::calendar::takes_clock_button(c), c.uses_windows_battery_icon(), c.injects())
        };
        if drawn(&before) != drawn(c) {
            self.apply_taskbar();
        }
        if before.calendar.links != self.config.calendar.links {
            crate::calendar::links_changed();
        }
        follow_config(Some(&before), &self.config);
        if before.calendar != self.config.calendar || before.clock != self.config.clock {
            wake_calendar();
        }
        if before.icons != self.config.icons {
            self.refresh();
        }
        self.configure_batteries(&before.batteries);
        if before.keyboards != self.config.keyboards {
            let wanted = &self.config.keyboards;
            match (&self.keyboards, wanted.enabled) {
                (Some(switcher), true) => switcher.configure(wanted),
                (None, true) => self.keyboards = crate::keyboards::Switcher::spawn(wanted),
                (_, false) => self.keyboards = None,
            }
        }
        // MPC-HC's session goes at once, and comes back with a fresh reading.
        if self.config.mpc.enabled != before.mpc.enabled {
            if self.config.mpc.enabled {
                if let Some(music) = &self.music {
                    music.reread();
                }
            } else {
                self.hub.media.retain(|m| crate::integrations::mpc::followed(&m.app_id));
                self.hub_changed();
            }
        }
        // Tabs now left out go at once; ones let back in come with the browser's next update.
        let sites = |c: &Config| (c.browser.every_site, c.browser.sites.clone());
        if sites(&self.config) != sites(&before) {
            self.hub.browser.retain(|m| crate::integrations::browser::followed(&m.app));
            self.hub_changed();
        }
    }

    /// The taskbar settings changed: take the strip off and draw what is asked for now (a TAP
    /// already loaded takes the new settings on the handover), and restart the music feed, whose
    /// progress bar and state file follow the tile.
    fn apply_taskbar(&mut self) {
        taskbar::revert(0);
        // The TAP that said the pointer was over the clock goes: its word with it.
        hook::forget_clock();
        self.strip = None;
        crate::calendar::schedule::forget_clock();
        // Windows' battery icon is told again once the TAP has taken the revert.
        self.battery.forget_windows();
        set_timer(BATTERY_RETRY_TIMER, 1500);
        if self.config.injects() {
            taskbar::apply_at_startup(endpoints::current_strip_icons(&self.backend, &self.config), &self.config);
        }
        // The revert dropped the clock's text: say it again once the TAP has taken the revert
        // (posted, so a text sent at once would be taken first and then wiped by it).
        set_timer(CALENDAR_TIMER, 1500);
        self.music = None;
        self.music = crate::music::spawn(&self.config.music, self.config.tile_shown());
    }

    /// The endpoint source: tell the hub the defaults, and follow the default output with the
    /// session watch (a session manager belongs to one endpoint).
    fn report_endpoints(&mut self, endpoints: crate::hub::Endpoints) {
        let output = endpoints.output.as_ref().map(|e| e.id.clone());
        if self.sessions.as_ref().map(|(id, _)| id) != output.as_ref() {
            self.sessions = output.and_then(|id| match sessions::Watch::new() {
                Ok(watch) => Some((id, watch)),
                Err(e) => {
                    eprintln!("tray: cannot watch app sessions ({e:#})");
                    None
                }
            });
            self.report_sessions();
        }
        crate::hub::emit(crate::hub::Event::Endpoints(endpoints));
    }

    /// The app-session source: list the default output's sessions for the hub.
    fn report_sessions(&mut self) {
        match sessions::list() {
            Ok(list) => crate::hub::emit(crate::hub::Event::AppSessions(list)),
            Err(e) => eprintln!("tray: cannot list app sessions ({e:#})"),
        }
    }

    /// Fold every queued hub event into the state the surfaces render.
    fn fold_events(&mut self) {
        let mut changed = false;
        let mut batteries = false;
        for event in crate::hub::take() {
            batteries |= matches!(event, crate::hub::Event::Batteries(_));
            // Teams on the microphone means a meeting: the moment to connect its integration.
            if let crate::hub::Event::MicUsers(users) = &event {
                crate::integrations::teams_on_mic(users.iter().any(|u| crate::hub::names::app_name(u) == "Teams"));
                if users.iter().any(|u| crate::hub::names::app_name(u) == "Discord") {
                    crate::integrations::discord_rpc::poke();
                }
            }
            // Discord playing audio (a call, a stream) is the other moment to connect to it.
            if let crate::hub::Event::AppSessions(sessions) = &event {
                if sessions.iter().any(|s| s.app == "Discord") {
                    crate::integrations::discord_rpc::poke();
                }
            }
            changed |= self.hub.apply(event);
        }
        if batteries {
            self.follow_batteries();
        }
        for notice in std::mem::take(&mut self.hub.outbox) {
            if let crate::hub::Notice::NewTrack { app_id, title, artist } = &notice {
                self.announce_song(app_id, title, artist);
                continue;
            }
            crate::hub::hand_over(&notice, &self.config);
        }
        if changed {
            self.hub_changed();
        }
    }

    /// A new song, when Settings asks for it: a glance of Audio Tray's over the bell, its cover in
    /// the disc, "artist · player" under the title. Never on the pill (it is about what plays, not
    /// news), never in the list.
    fn announce_song(&self, app_id: &str, title: &str, artist: &str) {
        if !self.config.pill.announce_tracks {
            return;
        }
        let media = self.hub.all_media().into_iter().find(|m| m.app_id == app_id).cloned();
        let player = media.as_ref().map_or_else(String::new, |m| m.app.clone());
        let body = [artist.trim(), player.trim()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
        // Keyed by the song, so its decoded picture is drawn again only for another song.
        let cover = media.and_then(|m| m.cover).map(|bytes| (format!("cover:{app_id}:{title}"), bytes));
        crate::calendar::now_playing(title, &body, cover, app_id);
    }

    /// The levels moved: the tooltip follows, and a battery that just went low gets its Windows
    /// notification.
    fn follow_batteries(&mut self) {
        self.apply_tooltip();
        // The batteries' card, and their icon.
        crate::calendar::set_batteries(self.hub.batteries.clone());
        self.battery.sync(&self.config, &self.hub.batteries);
    }

    /// Start or stop the battery source, and carry the battery settings to the hub and the pill.
    fn configure_batteries(&mut self, before: &crate::config::Batteries) {
        let b = self.config.batteries.clone();
        if b == *before {
            return;
        }
        match (self.batteries.is_some(), b.enabled) {
            (false, true) => self.batteries = crate::batteries::Watch::spawn(),
            (true, false) => {
                self.batteries = None;
                self.hub.batteries.clear();
            }
            _ => {}
        }
        self.hub.set_low_battery_at(b.warn_at());
        // Warned under the new threshold: no notification for a settings change.
        self.hub.outbox.retain(|n| !matches!(n, crate::hub::Notice::LowBattery { .. }));
        self.apply_tooltip();
        crate::calendar::set_batteries(self.hub.batteries.clone());
        // Their icon says it (a new low level moves its dot, a new style redraws it).
        self.battery.sync(&self.config, &self.hub.batteries);
        self.hub_changed();
    }

    /// The hub state changed: bring the surfaces in line.
    fn hub_changed(&mut self) {
        HUB_GENERATION.fetch_add(1, SeqCst);
        // A call starting or ending can turn Do not disturb on or off.
        let in_call = self.hub.call().is_some() || self.hub.mic_users.iter().any(|u| crate::hub::names::is_meeting_app(&crate::hub::names::app_name(u)));
        if crate::calendar::schedule::set_in_call(in_call) {
            wake_calendar();
        }
        crate::pill::update(&self.hub);
        // An app listening: a red banner of Audio Tray's, whatever the bell (never the pill's: it is
        // about audio activity alone).
        let listening = self.config.pill.listening_alerts.then(|| self.hub.listening.last().map(|l| (l.app.clone(), l.user.clone()))).flatten();
        crate::calendar::listening(listening);
        crate::settings::follow(&self.hub);
    }

    /// Carry out what the pill was clicked for.
    fn pill_actions(&mut self) {
        for action in crate::pill::take_actions() {
            println!("pill: {action:?}");
            self.perform(action);
        }
    }

    /// One action from a surface (the pill or the flyout).
    fn perform(&mut self, action: crate::hub::Action) {
        use crate::hub::Action;
        match action {
            Action::Media(app_id, command) => {
                if let Some(tab) = app_id.strip_prefix(crate::integrations::browser::APP_ID_PREFIX) {
                    // A browser tab: play/pause is all the extension does.
                    crate::integrations::browser_command(tab, "toggle");
                } else if let Some(music) = self.music.as_ref() {
                    music.command_to(&app_id, command);
                }
            }
            Action::ToggleMute(Some(app)) => crate::integrations::toggle_mute(app),
            Action::ToggleMute(None) => {
                let muted = self.hub.endpoints.input.as_ref().is_some_and(|e| e.muted);
                self.set_microphone_muted(!muted);
            }
            Action::Unmute(Some(app)) => crate::integrations::set_muted(app, false),
            Action::Unmute(None) => self.set_microphone_muted(false),
            Action::LeaveCall(app) => crate::integrations::leave_call(app),
            Action::Focus(app_id) => {
                if let Some(tab) = app_id.strip_prefix(crate::integrations::browser::APP_ID_PREFIX) {
                    // The browser raises its own window: let it take the foreground we were given.
                    unsafe {
                        let _ = windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(
                            windows::Win32::UI::WindowsAndMessaging::ASFW_ANY,
                        );
                    }
                    crate::integrations::browser_command(tab, "focus");
                } else {
                    let media = self.hub.all_media().into_iter().find(|m| m.app_id == app_id).cloned();
                    let (title, artist) = media.map(|m| (m.title, m.artist)).unwrap_or_default();
                    if let Err(e) = crate::music::player::focus_session(&app_id, &title, &artist) {
                        eprintln!("pill: could not bring {app_id} to the front ({e:#})");
                    }
                }
            }
            Action::DisplaysChanged => {
                if self.config.pill.recall(&crate::win::displays()) {
                    println!("pill: back where it was on these displays");
                    if let Err(e) = self.config.save() {
                        eprintln!("pill: could not save its place ({e:#})");
                    }
                    crate::pill::configure(&self.config.pill);
                    crate::settings::sync(&self.config);
                }
            }
        }
    }

    /// Mute or unmute the default microphone. Its volume watch brings the change back round.
    fn set_microphone_muted(&mut self, muted: bool) {
        match self.backend.default_of(Flow::Input) {
            Ok(Some(id)) => {
                if let Err(e) = self.backend.set_muted(&id, muted) {
                    eprintln!("pill: could not {} the microphone ({e:#})", if muted { "mute" } else { "unmute" });
                }
                AUDIO_CHANGED.post();
            }
            _ => eprintln!("pill: no default microphone to mute"),
        }
    }

    /// Apply the folded scroll: nudge each endpoint, and show the level beside the buttons
    /// (not while the flyout is open — its sliders already follow the volume).
    fn apply_scroll(&mut self) {
        let slot = self.refresh_icon_rect();
        for (code, notches) in self.scroll.take().into_iter().enumerate() {
            if notches == 0.0 {
                continue;
            }
            let flow = taskbar::flow_from_code(code);
            match self.backend.nudge_volume(flow, notches * SCROLL_STEP) {
                Ok((level, muted)) if !self.flyout_open => self.osd.show(flow, level, muted, slot),
                Ok(_) => {}
                Err(e) => eprintln!("scroll: could not change the {flow:?} volume ({e:#})"),
            }
        }
    }

    /// Explorer restarted: the strip, the progress bar and the thumbnail toolbar died with it.
    fn taskbar_restarted(&mut self) {
        self.strip = None;
        crate::calendar::schedule::forget_clock();
        // Both battery icons went with the old shell: shown and told again.
        self.battery.taskbar_restarted();
        post(WM_BATTERY_SYNC, 0, 0);
        wake_calendar();
        if self.config.injects() {
            taskbar::apply_at_restart(endpoints::current_strip_icons(&self.backend, &self.config), &self.config);
        }
        if let Some(music) = self.music.as_ref() {
            music.taskbar_restarted();
        }
        // This thread's cached shell interface is a proxy into the dead Explorer.
        crate::music::player::forget_taskbar_list();
        crate::pill::taskbar_restarted();
        // tray-icon re-registered the icon with its build-time defaults; put ours back.
        self.refresh();
        // ...unless that re-registration failed: see `ensure_icon`.
        post_after(WM_ICON_CHECK, 0, icon::ICON_CHECK_FIRST);
    }
}

/// Ask a running tray (another process) to open its settings window, letting it come to the front.
/// `false` when none is running.
pub fn forward_open_settings() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{AllowSetForegroundWindow, GetWindowThreadProcessId};
    crate::win::windows_by_class(tap_proto::RECEIVER_CLASS).into_iter().any(|hwnd| {
        let mut pid = 0;
        unsafe {
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            let _ = AllowSetForegroundWindow(pid);
            PostMessageW(Some(hwnd), crate::settings::WM_OPEN_SETTINGS, WPARAM(0), LPARAM(0)).is_ok()
        }
    })
}

/// Relaunch the (already self-updated on disk) exe and quit this one. Best-effort: if the
/// relaunch fails we stay running.
///
/// The strip is not reverted: ownership is transferred to the child first, so the TAP's owner
/// watch ignores our exit; the child waits for the instance mutex ([`crate::instance`]) and then
/// offers its own handover, or restarts Explorer if it is a newer build than the loaded TAP.
fn restart_app(backend: &WasapiBackend, config: &Config) {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => return eprintln!("restart: current_exe() failed: {e:#}"),
    };
    match std::process::Command::new(exe).arg(crate::instance::RELAUNCHED).spawn() {
        Ok(child) => {
            if taskbar::strip_is_up() {
                taskbar::transfer_owner(child.id(), endpoints::current_strip_icons(backend, config), config);
            }
            unsafe { PostQuitMessage(0) }
        }
        Err(e) => eprintln!("restart: failed to relaunch: {e:#}"),
    }
}

/// Bumped on every hub change, so the open flyout can tell when to re-read.
static HUB_GENERATION: AtomicU64 = AtomicU64::new(1);

pub(crate) fn hub_generation() -> u64 {
    HUB_GENERATION.load(SeqCst)
}

/// The hub state and its generation, for the flyout; `None` outside the tray (the previews).
pub(crate) fn hub_snapshot() -> Option<(u64, crate::hub::State)> {
    with_tray(|tray| (hub_generation(), tray.hub.clone()))
}

/// Carry out a surface's action from outside a tray handler (the flyout's modal loop).
pub(crate) fn perform(action: crate::hub::Action) {
    with_tray(|tray| tray.perform(action));
}

/// Keep the token Teams handed over when the user allowed Audio Tray. Tray thread.
pub(crate) fn set_teams_token(token: String) {
    crate::integrations::teams_token(token.clone());
    with_tray(|tray| {
        tray.config.teams.token = Some(token);
        if let Err(e) = tray.config.save() {
            eprintln!("teams: could not save the token ({e:#})");
        }
        crate::settings::sync(&tray.config);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wake_posts_once_until_taken() {
        let wake = Wake::new(0);
        let mut sent = 0;
        wake.post_with(|_| {
            sent += 1;
            true
        });
        wake.post_with(|_| {
            sent += 1;
            true
        });
        assert_eq!(sent, 1, "a second change while one is queued costs nothing");
        wake.take();
        wake.post_with(|_| {
            sent += 1;
            true
        });
        assert_eq!(sent, 2, "after the handler took it, the next change posts again");
    }

    #[test]
    fn a_failed_wake_post_does_not_stick() {
        let wake = Wake::new(0);
        wake.post_with(|_| false);
        let mut sent = false;
        wake.post_with(|_| {
            sent = true;
            true
        });
        assert!(sent);
    }

    #[test]
    fn scrolls_fold_into_one_apply_per_batch() {
        let mut scroll = ScrollTotals::default();
        assert!(scroll.add(0, 120), "the first delta asks for an apply");
        assert!(!scroll.add(0, 60), "later ones ride on it");
        assert!(!scroll.add(1, -240));
        assert_eq!(scroll.take(), [1.5, -2.0]);
        assert_eq!(scroll.take(), [0.0, 0.0]);
        assert!(scroll.add(5, 120), "a new batch after the apply; unknown codes count as input");
        assert_eq!(scroll.take(), [0.0, 1.0]);
    }

    #[test]
    fn the_flyout_holds_back_refreshes_but_not_live_state() {
        assert!(waits_for_flyout(notify::WM_AUDIO_REFRESH));
        assert!(waits_for_flyout(WM_TASKBAR_RESTARTED));
        assert!(!waits_for_flyout(mic::WM_MIC_CHANGED));
        assert!(!waits_for_flyout(WM_TASKBAR_SCROLL));
        assert!(!waits_for_flyout(WM_MUSIC_PROGRESS));
    }
}
