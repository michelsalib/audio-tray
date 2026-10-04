//! A custom Windows-11-style flyout, modelled on the system sound flyout: an
//! acrylic-blurred, rounded, dark surface with an accent "pill" on the selected row.
//!
//! Volume sliders, mute toggles, device switching, an icon picker and a footer (settings,
//! update, quit), hand-painted into a layered window and hit-tested by hand. Modal via mouse
//! capture, like a menu.
//!
//! This module is the controller: it owns the modal pump and delegates to [`model`],
//! [`layout`], [`render`], [`window`] and [`theme`].

mod layout;
mod model;
mod render;
pub(crate) mod theme;
mod window;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{LPARAM, POINT};
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, VK_ESCAPE};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyWindow, DispatchMessageW, GetCursorPos, GetMessageW, KillTimer, LoadCursorW, SetCursor,
    SetForegroundWindow, SetTimer, ShowWindow, TranslateMessage, IDC_ARROW,
    MSG, SW_SHOWNA, SW_SHOWNORMAL, WHEEL_DELTA,
    WM_APP, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_RBUTTONDOWN,
    WM_TIMER,
};

use crate::audio::wasapi::{Meter, VolumeWatch, WasapiBackend};
use crate::audio::Flow;
use crate::config::Config;
use crate::icons::IconId;

use crate::canvas::Canvas;
use layout::{ActionKind, Elem, LaidElem, View};
use model::{build_groups, Model};
use theme::{accent_rgb, TRACK_X0};
use window::Surface;

/// What the caller must do after the flyout closes.
#[derive(Default)]
pub struct Outcome {
    pub quit: bool,
    pub config_changed: bool,
    /// Relaunch the already-updated exe and exit.
    pub restart: bool,
}

/// Where to open the flyout: centred on `cx`, its bottom at `bottom` (the tray icon's top).
#[derive(Clone, Copy)]
pub struct Anchor {
    pub cx: i32,
    pub bottom: i32,
}

// Posted by the volume callback when a watched endpoint's volume/mute changes.
const WM_VOL_CHANGED: u32 = WM_APP + 10;
// Posted on capture loss (`WM_CAPTURECHANGED` is sent, so [`window`] re-posts it for the pump).
const WM_FLYOUT_CLOSE: u32 = WM_APP + 11;

// ~30 fps peak-meter sampling.
const METER_TIMER_ID: usize = 1;
const METER_INTERVAL_MS: u32 = 33;
// The screen slide's timer (10 ms is the Win32 floor); frames are placed by elapsed time.
const ANIM_TIMER_ID: usize = 2;
const ANIM_INTERVAL_MS: u32 = 10;
const ANIM_SECS: f32 = 0.14;
// Per-tick fall-off of the displayed peak: instant attack, gentle release (a VU-meter feel).
const METER_DECAY: f32 = 0.82;
// Volume change per wheel notch, applied proportionally so sub-notch touchpad deltas are smooth.
const SCROLL_STEP: f32 = 0.02;

/// Transient pointer-interaction state: what the cursor is over, and any in-flight
/// press/drag. Cleared on every relayout and screen change (see [`Flyout::reset_hover`]).
#[derive(Default)]
struct Interaction {
    hover: Option<usize>,
    hover_pencil: bool,        // the cursor is over the hovered device row's edit pencil
    hover_back: bool,          // the cursor is over the picker's back button
    hover_chip: Option<usize>, // chip index the cursor is over, within the icon grid
    hover_footer: Option<ActionKind>, // which footer action the cursor is over, if any
    drag: Option<usize>,       // index into elems of the slider being dragged
    pending: Option<usize>,    // index pressed on button-down, acted on button-up
}

/// A slide between two screens, in flight, advanced by [`ANIM_TIMER_ID`] ticks. Never animate
/// in a blocking loop: with the mouse captured, the cursor freezes while the pump sleeps.
struct Transition {
    to: View,
    elems: Vec<LaidElem>, // the destination screen's layout, adopted when the slide lands
    src: Vec<u8>,         // the outgoing screen, as it looked when the slide started
    dst: Vec<u8>,         // the incoming screen, rendered once up front
    frame: Vec<u8>,       // scratch the two are composited into, reused every tick
    forward: bool,        // drilling in (new screen enters from the right) vs backing out
    start: Instant,
}

/// The flyout controller: borrowed services, render context, [`Model`], [`Interaction`],
/// [`Surface`], and the live audio subscriptions (COM, so kept out of the plain-data model).
struct Flyout<'a> {
    backend: &'a WasapiBackend,
    config: &'a mut Config,
    scale: f32,
    accent: [u8; 3],
    model: Model,
    hit: Interaction,
    surface: Surface,
    watches: Vec<Option<VolumeWatch>>, // per-group volume/mute change subscriptions
    meters: Vec<Option<Meter>>,        // per-group live peak meters (polled on a timer)
    vol_dirty: Arc<AtomicBool>,        // shared coalescing flag for the volume callbacks
    anim: Option<Transition>,          // the in-flight screen slide, if any
}

/// Show the flyout near the tray and operate it until the user dismisses it.
pub fn show(
    backend: &WasapiBackend,
    config: &mut Config,
    anchor: Option<Anchor>,
) -> Outcome {
    unsafe { show_inner(backend, config, anchor, false) }
}

/// Dev preview: open straight onto the first output device's icon-picker screen.
#[cfg(feature = "dev")]
pub fn show_icons_preview(
    backend: &WasapiBackend,
    config: &mut Config,
    anchor: Option<Anchor>,
) -> Outcome {
    unsafe { show_inner(backend, config, anchor, true) }
}

unsafe fn show_inner(
    backend: &WasapiBackend,
    config: &mut Config,
    anchor: Option<Anchor>,
    start_icons: bool,
) -> Outcome {
    // Centred above the tray icon, else at the cursor; that monitor sets scale and work area.
    let (cx, bottom) = match anchor {
        Some(a) => (a.cx, a.bottom),
        None => {
            let mut cur = POINT::default();
            let _ = GetCursorPos(&mut cur);
            (cur.x, cur.y)
        }
    };
    let (scale, work_area) = crate::win::monitor_at(POINT { x: cx, y: bottom });
    let accent = accent_rgb();

    let groups = build_groups(backend, config);
    let update = crate::update::pending_version();

    let mut fly = Flyout {
        backend,
        config,
        scale,
        accent,
        model: Model::new(groups, update),
        hit: Interaction::default(),
        surface: Surface::new((8.0 * scale) as i32),
        watches: Vec::new(),
        meters: Vec::new(),
        vol_dirty: Arc::new(AtomicBool::new(false)),
        anim: None,
    };

    fly.surface.wa = work_area;
    let gap = (8.0 * scale) as i32;
    let bottom = if anchor.is_some() { bottom - gap } else { bottom };
    fly.surface.base_cx = cx;
    fly.surface.base_bottom = bottom.min(fly.surface.wa.bottom - fly.surface.margin);

    // Dev preview: jump straight to the first output device's icon picker.
    if start_icons && fly.model.groups.first().is_some_and(|g| !g.devices.is_empty()) {
        fly.model.view = View::IconPicker { group: 0, dev: 0 };
    }

    fly.rebuild_layout();
    fly.surface.reposition();

    if let Err(e) = fly.surface.create_window() {
        eprintln!("flyout: create_window failed: {e:?}");
        return Outcome::default();
    }

    fly.render_base();
    fly.compose();
    let _ = ShowWindow(fly.surface.hwnd, SW_SHOWNA);
    fly.surface.animate_in(fly.scale);
    // Foreground + capture, like a menu: an outside click dismisses it.
    let _ = SetForegroundWindow(fly.surface.hwnd);
    SetCapture(fly.surface.hwnd);
    // No WM_SETCURSOR arrives under capture, so force the arrow.
    let _ = SetCursor(LoadCursorW(None, IDC_ARROW).ok());
    fly.setup_watches();
    let _ = SetTimer(Some(fly.surface.hwnd), METER_TIMER_ID, METER_INTERVAL_MS, None);

    let mut msg = MSG::default();
    'pump: while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
        // Mid-slide, `elems` still describes the outgoing screen, so pointer input is dropped;
        // Escape and capture loss still dismiss.
        let sliding = fly.anim.is_some();
        match msg.message {
            WM_MOUSEMOVE | WM_LBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONDOWN | WM_MOUSEWHEEL
                if sliding => {}
            WM_MOUSEMOVE => {
                let (mx, my) = mouse_xy(msg.lParam);
                if let Some(si) = fly.hit.drag {
                    if let Elem::Slider { group } = fly.surface.elems[si].elem {
                        let level = layout::level_from_x(fly.surface.width, fly.scale, mx);
                        fly.set_group_level(group, level);
                        fly.compose();
                        fly.surface.flush();
                    }
                } else if fly.set_hover(mx, my) {
                    fly.compose();
                    fly.surface.flush();
                }
            }
            WM_LBUTTONDOWN => {
                let (mx, my) = mouse_xy(msg.lParam);
                if !layout::inside(fly.surface.width, fly.surface.height, mx, my) {
                    break 'pump; // click outside → dismiss
                }
                fly.hit.pending = None;
                if let Some(i) = layout::elem_at(&fly.surface.elems, my) {
                    if let Elem::Slider { group } = fly.surface.elems[i].elem {
                        fly.press_slider(i, group, mx);
                    } else {
                        fly.hit.pending = Some(i);
                    }
                }
            }
            WM_LBUTTONUP => {
                if let Some(si) = fly.hit.drag.take() {
                    // Only output volume changes ding.
                    if let Elem::Slider { group } = fly.surface.elems[si].elem {
                        if fly.model.groups[group].flow == Flow::Output {
                            beep_volume();
                        }
                    }
                    continue; // stay open
                }
                let (mx, my) = mouse_xy(msg.lParam);
                if layout::inside(fly.surface.width, fly.surface.height, mx, my) {
                    if let Some(i) = layout::elem_at(&fly.surface.elems, my) {
                        if fly.hit.pending == Some(i) && fly.activate(i, mx, my) {
                            break 'pump;
                        }
                    }
                }
                fly.hit.pending = None;
            }
            WM_RBUTTONDOWN => {
                let (mx, my) = mouse_xy(msg.lParam);
                if !layout::inside(fly.surface.width, fly.surface.height, mx, my) {
                    break 'pump;
                }
            }
            WM_MOUSEWHEEL => {
                // Screen coordinates, unlike the other mouse messages.
                let delta = (msg.wParam.0 >> 16) as u16 as i16;
                let (_, sy) = mouse_xy(msg.lParam);
                fly.scroll_volume(sy - fly.surface.y, delta);
            }
            WM_VOL_CHANGED => fly.refresh_volumes(),
            // Only our own timers: the tray's message window has timers too, and they must be dispatched.
            WM_TIMER if msg.hwnd == fly.surface.hwnd && msg.wParam.0 == ANIM_TIMER_ID => fly.tick_transition(),
            WM_TIMER if msg.hwnd == fly.surface.hwnd => fly.tick_meters(),
            WM_KEYDOWN if msg.wParam.0 as u16 == VK_ESCAPE.0 => break 'pump,
            WM_FLYOUT_CLOSE => break 'pump, // lost capture (Start menu, Alt-Tab, …)
            _ => {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    let _ = KillTimer(Some(fly.surface.hwnd), METER_TIMER_ID);
    if fly.anim.is_some() {
        let _ = KillTimer(Some(fly.surface.hwnd), ANIM_TIMER_ID); // dismissed mid-slide
    }
    fly.watches.clear(); // unregister the volume callbacks before the window goes away
    fly.meters.clear(); // release the peak-meter interfaces too
    let _ = ReleaseCapture();
    let _ = DestroyWindow(fly.surface.hwnd);
    Outcome {
        quit: fly.model.quit,
        config_changed: fly.model.config_changed,
        restart: fly.model.restart,
    }
}

impl Flyout<'_> {
    fn reset_hover(&mut self) {
        self.hit = Interaction::default();
    }

    /// Size the panel, lay out the current view and allocate buffers; clears hit state. The size
    /// fits every screen, so navigating never resizes the window.
    fn rebuild_layout(&mut self) {
        self.reset_hover();
        self.surface.width = layout::content_width(&self.model, self.scale);
        self.surface.height = layout::panel_height(&self.model, self.scale, self.surface.width);
        let (elems, _) = layout::build_view(&self.model, self.scale, self.surface.width, self.model.view, self.surface.height);
        self.surface.elems = elems;
        let bytes = (self.surface.width * self.surface.height * 4) as usize;
        self.surface.base = vec![0u8; bytes];
        self.surface.buf = vec![0u8; bytes];
    }

    /// Push the current glyphs to the taskbar strip on each live change: the tray's refresh is
    /// held back until the flyout closes. Goes through the tray, which dedupes restyles.
    fn sync_strip(&self) {
        crate::tray::restyle_strip(self.backend, self.config);
    }

    fn set_group_level(&mut self, group: usize, level: f32) {
        self.model.groups[group].level = level;
        if let Some(id) = self.model.groups[group].default_id.clone() {
            let _ = self.backend.set_volume_of(&id, level);
        }
    }

    /// Adjust volume for a wheel notch at client-y `cy` (see [`Self::scroll_target_group`]).
    fn scroll_volume(&mut self, cy: i32, delta: i16) {
        let Some(group) = self.scroll_target_group(cy) else {
            return;
        };
        let notches = delta as f32 / WHEEL_DELTA as f32;
        let level = (self.model.groups[group].level + notches * SCROLL_STEP).clamp(0.0, 1.0);
        self.set_group_level(group, level);
        self.compose();
        self.surface.flush();
    }

    /// The slider under the cursor, else the first output slider; `None` on a screen without
    /// sliders (the icon picker).
    fn scroll_target_group(&self, cy: i32) -> Option<usize> {
        if let Some(i) = layout::elem_at(&self.surface.elems, cy) {
            if let Elem::Slider { group } = self.surface.elems[i].elem {
                return Some(group);
            }
        }
        self.surface.elems.iter().find_map(|le| match le.elem {
            Elem::Slider { group } if self.model.groups[group].flow == Flow::Output => Some(group),
            _ => None,
        })
    }

    /// Subscribe to volume/mute changes and the peak meter on each group's default endpoint.
    fn setup_watches(&mut self) {
        self.watches = (0..self.model.groups.len()).map(|_| None).collect();
        self.meters = (0..self.model.groups.len()).map(|_| None).collect();
        for group in 0..self.model.groups.len() {
            self.rewatch(group);
        }
    }

    /// (Re)subscribe a group's watch and peak meter to its current default endpoint; dropping
    /// the old watch unregisters it.
    fn rewatch(&mut self, group: usize) {
        if group >= self.watches.len() {
            return;
        }
        let hwnd = self.surface.hwnd.0 as isize;
        let backend = self.backend;
        let pending = Arc::clone(&self.vol_dirty);
        let flow = self.model.groups[group].flow;
        let id = self.model.groups[group].default_id.clone();
        self.watches[group] = id
            .as_ref()
            .and_then(|id| backend.watch_volume(id, hwnd, WM_VOL_CHANGED, pending).ok());
        // Output only: metering an input needs a capture stream, which lights "microphone in use".
        self.meters[group] = match flow {
            Flow::Output => id.as_ref().and_then(|id| backend.meter_for(id, flow).ok()),
            Flow::Input => None,
        };
    }

    /// Fold each endpoint's live peak into the smoothed `peak` (muted reads as silent) and
    /// repaint if anything moved.
    fn tick_meters(&mut self) {
        let mut changed = false;
        let mut base_changed = false;
        // The recording dot rides this tick (an atomic load); it is in the base layer, so a flip re-renders it.
        let recording = crate::audio::mic::in_use();
        for group in 0..self.model.groups.len() {
            let raw = if self.model.groups[group].muted {
                0.0
            } else {
                self.meters.get(group).and_then(|m| m.as_ref()).map_or(0.0, |m| m.peak())
            };
            let g = &mut self.model.groups[group];
            let shown = if raw >= g.peak { raw } else { (g.peak * METER_DECAY).max(raw) };
            if (shown - g.peak).abs() > 0.004 {
                changed = true;
            }
            g.peak = shown;
            if g.flow == Flow::Input && g.recording != recording {
                g.recording = recording;
                base_changed = true;
            }
        }
        // Mid-slide the transition paints.
        if (changed || base_changed) && self.anim.is_none() {
            if base_changed {
                self.render_base();
            }
            self.compose();
            self.surface.flush();
        }
    }

    /// Re-read each default endpoint's volume/mute from its watch. Skipped mid-drag.
    fn refresh_volumes(&mut self) {
        // Clear the coalescing flag before reading, so a change during the read posts again.
        self.vol_dirty.store(false, Ordering::SeqCst);
        if self.hit.drag.is_some() {
            return;
        }
        let backend = self.backend;
        let mut vol_changed = false;
        let mut mute_changed = false;
        for group in 0..self.model.groups.len() {
            let reading = match self.watches.get(group).and_then(|w| w.as_ref()) {
                Some(w) => w.read(),
                None => self.model.groups[group].default_id.as_ref().and_then(|id| {
                    Some((backend.volume_of(id).ok()?, backend.is_muted(id).ok()?))
                }),
            };
            if let Some((v, m)) = reading {
                let g = &mut self.model.groups[group];
                if (v - g.level).abs() > 0.001 {
                    g.level = v;
                    vol_changed = true;
                }
                if m != g.muted {
                    g.muted = m;
                    mute_changed = true;
                }
            }
        }
        // Mid-slide the transition paints.
        if self.anim.is_some() {
            return;
        }
        // Only a mute flip touches `base` (the glyph); volume ticks (frequent with mic auto-gain) just recompose.
        if mute_changed {
            self.render_base();
        }
        if mute_changed || vol_changed {
            self.compose();
            self.surface.flush();
        }
    }

    /// A press on a slider row: the leading icon area toggles mute, the rest starts a drag.
    fn press_slider(&mut self, elem: usize, group: usize, mx: i32) {
        let scale = self.scale;
        if (mx as f32) < (TRACK_X0 - 6.0) * scale {
            if let Some(id) = self.model.groups[group].default_id.clone() {
                let muted = !self.model.groups[group].muted;
                if let Err(e) = self.backend.set_muted(&id, muted) {
                    eprintln!("mute failed: {e:#}");
                }
                self.model.groups[group].muted = muted;
                self.sync_strip();
                self.render_base();
                self.compose();
                self.surface.flush();
            }
        } else {
            self.hit.drag = Some(elem);
            let level = layout::level_from_x(self.surface.width, self.scale, mx);
            self.set_group_level(group, level);
            self.compose();
            self.surface.flush();
        }
    }

    /// Act on a click at button-up. Returns true if the flyout should close.
    fn activate(&mut self, i: usize, mx: i32, my: i32) -> bool {
        match self.surface.elems[i].elem {
            Elem::Device { group, dev } => {
                if layout::over_pencil(self.surface.width, self.scale, mx) {
                    // Slide to the device's dedicated icon-picker screen.
                    self.navigate(View::IconPicker { group, dev }, true);
                } else {
                    let id = self.model.groups[group].devices[dev].id.clone();
                    if self.model.groups[group].default_id.as_ref() != Some(&id) {
                        if let Err(e) = self.backend.set_default_of(&id) {
                            eprintln!("switch failed: {e:#}");
                        }
                        for row in &mut self.model.groups[group].devices {
                            row.selected = row.id == id;
                        }
                        self.model.groups[group].level =
                            self.backend.volume_of(&id).unwrap_or(self.model.groups[group].level);
                        self.model.groups[group].muted = self.backend.is_muted(&id).unwrap_or(false);
                        self.model.groups[group].default_id = Some(id);
                        self.rewatch(group); // follow volume/mute of the new default
                        self.sync_strip();
                        self.render_base();
                        self.compose();
                        self.surface.flush();
                    }
                }
                false
            }
            Elem::PickerHeader { .. } => {
                // The back arrow cancels the picker and slides back to the main panel.
                if layout::over_back(self.scale, mx) {
                    self.navigate(View::Main, false);
                }
                false
            }
            Elem::IconGrid { group, dev } => {
                // Clicking an icon validates the choice: persist it, then slide back.
                if let Some(ci) = layout::grid_chip_at(self.surface.width, self.scale, mx, my, self.surface.elems[i].top) {
                    let icon = IconId::ALL[ci];
                    let id = self.model.groups[group].devices[dev].id.0.clone();
                    self.model.groups[group].devices[dev].icon = icon;
                    self.config.set_icon(id, icon);
                    self.model.config_changed = true; // saved by the caller, once, on close
                    self.sync_strip();
                    self.navigate(View::Main, false);
                }
                false
            }
            // The inert gap between footer targets (`None`) leaves the flyout open.
            Elem::Footer => match layout::footer_hit(&self.model, self.surface.width, self.scale, mx) {
                Some(ActionKind::SoundSettings) => {
                    open_sound_settings();
                    true
                }
                Some(ActionKind::Quit) => {
                    self.model.quit = true;
                    true
                }
                Some(ActionKind::Restart) => {
                    // The update is already on disk; the caller relaunches the exe and exits.
                    self.model.restart = true;
                    true
                }
                None => false,
            },
            _ => false,
        }
    }

    /// Start a horizontal slide to `to` (`forward`: new screen enters from the right). Returns
    /// after the first frame; [`ANIM_TIMER_ID`] drives the rest (see [`Transition`]).
    fn navigate(&mut self, to: View, forward: bool) {
        let (w, h) = (self.surface.width, self.surface.height);
        let n = (w * h * 4) as usize;
        // Outgoing screen: reuse the current composed frame (keeps its slider fills etc.).
        let src = self.surface.buf.clone();
        let (elems, _) = layout::build_view(&self.model, self.scale, w, to, h);
        let mut dst = vec![0u8; n];
        render::render_page(&self.ctx(), &elems, &mut dst);
        self.anim = Some(Transition {
            to,
            elems,
            src,
            dst,
            frame: vec![0u8; n],
            forward,
            start: Instant::now(),
        });
        unsafe { SetTimer(Some(self.surface.hwnd), ANIM_TIMER_ID, ANIM_INTERVAL_MS, None) };
        self.tick_transition(); // put the first frame up without waiting for a tick
    }

    /// Draw the slide at its elapsed-time position, and adopt the destination once it lands.
    fn tick_transition(&mut self) {
        let Some(mut anim) = self.anim.take() else {
            return;
        };
        let (w, h) = (self.surface.width, self.surface.height);
        let t = (anim.start.elapsed().as_secs_f32() / ANIM_SECS).clamp(0.0, 1.0);
        let ease = 1.0 - (1.0 - t) * (1.0 - t); // ease-out quad
        let off = (ease * w as f32).round() as i32;
        // Forward: old slides left out, new enters from the right; back is the mirror.
        let (dx_src, dx_dst) = if anim.forward { (-off, w - off) } else { (off, off - w) };
        {
            let mut cv = Canvas::new(&mut anim.frame, w, h);
            cv.clear();
            cv.blit_shift(&anim.src, dx_src);
            cv.blit_shift(&anim.dst, dx_dst);
        }
        self.surface.present(&anim.frame, self.surface.x, self.surface.y, 255);
        if t >= 1.0 {
            self.land(anim);
        } else {
            self.anim = Some(anim);
        }
    }

    /// The slide has arrived: adopt the destination screen and go back to normal painting.
    fn land(&mut self, anim: Transition) {
        let _ = unsafe { KillTimer(Some(self.surface.hwnd), ANIM_TIMER_ID) };
        let n = (self.surface.width * self.surface.height * 4) as usize;
        self.model.view = anim.to;
        self.surface.elems = anim.elems;
        self.reset_hover();
        self.surface.base = vec![0u8; n];
        self.surface.buf = vec![0u8; n];
        // The pointer may not move again soon; pick up what it is over now.
        self.sync_hover_to_cursor();
        self.render_base();
        self.compose();
        self.surface.flush();
    }

    /// Recompute what the pointer is over at client `(mx, my)`; returns whether to repaint.
    fn set_hover(&mut self, mx: i32, my: i32) -> bool {
        let inside = layout::inside(self.surface.width, self.surface.height, mx, my);
        let hover = if inside { layout::elem_at(&self.surface.elems, my) } else { None };
        let kind = hover.map(|i| self.surface.elems[i].elem);
        let on_pencil = matches!(kind, Some(Elem::Device { .. }))
            && layout::over_pencil(self.surface.width, self.scale, mx);
        let on_back =
            matches!(kind, Some(Elem::PickerHeader { .. })) && layout::over_back(self.scale, mx);
        let on_chip = match (kind, hover) {
            (Some(Elem::IconGrid { .. }), Some(i)) => {
                layout::grid_chip_at(self.surface.width, self.scale, mx, my, self.surface.elems[i].top)
            }
            _ => None,
        };
        let on_footer = match kind {
            Some(Elem::Footer) => layout::footer_hit(&self.model, self.surface.width, self.scale, mx),
            _ => None,
        };
        let changed = hover != self.hit.hover
            || on_pencil != self.hit.hover_pencil
            || on_back != self.hit.hover_back
            || on_chip != self.hit.hover_chip
            || on_footer != self.hit.hover_footer;
        self.hit.hover = hover;
        self.hit.hover_pencil = on_pencil;
        self.hit.hover_back = on_back;
        self.hit.hover_chip = on_chip;
        self.hit.hover_footer = on_footer;
        changed
    }

    /// Seed the hover state from where the cursor actually is.
    fn sync_hover_to_cursor(&mut self) {
        let mut p = POINT::default();
        if unsafe { GetCursorPos(&mut p) }.is_ok() {
            self.set_hover(p.x - self.surface.x, p.y - self.surface.y);
        }
    }

    /// Render the current view's static layer into the surface's `base` buffer. Slider
    /// fill/thumb/value and hover live in [`Self::compose`].
    fn render_base(&mut self) {
        let mut base = vec![0u8; (self.surface.width * self.surface.height * 4) as usize];
        render::render_page(&self.ctx(), &self.surface.elems, &mut base);
        self.surface.base = base;
    }

    /// Copy the static base, then draw the dynamic overlays (see [`render::compose`]).
    fn compose(&mut self) {
        // `Ctx::new` borrows only `self.model`, leaving `self.surface.buf` free; `self.ctx()` would not.
        let ctx = render::Ctx::new(&self.model, self.accent, self.scale, self.surface.size());
        render::compose(&ctx, &self.hit, &self.surface.elems, &self.surface.base, &mut self.surface.buf);
    }

    /// The render context for a pass that is *not* painting into the surface.
    fn ctx(&self) -> render::Ctx<'_> {
        render::Ctx::new(&self.model, self.accent, self.scale, self.surface.size())
    }
}

fn mouse_xy(lp: LPARAM) -> (i32, i32) {
    let x = (lp.0 & 0xFFFF) as u16 as i16 as i32;
    let y = ((lp.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
    (x, y)
}

/// Play the Windows volume-change ding (`SystemDefault`), async and best-effort.
fn beep_volume() {
    use windows::Win32::System::Diagnostics::Debug::MessageBeep;
    use windows::Win32::UI::WindowsAndMessaging::MB_OK;
    unsafe {
        let _ = MessageBeep(MB_OK);
    }
}

/// Open Settings ▸ System ▸ Sound.
fn open_sound_settings() {
    unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            w!("ms-settings:sound"),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
    }
}
