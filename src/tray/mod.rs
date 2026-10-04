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
//! Left click on a strip segment cycles that endpoint ([`Tray::taskbar_action`]); right click, or
//! either click on the plain icon when there is no strip, opens the flyout. Scrolling a button
//! changes that endpoint's volume: the wheel through [`hook`], a precision touchpad through the
//! TAP, both as [`WM_TASKBAR_SCROLL`]; the level is shown by [`crate::osd`].

mod hook;

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::time::{Duration, Instant};

use anyhow::Result;
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, DispatchMessageW, GetMessageW, PostMessageW, PostQuitMessage, TranslateMessage,
    MSG, WHEEL_DELTA, WM_APP, WM_TIMER,
};

use crate::audio::wasapi::WasapiBackend;
use crate::audio::{mic, notify, Flow};
use crate::config::Config;
use crate::flyout;
use crate::icons::IconId;
use crate::osd::Osd;
use crate::taskbar::{
    self, Action, StripIcons, WM_MUSIC_PROGRESS, WM_TASKBAR_ACTION, WM_TASKBAR_RESTARTED,
    WM_TASKBAR_SCROLL,
};

/// Volume change per wheel notch — 2%, the step Windows' own volume keys take.
pub(crate) const SCROLL_STEP: f32 = 0.02;

/// Stable marker appended to the tray icon's tooltip, which is also its accessible name: the only
/// thing the TAP can pick *our* icon out by. Keep in step with `tooltip=` in [`crate::taskbar`].
pub const TRAY_MARKER: &str = "Audio Tray";

fn tooltip_for(device: &str) -> String {
    format!("{device} — {TRAY_MARKER}")
}

/// The tooltip the icon is *registered* with. **Do not change it**: Windows keys the icon's
/// "always show on the taskbar" choice on the exe path plus this string, and a new string drops
/// every user's icon (and with it the strip) into the overflow. See docs/NOTES.md.
const INITIAL_TOOLTIP: &str = "Audio output";

/// Apply the scroll folded so far; self-posted once per batch ([`ScrollTotals`]).
const WM_APPLY_SCROLL: u32 = WM_APP + 30;
/// A click on the plain icon, from tray-icon's handler: `wParam` 1 = left, 2 = right; `lParam` the
/// anchor (icon centre x, icon top), packed by [`pack_point`].
const WM_TRAY_CLICK: u32 = WM_APP + 31;

/// After the flyout closes, ignore clicks for this long: the dismissing click's release half
/// arrives as a strip `Tapped` or tray-icon click after the panel has gone.
const SETTLE: Duration = Duration::from_millis(350);

/// The message window, for producers on other threads.
static WINDOW: AtomicIsize = AtomicIsize::new(0);

/// Post to the tray's message window from any thread. `false` when there is no tray (the dev CLI
/// modes) or the post failed.
pub(crate) fn post(message: u32, wparam: usize, lparam: isize) -> bool {
    let raw = WINDOW.load(Ordering::SeqCst);
    raw != 0
        && unsafe { PostMessageW(Some(HWND(raw as *mut _)), message, WPARAM(wparam), LPARAM(lparam)) }.is_ok()
}

/// A "the world changed, go and look" message, coalesced: at most one is queued at a time, and
/// the handler re-arms it before reading the state, so the last change is never missed.
pub(crate) struct Wake {
    message: u32,
    queued: AtomicBool,
}

impl Wake {
    const fn new(message: u32) -> Self {
        Self { message, queued: AtomicBool::new(false) }
    }

    /// Ask for one handling of this message; free when one is already queued.
    pub(crate) fn post(&self) {
        self.post_with(|message| post(message, 0, 0));
    }

    fn post_with(&self, send: impl FnOnce(u32) -> bool) {
        if !self.queued.swap(true, Ordering::SeqCst) && !send(self.message) {
            self.queued.store(false, Ordering::SeqCst);
        }
    }

    /// Called by the handler, before it reads the state.
    fn take(&self) {
        self.queued.store(false, Ordering::SeqCst);
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

/// Build the tray icon and run the message loop until the user quits.
pub fn run(backend: WasapiBackend) -> Result<()> {
    let config = Config::load();

    // First: everything below posts here, and the TAP needs a receiver before the strip exists.
    let window = taskbar::create_receiver(Some(wndproc))?;
    WINDOW.store(window.0 as isize, Ordering::SeqCst);
    println!("taskbar: click receiver window {:?}", window.0);
    mic::in_use(); // start the watcher, so a recording that starts during startup is heard

    // Before the tray icon exists: decorating inside the initial visual-tree replay, while the
    // shell is still building the icon's subtree, hangs `put_Content` (see FINDINGS.md).
    taskbar::apply_at_startup(strip_icons(&backend, &config));

    // At logon audio-tray can start ahead of `explorer.exe`; `build_tray` retries until it takes.
    let icon = build_tray(&backend, &config)?;
    TrayIconEvent::set_event_handler(Some(on_tray_event));
    let _notifications = notify::register()?;
    let _wheel = hook::WheelHook::spawn();

    let devices = backend.enumerate_flow(Flow::Output).map(|d| d.len()).unwrap_or(0);
    let gestures = if taskbar::strip_is_up() {
        "left click cycles a segment, right click opens the panel"
    } else {
        "no strip — either button opens the panel"
    };
    println!("tray: created ({devices} output device(s)); {gestures}.");

    // On a thread of its own: every SMTC call blocks on an async operation, which deadlocks this
    // STA (see `music::on_mta_thread`). `None` = off in config, or SMTC would not open.
    let music = crate::music::spawn(&config.music);
    if music.is_some() {
        println!("music: following YouTube Music");
    }

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
        })
    });
    with_tray(Tray::refresh);

    let mut msg = MSG::default();
    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    // The progress bar sits on *another app's* button, so it has to come off here, while this
    // thread still pumps; dropping the music handle below is what stops the feed re-adding it.
    taskbar::clear_player_progress();
    WINDOW.store(0, Ordering::SeqCst);
    drop(TRAY.with(|cell| cell.borrow_mut().take()));
    Ok(())
}

/// The message window's procedure: app messages are queued and drained, the rest is default.
unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // The shell *sends* `TaskbarCreated`; answer at once and act on it from the queue.
    if msg == taskbar::taskbar_created_message() {
        let _ = unsafe { PostMessageW(Some(hwnd), WM_TASKBAR_RESTARTED, WPARAM(0), LPARAM(0)) };
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
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

fn is_ours(msg: u32, wparam: usize) -> bool {
    matches!(
        msg,
        notify::WM_AUDIO_REFRESH
            | mic::WM_MIC_CHANGED
            | WM_TASKBAR_ACTION
            | WM_TASKBAR_RESTARTED
            | WM_TASKBAR_SCROLL
            | WM_MUSIC_PROGRESS
            | WM_APPLY_SCROLL
            | WM_TRAY_CLICK
    ) || (msg == WM_TIMER && wparam == crate::osd::TIMER_ID)
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
    matches!(message, notify::WM_AUDIO_REFRESH | WM_TASKBAR_RESTARTED)
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
            if with_tray(|tray| tray.scroll.add(msg.wparam, msg.lparam as i32)) == Some(true) {
                post(WM_APPLY_SCROLL, 0, 0);
            }
        }
        WM_APPLY_SCROLL => {
            with_tray(Tray::apply_scroll);
        }
        WM_TIMER => {
            with_tray(|tray| tray.osd.tick());
        }
        WM_TASKBAR_ACTION => {
            if let Some(action) = Action::from_code(msg.wparam) {
                on_action(action);
            }
        }
        WM_TRAY_CLICK => on_tray_click(msg.wparam, msg.lparam),
        WM_TASKBAR_RESTARTED => {
            with_tray(Tray::taskbar_restarted);
        }
        _ => {}
    }
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
fn on_action(action: Action) {
    if !click_allowed(&action) {
        return;
    }
    if action == Action::OpenPanel {
        // No anchor: the strip's rect is not ours to know; the flyout uses its default place.
        return open_flyout(None);
    }
    with_tray(|tray| {
        // Never propagated: COM against endpoints that can vanish mid-click must not end the loop.
        if let Err(e) = tray.taskbar_action(action) {
            eprintln!("taskbar: {action:?} failed ({e:#})");
            // The strip may be previewing a switch that failed: put the truth back.
            tray.strip = None;
            tray.refresh();
        }
    });
}

/// A click on the plain notification icon. Right click opens the flyout; left click does too,
/// unless a strip is up — the shell invokes the icon under the strip, so a segment's click also
/// arrives here, and the strip's own handler is the one that cycles.
fn on_tray_click(button: usize, packed: isize) {
    if button == 1 && taskbar::strip_is_up() {
        println!("tray: left click on the strip — cycling, not opening");
        return;
    }
    if !click_allowed(&"tray click") {
        return;
    }
    let (cx, bottom) = unpack_point(packed);
    open_flyout(Some(flyout::Anchor { cx, bottom }));
}

/// tray-icon's event handler (called on this thread, from its own window's procedure): turn an
/// icon click into a message, so it is handled — or dropped — like every other gesture.
fn on_tray_event(event: TrayIconEvent) {
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

/// Two signed 16-bit screen coordinates in one `LPARAM` (multi-monitor coordinates can be negative).
fn pack_point(x: i32, y: i32) -> isize {
    (x as u16 as u32 | (y as u16 as u32) << 16) as isize
}

fn unpack_point(packed: isize) -> (i32, i32) {
    ((packed & 0xFFFF) as u16 as i16 as i32, ((packed >> 16) & 0xFFFF) as u16 as i16 as i32)
}

/// Show the flyout and apply its outcome.
///
/// No tray state is borrowed while it runs: it gets its own handle on the backend and a copy of
/// the config, adopted back when it closes. Draining is re-enabled for its modal loop, so the
/// microphone dot, scrolls and the progress bar stay live; refreshes wait for it (see
/// [`waits_for_flyout`]) and clicks are dropped ([`click_allowed`]).
fn open_flyout(anchor: Option<flyout::Anchor>) {
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
    let outcome = flyout::show(&backend, &mut config, anchor);
    DRAINING.with(|draining| draining.set(was_draining));

    with_tray(|tray| {
        tray.flyout_open = false;
        tray.reopen_guard = Instant::now() + SETTLE;
        tray.config = config;
        if outcome.config_changed {
            if let Err(e) = tray.config.save() {
                eprintln!("save config failed: {e:#}");
            }
            tray.refresh();
        }
        if outcome.restart {
            restart_app(&tray.backend, &tray.config);
        }
        if outcome.quit {
            // Put the taskbar back before we go. The TAP's owner watch would do it too, but asking
            // means a normal quit tidies up promptly instead of racing our own teardown.
            taskbar::revert(std::process::id());
            unsafe { PostQuitMessage(0) };
        }
    });
}

impl Tray {
    /// Update the strip, the icon slot and the tray icon from the current defaults.
    ///
    /// Strip first: it is drawn over the icon, so it is what the user is watching. Failure to set
    /// the icon is non-fatal — the shell refuses icons while it restarts, and `TaskbarCreated`
    /// brings us back here.
    fn refresh(&mut self) {
        let state = Current::read(&self.backend, &self.config);
        self.push_strip(state.strip_icons());
        self.refresh_icon_rect();
        if let Err(e) = state.output.apply_to(&self.icon) {
            eprintln!("tray: could not update the icon, will retry when the taskbar is back ({e:#})");
        }
    }

    /// Tell the strip what to draw unless it already is. Returns whether it can be taken to show
    /// `icons` now; a restyle that could not be posted (no control window yet) is not remembered.
    fn push_strip(&mut self, icons: StripIcons) -> bool {
        if self.strip == Some(icons) {
            return true;
        }
        if taskbar::restyle(icons) {
            self.strip = Some(icons);
            return true;
        }
        false
    }

    /// Put the recording dot on (or off) without re-reading the devices. `false`: nothing recorded
    /// to amend, or the restyle did not go out — the caller falls back to a full refresh.
    fn push_mic_state(&mut self, recording: bool) -> bool {
        let Some(mut icons) = self.strip else {
            return false;
        };
        icons.input_recording = recording;
        self.push_strip(icons)
    }

    /// Show where a strip click lands before doing the (slow) audio work behind it. `None` for
    /// `icon` keeps the glyph and changes only the mute. A no-op until a refresh recorded a state.
    fn preview_strip(&mut self, flow: Flow, icon: Option<IconId>, muted: bool) {
        let Some(mut icons) = self.strip else {
            return;
        };
        let glyph = icon.map(taskbar::strip_glyph);
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
        self.push_strip(icons);
    }

    /// Ask the shell where our icon is (it moves with every tray change and Explorer restart),
    /// record it for the wheel hook, and return it for placing the readout. `None` while the icon
    /// is not on the taskbar.
    fn refresh_icon_rect(&self) -> Option<RECT> {
        let rect = self.icon.rect()?;
        let (left, top) = (rect.position.x as i32, rect.position.y as i32);
        let slot = RECT {
            left,
            top,
            right: left + rect.size.width as i32,
            bottom: top + rect.size.height as i32,
        };
        if hook::set_icon_rect(Some(slot)) {
            println!("tray: icon slot at {left},{top} {}x{}", rect.size.width, rect.size.height);
        }
        Some(slot)
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
        taskbar::apply_at_restart(Current::read(&self.backend, &self.config).strip_icons());
        if let Some(music) = self.music.as_ref() {
            music.taskbar_restarted();
        }
        // This thread's cached shell interface is a proxy into the dead Explorer.
        crate::music::player::forget_taskbar_list();
        // tray-icon re-registered the icon with its build-time defaults; put ours back.
        self.refresh();
    }

    /// Act on a click from the injected strip.
    ///
    /// Left click steps that endpoint around a cycle of the active devices, each unmuted, plus
    /// one muted stop on the last device before the wrap:
    ///
    /// ```text
    /// device 1 → device 2 → … → device N → device N, muted → device 1 → …
    /// ```
    ///
    /// Every device stop is unmuted: arriving on a muted device unmutes it, and stepping off the
    /// muted stop takes the mute with it, so the cycle never leaves a muted device behind.
    fn taskbar_action(&mut self, action: Action) -> Result<()> {
        use crate::music::smtc::Command;

        let flow = match action {
            Action::CycleOutput => Flow::Output,
            Action::CycleInput => Flow::Input,
            Action::OpenPanel => return Ok(()), // handled by `on_action`
            // Handed to the feed thread and not awaited: a slow media session must not stall this
            // thread. The strip catches up on the next poll.
            Action::MusicPrevious | Action::MusicPlayPause | Action::MusicNext => {
                let Some(music) = self.music.as_ref() else {
                    println!("taskbar: {action:?} ignored — the music half is not running");
                    return Ok(());
                };
                music.command(match action {
                    Action::MusicPrevious => Command::Previous,
                    Action::MusicNext => Command::Next,
                    _ => Command::TogglePlayPause,
                });
                return Ok(());
            }
        };

        let devices = self.backend.enumerate_flow(flow)?;
        if devices.is_empty() {
            return Ok(());
        }
        let current = self.backend.default_of(flow)?;
        let at = current.as_ref().and_then(|id| devices.iter().position(|d| &d.id == id));
        let muted = current.as_ref().and_then(|id| self.backend.is_muted(id).ok()).unwrap_or(false);

        // Which device the click lands on, or `None` for the muted stop.
        let next = match at {
            Some(i) if muted => Some((i + 1) % devices.len()),
            Some(i) if i + 1 == devices.len() => None,
            Some(i) => Some(i + 1),
            None => Some(0),
        };

        match next {
            Some(next) => self.preview_strip(flow, Some(self.config.icon_of(&devices[next])), false),
            None => self.preview_strip(flow, None, true),
        }

        match next {
            Some(next) => {
                let id = &devices[next].id;
                if current.as_ref() != Some(id) {
                    self.backend.set_default_of(id)?;
                }
                // Unmute the device we left *after* the switch, so it makes no sound.
                if muted {
                    if let Some(left) = current.as_ref().filter(|left| *left != id) {
                        self.backend.set_muted(left, false)?;
                    }
                }
                if self.backend.is_muted(id).unwrap_or(false) {
                    self.backend.set_muted(id, false)?;
                }
            }
            None => {
                if let Some(id) = &current {
                    self.backend.set_muted(id, true)?;
                }
            }
        }

        // Confirms the preview, and is the only strip update after a mute (which raises no
        // endpoint notification).
        self.refresh();
        Ok(())
    }
}

/// Push the current defaults to the strip, for the flyout's live changes. A no-op outside the
/// tray (the `--flyout` preview).
pub(crate) fn restyle_strip(backend: &WasapiBackend, config: &Config) {
    let icons = strip_icons(backend, config);
    with_tray(|tray| tray.push_strip(icons));
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
                taskbar::transfer_owner(child.id(), strip_icons(backend, config));
            }
            unsafe { PostQuitMessage(0) }
        }
        Err(e) => eprintln!("restart: failed to relaunch: {e:#}"),
    }
}

/// Register the tray icon, retrying until the shell actually accepts it.
///
/// At logon audio-tray can start before Explorer takes icons, and a failed add is terminal
/// (`TaskbarCreated` has already been and gone). `build` reports success even when the add did
/// not land, so setting a property is the real test. Retries without a deadline: an audio-tray
/// with no icon has nothing to offer, so waiting beats quitting.
fn build_tray(backend: &WasapiBackend, config: &Config) -> Result<TrayIcon> {
    const FIRST_GAP: Duration = Duration::from_millis(500);
    const MAX_GAP: Duration = Duration::from_secs(5);
    const NAG_EVERY: u32 = 12;

    let initial_icon = Endpoint::read(backend, config, Flow::Output).icon;
    let mut gap = FIRST_GAP;
    for attempt in 1.. {
        let built = TrayIconBuilder::new()
            .with_tooltip(INITIAL_TOOLTIP)
            .with_icon(icon_image(initial_icon)?)
            .build();
        let failure = match built {
            Ok(tray) => match Endpoint::read(backend, config, Flow::Output).apply_to(&tray) {
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

/// The current default of one flow, as both surfaces need it.
struct Endpoint {
    name: String,
    icon: IconId,
    muted: bool,
}

impl Endpoint {
    /// One `enumerate_flow`, one `default_of` and one `is_muted`.
    fn read(backend: &WasapiBackend, config: &Config, flow: Flow) -> Self {
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
            Some(d) => Self { icon: config.icon_of(&d), name: d.friendly_name, muted },
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
        Self {
            output: Endpoint::read(backend, config, Flow::Output),
            input: Endpoint::read(backend, config, Flow::Input),
        }
    }

    fn strip_icons(&self) -> StripIcons {
        StripIcons {
            output: taskbar::strip_glyph(self.output.icon),
            input: taskbar::strip_glyph(self.input.icon),
            output_muted: self.output.muted,
            input_muted: self.input.muted,
            // A property of the microphone, not of an endpoint; a cached atomic, not COM.
            input_recording: mic::in_use(),
        }
    }
}

/// The glyphs for the current defaults.
pub(crate) fn strip_icons(backend: &WasapiBackend, config: &Config) -> StripIcons {
    Current::read(backend, config).strip_icons()
}

fn icon_image(id: IconId) -> Result<Icon> {
    // Monochrome like the shell's own tray icons, at the exact small-icon size.
    let tint = if taskbar_is_light() { [0x20, 0x20, 0x20] } else { [0xff, 0xff, 0xff] };
    let (rgba, w, h) = id.render(crate::win::small_icon_size(), tint)?;
    Ok(Icon::from_rgba(rgba, w, h)?)
}

/// Whether the taskbar uses the light theme (`SystemUsesLightTheme`); absent means dark.
fn taskbar_is_light() -> bool {
    use windows::core::w;

    crate::win::hkcu_dword(
        w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
        w!("SystemUsesLightTheme"),
    ) == Some(1)
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
    fn points_survive_packing_including_negative_coordinates() {
        for (x, y) in [(0, 0), (1919, 1032), (-1280, 1400), (-1, -32768), (32767, 5)] {
            assert_eq!(unpack_point(pack_point(x, y)), (x, y));
        }
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
