//! A custom Windows-11-style flyout, modelled on the system sound flyout: an
//! acrylic-blurred, rounded, dark surface with an accent "pill" on the selected row.
//!
//! Volume sliders, mute toggles, device switching and a footer (settings, update, quit),
//! hand-painted into a layered window and hit-tested by hand. Modal via mouse
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

use windows::Win32::Foundation::{LPARAM, POINT};
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, VK_ESCAPE};
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyWindow, DispatchMessageW, GetCursorPos, KillTimer, LoadCursorW, SetCursor,
    SetForegroundWindow, SetTimer, ShowWindow, TranslateMessage, IDC_ARROW,
    MSG, SW_SHOWNA, WHEEL_DELTA,
    WM_APP, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_RBUTTONDOWN,
    WM_TIMER,
};

use crate::audio::wasapi::{Meter, VolumeWatch, WasapiBackend};
use crate::audio::Flow;
use crate::config::Config;

use layout::{ActionKind, Elem};
use model::{build_groups, Model};
use theme::{accent_rgb, TRACK_X0};
use window::Surface;

/// What the caller must do after the flyout closes.
#[derive(Default)]
pub struct Outcome {
    pub quit: bool,
    /// Relaunch the already-updated exe and exit.
    pub restart: bool,
    /// Open the settings window.
    pub settings: bool,
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
// Per-tick fall-off of the displayed peak: instant attack, gentle release (a VU-meter feel).
const METER_DECAY: f32 = 0.82;
// Volume change per wheel notch, applied proportionally so sub-notch touchpad deltas are smooth.
const SCROLL_STEP: f32 = 0.02;

/// Transient pointer-interaction state: what the cursor is over, and any in-flight
/// press/drag. Cleared on every relayout (see [`Flyout::reset_hover`]).
#[derive(Default)]
struct Interaction {
    hover: Option<usize>,
    hover_footer: Option<ActionKind>, // which footer action the cursor is over, if any
    hover_media: Option<usize>, // the Now Playing transport button under the cursor
    hover_voice: bool,          // the cursor is over the voice panel's mute button
    hover_pip: Option<usize>,   // the carousel pip under the cursor
    wheel: i32,                 // wheel travel not yet turned into a page or a row (touchpads send bits)
    drag: Option<usize>,       // index into elems of the slider being dragged
    pending: Option<usize>,    // index pressed on button-down, acted on button-up
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
    hub_seen: u64,                     // the hub generation the media/apps/voice rows show
}

/// Show the flyout near the tray and operate it until the user dismisses it.
pub fn show(
    backend: &WasapiBackend,
    config: &mut Config,
    anchor: Option<Anchor>,
) -> Outcome {
    unsafe { show_inner(backend, config, anchor) }
}

unsafe fn show_inner(
    backend: &WasapiBackend,
    config: &mut Config,
    anchor: Option<Anchor>,
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

    let batteries = model::batteries_now(config);
    let groups = build_groups(backend, config, &batteries);
    let batteries = model::in_section(config, batteries);
    let update = crate::update::pending_version();

    let mut fly = Flyout {
        backend,
        config,
        scale,
        accent,
        model: Model {
            notice: crate::taskbar::current_notice(),
            batteries,
            ..Model::new(groups, update)
        },
        hit: Interaction::default(),
        surface: Surface::new((8.0 * scale) as i32),
        watches: Vec::new(),
        meters: Vec::new(),
        vol_dirty: Arc::new(AtomicBool::new(false)),
        hub_seen: 0,
    };
    fly.load_hub();

    fly.surface.wa = work_area;
    let gap = (8.0 * scale) as i32;
    let bottom = if anchor.is_some() { bottom - gap } else { bottom };
    fly.surface.base_cx = cx;
    fly.surface.base_bottom = bottom.min(fly.surface.wa.bottom - fly.surface.margin);

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
    let mut quit = false;
    // `wait::get_message`, not `GetMessageW`: the integrations keep being served while open.
    'pump: loop {
        if !crate::wait::get_message(&mut msg) {
            quit = true;
            break;
        }
        match msg.message {
            WM_MOUSEMOVE => {
                let (mx, my) = mouse_xy(msg.lParam);
                if let Some(si) = fly.hit.drag {
                    match fly.surface.elems[si].elem {
                        Elem::Slider { group } => {
                            let level = layout::level_from_x(fly.surface.width, fly.scale, mx);
                            fly.set_group_level(group, level);
                        }
                        Elem::AppSlider { app } => {
                            let level = layout::app_level_from_x(fly.surface.width, fly.scale, mx);
                            fly.set_app_level(app, level);
                        }
                        _ => {}
                    }
                    fly.compose();
                    fly.surface.flush();
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
                    match fly.surface.elems[i].elem {
                        Elem::Slider { group } => fly.press_slider(i, group, mx),
                        Elem::AppSlider { app } => fly.press_app(i, app, mx),
                        _ => fly.hit.pending = Some(i),
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
                        if fly.hit.pending == Some(i) && fly.activate(i, mx) {
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
                let raw = (msg.wParam.0 >> 16) as u16 as i16 as i32;
                let delta = crate::win::volume_delta(raw, crate::win::touchpad_delta(raw)) as i16;
                let (_, sy) = mouse_xy(msg.lParam);
                if !fly.wheel_rows(sy - fly.surface.y, raw) {
                    fly.scroll_volume(sy - fly.surface.y, delta);
                }
            }
            WM_VOL_CHANGED => fly.refresh_volumes(),
            // Only our own timers: the tray's message window has timers too, and they must be dispatched.
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
    fly.watches.clear(); // unregister the volume callbacks before the window goes away
    fly.meters.clear(); // release the peak-meter interfaces too
    let _ = ReleaseCapture();
    let _ = DestroyWindow(fly.surface.hwnd);
    // The outer loop must still see the quit this pump took.
    if quit {
        windows::Win32::UI::WindowsAndMessaging::PostQuitMessage(0);
    }
    Outcome {
        quit: fly.model.quit,
        restart: fly.model.restart,
        settings: fly.model.settings,
    }
}

impl Flyout<'_> {
    fn reset_hover(&mut self) {
        self.hit = Interaction::default();
    }

    /// Fill the Now Playing, voice and apps rows from the hub (or, in the `--flyout` preview, the
    /// mixer straight from the sessions). Returns whether the panel's rows changed shape.
    fn load_hub(&mut self) -> bool {
        let art_px = (model::ART_DIP * self.scale).round() as u32;
        let icon_px = (theme::APP_ICON_DIP * self.scale).round() as u32;
        let shape = |m: &Model| (m.now_playing.len(), m.voice.is_some(), m.apps.len(), m.batteries.len());
        let before = shape(&self.model);
        match crate::tray::hub_snapshot() {
            Some((generation, state)) => {
                let (now_playing, apps, voice) = model::from_hub(&state, art_px, icon_px, &self.model.now_playing);
                // Cards keep their slots (see `from_hub`), so the carousel stays on its card; on
                // opening, it starts on the first one playing.
                if self.model.now_playing.is_empty() {
                    self.model.page = now_playing.iter().position(|np| np.playing).unwrap_or(0);
                }
                self.model.page = self.model.page.min(now_playing.len().saturating_sub(1));
                self.model.now_playing = now_playing;
                // Mid-drag, keep the level under the finger.
                if self.hit.drag.is_none() {
                    self.model.apps = apps;
                }
                self.model.voice = voice;
                if self.config.batteries.enabled {
                    model::follow_batteries(&mut self.model, self.config, &state.batteries);
                }
                self.hub_seen = generation;
            }
            None => {
                self.model.apps = crate::audio::sessions::list()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|s| model::AppRow::of(&s, icon_px))
                    .collect();
            }
        }
        shape(&self.model) != before
    }

    /// The hub moved on while the panel is open: repaint its rows, resizing if they came or went.
    fn follow_hub(&mut self) {
        if crate::tray::hub_generation() == self.hub_seen {
            return;
        }
        if self.load_hub() {
            self.resize();
        } else {
            self.render_base();
            self.compose();
            self.surface.flush();
        }
    }

    /// Re-lay out for rows that came or went (a list expanded, a call started), keeping the
    /// panel's bottom edge where it was.
    fn resize(&mut self) {
        self.rebuild_layout();
        self.surface.reposition();
        self.sync_hover_to_cursor();
        self.render_base();
        self.compose();
        self.surface.flush();
    }

    fn set_app_level(&mut self, app: usize, level: f32) {
        let row = &mut self.model.apps[app];
        row.level = level;
        if let Err(e) = crate::audio::sessions::set_volume(&row.app, level) {
            eprintln!("flyout: could not set {}'s volume ({e:#})", row.app);
        }
        // The hub re-lists sessions only when one opens: without this its old level would come
        // back with the next hub change.
        crate::audio::sessions::CHANGED.post();
    }

    /// A press on an app's row: its glyph toggles the mute, the rest starts a drag.
    fn press_app(&mut self, elem: usize, app: usize, mx: i32) {
        if (mx as f32) < (theme::TEXT_X - 10.0) * self.scale {
            let row = &mut self.model.apps[app];
            row.muted = !row.muted;
            if let Err(e) = crate::audio::sessions::set_muted(&row.app, row.muted) {
                eprintln!("flyout: could not mute {} ({e:#})", row.app);
            }
            crate::audio::sessions::CHANGED.post();
            self.render_base();
        } else if (mx as f32) >= (theme::APP_TRACK_X0 - 10.0) * self.scale {
            self.hit.drag = Some(elem);
            let level = layout::app_level_from_x(self.surface.width, self.scale, mx);
            self.set_app_level(app, level);
        }
        self.compose();
        self.surface.flush();
    }

    /// The wheel over a carousel turns its page; over an apps list cut short, it scrolls the
    /// list. Returns whether it did, else the wheel sets the volume as anywhere else.
    fn wheel_rows(&mut self, cy: i32, raw: i32) -> bool {
        let kind = layout::elem_at(&self.surface.elems, cy).map(|i| self.surface.elems[i].elem);
        let pages = matches!(kind, Some(Elem::NowPlaying { .. } | Elem::Pager)) && layout::carousel(&self.model);
        let rows = matches!(kind, Some(Elem::AppSlider { .. })) && layout::shown_apps(&self.model).len() < self.model.apps.len();
        if !pages && !rows {
            self.hit.wheel = 0;
            return false;
        }
        const NOTCH: i32 = 120; // WHEEL_DELTA
        self.hit.wheel += raw;
        let steps = self.hit.wheel / NOTCH;
        self.hit.wheel %= NOTCH;
        // Wheel down (negative) goes forward: the next card, the rows below.
        if pages {
            let n = self.model.now_playing.len() as i32;
            self.turn_page((self.model.page as i32 - steps).rem_euclid(n) as usize);
        } else if steps != 0 {
            let shown = layout::shown_apps(&self.model);
            let last_first = self.model.apps.len() - shown.len();
            self.model.apps_scroll = (shown.start as i32 - steps).clamp(0, last_first as i32) as usize;
            // Same height, other rows: no refit needed, but the rows are laid out anew.
            let wheel = self.hit.wheel;
            self.resize();
            self.hit.wheel = wheel;
        }
        true
    }

    /// Show Now Playing card `page` in the carousel.
    fn turn_page(&mut self, page: usize) {
        if page == self.model.page {
            return;
        }
        self.model.page = page;
        for le in &mut self.surface.elems {
            if let Elem::NowPlaying { index } = &mut le.elem {
                *index = page;
            }
        }
        self.render_base();
        self.compose();
        self.surface.flush();
    }

    /// Size the panel, lay it out and allocate buffers; clears hit state.
    fn rebuild_layout(&mut self) {
        self.reset_hover();
        self.surface.width = layout::content_width(&self.model, self.scale);
        // Room above the anchor, down from the work area's top.
        let max_h = self.surface.base_bottom - (self.surface.wa.top + self.surface.margin);
        let (elems, height) = layout::fit(&mut self.model, self.scale, max_h);
        self.surface.height = height;
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

    /// Adjust volume for a wheel notch at client-y `cy`: the app whose row it is over, else the
    /// endpoint [`Self::scroll_target_group`] picks.
    fn scroll_volume(&mut self, cy: i32, delta: i16) {
        let notches = delta as f32 / WHEEL_DELTA as f32;
        let over = layout::elem_at(&self.surface.elems, cy).map(|i| self.surface.elems[i].elem);
        if let Some(Elem::AppSlider { app }) = over {
            let level = (self.model.apps[app].level + notches * SCROLL_STEP).clamp(0.0, 1.0);
            self.set_app_level(app, level);
        } else {
            let Some(group) = self.scroll_target_group(cy) else {
                return;
            };
            let level = (self.model.groups[group].level + notches * SCROLL_STEP).clamp(0.0, 1.0);
            self.set_group_level(group, level);
        }
        self.compose();
        self.surface.flush();
    }

    /// The slider under the cursor, else the first output slider; `None` when there is none.
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
        // The hub's rows ride the meter tick too: a generation compare, nothing more when idle.
        self.follow_hub();
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
        if changed || base_changed {
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
    fn activate(&mut self, i: usize, mx: i32) -> bool {
        match self.surface.elems[i].elem {
            Elem::GroupHeader { group } => {
                if self.model.groups[group].devices.len() > 1 {
                    self.model.expanded[group] = !self.model.expanded[group];
                    self.resize();
                }
                false
            }
            Elem::AppsHeader => {
                self.model.apps_open = Some(self.model.fit.apps_folded);
                self.resize();
                false
            }
            Elem::Pager => {
                if let Some(i) = layout::pip_at(self.surface.width, self.scale, self.model.now_playing.len(), mx) {
                    self.turn_page(i);
                }
                false
            }
            Elem::NowPlaying { index } => {
                use crate::music::smtc::Command;
                let Some(app_id) = self.model.now_playing.get(index).map(|np| np.app_id.clone()) else {
                    return false;
                };
                // The cover brings the app or tab to the front, and the panel steps aside.
                if (mx as f32) < (theme::ART_X + model::ART_DIP) * self.scale {
                    crate::tray::perform(crate::pill::Action::Focus(app_id));
                    return true;
                }
                let live = !self.model.now_playing.get(index).is_some_and(|np| np.skip);
                let command = match layout::media_button_at(self.surface.width, self.scale, mx) {
                    Some(1) => Command::TogglePlayPause,
                    // A live stream shows no previous/next.
                    _ if live => return false,
                    Some(0) => Command::Previous,
                    Some(_) => Command::Next,
                    None => return false,
                };
                crate::tray::perform(crate::pill::Action::Media(app_id, command));
                // Show the toggle at once; the hub confirms it a moment later.
                if command == Command::TogglePlayPause {
                    if let Some(np) = self.model.now_playing.get_mut(index) {
                        np.playing = !np.playing;
                    }
                    self.render_base();
                    self.compose();
                    self.surface.flush();
                }
                false
            }
            Elem::Voice => {
                if layout::over_voice_button(self.surface.width, self.scale, mx) {
                    let app = self.model.voice.as_ref().and_then(|v| v.app);
                    crate::tray::perform(crate::pill::Action::ToggleMute(app));
                    if let Some(voice) = self.model.voice.as_mut() {
                        voice.muted = !voice.muted;
                    }
                    self.render_base();
                    self.compose();
                    self.surface.flush();
                }
                false
            }
            Elem::Device { group, dev } => {
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
                false
            }
            // The inert gap between footer targets (`None`) leaves the flyout open.
            Elem::Footer => match layout::footer_hit(&self.model, self.surface.width, self.scale, mx) {
                Some(ActionKind::Settings) => {
                    // The settings window opens once the flyout has gone.
                    self.model.settings = true;
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

    /// Recompute what the pointer is over at client `(mx, my)`; returns whether to repaint.
    fn set_hover(&mut self, mx: i32, my: i32) -> bool {
        let inside = layout::inside(self.surface.width, self.surface.height, mx, my);
        let hover = if inside { layout::elem_at(&self.surface.elems, my) } else { None };
        let kind = hover.map(|i| self.surface.elems[i].elem);
        let on_footer = match kind {
            Some(Elem::Footer) => layout::footer_hit(&self.model, self.surface.width, self.scale, mx),
            _ => None,
        };
        let on_media = match kind {
            Some(Elem::NowPlaying { index }) => {
                let live = !self.model.now_playing.get(index).is_some_and(|np| np.skip);
                layout::media_button_at(self.surface.width, self.scale, mx).filter(|&i| !live || i == 1)
            }
            _ => None,
        };
        let on_voice = matches!(kind, Some(Elem::Voice)) && layout::over_voice_button(self.surface.width, self.scale, mx);
        let on_pip = match kind {
            Some(Elem::Pager) => layout::pip_at(self.surface.width, self.scale, self.model.now_playing.len(), mx),
            _ => None,
        };
        let changed = hover != self.hit.hover
            || on_footer != self.hit.hover_footer
            || on_media != self.hit.hover_media
            || on_voice != self.hit.hover_voice
            || on_pip != self.hit.hover_pip;
        self.hit.hover_pip = on_pip;
        self.hit.hover_media = on_media;
        self.hit.hover_voice = on_voice;
        self.hit.hover = hover;
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

    /// Render the panel's static layer into the surface's `base` buffer. Slider
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

