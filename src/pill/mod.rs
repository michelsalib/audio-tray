//! The floating edge pill: the passive surface. It sits on a display's left, right or top edge
//! (Settings > Edge pill), reserves no space, and shows the loudest state from [`view::decide`].
//!
//! **Nothing ticks.** Each state is painted once into a DirectComposition surface; slide-ins and
//! the progress ring are compositor animations set once per change (the ring is two half-rings
//! whose rotation DWM animates to the end of the track). The only timer is the 10 Hz microphone
//! meter while muted in a call.
//!
//! **Windows.** The pill window is `NOACTIVATE`/`TOOLWINDOW`/`TOPMOST` and never takes focus;
//! quiet states add `WS_EX_TRANSPARENT` so clicks pass through. Hover is caught by a 2 px,
//! barely-opaque strip on the edge with `TrackMouseEvent`, never a global hook. A fullscreen app
//! is reported through an appbar registration made only for that notification.
//!
//! Clicks become [`Action`]s queued for the tray thread, which owns the audio and the integrations.

pub mod draw;
pub mod gfx;
pub mod view;
mod waves;

use std::cell::RefCell;
use std::collections::VecDeque;

use anyhow::{Context, Result};
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::DirectComposition::{
    IDCompositionAnimation, IDCompositionRectangleClip, IDCompositionVisual2,
    DCOMPOSITION_BORDER_MODE_SOFT,
};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::Media::Audio::Endpoints::IAudioMeterInformation;
use windows::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows::Win32::UI::Shell::{
    SHAppBarMessage, SHQueryUserNotificationState, ABM_NEW, ABM_REMOVE, APPBARDATA, QUNS_BUSY,
    QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, GetCursorPos, GetWindowLongPtrW, GetWindowRect, KillTimer, SetLayeredWindowAttributes,
    SetTimer, SetWindowLongPtrW, SetWindowPos, ShowWindow, GWL_EXSTYLE, HWND_TOPMOST, LWA_ALPHA, MA_NOACTIVATE,
    SWP_NOACTIVATE, SWP_SHOWWINDOW, SW_HIDE, WM_APP, WM_DISPLAYCHANGE, WM_DPICHANGED, WM_LBUTTONUP,
    WM_LBUTTONDOWN, WM_MOUSEACTIVATE, WM_MOUSEMOVE, WM_MOUSEWHEEL, WHEEL_DELTA, WM_RBUTTONUP, WM_SETTINGCHANGE, WM_TIMER, WS_EX_LAYERED, SPI_SETWORKAREA,
    WS_EX_NOACTIVATE, WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
};

use crate::config::{PillAlign, PillEdge};
use crate::hub::{State, VoiceApp};
use crate::win::Monitor;
use crate::music::smtc::Command;
use draw::{Hit, Ring, Scene};
use crate::config::MotionStyle;
use gfx::Gfx;
use view::{Inputs, View};

/// What a click on the pill asks the tray to do.
#[derive(Clone, PartialEq, Debug)]
pub enum Action {
    /// A transport command for the session of this app id.
    Media(String, Command),
    /// Toggle the user's mute: in the voice app, or on the default microphone.
    ToggleMute(Option<VoiceApp>),
    Unmute(Option<VoiceApp>),
    /// Leave the call in the voice app.
    LeaveCall(VoiceApp),
    /// Mute the default microphone (the "is listening" alert).
    MuteMicrophone,
    /// Bring the app or tab behind a media session (its app id) to the front.
    Focus(String),
    /// Switch this flow to its next device, as the taskbar button's click does.
    CycleDevice(crate::audio::Flow),
    /// Make this device the flow's default (picked in a fanned-out device row).
    SetDevice(crate::audio::Flow, crate::audio::DeviceId),
    /// Change this flow's volume by so many wheel notches.
    Volume(crate::audio::Flow, f32),
    Acknowledge(crate::hub::Notice),
    /// Screens came or went: the pill goes back where it was placed on this set of them.
    DisplaysChanged,
}

/// Posted to the tray's message window when [`take_actions`] has something.
pub const WM_PILL_ACTION: u32 = WM_APP + 36;

/// The appbar notification message (fullscreen app opened or closed).
const WM_APPBAR: u32 = WM_APP + 37;
/// `WM_MOUSELEAVE`, filed under UI::Controls in windows-rs.
const WM_MOUSELEAVE: u32 = 0x02A3;
/// `WM_DWMCOLORIZATIONCOLORCHANGED`: the accent changed (sent to top-level windows).
const WM_DWMCOLORIZATIONCOLORCHANGED: u32 = 0x0320;
const METER_TIMER: usize = 1;
/// The microphone meter's rate while muted in a call: 10 Hz.
const METER_MS: u32 = 100;
/// Peak above which the microphone hears speech, and how many ticks make talking / silence.
const TALK_PEAK: f32 = 0.04;
const TALK_TICKS: u32 = 3;
const QUIET_TICKS: u32 = 15;
/// The hover strip's length, centred on the pill's place.
const EDGE_H: f32 = 160.0;
/// The gap between the pill and the end of its edge, when pinned to one.
const EDGE_MARGIN: f32 = 16.0;
const SLIDE_SECONDS: f64 = 0.2;
const GROW_SECONDS: f64 = 0.18;
/// The card growing out of the pill, and shrinking back into it.
const EXPAND_SECONDS: f64 = 0.22;
const COLLAPSE_SECONDS: f64 = 0.14;
/// A button's highlight fading in.
const HIGHLIGHT_SECONDS: f64 = 0.09;
/// One-shot: the card's collapse has played.
const COLLAPSE_TIMER: usize = 2;
/// One-shot: look at the fullscreen state again after an appbar notification.
const FULLSCREEN_TIMER: usize = 3;
const FULLSCREEN_RECHECK_MS: u32 = 1000;
/// Repeating while a Samsung-style ring or bar moves: each tick paints it again (see [`Live`]).
const LIVE_TIMER: usize = 4;
const LIVE_MS: u32 = 16;
/// One beat of the speaking pulse.
const PULSE_SECONDS: f64 = 1.1;
/// A playing track's progress ripples, as Android's media player does: the wave travels one
/// wavelength per this many seconds.
const WAVE_SECONDS: f64 = 1.0;
/// The ring's wavelength along its circle, and its amplitude per DIP of radius.
const RING_WAVELENGTH: f32 = 12.0;
const RING_AMP: f32 = 0.05;
/// The card bar's wavelength and amplitude.
const BAR_WAVELENGTH: f32 = 20.0;
const BAR_AMP: f32 = 2.5;
/// The card bar's playhead: a short upright bar, and the gap either side of it.
const THUMB_W: f32 = 4.0;
const THUMB_H: f32 = 14.0;
const THUMB_GAP: f32 = 3.0;
/// Pausing settles the ripple flat, and playing raises it again, over this long.
const SETTLE_SECONDS: f64 = 0.4;

/// The shape that lights a hovered target.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Highlight {
    /// A ring around a cover.
    Outline,
    /// A round button's own disc, lightened.
    Disc,
    /// A glyph button's square.
    Square,
    /// A wide target (a device's row, a song's note).
    Pill,
}

/// How a new scene arrives.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Entrance {
    /// Content changed in place (a track, a mute): no movement.
    None,
    /// Another state: slides in from the edge.
    Slide,
    /// The same state changed shape (a disc or the devices came, another size): unfolds.
    Grow,
    /// The card opening: grows out of the pill that was showing (its size, in DIPs).
    Expand { from: (f32, f32) },
}

/// How `next` should arrive after `shown`.
fn entrance(shown: &View, next: &View) -> Entrance {
    if std::mem::discriminant(shown) != std::mem::discriminant(next) || matches!(next, View::Banner { .. }) {
        return Entrance::Slide;
    }
    let shape = |view: &View| match view {
        View::Idle { call, devices } => (0, call.as_ref().map(|c| c.members.len() + 1), devices.is_some(), false, 0),
        View::Music { media, card, call, devices, batteries, .. } => (media.len(), call.as_ref().map(|c| c.members.len() + 1), devices.is_some(), *card, batteries.len().div_ceil(2)),
        _ => (0, None, false, false, 0),
    };
    if shape(shown) != shape(next) {
        Entrance::Grow
    } else {
        Entrance::None
    }
}

thread_local! {
    static PILL: RefCell<Option<Pill>> = const { RefCell::new(None) };
    static ACTIONS: RefCell<VecDeque<Action>> = const { RefCell::new(VecDeque::new()) };
}

struct Pill {
    hwnd: HWND,
    edge: HWND,
    gfx: Option<Gfx>,
    view: View,
    inputs: Inputs,
    /// Where it goes: the edge, the place along it and the display.
    side: PillEdge,
    align: PillAlign,
    monitor: Option<String>,
    /// Click targets of the shown scene, in DIPs from the window's origin.
    hits: Vec<([f32; 4], Hit)>,
    /// Where some hits are lit instead ([`draw::Scene::spots`]).
    spots: Vec<(usize, [f32; 4])>,
    /// Kept alive while shown: the visuals and their animations.
    _visuals: Vec<IDCompositionVisual2>,
    /// Whether each shown ring and card bar was rippling (playing): a change settles the ripple
    /// flat or raises it, where a repaint that did not change it shows it as it was.
    waving: Waving,
    /// The shown Samsung-style rings and bars, painted again on [`LIVE_TIMER`] while they move.
    live: Vec<Live>,
    meter: Option<IAudioMeterInformation>,
    loud: u32,
    quiet: u32,
    appbar: bool,
    /// Whether the hover strip is up (see [`Pill::hoverable`]).
    edge_up: bool,
    /// The size the shown scene was drawn at.
    drawn_size: Option<crate::config::PillSize>,
    /// The shown scene's size, and the last quiet (not card) one's, in DIPs: the card grows out
    /// of the quiet pill and shrinks back into it.
    scene_size: (f32, f32),
    quiet_size: (f32, f32),
    /// The last quiet pill's hover strip, along the edge in pixels (start, length). The card's
    /// strip covers it too: a quiet column longer than the card would otherwise leave the pointer
    /// outside both once the card opened, collapse it, and open it again, over and over.
    quiet_strip: Option<(i32, i32)>,
    /// The soft highlight behind the button under the pointer, and which hit it is on.
    highlight: Option<IDCompositionVisual2>,
    hover_hit: Option<usize>,
    /// The card is shrinking back; [`COLLAPSE_TIMER`] lets the quiet pill in when it is done.
    collapsing: bool,
    /// The last hub state, for re-deciding on hover, meter and fullscreen changes.
    last: State,
}

/// Create the pill's windows (hidden until the first [`update`]). Errors leave no pill: the app
/// works without it.
pub fn create(config: &crate::config::Pill) -> Result<()> {
    let ex = WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST | WS_EX_NOREDIRECTIONBITMAP | WS_EX_LAYERED;
    let hwnd = crate::win::create_popup(w!("AudioTrayPill"), w!("Audio Tray"), Some(wndproc), ex, (0, 0, 1, 1))
        .context("create the pill window")?;
    unsafe { SetLayeredWindowAttributes(hwnd, Default::default(), 255, LWA_ALPHA) }?;
    // The hover strip: alpha 1 of 255 is invisible but still takes the pointer.
    let edge = crate::win::create_popup(
        w!("AudioTrayPillEdge"),
        w!("Audio Tray"),
        Some(wndproc),
        WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST | WS_EX_LAYERED,
        (0, 0, 1, 1),
    )
    .context("create the pill's hover strip")?;
    unsafe { SetLayeredWindowAttributes(edge, Default::default(), 1, LWA_ALPHA) }?;

    let mut pill = Pill {
        hwnd,
        edge,
        gfx: None,
        view: View::Hidden,
        inputs: Inputs {
            enabled: config.enabled,
            listening_alerts: config.listening_alerts,
            devices: config.devices,
            size: config.size,
            announce_tracks: config.announce_tracks,
            fullscreen: fullscreen_now(),
            ..Default::default()
        },
        side: config.edge,
        align: config.align,
        monitor: config.monitor.clone(),
        hits: Vec::new(),
        spots: Vec::new(),
        _visuals: Vec::new(),
        waving: Waving::default(),
        live: Vec::new(),
        meter: None,
        loud: 0,
        quiet: 0,
        appbar: false,
        edge_up: false,
        drawn_size: None,
        scene_size: (0.0, 0.0),
        quiet_size: if config.edge.vertical() { (6.0, 56.0) } else { (56.0, 6.0) },
        quiet_strip: None,
        highlight: None,
        hover_hit: None,
        collapsing: false,
        last: State::default(),
    };
    pill.register_appbar();
    PILL.with(|cell| *cell.borrow_mut() = Some(pill));
    Ok(())
}

/// Bring the pill in line with the hub.
pub fn update(state: &State) {
    with_pill(|pill| {
        pill.last = state.clone();
        pill.refresh();
    });
}

/// Warn of a low battery (`[batteries] pill_alert`), and list the levels on the card
/// (`[batteries] pill_card`): kept with the other battery settings.
pub fn set_batteries(alerts: bool, card: bool) {
    with_pill(|pill| {
        if (pill.inputs.battery_alerts, pill.inputs.battery_card) != (alerts, card) {
            pill.inputs.battery_alerts = alerts;
            pill.inputs.battery_card = card;
            pill.refresh();
        }
    });
}

/// New settings.
pub fn configure(config: &crate::config::Pill) {
    with_pill(|pill| {
        pill.inputs.enabled = config.enabled;
        pill.inputs.listening_alerts = config.listening_alerts;
        pill.inputs.devices = config.devices;
        pill.inputs.size = config.size;
        pill.inputs.announce_tracks = config.announce_tracks;
        let moved = (pill.side, pill.align, &pill.monitor) != (config.edge, config.align, &config.monitor);
        pill.side = config.edge;
        pill.align = config.align;
        pill.monitor = config.monitor.clone();
        if moved {
            // Elsewhere, the same state is drawn again from scratch (another edge tucks another side).
            pill.quiet_size = if config.edge.vertical() { (6.0, 56.0) } else { (56.0, 6.0) };
            // Its strip was on another edge or display.
            pill.quiet_strip = None;
            pill.redraw();
        }
        pill.refresh();
    });
}

/// Dev preview: open the card as if the pointer were on the pill.
#[cfg(feature = "dev")]
pub fn preview_hover() {
    with_pill(|pill| pill.set_hover(true));
}

/// The bars' motion settings changed (see [`crate::config::motion`]): draw them again.
pub fn motion_changed() {
    with_pill(|pill| {
        pill.waving = Waving::default();
        pill.redraw();
    });
}

/// Explorer restarted: the appbar registration went with it.
pub fn taskbar_restarted() {
    with_pill(|pill| {
        pill.appbar = false;
        pill.register_appbar();
    });
}

/// Everything clicked since the last call. Tray thread.
pub fn take_actions() -> Vec<Action> {
    ACTIONS.with(|queue| queue.borrow_mut().drain(..).collect())
}

pub fn destroy() {
    PILL.with(|cell| {
        if let Some(pill) = cell.borrow_mut().take() {
            pill.unregister_appbar();
            unsafe {
                let _ = windows::Win32::UI::WindowsAndMessaging::DestroyWindow(pill.hwnd);
                let _ = windows::Win32::UI::WindowsAndMessaging::DestroyWindow(pill.edge);
            }
        }
    });
}

/// The display the pill goes on, as an index into `monitors`: the one the settings name, else the
/// outermost on its edge's side of the desktop, the primary among equals. On a display with
/// another beyond that edge, the edge is mid-desktop and the hover strip would open the card
/// whenever the pointer crossed from one screen to the other.
pub fn display_for(edge: PillEdge, monitor: Option<&str>, monitors: &[Monitor]) -> Option<usize> {
    if let Some(named) = monitor.and_then(|name| monitors.iter().position(|m| m.name == name)) {
        return Some(named);
    }
    let outermost = |m: &Monitor| match edge {
        PillEdge::Left => m.rect.left,
        PillEdge::Right => -m.rect.right,
        PillEdge::Top => m.rect.top,
    };
    monitors.iter().enumerate().min_by_key(|(_, m)| (outermost(m), !m.primary)).map(|(i, _)| i)
}

fn with_pill<R>(f: impl FnOnce(&mut Pill) -> R) -> Option<R> {
    PILL.with(|cell| cell.try_borrow_mut().ok().and_then(|mut pill| pill.as_mut().map(f)))
}

fn act(action: Action) {
    ACTIONS.with(|queue| queue.borrow_mut().push_back(action));
    crate::tray::post(WM_PILL_ACTION, 0, 0);
}

fn fullscreen_now() -> bool {
    matches!(
        unsafe { SHQueryUserNotificationState() },
        Ok(QUNS_BUSY | QUNS_RUNNING_D3D_FULL_SCREEN | QUNS_PRESENTATION_MODE)
    )
}

impl Pill {
    fn register_appbar(&mut self) {
        let mut data = APPBARDATA {
            cbSize: std::mem::size_of::<APPBARDATA>() as u32,
            hWnd: self.hwnd,
            uCallbackMessage: WM_APPBAR,
            ..Default::default()
        };
        // Registered for the fullscreen notification only: never ABM_SETPOS, so no space is reserved.
        self.appbar = unsafe { SHAppBarMessage(ABM_NEW, &mut data) } != 0;
    }

    fn unregister_appbar(&self) {
        if self.appbar {
            let mut data = APPBARDATA { cbSize: std::mem::size_of::<APPBARDATA>() as u32, hWnd: self.hwnd, ..Default::default() };
            unsafe { SHAppBarMessage(ABM_REMOVE, &mut data) };
        }
    }

    /// Re-decide and, when the state changed, repaint.
    fn refresh(&mut self) {
        // Mid-collapse the card keeps shrinking; `finish_collapse` refreshes when it is done.
        if self.collapsing {
            return;
        }
        // Asked each time rather than kept from the appbar notifications: a state read at logon
        // (reported busy) or a lost "closed" notification would otherwise hide the pill for good.
        self.inputs.fullscreen = fullscreen_now();
        let state = self.last.clone();
        self.follow_meter(&state);
        let mut next = view::decide(&state, self.inputs);
        // While the card is open its rows stay put: one whose session went away (a tab closed,
        // a player reset on pause) stays, paused, rather than slide the next under the pointer.
        if let (View::Music { media: new, card: true, .. }, View::Music { media: old, card: true, .. }) = (&mut next, &self.view) {
            let mut kept: Vec<crate::hub::Media> = old
                .iter()
                .map(|o| new.iter().find(|n| n.app_id == o.app_id).cloned().unwrap_or_else(|| crate::hub::Media { playing: false, ..o.clone() }))
                .collect();
            kept.extend(new.iter().filter(|n| !old.iter().any(|o| o.app_id == n.app_id)).cloned());
            *new = kept;
        }
        // Likewise a fanned-out device row keeps its discs in place while it stays open.
        if let (Some(new), Some(old)) = (next.fan_mut(), self.view.fan()) {
            if new.flow == old.flow {
                new.keep_order(old);
            }
        }
        // Same state, same hover strip: nothing to repaint.
        let resized = self.drawn_size != Some(self.inputs.size);
        if next == self.view && self.gfx.is_some() && self.hoverable(&next) == self.edge_up && !resized {
            return;
        }
        let opens_card = matches!(next, View::Music { card: true, .. }) && !matches!(self.view, View::Music { card: true, .. });
        let entrance = match entrance(&self.view, &next) {
            // The card grows out of the pill that was showing.
            _ if opens_card => Entrance::Expand { from: self.scene_size },
            // Same state at another size: it unfolds into the new one.
            Entrance::None if resized => Entrance::Grow,
            other => other,
        };
        if let Err(e) = self.show(&next, entrance) {
            eprintln!("pill: could not draw ({e:#}); rebuilding the graphics next time");
            self.gfx = None;
        }
        self.view = next;
    }

    /// Draw the shown state again from scratch: re-placed, and re-scaled for its display.
    fn redraw(&mut self) {
        // The quiet strip is kept: a quiet view records its new one as it is shown, and an open
        // card without it would shrink its strip and loop (see `quiet_strip`).
        let view = std::mem::replace(&mut self.view, View::Hidden);
        if let Err(e) = self.show(&view, Entrance::None) {
            eprintln!("pill: could not re-place ({e:#})");
            self.gfx = None;
        }
        self.view = view;
    }

    /// The scale and work area of the display the pill goes on (see [`display_for`]).
    fn screen(&self) -> (f32, RECT) {
        let monitors = crate::win::monitors();
        let centre = match display_for(self.side, self.monitor.as_deref(), &monitors) {
            Some(i) => {
                let r = monitors[i].rect;
                POINT { x: (r.left + r.right) / 2, y: (r.top + r.bottom) / 2 }
            }
            None => POINT::default(),
        };
        crate::win::monitor_at(centre)
    }

    /// The point, in pixels of a `w` × `h` DIP scene, that it grows out of and shrinks back into:
    /// on the edge, where along it the pill is pinned.
    fn anchor(&self, (w, h): (f32, f32), scale: f32) -> (f32, f32) {
        let along = match self.align {
            PillAlign::Start => 0.0,
            PillAlign::Center => 0.5,
            PillAlign::End => 1.0,
        };
        let (x, y) = match self.side {
            PillEdge::Left => (0.0, h * along),
            PillEdge::Right => (w, h * along),
            PillEdge::Top => (w * along, 0.0),
        };
        (x * scale, y * scale)
    }

    /// Paint and place `view`; `slide` animates it in from the edge.
    fn show(&mut self, view: &View, entrance: Entrance) -> Result<()> {
        let (scale, work) = self.screen();
        if self.gfx.as_ref().is_none_or(|gfx| gfx.scale != scale) {
            self.gfx = Some(Gfx::new(self.hwnd, scale)?);
        }
        let gfx = self.gfx.as_ref().expect("just built");
        let small = self.inputs.size == crate::config::PillSize::Small;
        self.drawn_size = Some(self.inputs.size);
        // At the start of an edge there is no room before the pills for a song's note.
        let place = draw::Place { edge: self.side, note_after: self.align == PillAlign::Start };
        let Some(scene) = draw::scene(gfx, view, small, place)? else {
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
                let _ = ShowWindow(self.edge, SW_HIDE);
            }
            self.hits.clear();
            self.spots.clear();
            self._visuals.clear();
            self.live.clear();
            self.follow_live();
            self.edge_up = false;
            return Ok(());
        };
        let (visuals, live) = self.compose(gfx, &scene, entrance)?;
        self.waving = Waving::of(&scene);
        self.live = live;
        let px = |v: f32| (v * scale).round() as i32;
        let (w, h) = (px(scene.w), px(scene.h));
        let vertical = self.side.vertical();
        let (start, end) = if vertical { (work.top, work.bottom) } else { (work.left, work.right) };
        let len = if vertical { h } else { w };
        // Placed by what stays (the pills), whatever comes before it (a song's note): the pills
        // never move for it.
        let lead = px(scene.lead);
        let along = match self.align {
            PillAlign::Start => start + px(EDGE_MARGIN),
            PillAlign::Center => start + (end - start - (len - lead)) / 2 - lead,
            PillAlign::End => end - px(EDGE_MARGIN) - len,
        };
        let (x, y) = match self.side {
            PillEdge::Left => (work.left, along),
            PillEdge::Right => (work.right - w, along),
            PillEdge::Top => (along, work.top),
        };
        unsafe {
            set_click_through(self.hwnd, view.click_through());
            SetWindowPos(self.hwnd, Some(HWND_TOPMOST), x, y, w, h, SWP_NOACTIVATE | SWP_SHOWWINDOW)?;
            if self.hoverable(view) {
                // Centred on the pills, kept on the edge.
                let mut strip = px(EDGE_H).max(len - lead);
                let mut strip_at = (along + lead + (len - lead) / 2 - strip / 2).clamp(start, (end - strip).max(start));
                if !matches!(view, View::Music { card: true, .. }) {
                    self.quiet_strip = Some((strip_at, strip));
                } else if let Some((quiet_at, quiet)) = self.quiet_strip {
                    let stop = (strip_at + strip).max(quiet_at + quiet);
                    strip_at = strip_at.min(quiet_at);
                    strip = stop - strip_at;
                }
                let (sx, sy, sw, sh) = match self.side {
                    PillEdge::Left => (work.left, strip_at, 2, strip),
                    PillEdge::Right => (work.right - 2, strip_at, 2, strip),
                    PillEdge::Top => (strip_at, work.top, strip, 2),
                };
                SetWindowPos(self.edge, Some(HWND_TOPMOST), sx, sy, sw, sh, SWP_NOACTIVATE | SWP_SHOWWINDOW)?;
            } else {
                let _ = ShowWindow(self.edge, SW_HIDE);
            }
            self.edge_up = self.hoverable(view);
        }
        gfx.commit()?;
        let lit = self.hover_hit.and_then(|i| self.hits.get(i).copied());
        self.hits = scene.hits;
        self.spots = scene.spots;
        self._visuals = visuals;
        self.follow_live();
        self.scene_size = (scene.w, scene.h);
        if !matches!(view, View::Music { card: true, .. }) {
            self.quiet_size = (scene.w, scene.h);
        }
        // The highlight went with the old visuals: put it back under a pointer that stayed put, at
        // once when on the same button (a repaint for someone speaking must not blink it).
        self.highlight = None;
        self.hover_hit = None;
        self.rehover(lit);
        Ok(())
    }

    /// The card's hover just ended: shrink it back toward the quiet pill and fade it, then let the
    /// quiet pill in when [`COLLAPSE_TIMER`] fires. `false` when there is nothing to animate.
    fn start_collapse(&mut self) -> bool {
        let (Some(gfx), Some(holder)) = (self.gfx.as_ref(), self._visuals.first()) else {
            return false;
        };
        let (w, h) = self.scene_size;
        let (qw, qh) = self.quiet_size;
        let (ax, ay) = self.anchor((w, h), gfx.scale);
        let shrank = (|| -> Result<()> {
            unsafe {
                let scale = gfx.dcomp.CreateScaleTransform()?;
                scale.SetCenterX2(ax)?;
                scale.SetCenterY2(ay)?;
                let sx = gfx.dcomp.CreateAnimation()?;
                ease_out(&sx, 1.0, (qw / w).clamp(0.05, 1.0), COLLAPSE_SECONDS)?;
                let sy = gfx.dcomp.CreateAnimation()?;
                ease_out(&sy, 1.0, (qh / h).clamp(0.05, 1.0), COLLAPSE_SECONDS)?;
                scale.SetScaleX(&sx)?;
                scale.SetScaleY(&sy)?;
                holder.SetTransform(&scale)?;
                let fade = gfx.dcomp.CreateAnimation()?;
                ease_out(&fade, 1.0, 0.0, COLLAPSE_SECONDS)?;
                let effect = gfx.dcomp.CreateEffectGroup()?;
                effect.SetOpacity(&fade)?;
                holder.SetEffect(&effect)?;
            }
            gfx.commit()
        })();
        if shrank.is_err() {
            return false;
        }
        self.collapsing = true;
        unsafe { SetTimer(Some(self.hwnd), COLLAPSE_TIMER, (COLLAPSE_SECONDS * 1000.0) as u32 + 10, None) };
        true
    }

    /// The collapse has played: show the quiet pill.
    fn finish_collapse(&mut self) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), COLLAPSE_TIMER);
        }
        if !self.collapsing {
            return;
        }
        self.collapsing = false;
        self.inputs.hover = false;
        self.refresh();
    }

    /// The pointer came back while the card was shrinking: keep the card.
    fn cancel_collapse(&mut self) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), COLLAPSE_TIMER);
        }
        self.collapsing = false;
        let view = self.view.clone();
        if let Err(e) = self.show(&view, Entrance::None) {
            eprintln!("pill: could not draw ({e:#})");
            self.gfx = None;
        }
    }

    /// Put the soft highlight on hit `index` (or take it away): fading in under a hovering
    /// pointer (unless `fade` is off), springing in on a press.
    fn set_highlight(&mut self, index: Option<usize>, pressed: bool, fade: bool) {
        let Some(gfx) = self.gfx.as_ref() else { return };
        let Some(holder) = self._visuals.first() else { return };
        // A whole-scene target (a banner, the call) is a button in itself, and a fanned row's
        // background no button at all: no spot to light.
        let index = index.filter(|&i| !matches!(self.hits.get(i), Some((_, Hit::Whole | Hit::Fan(_))) | None));
        if index == self.hover_hit && !pressed {
            return;
        }
        let lit = (|| -> Result<Option<IDCompositionVisual2>> {
            unsafe {
                if let Some(old) = &self.highlight {
                    holder.RemoveVisual(old)?;
                }
                let Some(i) = index else { return Ok(None) };
                let (rect, hit) = self.hits[i];
                // A target lit elsewhere than where it is hit (the call pill's microphone) is a disc there.
                let spot = self.spots.iter().find(|(at, _)| *at == i).map(|(_, r)| *r);
                let [x, y, w, h] = spot.unwrap_or(rect);
                let card = matches!(self.view, View::Music { card: true, .. });
                // Drawn over the scene, so never a wash over a picture: a cover gets an outline; a
                // button a soft shape behind its glyph's look, its own (a disc on a disc).
                let shape = match hit {
                    // The quiet column's covers carry the progress ring: no outline to add there.
                    _ if spot.is_some() => Highlight::Disc,
                    Hit::Focus(_) if !card => return Ok(None),
                    Hit::Focus(_) => Highlight::Outline,
                    Hit::Device(_) | Hit::Choice(..) | Hit::CallMute | Hit::CallLeave if w < 2.0 * h => Highlight::Disc,
                    Hit::Media(..) => Highlight::Square,
                    _ => Highlight::Pill,
                };
                let pad = if shape == Highlight::Outline { -3.0 } else { 2.0 };
                let (x, y, w, h) = (x + pad, y + pad, w - 2.0 * pad, h - 2.0 * pad);
                // Pressed is quieter than hovered, as Windows' buttons are.
                let alpha = match (shape, pressed) {
                    (Highlight::Outline, false) => 0.55,
                    (Highlight::Outline, true) => 0.85,
                    (_, false) => 0.10,
                    (_, true) => 0.06,
                };
                let color = gfx::Rgba::hex(0xFFFFFF, alpha);
                // The visual on whole pixels, what is left of the offset drawn inside it: a visual
                // between pixels has its anti-aliased curves resampled into steps.
                let (px, py) = ((x * gfx.scale).floor(), (y * gfx.scale).floor());
                let (dx, dy) = (x - px / gfx.scale, y - py / gfx.scale);
                let (visual, _) = gfx.painted(w + 1.0, h + 1.0, |p| match shape {
                    // Concentric with the cover's own corners (8): its centre line 2 out.
                    Highlight::Outline => p.round_rect_outline((dx, dy, w, h), 10.0, 2.0, color),
                    Highlight::Disc => p.circle(dx + w / 2.0, dy + h / 2.0, w.min(h) / 2.0, color),
                    Highlight::Square => p.round_rect(dx, dy, w, h, 6.0, color),
                    Highlight::Pill => p.round_rect(dx, dy, w, h, w.min(h) / 2.0, color),
                })?;
                visual.SetOffsetX2(px)?;
                visual.SetOffsetY2(py)?;
                if !pressed && fade {
                    let fade = gfx.dcomp.CreateAnimation()?;
                    ease_out(&fade, 0.0, 1.0, HIGHLIGHT_SECONDS)?;
                    let effect = gfx.dcomp.CreateEffectGroup()?;
                    effect.SetOpacity(&fade)?;
                    visual.SetEffect(&effect)?;
                }
                holder.AddVisual(&visual, false, None)?;
                Ok(Some(visual))
            }
        })();
        match lit {
            Ok(visual) => {
                self.highlight = visual;
                self.hover_hit = index;
                let _ = gfx.commit();
            }
            Err(e) => eprintln!("pill: could not light a button ({e:#})"),
        }
    }

    /// Light whatever button is under the pointer now (after a repaint). `lit` was lit before it:
    /// still there, the light comes back without fading in again.
    fn rehover(&mut self, lit: Option<([f32; 4], Hit)>) {
        let mut point = POINT::default();
        if unsafe { GetCursorPos(&mut point) }.is_err() || !cursor_in(self.hwnd) {
            return;
        }
        if unsafe { ScreenToClient(self.hwnd, &mut point) }.as_bool() {
            let index = self.hit_index(point.x, point.y);
            let same = lit.is_some() && index.map(|i| self.hits[i]) == lit;
            self.set_highlight(index, false, !same);
        }
    }

    /// Which hit is under client pixel `(x, y)`.
    fn hit_index(&self, x: i32, y: i32) -> Option<usize> {
        let scale = self.gfx.as_ref().map_or(1.0, |g| g.scale);
        let (x, y) = (x as f32 / scale, y as f32 / scale);
        self.hits.iter().position(|([hx, hy, hw, hh], _)| x >= *hx && x < hx + hw && y >= *hy && y < hy + hh)
    }

    /// Hang the scene under the root: its content, the ring's two rotating halves, the speaking
    /// pulses, and the entrance. Returns the visuals to keep alive. Every movement is a
    /// compositor animation set here once.
    fn compose(&self, gfx: &Gfx, scene: &Scene, entrance: Entrance) -> Result<(Vec<IDCompositionVisual2>, Vec<Live>)> {
        let motion = crate::config::motion();
        unsafe {
            gfx.root.RemoveAllVisuals()?;
            let holder = gfx.dcomp.CreateVisual()?;
            holder.AddVisual(&scene.content, false, None)?;
            let mut keep = vec![holder.clone(), scene.content.clone()];
            let mut live = Vec::new();
            for (i, &ring) in scene.rings.iter().enumerate() {
                let was = self.waving.rings.get(i).copied();
                match motion.ring {
                    MotionStyle::None => keep.extend(ring_visuals(gfx, &holder, ring)?),
                    style => live.push(Live::ring(gfx, &holder, ring, was, style)?),
                }
            }
            for (i, &bar) in scene.bars.iter().enumerate() {
                let was = self.waving.bars.get(i).copied();
                match motion.bar {
                    MotionStyle::Samsung => live.push(Live::bar(gfx, &holder, bar, was)?),
                    MotionStyle::Google => keep.extend(bar_visuals(gfx, &holder, bar, was)?),
                    MotionStyle::None => keep.extend(plain_bar_visuals(gfx, &holder, bar)?),
                }
            }
            keep.extend(live.iter().map(|l| l.visual.clone()));
            for &pulse in &scene.pulses {
                keep.push(pulse_visual(gfx, &holder, pulse)?);
            }
            gfx.root.AddVisual(&holder, false, None)?;
            let fade_from = match entrance {
                Entrance::None => return Ok((keep, live)),
                Entrance::Slide => {
                    // From just off the edge to its place, easing out.
                    let offset = gfx.dcomp.CreateAnimation()?;
                    match self.side {
                        PillEdge::Left => ease_out(&offset, -scene.w * gfx.scale, 0.0, SLIDE_SECONDS)?,
                        PillEdge::Right => ease_out(&offset, scene.w * gfx.scale, 0.0, SLIDE_SECONDS)?,
                        PillEdge::Top => ease_out(&offset, -scene.h * gfx.scale, 0.0, SLIDE_SECONDS)?,
                    }
                    if self.side.vertical() {
                        holder.SetOffsetX(&offset)?;
                    } else {
                        holder.SetOffsetY(&offset)?;
                    }
                    0.0
                }
                Entrance::Grow => {
                    // Unfolds from the edge: a slight scale up from where it is pinned.
                    let (ax, ay) = self.anchor((scene.w, scene.h), gfx.scale);
                    let scale = gfx.dcomp.CreateScaleTransform()?;
                    scale.SetCenterX2(ax)?;
                    scale.SetCenterY2(ay)?;
                    let grow = gfx.dcomp.CreateAnimation()?;
                    ease_out(&grow, 0.9, 1.0, GROW_SECONDS)?;
                    scale.SetScaleX(&grow)?;
                    scale.SetScaleY(&grow)?;
                    holder.SetTransform(&scale)?;
                    0.35
                }
                Entrance::Expand { from: (fw, fh) } => {
                    // From the pill's own size to the card's, anchored where it is pinned.
                    let (ax, ay) = self.anchor((scene.w, scene.h), gfx.scale);
                    let scale = gfx.dcomp.CreateScaleTransform()?;
                    scale.SetCenterX2(ax)?;
                    scale.SetCenterY2(ay)?;
                    let sx = gfx.dcomp.CreateAnimation()?;
                    ease_out(&sx, (fw / scene.w).clamp(0.05, 1.0), 1.0, EXPAND_SECONDS)?;
                    let sy = gfx.dcomp.CreateAnimation()?;
                    ease_out(&sy, (fh / scene.h).clamp(0.05, 1.0), 1.0, EXPAND_SECONDS)?;
                    scale.SetScaleX(&sx)?;
                    scale.SetScaleY(&sy)?;
                    holder.SetTransform(&scale)?;
                    0.0
                }
            };
            let seconds = match entrance {
                Entrance::Slide => SLIDE_SECONDS,
                // The content arrives a little before the card has finished growing.
                Entrance::Expand { .. } => EXPAND_SECONDS * 0.7,
                _ => GROW_SECONDS,
            };
            let fade = gfx.dcomp.CreateAnimation()?;
            ease_out(&fade, fade_from, 1.0, seconds)?;
            let effect = gfx.dcomp.CreateEffectGroup()?;
            effect.SetOpacity(&fade)?;
            holder.SetEffect(&effect)?;
            Ok((keep, live))
        }
    }

    /// Run [`LIVE_TIMER`] while a live ring or bar moves, and stop it once they are all still.
    fn follow_live(&mut self) {
        let moving = self.live.iter().any(Live::moving);
        unsafe {
            if moving {
                SetTimer(Some(self.hwnd), LIVE_TIMER, LIVE_MS, None);
            } else {
                let _ = KillTimer(Some(self.hwnd), LIVE_TIMER);
            }
        }
    }

    /// Paint each live ring and bar again.
    fn live_tick(&mut self) {
        let Some(gfx) = self.gfx.as_ref() else { return };
        let now = std::time::Instant::now();
        for live in &mut self.live {
            if let Err(e) = live.paint(gfx, now) {
                eprintln!("pill: could not paint a live bar ({e:#})");
            }
        }
        let _ = gfx.commit();
        self.follow_live();
    }

    /// Whether the hover strip is up: on the music pill, on the sliver while some media is paused
    /// (hovering opens its card to resume it), and always in compact mode.
    fn hoverable(&self, view: &View) -> bool {
        match view {
            View::Music { .. } => true,
            View::Idle { .. } => self.inputs.size != crate::config::PillSize::Big || !self.last.all_media().is_empty(),
            _ => false,
        }
    }

    /// Start or stop the 10 Hz microphone meter: it runs only while muted in a call.
    fn follow_meter(&mut self, state: &State) {
        // Only for the "you're muted" banner: no pill, or a fullscreen app hiding it, no meter.
        let shown = self.inputs.enabled && !self.inputs.fullscreen;
        let wanted = shown && view::call_mute(state).is_some_and(|(_, muted)| muted);
        if wanted && self.meter.is_none() {
            match capture_meter() {
                Ok(meter) => {
                    self.meter = Some(meter);
                    unsafe { SetTimer(Some(self.hwnd), METER_TIMER, METER_MS, None) };
                }
                Err(e) => eprintln!("pill: no microphone meter ({e:#})"),
            }
        } else if !wanted && self.meter.is_some() {
            self.meter = None;
            unsafe {
                let _ = KillTimer(Some(self.hwnd), METER_TIMER);
            }
            self.loud = 0;
            self.quiet = 0;
            self.inputs.talking = false;
        }
    }

    fn meter_tick(&mut self) {
        let peak = self.meter.as_ref().and_then(|m| unsafe { m.GetPeakValue() }.ok()).unwrap_or(0.0);
        if peak > TALK_PEAK {
            self.loud += 1;
            self.quiet = 0;
        } else {
            self.quiet += 1;
            self.loud = 0;
        }
        let talking = if self.inputs.talking { self.quiet < QUIET_TICKS } else { self.loud >= TALK_TICKS };
        if talking != self.inputs.talking {
            self.inputs.talking = talking;
            self.refresh();
        }
    }

    fn set_hover(&mut self, hover: bool) {
        if hover && self.collapsing {
            return self.cancel_collapse();
        }
        if self.inputs.hover == hover {
            return;
        }
        // Leaving the card: it shrinks back before the quiet pill returns.
        if !hover && matches!(self.view, View::Music { card: true, .. }) && self.start_collapse() {
            return;
        }
        self.inputs.hover = hover;
        self.refresh();
    }

    /// Fan out the device row under the pointer (hit `index`), naming the disc it is on; close it
    /// when the pointer is elsewhere. The card has its own device row.
    fn follow_fan(&mut self, index: Option<usize>) {
        if matches!(self.view, View::Music { card: true, .. }) {
            return;
        }
        let fan = match index.map(|i| self.hits[i].1) {
            // Just opened: the current device comes first.
            Some(Hit::Device(flow)) => Some((flow, Some(0))),
            Some(Hit::Choice(flow, i)) => Some((flow, Some(i))),
            // Between two discs: as it was.
            Some(Hit::Fan(flow)) => self.inputs.fan.filter(|(f, _)| *f == flow),
            _ => None,
        };
        self.set_fan(fan);
    }

    fn set_fan(&mut self, fan: Option<(crate::audio::Flow, Option<usize>)>) {
        if self.inputs.fan != fan {
            self.inputs.fan = fan;
            self.refresh();
        }
    }

    /// What is under client pixel `(x, y)`.
    fn hit_at(&self, x: i32, y: i32) -> Option<Hit> {
        self.hit_index(x, y).map(|i| self.hits[i].1)
    }

    fn click(&mut self, x: i32, y: i32, right: bool) {
        let hit = self.hit_at(x, y);
        match (&self.view, hit) {
            (_, Some(Hit::Device(flow))) => act(Action::CycleDevice(flow)),
            // Another device of a fanned row; the current one is already the default.
            (view, Some(Hit::Choice(flow, i))) => {
                if let Some(choice) = view.fan().and_then(|f| f.choices.get(i).filter(|c| c.id != f.current)) {
                    act(Action::SetDevice(flow, choice.id.clone()));
                }
            }
            (View::Music { media, .. }, Some(Hit::Focus(i))) => {
                if let Some(m) = media.get(i) {
                    act(Action::Focus(m.app_id.clone()));
                }
            }
            (View::Music { media, .. }, Some(Hit::Media(i, command))) => {
                if let Some(m) = media.get(i) {
                    act(Action::Media(m.app_id.clone(), command));
                }
            }
            // A song's note: the player to the front (a right click only dismisses).
            (View::Music { song: Some(song), .. }, Some(Hit::Song)) => {
                if !right {
                    act(Action::Focus(song.app_id.clone()));
                }
                act(Action::Acknowledge(song.notice.clone()));
            }
            (View::Idle { call: Some(call), .. } | View::Music { call: Some(call), .. }, Some(Hit::CallMute)) => {
                act(Action::ToggleMute(call.app))
            }
            (View::Music { call: Some(call), .. }, Some(Hit::CallLeave)) => {
                if let Some(app) = call.app {
                    act(Action::LeaveCall(app));
                }
            }
            (View::Banner { action, .. }, Some(_)) => match (action, right) {
                // Right click only dismisses.
                (view::Action::MuteAndAcknowledge(notice) | view::Action::Dismiss(notice), true) => {
                    act(Action::Acknowledge(notice.clone()))
                }
                (view::Action::MuteAndAcknowledge(notice), false) => {
                    act(Action::MuteMicrophone);
                    act(Action::Acknowledge(notice.clone()));
                }
                (view::Action::Dismiss(notice), false) => act(Action::Acknowledge(notice.clone())),
                (view::Action::Unmute(app), _) => act(Action::Unmute(*app)),
            },
            _ => {}
        }
    }
}

/// Whether each ring and card bar of a scene ripples (see [`Pill::waving`]).
#[derive(Default)]
struct Waving {
    rings: Vec<bool>,
    bars: Vec<bool>,
}

impl Waving {
    fn of(scene: &Scene) -> Self {
        Waving { rings: scene.rings.iter().map(|r| r.playing).collect(), bars: scene.bars.iter().map(|b| b.playing).collect() }
    }
}

/// The None style's ring, in two halves: a right-half clip for 0–180° and a left-half one for
/// 180–360°. In each, the filled arc is a whole ring behind a half-plane clip that turns with the
/// progress, so it shows from the top to the playhead; the rest of the track is a whole ring
/// behind the other half-plane, turned with it. Each angle is a compositor animation from now to
/// the end of the track, so a playing ring costs nothing between changes.
fn ring_visuals(gfx: &Gfx, holder: &IDCompositionVisual2, ring: Ring) -> Result<Vec<IDCompositionVisual2>> {
    let reach = ring.r + ring.stroke;
    let size = 2.0 * reach;
    let scale = gfx.scale;
    let (_, flat) = gfx.painted(size, size, |p| p.ring(reach, reach, ring.r, ring.stroke, ring.color))?;
    let (_, track) = gfx.painted(size, size, |p| p.ring(reach, reach, ring.r, ring.stroke, draw::TRACK))?;
    // Degrees now, and degrees per second.
    let now = ring.fraction.clamp(0.0, 1.0) * 360.0;
    let rate = ring.rate * 360.0;
    // The left or right half of the square, in pixels.
    let half = |left: bool| -> Result<IDCompositionRectangleClip> {
        unsafe {
            let clip = gfx.dcomp.CreateRectangleClip()?;
            let (l, r) = if left { (0.0, reach * scale) } else { (reach * scale, size * scale) };
            clip.SetLeft2(l)?;
            clip.SetRight2(r)?;
            clip.SetTop2(0.0)?;
            clip.SetBottom2(size * scale)?;
            Ok(clip)
        }
    };
    let mut keep = Vec::new();
    for clip_left in [false, true] {
        unsafe {
            let container = gfx.dcomp.CreateVisual()?;
            container.SetOffsetX2((ring.cx - reach) * scale)?;
            container.SetOffsetY2((ring.cy - reach) * scale)?;
            container.SetClip(&half(clip_left)?)?;
            // The right clip shows angles 0–180 (the first half), the left 180–360: an angle in
            // this half that holds until the playhead reaches it, then follows it to 180.
            let base = if clip_left { 180.0 } else { 0.0 };
            let angle = gfx.dcomp.CreateAnimation()?;
            piecewise(&angle, (now - base).clamp(0.0, 180.0), rate, if clip_left { (180.0 - now).max(0.0) } else { 0.0 })?;
            let rotate = gfx.dcomp.CreateRotateTransform()?;
            rotate.SetCenterX2(reach * scale)?;
            rotate.SetCenterY2(reach * scale)?;
            rotate.SetAngle(&angle)?;
            // The rest of the track behind this clip's own half-plane, the filled arc behind the
            // other, both turned to the playhead.
            for (content, own_half) in [(&track, true), (&flat, false)] {
                let part = gfx.dcomp.CreateVisual()?;
                part.SetContent(content)?;
                part.SetClip(&half(if own_half { clip_left } else { !clip_left })?)?;
                part.SetTransform(&rotate)?;
                part.SetBorderMode(DCOMPOSITION_BORDER_MODE_SOFT)?;
                container.AddVisual(&part, false, None)?;
                keep.push(part);
            }
            // `false` with no reference visual puts it on top of its siblings, over the content.
            holder.AddVisual(&container, false, None)?;
            keep.push(container);
        }
    }
    Ok(keep)
}

/// Which fills to show, rippling or not, each with the opacities it fades between: the one, or
/// both cross-fading when the ripple starts or stops.
fn fills(waving: bool, was: Option<bool>) -> Vec<(bool, Option<(f32, f32)>)> {
    match was {
        Some(was) if was != waving => vec![(was, Some((1.0, 0.0))), (waving, Some((0.0, 1.0)))],
        _ => vec![(waving, None)],
    }
}

/// Fade `visual` between two opacities over [`SETTLE_SECONDS`]; `None` leaves it opaque.
fn faded(gfx: &Gfx, visual: &IDCompositionVisual2, fade: Option<(f32, f32)>) -> Result<()> {
    if let Some((from, to)) = fade {
        unsafe {
            let opacity = gfx.dcomp.CreateAnimation()?;
            ease_out(&opacity, from, to, SETTLE_SECONDS)?;
            let effect = gfx.dcomp.CreateEffectGroup()?;
            effect.SetOpacity(&opacity)?;
            visual.SetEffect(&effect)?;
        }
    }
    Ok(())
}

/// A card bar's moving parts: the filled part up to just before the playhead, the playhead, and
/// the rest of the track from just after it, each moved by DWM from now to the end of the track.
/// While playing, the filled part is a wave strip a wavelength longer than the bar, slid by one
/// wavelength on repeat; paused, it is a flat bar. `was` as for [`ring_visuals`].
fn bar_visuals(gfx: &Gfx, holder: &IDCompositionVisual2, bar: draw::Bar, was: Option<bool>) -> Result<Vec<IDCompositionVisual2>> {
    let scale = gfx.scale;
    let band = THUMB_H.max(2.0 * BAR_AMP + bar.h + 2.0);
    let (top, mid) = (bar.y + bar.h / 2.0 - band / 2.0, band / 2.0);
    let w = bar.w;
    // The playhead, in pixels from the bar's start: now, and how fast it moves.
    let (now, speed, end) = (bar.fraction.clamp(0.0, 1.0) * w * scale, bar.rate * w * scale, w * scale);
    // A position that keeps `by` pixels from the playhead.
    let follow = |by: f32| -> Result<IDCompositionAnimation> {
        let at = unsafe { gfx.dcomp.CreateAnimation()? };
        glide(&at, now + by, end + by, speed)?;
        Ok(at)
    };
    // The playhead, and the gaps either side of it: none on a live stream.
    let beside = if bar.knob { (THUMB_W / 2.0 + THUMB_GAP) * scale } else { 0.0 };
    let mut keep = Vec::new();
    unsafe {
        // The rest of the track, from just after the playhead.
        let (rest, _) = gfx.painted(w, band, |p| p.round_rect(0.0, mid - bar.h / 2.0, w, bar.h, bar.h / 2.0, draw::TRACK))?;
        rest.SetOffsetX2(bar.x * scale)?;
        rest.SetOffsetY2(top * scale)?;
        let clip = gfx.dcomp.CreateRectangleClip()?;
        clip.SetLeft(&follow(beside)?)?;
        clip.SetRight2(end)?;
        clip.SetTop2(0.0)?;
        clip.SetBottom2(band * scale)?;
        rest.SetClip(&clip)?;
        holder.AddVisual(&rest, false, None)?;
        keep.push(rest);
        // The filled part, up to just before it.
        let filled = gfx.dcomp.CreateVisual()?;
        filled.SetOffsetX2(bar.x * scale)?;
        filled.SetOffsetY2(top * scale)?;
        let clip = gfx.dcomp.CreateRectangleClip()?;
        clip.SetLeft2(0.0)?;
        clip.SetRight(&follow(-beside)?)?;
        clip.SetTop2(0.0)?;
        clip.SetBottom2(band * scale)?;
        filled.SetClip(&clip)?;
        for (rippling, fade) in fills(bar.playing, was) {
            let fill = if rippling {
                let length = w + BAR_WAVELENGTH;
                let (strip, _) = gfx.painted(length, band, |p| p.wave_line((0.0, length), mid, BAR_AMP, BAR_WAVELENGTH, bar.h, bar.color))?;
                // From a wavelength back to its place, on repeat: the wave travels forwards.
                let slide = gfx.dcomp.CreateAnimation()?;
                cycle(&slide, -BAR_WAVELENGTH * scale, BAR_WAVELENGTH * scale, WAVE_SECONDS)?;
                strip.SetOffsetX(&slide)?;
                strip
            } else {
                gfx.painted(w, band, |p| p.round_rect(0.0, mid - bar.h / 2.0, w, bar.h, bar.h / 2.0, bar.color))?.0
            };
            faded(gfx, &fill, fade)?;
            filled.AddVisual(&fill, false, None)?;
            keep.push(fill);
        }
        holder.AddVisual(&filled, false, None)?;
        keep.push(filled);
        // The playhead.
        if bar.knob {
            let (head, _) = gfx.painted(THUMB_W, THUMB_H, |p| p.round_rect(0.0, 0.0, THUMB_W, THUMB_H, THUMB_W / 2.0, bar.color))?;
            head.SetOffsetX(&follow((bar.x - THUMB_W / 2.0) * scale)?)?;
            head.SetOffsetY2((top + mid - THUMB_H / 2.0) * scale)?;
            holder.AddVisual(&head, false, None)?;
            keep.push(head);
        }
    }
    Ok(keep)
}

/// A Samsung-style ring or card bar (see [`waves`]): a surface painted again on every
/// [`LIVE_TIMER`] tick while it moves, then left as it is.
struct Live {
    visual: IDCompositionVisual2,
    surface: windows::Win32::Graphics::DirectComposition::IDCompositionSurface,
    shape: LiveShape,
    /// When it was shown: its progress moves on from there.
    shown: std::time::Instant,
    last: std::time::Instant,
    /// The waves' height, springing to 1 while playing and 0 paused, and its speed.
    amp: f32,
    speed: f32,
}

enum LiveShape {
    /// The ring, in a square `side` DIPs wide centred on it, in the Google or the Samsung style.
    Ring(Ring, f32, MotionStyle),
    /// The Samsung bar, in a band from `(x, y)` DIPs.
    Bar(draw::Bar, (f32, f32)),
}

/// The Samsung style's dimensions: the room the waves rise into, the line's width, the knob.
const LIVE_RING_ROOM: f32 = 0.22;
/// The Samsung ring's line, as a share of the other styles' stroke: thinner, so the waves lead.
const LIVE_RING_STROKE: f32 = 0.7;
const LIVE_BAR_ROOM: f32 = 7.0;
const LIVE_BAR_KNOB: f32 = 5.0;
/// How far the knob's glow reaches, in knob radii.
const KNOB_GLOW: f32 = 2.1;

impl Live {
    fn ring(gfx: &Gfx, holder: &IDCompositionVisual2, ring: Ring, was: Option<bool>, style: MotionStyle) -> Result<Live> {
        let reach = if style == MotionStyle::Samsung {
            let stroke = ring.stroke * LIVE_RING_STROKE;
            ring.r + (ring.r * LIVE_RING_ROOM + stroke).max(stroke * 1.3 * KNOB_GLOW) + 1.0
        } else {
            ring.r + ring.stroke + ring.r * RING_AMP + 1.0
        };
        let side = 2.0 * reach;
        Live::new(gfx, holder, (ring.cx - reach, ring.cy - reach), (side, side), LiveShape::Ring(ring, side, style), ring.playing, was)
    }

    fn bar(gfx: &Gfx, holder: &IDCompositionVisual2, bar: draw::Bar, was: Option<bool>) -> Result<Live> {
        let pad = LIVE_BAR_KNOB * KNOB_GLOW;
        let cy = bar.y + bar.h / 2.0;
        let top = cy - bar.h / 2.0 - LIVE_BAR_ROOM - 1.0;
        let (x, y) = (bar.x - pad, top.min(cy - pad));
        let size = (bar.w + 2.0 * pad, (cy + pad) - y);
        Live::new(gfx, holder, (x, y), size, LiveShape::Bar(bar, (x, y)), bar.playing, was)
    }

    fn new(gfx: &Gfx, holder: &IDCompositionVisual2, (x, y): (f32, f32), (w, h): (f32, f32), shape: LiveShape, playing: bool, was: Option<bool>) -> Result<Live> {
        let (visual, surface) = gfx.painted(w, h, |_| Ok(()))?;
        unsafe {
            visual.SetOffsetX2(x * gfx.scale)?;
            visual.SetOffsetY2(y * gfx.scale)?;
            holder.AddVisual(&visual, false, None)?;
        }
        let now = std::time::Instant::now();
        let amp = if was.unwrap_or(playing) { 1.0 } else { 0.0 };
        let mut live = Live { visual, surface, shape, shown: now, last: now, amp, speed: 0.0 };
        live.paint(gfx, now)?;
        Ok(live)
    }

    fn playing(&self) -> bool {
        match &self.shape {
            LiveShape::Ring(ring, ..) => ring.playing,
            LiveShape::Bar(bar, _) => bar.playing,
        }
    }

    /// Whether it still has to be painted again: playing, or its waves still settling.
    fn moving(&self) -> bool {
        self.playing() || self.amp > 0.002 || self.speed.abs() > 0.002
    }

    /// Paint it as it is at `now`.
    fn paint(&mut self, gfx: &Gfx, now: std::time::Instant) -> Result<()> {
        // The waves' height springs to where it goes, without overshoot.
        let target = if self.playing() { 1.0 } else { 0.0 };
        let dt = now.duration_since(self.last).as_secs_f32().min(0.05);
        self.last = now;
        for _ in 0..4 {
            let pull = 120.0 * (target - self.amp) - 22.0 * self.speed;
            self.speed += pull * dt / 4.0;
            self.amp += self.speed * dt / 4.0;
        }
        if (target - self.amp).abs() < 0.002 && self.speed.abs() < 0.01 {
            (self.amp, self.speed) = (target, 0.0);
        }
        let elapsed = now.duration_since(self.shown).as_secs_f32();
        let t = clock() as f32;
        let amp = self.amp.max(0.0);
        match &self.shape {
            LiveShape::Ring(ring, side, style) => {
                let fraction = ring.fraction + ring.rate * elapsed;
                let centre = (side / 2.0, side / 2.0);
                if *style == MotionStyle::Samsung {
                    let stroke = ring.stroke * LIVE_RING_STROKE;
                    gfx.repaint(&self.surface, |p| waves::ring(p, centre, ring.r, stroke, ring.r * LIVE_RING_ROOM, fraction, (t, amp), ring.color, ring.knob))
                } else {
                    let periods = (std::f32::consts::TAU * ring.r / RING_WAVELENGTH).round().max(5.0);
                    let wave = (ring.r * RING_AMP, periods, WAVE_SECONDS as f32);
                    gfx.repaint(&self.surface, |p| waves::ripple_ring(p, centre, ring.r, ring.stroke, wave, fraction, (t, amp), ring.color, draw::TRACK))
                }
            }
            LiveShape::Bar(bar, (x, y)) => {
                let fraction = bar.fraction + bar.rate * elapsed;
                let at = (bar.x - x, bar.y + bar.h / 2.0 - y);
                gfx.repaint(&self.surface, |p| waves::bar(p, at, bar.w, bar.h, LIVE_BAR_ROOM, fraction, (t, amp), bar.color, if bar.knob { LIVE_BAR_KNOB } else { 0.0 }))
            }
        }
    }
}

/// Seconds since the app started, on one clock shared by every repeating animation (see
/// [`cycle`]).
fn clock() -> f64 {
    static EPOCH: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    EPOCH.get_or_init(std::time::Instant::now).elapsed().as_secs_f64()
}

/// A progress bar as it always was: its track, and over it the whole filled bar, painted once,
/// behind a clip whose right side DWM moves from now to the end of the track.
fn plain_bar_visuals(gfx: &Gfx, holder: &IDCompositionVisual2, bar: draw::Bar) -> Result<Vec<IDCompositionVisual2>> {
    let (track, _) = gfx.painted(bar.w, bar.h, |p| p.round_rect(0.0, 0.0, bar.w, bar.h, bar.h / 2.0, draw::TRACK))?;
    let (filled, _) = gfx.painted(bar.w, bar.h, |p| p.round_rect(0.0, 0.0, bar.w, bar.h, bar.h / 2.0, bar.color))?;
    let scale = gfx.scale;
    let (w, now) = (bar.w * scale, bar.fraction.clamp(0.0, 1.0) * bar.w * scale);
    unsafe {
        for v in [&track, &filled] {
            v.SetOffsetX2(bar.x * scale)?;
            v.SetOffsetY2(bar.y * scale)?;
            holder.AddVisual(v, false, None)?;
        }
        let clip = gfx.dcomp.CreateRectangleClip()?;
        clip.SetLeft2(0.0)?;
        clip.SetTop2(0.0)?;
        clip.SetBottom2(bar.h * scale)?;
        let right = gfx.dcomp.CreateAnimation()?;
        glide(&right, now, w, bar.rate * w)?;
        clip.SetRight(&right)?;
        filled.SetClip(&clip)?;
    }
    Ok(vec![track, filled])
}

/// From `from` to `to` at `speed` per second, then still; `speed` 0 holds at `from`.
fn glide(animation: &IDCompositionAnimation, from: f32, to: f32, speed: f32) -> Result<()> {
    unsafe {
        if speed > 0.0 && from < to {
            animation.AddCubic(0.0, from, speed, 0.0, 0.0)?;
            animation.End(((to - from) / speed) as f64, to)?;
        } else {
            animation.AddCubic(0.0, from, 0.0, 0.0, 0.0)?;
        }
    }
    Ok(())
}

/// A green ring around a speaking face that swells and fades, over and over: one animation set
/// once, repeated by DWM forever.
fn pulse_visual(gfx: &Gfx, holder: &IDCompositionVisual2, pulse: draw::Pulse) -> Result<IDCompositionVisual2> {
    const SWELL: f32 = 1.45;
    let reach = pulse.r * SWELL + 2.0;
    let size = 2.0 * reach;
    let (visual, _) = gfx.painted(size, size, |p| p.ring(reach, reach, pulse.r, 2.0, draw::PULSE))?;
    let scale = gfx.scale;
    unsafe {
        visual.SetOffsetX2((pulse.cx - reach) * scale)?;
        visual.SetOffsetY2((pulse.cy - reach) * scale)?;
        let grow = gfx.dcomp.CreateScaleTransform()?;
        grow.SetCenterX2(reach * scale)?;
        grow.SetCenterY2(reach * scale)?;
        let swell = gfx.dcomp.CreateAnimation()?;
        beat(&swell, 1.0, SWELL - 1.0)?;
        grow.SetScaleX(&swell)?;
        grow.SetScaleY(&swell)?;
        visual.SetTransform(&grow)?;
        let fade = gfx.dcomp.CreateAnimation()?;
        beat(&fade, 0.9, -0.9)?;
        let effect = gfx.dcomp.CreateEffectGroup()?;
        effect.SetOpacity(&fade)?;
        visual.SetEffect(&effect)?;
        // On top: the call pill's background is opaque.
        holder.AddVisual(&visual, false, None)?;
    }
    Ok(visual)
}

/// A ramp from `from` by `by` over each [`PULSE_SECONDS`], repeated forever (see [`cycle`]).
fn beat(animation: &IDCompositionAnimation, from: f32, by: f32) -> Result<()> {
    cycle(animation, from, by, PULSE_SECONDS)
}

/// A ramp from `from` by `by` over each `period`, repeated forever, in step with one clock shared
/// by every animation since the app started: a repaint (another face speaking, a hover) picks the
/// pulses and waves up where they were instead of restarting them all from the first frame.
fn cycle(animation: &IDCompositionAnimation, from: f32, by: f32, period: f64) -> Result<()> {
    let phase = clock() % period;
    let slope = (by as f64 / period) as f32;
    // What is left of the current cycle, then whole cycles from the next one on.
    let next = period - phase;
    unsafe {
        animation.AddCubic(0.0, from + slope * phase as f32, slope, 0.0, 0.0)?;
        animation.AddCubic(next, from, slope, 0.0, 0.0)?;
        animation.AddRepeat(next + period, period)?;
    }
    Ok(())
}

/// An angle that holds at `start` for `wait` seconds, then grows at `rate` per second until 180,
/// where it stays. `rate` 0 holds forever.
fn piecewise(animation: &IDCompositionAnimation, start: f32, rate: f32, wait: f32) -> Result<()> {
    unsafe {
        if rate <= 0.0 || start >= 180.0 {
            animation.AddCubic(0.0, start, 0.0, 0.0, 0.0)?;
            return Ok(());
        }
        let wait = if start > 0.0 { 0.0 } else { (wait / rate) as f64 };
        if wait > 0.0 {
            animation.AddCubic(0.0, start, 0.0, 0.0, 0.0)?;
        }
        animation.AddCubic(wait, start, rate, 0.0, 0.0)?;
        let full = wait + ((180.0 - start) / rate) as f64;
        animation.End(full, 180.0)?;
    }
    Ok(())
}

/// `from` to `to` over `seconds`, decelerating (a cubic that lands with zero speed).
fn ease_out(animation: &IDCompositionAnimation, from: f32, to: f32, seconds: f64) -> Result<()> {
    let d = (to - from) as f64;
    let t = seconds;
    // x(t) = from + d * (3s - 3s² + s³), s = t / T, written in t.
    unsafe {
        animation.AddCubic(0.0, from, (3.0 * d / t) as f32, (-3.0 * d / (t * t)) as f32, (d / (t * t * t)) as f32)?;
        animation.End(seconds, to)?;
    }
    Ok(())
}

fn set_click_through(hwnd: HWND, through: bool) {
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let want = if through { ex | WS_EX_TRANSPARENT.0 as isize } else { ex & !(WS_EX_TRANSPARENT.0 as isize) };
        if want != ex {
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, want);
        }
    }
}

/// The default microphone's meter, live while an app captures (no stream of our own, so the
/// "microphone in use" indicator is not ours).
fn capture_meter() -> Result<IAudioMeterInformation> {
    use windows::Win32::Media::Audio::{eCapture, eCommunications, IMMDeviceEnumerator, MMDeviceEnumerator};
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
    unsafe {
        let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let device = enumerator.GetDefaultAudioEndpoint(eCapture, eCommunications)?;
        Ok(device.Activate(CLSCTX_ALL, None)?)
    }
}

fn cursor_in(hwnd: HWND) -> bool {
    let mut point = POINT::default();
    let mut rect = RECT::default();
    let read = unsafe { GetCursorPos(&mut point).is_ok() && GetWindowRect(hwnd, &mut rect).is_ok() };
    read
        && point.x >= rect.left
        && point.x < rect.right
        && point.y >= rect.top
        && point.y < rect.bottom
}

/// `WM_SETTINGCHANGE` for "ImmersiveColorSet": the accent or the theme changed.
fn is_colour_set(lparam: LPARAM) -> bool {
    lparam.0 != 0 && unsafe { windows::core::PCWSTR(lparam.0 as *const u16).to_string() }.is_ok_and(|area| area == "ImmersiveColorSet")
}

/// Forget the accent and paint the shown state again in the new one. Both windows get the
/// broadcasts: one repaint.
fn accent_repaint(hwnd: HWND) {
    draw::accent_changed();
    with_pill(|pill| {
        if hwnd == pill.hwnd {
            pill.redraw();
        }
    });
}

fn track_leave(hwnd: HWND) {
    let mut track = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
    let _ = unsafe { TrackMouseEvent(&mut track) };
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let handled = std::panic::catch_unwind(|| handle(hwnd, msg, wparam, lparam)).unwrap_or_else(|_| {
        eprintln!("pill: a message handler panicked");
        None
    });
    match handled {
        Some(result) => result,
        None => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn handle(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
    let point = || ((lparam.0 & 0xFFFF) as i16 as i32, ((lparam.0 >> 16) & 0xFFFF) as i16 as i32);
    match msg {
        WM_MOUSEACTIVATE => Some(LRESULT(MA_NOACTIVATE as isize)),
        WM_MOUSEMOVE => {
            track_leave(hwnd);
            let (x, y) = point();
            with_pill(|pill| {
                if hwnd == pill.edge && pill.hoverable(&pill.view) {
                    pill.set_hover(true);
                } else if hwnd == pill.hwnd {
                    if pill.collapsing {
                        pill.cancel_collapse();
                    }
                    let index = pill.hit_index(x, y);
                    pill.set_highlight(index, false, true);
                    pill.follow_fan(index);
                }
            });
            Some(LRESULT(0))
        }
        WM_LBUTTONDOWN => {
            let (x, y) = point();
            with_pill(|pill| {
                if hwnd == pill.hwnd {
                    let index = pill.hit_index(x, y);
                    pill.set_highlight(index, true, true);
                }
            });
            Some(LRESULT(0))
        }
        WM_MOUSELEAVE => {
            with_pill(|pill| {
                // Moving between the strip and the card is not leaving.
                if !cursor_in(pill.hwnd) {
                    pill.set_highlight(None, false, true);
                    pill.set_fan(None);
                }
                if !cursor_in(pill.hwnd) && !cursor_in(pill.edge) {
                    pill.set_hover(false);
                }
            });
            Some(LRESULT(0))
        }
        WM_LBUTTONUP | WM_RBUTTONUP => {
            let (x, y) = point();
            with_pill(|pill| {
                if hwnd == pill.hwnd {
                    pill.click(x, y, msg == WM_RBUTTONUP);
                }
            });
            Some(LRESULT(0))
        }
        WM_MOUSEWHEEL => {
            // Screen coordinates, unlike the other mouse messages.
            let mut point = POINT { x: (lparam.0 & 0xFFFF) as i16 as i32, y: ((lparam.0 >> 16) & 0xFFFF) as i16 as i32 };
            let raw = ((wparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
            let delta = crate::win::volume_delta(raw, crate::win::touchpad_delta(raw));
            with_pill(|pill| {
                if hwnd != pill.hwnd || unsafe { !ScreenToClient(pill.hwnd, &mut point).as_bool() } {
                    return;
                }
                if let Some(Hit::Device(flow) | Hit::Choice(flow, _) | Hit::Fan(flow)) = pill.hit_at(point.x, point.y) {
                    act(Action::Volume(flow, delta as f32 / WHEEL_DELTA as f32));
                }
            });
            Some(LRESULT(0))
        }
        WM_TIMER if wparam.0 == COLLAPSE_TIMER => {
            with_pill(Pill::finish_collapse);
            Some(LRESULT(0))
        }
        WM_TIMER if wparam.0 == METER_TIMER => {
            with_pill(Pill::meter_tick);
            Some(LRESULT(0))
        }
        WM_TIMER if wparam.0 == LIVE_TIMER => {
            with_pill(Pill::live_tick);
            Some(LRESULT(0))
        }
        WM_APPBAR => {
            // ABN_FULLSCREENAPP = 2. Its lParam (opening or closing) is per taskbar, so with several
            // displays the last one to arrive is not the answer: refresh asks Windows, now and once
            // more a moment later in case its state trails the notification.
            if wparam.0 == 2 {
                with_pill(|pill| {
                    pill.refresh();
                    unsafe { SetTimer(Some(pill.hwnd), FULLSCREEN_TIMER, FULLSCREEN_RECHECK_MS, None) };
                });
            }
            Some(LRESULT(0))
        }
        WM_TIMER if wparam.0 == FULLSCREEN_TIMER => {
            with_pill(|pill| {
                unsafe {
                    let _ = KillTimer(Some(pill.hwnd), FULLSCREEN_TIMER);
                }
                pill.refresh();
            });
            Some(LRESULT(0))
        }
        // The accent changed: the playing rings and bars are drawn in it.
        WM_DWMCOLORIZATIONCOLORCHANGED => {
            accent_repaint(hwnd);
            None
        }
        WM_SETTINGCHANGE if is_colour_set(lparam) => {
            accent_repaint(hwnd);
            None
        }
        // Settings broadcasts arrive often and for anything: only a work-area change moves us.
        WM_SETTINGCHANGE if wparam.0 as u32 != SPI_SETWORKAREA.0 => None,
        WM_DISPLAYCHANGE | WM_DPICHANGED | WM_SETTINGCHANGE => {
            with_pill(|pill| {
                // Both windows get the broadcasts: one repaint.
                if hwnd != pill.hwnd {
                    return;
                }
                if msg == WM_DISPLAYCHANGE {
                    act(Action::DisplaysChanged);
                }
                pill.redraw();
            });
            None
        }
        _ => None,
    }
}
