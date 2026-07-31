//! The scroll readout: the level bar that appears next to the taskbar buttons while the
//! wheel is turning.
//!
//! Scrolling over a button changes that endpoint's volume ([`crate::tray`] routes the
//! gesture), and this is the feedback for it — otherwise the only sign that anything
//! happened is the sound itself, which is no use at all on the input side. It shows the
//! endpoint's glyph, its level as a bar, and the number; it holds for [`HOLD`] after the
//! last change and then fades out over [`FADE`].
//!
//! A window of our own rather than something grown inside the strip, and that is the whole
//! design decision here. The strip is XAML we hand to Explorer, and widening it to make
//! room would reflow the notification area — every icon and the clock shifting sideways —
//! on every scroll, at the cost of a `put_Content` rebuild inside the shell's UI thread per
//! notch. A layered, click-through, topmost window costs the shell nothing, moves nothing,
//! and is drawn by the same [`crate::canvas`] rasteriser and the same
//! [`crate::flyout::theme`] palette as the control flyout, so it reads as part of the same
//! app. What it does do is sit *over* whatever is to the right of our buttons (another tray
//! icon, or the clock) for those three seconds.
//!
//! Everything here runs on the tray thread: it owns the window, and its message loop is
//! what drives [`Osd::tick`].

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use windows::core::w;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE,
};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromRect, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForSystem;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetCursorPos, GetSystemMetrics, KillTimer,
    RegisterClassW, SetTimer, SetWindowPos, ShowWindow, HWND_TOPMOST, SM_CXSCREEN, SM_CYSCREEN,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SW_HIDE, SW_SHOWNA, WNDCLASSW, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

use crate::audio::Flow;
use crate::canvas::{measure, Canvas, Rect};
use crate::flyout::theme::{
    accent_rgb, ui_font, GLYPH_MIC, GLYPH_MIC_OFF, GLYPH_MUTE, GLYPH_VOLUME, TEXT, TINT, TINT_A,
    TRACK_H,
};
use crate::icons;

/// How long the readout stays fully visible after the last change — the three seconds the
/// gesture asks for — and how long it then takes to fade.
const HOLD: Duration = Duration::from_secs(3);
const FADE: Duration = Duration::from_millis(400);

/// Repaint cadence while it is up. Only the fade actually paints; a tick inside [`HOLD`] is
/// a subtraction, so this costs nothing for most of the readout's life.
const TICK_MS: u32 = 33;
const TIMER_ID: usize = 1;

// Geometry, in DIPs, scaled by the display DPI when the window is built.
//
// `PANEL_H` and `RADIUS` are the taskbar pill's own height and corner radius (see the TAP's
// `decorate` module), so the readout reads as a piece detached from the buttons rather than
// as a foreign panel dropped on the taskbar.
const PANEL_W: f32 = 128.0;
const PANEL_H: f32 = 32.0;
const RADIUS: f32 = 6.0;
/// Standoff between the icon slot and the readout.
const GAP: f32 = 6.0;
const GLYPH_CX: f32 = 20.0; // centre of the leading glyph
const GLYPH_PX: f32 = 15.0;
const TRACK_X0: f32 = 34.0; // track's left edge
const TRACK_RIGHT: f32 = 36.0; // …and its right edge, measured from the panel's right
const VALUE_RIGHT: f32 = 11.0; // right inset of the level number
const VALUE_PX: f32 = 12.5;
/// Track background, and the fill of a *muted* endpoint — both borrowed from the flyout's
/// sliders, which is where the same two states are already drawn.
const TRACK_A: f32 = 0.28;
const MUTED_FILL_A: f32 = 0.34;
const MUTED_VALUE_A: f32 = 0.5;

/// Warm tint on a muted endpoint's glyph.
///
/// The flyout draws its muted slider glyph in the *accent* colour, and this deliberately
/// does not: the readout is only ever seen right beside the taskbar buttons, whose own muted
/// glyph is this warm tint, and side by side two different colours for one state read as a
/// mismatch. Keep it in step with `MUTED_TINT` in the TAP's `decorate` module.
const MUTED_GLYPH: [u8; 3] = [0xE8, 0x83, 0x6A];

/// The readout's window, pixels and fade state. Created on first use and then kept — it is
/// hidden between appearances, not destroyed, so a scroll never waits for a window.
pub(crate) struct Osd {
    hwnd: HWND,
    /// Display scale the current geometry and buffer were built for.
    scale: f32,
    width: i32,
    height: i32,
    buf: Vec<u8>,
    x: i32,
    y: i32,
    shown: bool,
    /// When the level last changed — the fade counts from here, so scrolling again while
    /// the readout is up simply restarts the three seconds.
    changed: Instant,
}

impl Osd {
    pub(crate) fn new() -> Self {
        Osd {
            hwnd: HWND(std::ptr::null_mut()),
            scale: 0.0, // no geometry yet; the first `show` builds it
            width: 0,
            height: 0,
            buf: Vec::new(),
            x: 0,
            y: 0,
            shown: false,
            changed: Instant::now(),
        }
    }

    /// Whether `hwnd` is this readout's window — how the tray loop tells our `WM_TIMER`
    /// apart from anyone else's.
    pub(crate) fn owns(&self, hwnd: HWND) -> bool {
        !self.hwnd.0.is_null() && self.hwnd.0 == hwnd.0
    }

    /// Show the level of one endpoint, or refresh it if it is already up, and restart the
    /// hold. `anchor` is the tray icon's screen rect (the readout sits just to its right);
    /// without one it falls back to the pointer, which is over the buttons anyway — that is
    /// how the scroll got here.
    pub(crate) fn show(&mut self, flow: Flow, level: f32, muted: bool, anchor: Option<RECT>) {
        if let Err(e) = self.ensure_window() {
            eprintln!("osd: could not create the readout window ({e})");
            return;
        }
        self.paint(flow, level, muted);
        (self.x, self.y) = self.place(anchor);
        // Painted before it is shown, so it never appears as an empty frame.
        crate::layered::present(self.hwnd, &self.buf, self.width, self.height, self.x, self.y, 255);
        if !self.shown {
            let _ = unsafe { ShowWindow(self.hwnd, SW_SHOWNA) };
            self.shown = true;
        }
        // Re-asserted on every appearance: the taskbar is itself topmost, and anything else
        // that has since claimed the front would otherwise be in front of us.
        let _ = unsafe {
            SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            )
        };
        self.changed = Instant::now();
        // Replaces the timer if one is already armed, which is what restarts the countdown.
        unsafe { SetTimer(Some(self.hwnd), TIMER_ID, TICK_MS, None) };
    }

    /// One frame of the hold-then-fade, driven by the tray loop's `WM_TIMER`.
    pub(crate) fn tick(&mut self) {
        if !self.shown {
            return;
        }
        let Some(fading) = self.changed.elapsed().checked_sub(HOLD) else {
            return; // still holding at full opacity
        };
        let t = fading.as_secs_f32() / FADE.as_secs_f32();
        if t >= 1.0 {
            self.hide();
            return;
        }
        // Ease-in: it barely dims to begin with and then goes quickly, so the readout looks
        // like it is being dismissed rather than slowly running out.
        let alpha = 255.0 * (1.0 - t * t);
        crate::layered::present(
            self.hwnd,
            &self.buf,
            self.width,
            self.height,
            self.x,
            self.y,
            alpha as u8,
        );
    }

    /// Take the readout away now. Also what the tray calls before opening the control
    /// panel: the panel supersedes it, and a frozen bar would otherwise sit there for as
    /// long as the panel is up (the panel's modal loop is not the one driving [`Self::tick`]).
    pub(crate) fn hide(&mut self) {
        if !self.shown {
            return;
        }
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_ID);
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
        self.shown = false;
    }

    /// Size the geometry to the current DPI and create the window if it does not exist yet.
    fn ensure_window(&mut self) -> windows::core::Result<()> {
        // Re-read every time: a monitor or scaling change while the tray runs would
        // otherwise leave the readout sized for the old DPI for the life of the process.
        let scale = (unsafe { GetDpiForSystem() } as f32 / 96.0).max(1.0);
        if (scale - self.scale).abs() > 0.01 {
            self.scale = scale;
            self.width = (PANEL_W * scale).round() as i32;
            self.height = (PANEL_H * scale).round() as i32;
            self.buf = vec![0u8; (self.width * self.height * 4) as usize];
        }
        if !self.hwnd.0.is_null() {
            return Ok(());
        }

        static REGISTERED: OnceLock<()> = OnceLock::new();
        let hinstance = HINSTANCE(unsafe { GetModuleHandleW(None) }?.0);
        REGISTERED.get_or_init(|| {
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: hinstance,
                lpszClassName: w!("AudioTrayVolumeOsd"),
                ..Default::default()
            };
            unsafe { RegisterClassW(&wc) };
        });

        // `WS_EX_TRANSPARENT` + `WS_EX_NOACTIVATE` make it a pure readout: clicks fall
        // straight through to the taskbar underneath, so covering the clock for three
        // seconds never costs the user a click.
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_LAYERED
                    | WS_EX_TOOLWINDOW
                    | WS_EX_TOPMOST
                    | WS_EX_NOACTIVATE
                    | WS_EX_TRANSPARENT,
                w!("AudioTrayVolumeOsd"),
                w!("Audio volume"),
                WS_POPUP,
                0,
                0,
                self.width,
                self.height,
                None,
                None,
                Some(hinstance),
                None,
            )
        }?;
        self.hwnd = hwnd;

        // Same treatment as the flyout: dark frame, rounded corners so the compositor's
        // blur follows the panel we paint, and the acrylic behind it.
        let dark: i32 = 1;
        let _ = unsafe {
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_USE_IMMERSIVE_DARK_MODE,
                &dark as *const _ as *const std::ffi::c_void,
                4,
            )
        };
        let round: i32 = 3; // DWMWCP_ROUNDSMALL — the panel is only 32 DIP tall
        let _ = unsafe {
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &round as *const _ as *const std::ffi::c_void,
                4,
            )
        };
        unsafe { crate::layered::enable_acrylic(hwnd) };
        Ok(())
    }

    /// Draw the panel, the endpoint's glyph, the level bar and the number.
    fn paint(&mut self, flow: Flow, level: f32, muted: bool) {
        let (scale, w, h) = (self.scale, self.width, self.height);
        let accent = accent_rgb();
        let level = level.clamp(0.0, 1.0);
        let cy = h as f32 / 2.0;
        let mut cv = Canvas::new(&mut self.buf, w, h);
        cv.clear();
        cv.fill_round_rect(Rect::new(0.0, 0.0, w as f32, h as f32), RADIUS * scale, TINT, TINT_A);

        // The same glyphs the flyout's sliders use, so "which endpoint, and is it muted" is
        // the same picture in both places — in the buttons' warm muted tint rather than the
        // flyout's accent, see [`MUTED_GLYPH`].
        let (glyph, glyph_col) = match (flow, muted) {
            (Flow::Output, false) => (GLYPH_VOLUME, TEXT),
            (Flow::Output, true) => (GLYPH_MUTE, MUTED_GLYPH),
            (Flow::Input, false) => (GLYPH_MIC, TEXT),
            (Flow::Input, true) => (GLYPH_MIC_OFF, MUTED_GLYPH),
        };
        let gpx = (GLYPH_PX * scale).round() as u32;
        if let Ok((rgba, gw, gh)) = icons::render_glyph(glyph, gpx, glyph_col) {
            let gx = (GLYPH_CX * scale).round() as i32 - gw as i32 / 2;
            cv.blit(gx, cy as i32 - gh as i32 / 2, &rgba, gw, gh, 1.0);
        }

        // Track, then the fill. No thumb, deliberately: this is a readout, and a thumb
        // would advertise a control that cannot be dragged (the window is click-through).
        let x0 = TRACK_X0 * scale;
        let x1 = w as f32 - TRACK_RIGHT * scale;
        let th = TRACK_H * scale;
        cv.fill_round_rect(Rect::new(x0, cy - th / 2.0, x1, cy + th / 2.0), th / 2.0, TEXT, TRACK_A);
        let fx = x0 + (x1 - x0) * level;
        if fx > x0 {
            let (col, alpha) = if muted { (TEXT, MUTED_FILL_A) } else { (accent, 1.0) };
            cv.fill_round_rect(Rect::new(x0, cy - th / 2.0, fx, cy + th / 2.0), th / 2.0, col, alpha);
        }

        // The level as a number, right-aligned — same as the flyout's sliders, percent sign
        // and all (i.e. none).
        if let Some(font) = ui_font() {
            let vpx = VALUE_PX * scale;
            let text = (level * 100.0).round().to_string();
            let vx = w as f32 - VALUE_RIGHT * scale - measure(font, vpx, &text);
            let alpha = if muted { MUTED_VALUE_A } else { 1.0 };
            cv.draw_text(font, vpx, (vx, cy + vpx * 0.34), TEXT, alpha, &text);
        }
    }

    /// Where the readout goes: to the right of the icon slot, vertically centred on it, and
    /// on the other side instead if there is no room (which is where a right-aligned
    /// taskbar's last icons put us).
    fn place(&self, anchor: Option<RECT>) -> (i32, i32) {
        let gap = (GAP * self.scale).round() as i32;
        let slot = anchor.unwrap_or_else(|| {
            let mut cursor = POINT::default();
            let _ = unsafe { GetCursorPos(&mut cursor) };
            RECT { left: cursor.x, top: cursor.y, right: cursor.x, bottom: cursor.y }
        });
        let mon = monitor_rect(slot);
        let y = slot.top + (slot.bottom - slot.top - self.height) / 2;
        let mut x = slot.right + gap;
        if x + self.width > mon.right {
            x = slot.left - gap - self.width;
        }
        (
            x.clamp(mon.left, (mon.right - self.width).max(mon.left)),
            y.clamp(mon.top, (mon.bottom - self.height).max(mon.top)),
        )
    }
}

impl Drop for Osd {
    fn drop(&mut self) {
        if self.hwnd.0.is_null() {
            return;
        }
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_ID);
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

/// Dev preview (`--osd`): put the readout up beside the cursor and pump until it has faded.
///
/// The readout only ever appears in response to a gesture on the taskbar buttons, so without
/// this there is no way to look at it — let alone screenshot it for a pixel comparison —
/// except by scrolling a live strip and racing the three-second hold. `level` overrides what
/// it draws (a percentage, `None` for the endpoint's own), and nothing here writes to the
/// device: this is the readout on its own, not a volume change.
pub(crate) fn preview(
    backend: &crate::audio::wasapi::WasapiBackend,
    flow: Flow,
    level: Option<f32>,
) -> anyhow::Result<()> {
    use anyhow::Context;
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, TranslateMessage, MSG, WM_TIMER,
    };

    let default = backend
        .default_of(flow)?
        .with_context(|| format!("no default {flow:?} endpoint to show"))?;
    let level = match level {
        Some(level) => level,
        None => backend.volume_of(&default)?,
    };
    let muted = backend.is_muted(&default).unwrap_or(false);

    let mut osd = Osd::new();
    osd.show(flow, level, muted, None);
    println!(
        "osd: {flow:?} at {:.0}%{} — {}x{} at {},{}, holding {}s then fading",
        level * 100.0,
        if muted { " (muted)" } else { "" },
        osd.width,
        osd.height,
        osd.x,
        osd.y,
        HOLD.as_secs()
    );

    let mut msg = MSG::default();
    while osd.shown {
        if unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 <= 0 {
            break;
        }
        if msg.message == WM_TIMER && osd.owns(msg.hwnd) {
            osd.tick();
            continue;
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    println!("osd: faded.");
    Ok(())
}

/// Bounds of the display `rect` sits on, to keep the readout on screen. Falls back to the
/// primary monitor, which is where the taskbar is unless the user has moved it.
fn monitor_rect(rect: RECT) -> RECT {
    let monitor = unsafe { MonitorFromRect(&rect, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        return info.rcMonitor;
    }
    RECT {
        left: 0,
        top: 0,
        right: unsafe { GetSystemMetrics(SM_CXSCREEN) },
        bottom: unsafe { GetSystemMetrics(SM_CYSCREEN) },
    }
}

/// A pass-through: the readout has no interaction of its own (it is click-through), and its
/// `WM_TIMER` is handled by the tray's message loop, which owns the state the fade needs.
unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    DefWindowProcW(hwnd, msg, wp, lp)
}
