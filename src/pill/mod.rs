//! The floating edge pill: the passive surface. It sits on the primary monitor's left edge,
//! vertically centred, reserves no space, and shows the loudest state from [`view::decide`].
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

use std::cell::RefCell;
use std::collections::VecDeque;

use anyhow::{Context, Result};
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::DirectComposition::{IDCompositionAnimation, IDCompositionVisual2};
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
    WM_MOUSEACTIVATE, WM_MOUSEMOVE, WM_MOUSEWHEEL, WHEEL_DELTA, WM_RBUTTONUP, WM_SETTINGCHANGE, WM_TIMER, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
};

use crate::hub::{State, VoiceApp};
use crate::music::smtc::Command;
use draw::{Hit, Ring, Scene};
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
    /// Mute the default microphone (the "is listening" alert).
    MuteMicrophone,
    /// Switch this flow to its next device, as the taskbar button's click does.
    CycleDevice(crate::audio::Flow),
    /// Change this flow's volume by so many wheel notches.
    Volume(crate::audio::Flow, f32),
    Acknowledge(crate::hub::Notice),
}

/// Posted to the tray's message window when [`take_actions`] has something.
pub const WM_PILL_ACTION: u32 = WM_APP + 36;

/// The appbar notification message (fullscreen app opened or closed).
const WM_APPBAR: u32 = WM_APP + 37;
/// `WM_MOUSELEAVE`, filed under UI::Controls in windows-rs.
const WM_MOUSELEAVE: u32 = 0x02A3;
const METER_TIMER: usize = 1;
/// The microphone meter's rate while muted in a call: 10 Hz.
const METER_MS: u32 = 100;
/// Peak above which the microphone hears speech, and how many ticks make talking / silence.
const TALK_PEAK: f32 = 0.04;
const TALK_TICKS: u32 = 3;
const QUIET_TICKS: u32 = 15;
/// The hover strip's height, centred on the pill's place.
const EDGE_H: f32 = 160.0;
const SLIDE_SECONDS: f64 = 0.2;
const GROW_SECONDS: f64 = 0.18;
/// One beat of the speaking pulse.
const PULSE_SECONDS: f64 = 1.1;

/// How a new scene arrives.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Entrance {
    /// Content changed in place (a track, a mute): no movement.
    None,
    /// Another state: slides in from the edge.
    Slide,
    /// The same state changed shape (the card opened, a disc or the devices came): unfolds.
    Grow,
}

/// How `next` should arrive after `shown`.
fn entrance(shown: &View, next: &View) -> Entrance {
    if std::mem::discriminant(shown) != std::mem::discriminant(next) || matches!(next, View::Banner { .. }) {
        return Entrance::Slide;
    }
    let shape = |view: &View| match view {
        View::Idle { devices } => (0, devices.is_some(), false),
        View::Music { media, card, devices } => (media.len(), devices.is_some(), *card),
        View::Call { members, .. } => (members.len(), false, false),
        _ => (0, false, false),
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
    /// Click targets of the shown scene, in DIPs from the window's origin.
    hits: Vec<([f32; 4], Hit)>,
    /// Kept alive while shown: the visuals and their animations.
    _visuals: Vec<IDCompositionVisual2>,
    meter: Option<IAudioMeterInformation>,
    loud: u32,
    quiet: u32,
    appbar: bool,
    /// Whether the hover strip is up (see [`Pill::hoverable`]).
    edge_up: bool,
    /// The size the shown scene was drawn at.
    drawn_size: Option<crate::config::PillSize>,
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
            fullscreen: fullscreen_now(),
            ..Default::default()
        },
        hits: Vec::new(),
        _visuals: Vec::new(),
        meter: None,
        loud: 0,
        quiet: 0,
        appbar: false,
        edge_up: false,
        drawn_size: None,
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

/// New settings from the flyout.
pub fn configure(config: &crate::config::Pill) {
    with_pill(|pill| {
        pill.inputs.enabled = config.enabled;
        pill.inputs.listening_alerts = config.listening_alerts;
        pill.inputs.devices = config.devices;
        pill.inputs.size = config.size;
        pill.refresh();
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
        // Same state, same hover strip: nothing to repaint.
        let resized = self.drawn_size != Some(self.inputs.size);
        if next == self.view && self.gfx.is_some() && self.hoverable(&next) == self.edge_up && !resized {
            return;
        }
        let entrance = match entrance(&self.view, &next) {
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

    /// Paint and place `view`; `slide` animates it in from the edge.
    fn show(&mut self, view: &View, entrance: Entrance) -> Result<()> {
        let (scale, work) = crate::win::monitor_at(POINT { x: 0, y: 0 });
        if self.gfx.as_ref().is_none_or(|gfx| gfx.scale != scale) {
            self.gfx = Some(Gfx::new(self.hwnd, scale)?);
        }
        let gfx = self.gfx.as_ref().expect("just built");
        let small = self.inputs.size == crate::config::PillSize::Small;
        self.drawn_size = Some(self.inputs.size);
        let Some(scene) = draw::scene(gfx, view, small)? else {
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
                let _ = ShowWindow(self.edge, SW_HIDE);
            }
            self.hits.clear();
            self._visuals.clear();
            self.edge_up = false;
            return Ok(());
        };
        let visuals = self.compose(gfx, &scene, entrance)?;
        let px = |v: f32| (v * scale).round() as i32;
        let (w, h) = (px(scene.w), px(scene.h));
        let x = work.left;
        let y = work.top + (work.bottom - work.top - h) / 2;
        unsafe {
            set_click_through(self.hwnd, view.click_through());
            SetWindowPos(self.hwnd, Some(HWND_TOPMOST), x, y, w, h, SWP_NOACTIVATE | SWP_SHOWWINDOW)?;
            if self.hoverable(view) {
                let edge_h = px(EDGE_H).max(h);
                let edge_y = work.top + (work.bottom - work.top - edge_h) / 2;
                SetWindowPos(self.edge, Some(HWND_TOPMOST), x, edge_y, 2, edge_h, SWP_NOACTIVATE | SWP_SHOWWINDOW)?;
            } else {
                let _ = ShowWindow(self.edge, SW_HIDE);
            }
            self.edge_up = self.hoverable(view);
        }
        gfx.commit()?;
        self.hits = scene.hits;
        self._visuals = visuals;
        Ok(())
    }

    /// Hang the scene under the root: its content, the ring's two rotating halves, the speaking
    /// pulses, and the entrance. Returns the visuals to keep alive. Every movement is a
    /// compositor animation set here once.
    fn compose(&self, gfx: &Gfx, scene: &Scene, entrance: Entrance) -> Result<Vec<IDCompositionVisual2>> {
        unsafe {
            gfx.root.RemoveAllVisuals()?;
            let holder = gfx.dcomp.CreateVisual()?;
            holder.AddVisual(&scene.content, false, None)?;
            let mut keep = vec![holder.clone(), scene.content.clone()];
            for &ring in &scene.rings {
                keep.extend(ring_visuals(gfx, &holder, ring)?);
            }
            for &pulse in &scene.pulses {
                keep.push(pulse_visual(gfx, &holder, pulse)?);
            }
            gfx.root.AddVisual(&holder, false, None)?;
            let fade_from = match entrance {
                Entrance::None => return Ok(keep),
                Entrance::Slide => {
                    // From just off the edge to its place, easing out.
                    let offset = gfx.dcomp.CreateAnimation()?;
                    ease_out(&offset, -scene.w * gfx.scale, 0.0, SLIDE_SECONDS)?;
                    holder.SetOffsetX(&offset)?;
                    0.0
                }
                Entrance::Grow => {
                    // Unfolds from the edge: a slight scale up from the left middle.
                    let scale = gfx.dcomp.CreateScaleTransform()?;
                    scale.SetCenterX2(0.0)?;
                    scale.SetCenterY2(scene.h * gfx.scale / 2.0)?;
                    let grow = gfx.dcomp.CreateAnimation()?;
                    ease_out(&grow, 0.9, 1.0, GROW_SECONDS)?;
                    scale.SetScaleX(&grow)?;
                    scale.SetScaleY(&grow)?;
                    holder.SetTransform(&scale)?;
                    0.35
                }
            };
            let seconds = if entrance == Entrance::Slide { SLIDE_SECONDS } else { GROW_SECONDS };
            let fade = gfx.dcomp.CreateAnimation()?;
            ease_out(&fade, fade_from, 1.0, seconds)?;
            let effect = gfx.dcomp.CreateEffectGroup()?;
            effect.SetOpacity(&fade)?;
            holder.SetEffect(&effect)?;
            Ok(keep)
        }
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
        let wanted = view::call_mute(state).is_some_and(|(_, muted)| muted);
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
        if self.inputs.hover != hover {
            self.inputs.hover = hover;
            self.refresh();
        }
    }

    /// What is under client pixel `(x, y)`.
    fn hit_at(&self, x: i32, y: i32) -> Option<Hit> {
        let scale = self.gfx.as_ref().map_or(1.0, |g| g.scale);
        let (x, y) = (x as f32 / scale, y as f32 / scale);
        self.hits.iter().find(|([hx, hy, hw, hh], _)| x >= *hx && x < hx + hw && y >= *hy && y < hy + hh).map(|(_, hit)| *hit)
    }

    fn click(&mut self, x: i32, y: i32, right: bool) {
        let hit = self.hit_at(x, y);
        match (&self.view, hit) {
            (_, Some(Hit::Device(flow))) => act(Action::CycleDevice(flow)),
            (View::Music { media, .. }, Some(Hit::Media(i, command))) => {
                if let Some(m) = media.get(i) {
                    act(Action::Media(m.app_id.clone(), command));
                }
            }
            (View::Call { app, .. }, Some(_)) => act(Action::ToggleMute(*app)),
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

/// The ring's moving part: a right-half clip holding the left half-ring, which rotating reveals
/// 0–180°, and a left-half clip holding the right half-ring for 180–360°. Each angle is a
/// compositor animation from now to the end of the track.
fn ring_visuals(gfx: &Gfx, holder: &IDCompositionVisual2, ring: Ring) -> Result<Vec<IDCompositionVisual2>> {
    let reach = ring.r + ring.stroke;
    let size = 2.0 * reach;
    let (cx, cy) = (reach, reach);
    let scale = gfx.scale;
    let mut keep = Vec::new();
    // Degrees now, and degrees per second.
    let now = ring.fraction.clamp(0.0, 1.0) * 360.0;
    let rate = ring.rate * 360.0;
    for (clip_left, half_left) in [(false, true), (true, false)] {
        let (half, _) = gfx.painted(size, size, |p| p.half_ring(cx, cy, ring.r, ring.stroke, draw::RING, half_left))?;
        let container = unsafe { gfx.dcomp.CreateVisual()? };
        unsafe {
            container.SetOffsetX2((ring.cx - reach) * scale)?;
            container.SetOffsetY2((ring.cy - reach) * scale)?;
            let clip = gfx.dcomp.CreateRectangleClip()?;
            let (left, right) = if clip_left { (0.0, reach * scale) } else { (reach * scale, size * scale) };
            clip.SetLeft2(left)?;
            clip.SetRight2(right)?;
            clip.SetTop2(0.0)?;
            clip.SetBottom2(size * scale)?;
            container.SetClip(&clip)?;
            let rotate = gfx.dcomp.CreateRotateTransform()?;
            rotate.SetCenterX2(reach * scale)?;
            rotate.SetCenterY2(reach * scale)?;
            // The right clip shows angles 0–180 (the first half), the left 180–360.
            let base = if clip_left { 180.0 } else { 0.0 };
            let angle = gfx.dcomp.CreateAnimation()?;
            piecewise(&angle, (now - base).clamp(0.0, 180.0), rate, if clip_left { (180.0 - now).max(0.0) } else { 0.0 })?;
            rotate.SetAngle(&angle)?;
            half.SetTransform(&rotate)?;
            container.AddVisual(&half, false, None)?;
            // `false` with no reference visual puts it on top of its siblings, over the content.
            holder.AddVisual(&container, false, None)?;
        }
        keep.push(container);
        keep.push(half);
    }
    Ok(keep)
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
        swell.AddCubic(0.0, 1.0, ((SWELL - 1.0) as f64 / PULSE_SECONDS) as f32, 0.0, 0.0)?;
        swell.AddRepeat(PULSE_SECONDS, PULSE_SECONDS)?;
        grow.SetScaleX(&swell)?;
        grow.SetScaleY(&swell)?;
        visual.SetTransform(&grow)?;
        let fade = gfx.dcomp.CreateAnimation()?;
        fade.AddCubic(0.0, 0.9, (-0.9 / PULSE_SECONDS) as f32, 0.0, 0.0)?;
        fade.AddRepeat(PULSE_SECONDS, PULSE_SECONDS)?;
        let effect = gfx.dcomp.CreateEffectGroup()?;
        effect.SetOpacity(&fade)?;
        visual.SetEffect(&effect)?;
        // On top: the call pill's background is opaque.
        holder.AddVisual(&visual, false, None)?;
    }
    Ok(visual)
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
            with_pill(|pill| {
                if hwnd == pill.edge && pill.hoverable(&pill.view) {
                    pill.set_hover(true);
                }
            });
            Some(LRESULT(0))
        }
        WM_MOUSELEAVE => {
            with_pill(|pill| {
                // Moving between the strip and the card is not leaving.
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
            let delta = ((wparam.0 >> 16) & 0xFFFF) as u16 as i16;
            with_pill(|pill| {
                if hwnd != pill.hwnd || unsafe { !ScreenToClient(pill.hwnd, &mut point).as_bool() } {
                    return;
                }
                if let Some(Hit::Device(flow)) = pill.hit_at(point.x, point.y) {
                    act(Action::Volume(flow, delta as f32 / WHEEL_DELTA as f32));
                }
            });
            Some(LRESULT(0))
        }
        WM_TIMER if wparam.0 == METER_TIMER => {
            with_pill(Pill::meter_tick);
            Some(LRESULT(0))
        }
        WM_APPBAR => {
            // ABN_FULLSCREENAPP = 2; lParam says opening (non-zero) or closing.
            if wparam.0 == 2 {
                with_pill(|pill| {
                    pill.inputs.fullscreen = lparam.0 != 0 || fullscreen_now();
                    pill.refresh();
                });
            }
            Some(LRESULT(0))
        }
        WM_DISPLAYCHANGE | WM_DPICHANGED | WM_SETTINGCHANGE => {
            with_pill(|pill| {
                // Re-place (and re-scale) by repainting from scratch.
                let view = std::mem::replace(&mut pill.view, View::Hidden);
                if let Err(e) = pill.show(&view, Entrance::None) {
                    eprintln!("pill: could not re-place ({e:#})");
                    pill.gfx = None;
                }
                pill.view = view;
            });
            None
        }
        _ => None,
    }
}
