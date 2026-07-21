//! A custom Windows-11-style flyout, modelled on the system sound flyout: an
//! acrylic-blurred, rounded, dark surface with an accent "pill" on the selected row.
//!
//! Unlike a classic `TrackPopupMenu`, this is a live control surface: volume sliders,
//! mute toggles, output + input device switching, and an inline per-device icon picker,
//! all painted by hand into a per-pixel-alpha *layered* window (`UpdateLayeredWindow`),
//! with acrylic blur from the compositor (`SetWindowCompositionAttribute`) and rounded
//! corners from DWM. Rows and controls are laid out and hit-tested by hand; the flyout is
//! modal via mouse capture, like a menu, but stays open while you operate it.
//!
//! Left-click opens the full [`Trigger::LeftClick`] panel; right-click opens the tiny
//! [`Trigger::RightClick`] menu (Sound settings + Quit).
//!
//! This module is the **controller**: it owns the modal message pump and coordinates the
//! focused pieces it delegates to — the display [`model`], pure [`layout`], the [`render`]
//! passes, the drawing [`canvas`], the [`window`] surface, and the [`theme`] tokens.

mod canvas;
mod layout;
mod model;
mod render;
mod theme;
mod window;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{LPARAM, POINT};
use windows::Win32::UI::HiDpi::GetDpiForSystem;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, VK_ESCAPE};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyWindow, DispatchMessageW, GetCursorPos, GetMessageW, KillTimer, LoadCursorW, SetCursor,
    SetForegroundWindow, SetTimer, ShowWindow, SystemParametersInfoW, TranslateMessage, IDC_ARROW,
    MSG, SPI_GETWORKAREA, SW_SHOWNA, SW_SHOWNORMAL, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, WM_APP,
    WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_RBUTTONDOWN, WM_TIMER,
};

use crate::audio::wasapi::{Meter, VolumeWatch, WasapiBackend};
use crate::audio::Flow;
use crate::config::Config;
use crate::icons::IconId;

use canvas::Canvas;
use layout::{ActionKind, Elem, View};
use model::{build_groups, Model};
use theme::{accent_rgb, TRACK_X0};
use window::Surface;

/// Which entry point opened the flyout.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// The full audio control panel.
    LeftClick,
    /// The tiny quick menu (Sound settings + Quit).
    RightClick,
}

/// What the caller must do after the flyout closes.
pub struct Outcome {
    pub quit: bool,
    pub config_changed: bool,
    /// The default *output* device was switched while the flyout was open. The
    /// endpoint-change notifications that would refresh the tray icon are consumed by our
    /// own modal message loop, so the caller must refresh explicitly.
    pub output_changed: bool,
    /// The user clicked the "restart to update" entry — the caller should relaunch the
    /// (already-updated on disk) exe and exit.
    pub restart: bool,
}

/// Where to open the flyout: horizontally centred on the tray icon (`cx`), sitting just
/// above it (`bottom` = the icon's top), like the native tray flyouts.
#[derive(Clone, Copy)]
pub struct Anchor {
    pub cx: i32,
    pub bottom: i32,
}

// Posted (by the WASAPI volume callback) when a watched endpoint's volume/mute changes,
// so external changes (media keys, other apps) are reflected live while we're open.
const WM_VOL_CHANGED: u32 = WM_APP + 10;
// Posted by the window proc when we lose mouse capture (another window/app took focus,
// e.g. the Start menu). WM_CAPTURECHANGED is *sent* to the proc, not queued, so it's
// bounced back (see [`window`]) as a posted message the modal loop can act on to dismiss.
const WM_FLYOUT_CLOSE: u32 = WM_APP + 11;

// A Win32 timer drives ~30 fps sampling of each default endpoint's live peak level
// (IAudioMeterInformation) so the slider fill reacts to real audio while we're open.
const METER_TIMER_ID: usize = 1;
const METER_INTERVAL_MS: u32 = 33;
// Per-tick fall-off of the displayed peak: instant attack, gentle release (a VU-meter feel).
const METER_DECAY: f32 = 0.82;

/// Transient pointer-interaction state: what the cursor is over, and any in-flight
/// press/drag. Cleared on every relayout and screen change (see [`Flyout::reset_hover`]).
#[derive(Default)]
struct Interaction {
    hover: Option<usize>,
    hover_pencil: bool,        // the cursor is over the hovered device row's edit pencil
    hover_back: bool,          // the cursor is over the picker's back button
    hover_chip: Option<usize>, // chip index the cursor is over, within the icon grid
    drag: Option<usize>,       // index into elems of the slider being dragged
    pending: Option<usize>,    // index pressed on button-down, acted on button-up
}

/// The flyout controller. Slim on purpose: the shared services it borrows (`backend`,
/// `config`), the render context (`scale`, `accent`), and three cohesive pieces — the
/// display [`Model`], the pointer [`Interaction`], and the [`Surface`] (window + pixels) —
/// plus the live audio subscriptions, which hold COM interfaces and so can't live in the
/// plain-data model.
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
}

/// Show the flyout near the tray and operate it until the user dismisses it.
pub fn show(
    backend: &WasapiBackend,
    config: &mut Config,
    anchor: Option<Anchor>,
    trigger: Trigger,
) -> Outcome {
    unsafe { show_inner(backend, config, anchor, trigger, false) }
}

/// Dev preview: open straight onto the first output device's icon-picker screen (so the
/// picker can be iterated on without first hovering + clicking a device's edit pencil).
pub fn show_icons_preview(
    backend: &WasapiBackend,
    config: &mut Config,
    anchor: Option<Anchor>,
) -> Outcome {
    unsafe { show_inner(backend, config, anchor, Trigger::LeftClick, true) }
}

unsafe fn show_inner(
    backend: &WasapiBackend,
    config: &mut Config,
    anchor: Option<Anchor>,
    trigger: Trigger,
    start_icons: bool,
) -> Outcome {
    let scale = (GetDpiForSystem() as f32 / 96.0).max(1.0);
    let accent = accent_rgb();

    let groups = match trigger {
        Trigger::LeftClick => build_groups(backend, config),
        Trigger::RightClick => Vec::new(),
    };
    // Only the full left-click panel offers the restart-to-update entry.
    let update = match trigger {
        Trigger::LeftClick => crate::update::pending_version(),
        Trigger::RightClick => None,
    };

    let mut fly = Flyout {
        backend,
        config,
        scale,
        accent,
        model: Model::new(trigger, groups, update),
        hit: Interaction::default(),
        surface: Surface::new((8.0 * scale) as i32),
        watches: Vec::new(),
        meters: Vec::new(),
        vol_dirty: Arc::new(AtomicBool::new(false)),
    };

    // Resolve the anchor: bottom-right above the tray icon, else the cursor.
    let _ = SystemParametersInfoW(
        SPI_GETWORKAREA,
        0,
        Some(&mut fly.surface.wa as *mut _ as *mut std::ffi::c_void),
        SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
    );
    let gap = (8.0 * scale) as i32;
    let (cx, bottom) = match anchor {
        Some(a) => (a.cx, a.bottom - gap),
        None => {
            let mut cur = POINT::default();
            let _ = GetCursorPos(&mut cur);
            (cur.x, cur.y)
        }
    };
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
        return Outcome { quit: false, config_changed: false, output_changed: false, restart: false };
    }

    fly.render_base();
    fly.compose();
    let _ = ShowWindow(fly.surface.hwnd, SW_SHOWNA);
    fly.surface.animate_in(fly.scale);
    // Foreground + capture so the flyout behaves like a menu: it sees every mouse
    // move/click, and an outside click reliably dismisses it.
    let _ = SetForegroundWindow(fly.surface.hwnd);
    SetCapture(fly.surface.hwnd);
    // While the mouse is captured Windows stops sending WM_SETCURSOR, so force the arrow.
    let _ = SetCursor(LoadCursorW(None, IDC_ARROW).ok());
    // Subscribe to external volume/mute changes (media keys, other apps) while we're open.
    fly.setup_watches();
    // Poll each endpoint's live peak meter ~30 fps so the slider fill reacts to audio.
    let _ = SetTimer(Some(fly.surface.hwnd), METER_TIMER_ID, METER_INTERVAL_MS, None);

    let mut msg = MSG::default();
    'pump: while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
        match msg.message {
            WM_MOUSEMOVE => {
                let (mx, my) = mouse_xy(msg.lParam);
                if let Some(si) = fly.hit.drag {
                    if let Elem::Slider { group } = fly.surface.elems[si].elem {
                        let level = layout::level_from_x(fly.surface.width, fly.scale, mx);
                        fly.set_group_level(group, level);
                        fly.compose();
                        fly.surface.flush();
                    }
                } else {
                    let inside = layout::inside(fly.surface.width, fly.surface.height, mx, my);
                    let hover = if inside { layout::elem_at(&fly.surface.elems, my) } else { None };
                    let kind = hover.map(|i| fly.surface.elems[i].elem);
                    let on_pencil = matches!(kind, Some(Elem::Device { .. }))
                        && layout::over_pencil(fly.surface.width, fly.scale, mx);
                    let on_back = matches!(kind, Some(Elem::PickerHeader { .. }))
                        && layout::over_back(fly.scale, mx);
                    let on_chip = match (kind, hover) {
                        (Some(Elem::IconGrid { .. }), Some(i)) => {
                            layout::grid_chip_at(fly.surface.width, fly.scale, mx, my, fly.surface.elems[i].top)
                        }
                        _ => None,
                    };
                    if hover != fly.hit.hover
                        || on_pencil != fly.hit.hover_pencil
                        || on_back != fly.hit.hover_back
                        || on_chip != fly.hit.hover_chip
                    {
                        fly.hit.hover = hover;
                        fly.hit.hover_pencil = on_pencil;
                        fly.hit.hover_back = on_back;
                        fly.hit.hover_chip = on_chip;
                        fly.compose();
                        fly.surface.flush();
                    }
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
                    // Only an *output* volume change plays the "ding"; changing the input
                    // (mic) level shouldn't trigger a notification sound.
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
            WM_VOL_CHANGED => fly.refresh_volumes(),
            WM_TIMER => fly.tick_meters(),
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
    Outcome {
        quit: fly.model.quit,
        config_changed: fly.model.config_changed,
        output_changed: fly.model.output_changed,
        restart: fly.model.restart,
    }
}

impl Flyout<'_> {
    fn reset_hover(&mut self) {
        self.hit = Interaction::default();
    }

    /// Size the panel and lay out the current view, allocating buffers. Invalidates
    /// transient hit state. Both the width and the height are fixed for the flyout's whole
    /// life — the width from the main list, the height from the tallest screen — so
    /// navigating between the main panel and an icon picker never resizes the window.
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

    fn set_group_level(&mut self, group: usize, level: f32) {
        self.model.groups[group].level = level;
        if let Some(id) = self.model.groups[group].default_id.clone() {
            let _ = self.backend.set_volume_of(&id, level);
        }
    }

    /// Subscribe to volume/mute changes and the peak meter on each group's default endpoint.
    fn setup_watches(&mut self) {
        self.watches = (0..self.model.groups.len()).map(|_| None).collect();
        self.meters = (0..self.model.groups.len()).map(|_| None).collect();
        for group in 0..self.model.groups.len() {
            self.rewatch(group);
        }
    }

    /// (Re)subscribe a group to its current default endpoint — called at open and whenever
    /// the default is switched from within the flyout (the old endpoint's watch is dropped,
    /// which unregisters it). Also re-activates the peak meter that feeds the activity glow.
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
        // The activity glow is output-only: metering an input endpoint needs a running
        // capture stream, which would keep Windows' "microphone in use" indicator lit the
        // whole time the flyout is open. So we only meter output; the input slider stays a
        // plain slider (its peak holds at 0, which the additive glow renders as no glow).
        self.meters[group] = match flow {
            Flow::Output => id.as_ref().and_then(|id| backend.meter_for(id, flow).ok()),
            Flow::Input => None,
        };
    }

    /// Sample each default endpoint's live peak level and fold it into the smoothed `peak`
    /// (fast attack, gentle release), then repaint if anything moved. Driven by the ~30 fps
    /// timer. A muted endpoint reads as silent so its fill settles back to the resting glow.
    fn tick_meters(&mut self) {
        let mut changed = false;
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
        }
        if changed {
            self.compose();
            self.surface.flush();
        }
    }

    /// Re-read each default endpoint's volume/mute (from the cached watch interface, no COM
    /// re-activation) so external changes show up live. Skipped mid-drag so it never fights
    /// the user's own slider.
    fn refresh_volumes(&mut self) {
        // Clear the coalescing flag *before* reading, so a change that lands during the
        // read re-arms and posts again (we never miss the final state).
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
        // A mute flip swaps the slider's leading glyph (which lives in `base`); a plain
        // volume change only moves the fill/thumb/number, all drawn in the cheap `compose`
        // overlay — so avoid re-rasterizing every glyph for a mere volume tick (a mic's
        // auto-gain can fire these constantly).
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
                        if self.model.groups[group].flow == Flow::Output {
                            self.model.output_changed = true;
                        }
                        for row in &mut self.model.groups[group].devices {
                            row.selected = row.id == id;
                        }
                        self.model.groups[group].level =
                            self.backend.volume_of(&id).unwrap_or(self.model.groups[group].level);
                        self.model.groups[group].muted = self.backend.is_muted(&id).unwrap_or(false);
                        self.model.groups[group].default_id = Some(id);
                        self.rewatch(group); // follow volume/mute of the new default
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
                    self.model.config_changed = true;
                    if let Err(e) = self.config.save() {
                        eprintln!("save config failed: {e:#}");
                    }
                    self.navigate(View::Main, false);
                }
                false
            }
            Elem::UpdateBanner => {
                // The update is already on disk; the caller relaunches the exe and exits.
                self.model.restart = true;
                true
            }
            Elem::Action(ActionKind::SoundSettings) => {
                open_sound_settings();
                true
            }
            Elem::Action(ActionKind::Quit) => {
                self.model.quit = true;
                true
            }
            _ => false,
        }
    }

    /// Slide-transition from the current screen to `to`, then commit it. `forward` slides
    /// the new screen in from the right (drilling into the picker); otherwise it comes from
    /// the left (backing out). The window keeps a constant size — the width and height are
    /// both fixed — so this is a pure horizontal slide with no resize.
    fn navigate(&mut self, to: View, forward: bool) {
        let (w, h) = (self.surface.width, self.surface.height);
        let n = (w * h * 4) as usize;
        // Outgoing screen: reuse the current composed frame (keeps its slider fills etc.).
        let src = self.surface.buf.clone();
        let (dst_elems, _) = layout::build_view(&self.model, self.scale, w, to, h);
        let mut dst = vec![0u8; n];
        let ctx = render::Ctx {
            model: &self.model,
            accent: self.accent,
            scale: self.scale,
            width: w,
            height: h,
        };
        render::render_page(&ctx, &dst_elems, &mut dst);
        let mut frame = vec![0u8; n];

        let frames = 9;
        for i in 1..=frames {
            let t = i as f32 / frames as f32;
            let ease = 1.0 - (1.0 - t) * (1.0 - t); // ease-out quad
            let off = (ease * w as f32).round() as i32;
            // Forward: old slides left out, new enters from the right; back is the mirror.
            let (dx_src, dx_dst) = if forward { (-off, w - off) } else { (off, off - w) };
            {
                let mut cv = Canvas::new(&mut frame, w, h);
                cv.clear();
                cv.blit_shift(&src, dx_src);
                cv.blit_shift(&dst, dx_dst);
            }
            self.surface.present_buf(&frame, w, h, self.surface.x, self.surface.y, 255);
            std::thread::sleep(std::time::Duration::from_millis(9));
        }

        // Commit the destination screen.
        self.model.view = to;
        self.surface.elems = dst_elems;
        self.reset_hover();
        self.surface.base = vec![0u8; n];
        self.surface.buf = vec![0u8; n];
        self.render_base();
        self.compose();
        self.surface.flush();
    }

    /// Render the current view's static layer into the surface's `base` buffer. Slider
    /// fill/thumb/value and hover live in [`Self::compose`].
    fn render_base(&mut self) {
        let n = (self.surface.width * self.surface.height * 4) as usize;
        let mut base = vec![0u8; n];
        // Build the context inline (borrowing only `self.model`) so the disjoint borrows of
        // the surface's fields stay legal alongside the `&mut base` target.
        let ctx = render::Ctx {
            model: &self.model,
            accent: self.accent,
            scale: self.scale,
            width: self.surface.width,
            height: self.surface.height,
        };
        render::render_page(&ctx, &self.surface.elems, &mut base);
        self.surface.base = base;
    }

    /// Copy the static base, then draw the dynamic overlays (see [`render::compose`]).
    fn compose(&mut self) {
        // `Ctx` borrows only `self.model`, leaving `self.surface.buf` free to borrow mutably
        // as the compose target while `elems`/`base` are read (disjoint surface fields).
        let ctx = render::Ctx {
            model: &self.model,
            accent: self.accent,
            scale: self.scale,
            width: self.surface.width,
            height: self.surface.height,
        };
        render::compose(&ctx, &self.hit, &self.surface.elems, &self.surface.base, &mut self.surface.buf);
    }
}

fn mouse_xy(lp: LPARAM) -> (i32, i32) {
    let x = (lp.0 & 0xFFFF) as u16 as i16 as i32;
    let y = ((lp.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
    (x, y)
}

/// Play the Windows "Default Beep" — the same ding Windows itself plays on a volume
/// change (the `SystemDefault` sound). Async + best-effort; it honours the user's sound
/// scheme and plays at the current level, so it doubles as audible volume feedback.
fn beep_volume() {
    use windows::Win32::System::Diagnostics::Debug::MessageBeep;
    use windows::Win32::UI::WindowsAndMessaging::MB_OK;
    unsafe {
        let _ = MessageBeep(MB_OK);
    }
}

/// Open Settings ▸ System ▸ Sound (the native page the right-click menu offers).
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
