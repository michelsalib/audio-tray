//! The scroll readout: the level bar that appears next to the taskbar buttons while the
//! wheel is turning.
//!
//! Shows the endpoint's glyph, level bar and number; holds for [`HOLD`] after the last
//! change, then fades over [`FADE`]. A layered, click-through, topmost window of our own:
//! growing the XAML strip instead would reflow the notification area on every notch.
//!
//! Everything here runs on the tray thread; the fade timer ticks on the tray's message window
//! ([`Osd::new`]), which calls [`Osd::tick`].

use std::time::{Duration, Instant};

use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromRect, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, DestroyWindow, GetCursorPos, GetSystemMetrics, KillTimer, SetTimer, SetWindowPos,
    ShowWindow, HWND_TOPMOST, SM_CXSCREEN, SM_CYSCREEN, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SW_HIDE, SW_SHOWNA, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_EX_TRANSPARENT,
};

use crate::audio::Flow;
use crate::canvas::{measure, Canvas, Rect};
use crate::flyout::theme::{
    accent_rgb, endpoint_glyph, recording_dot, ui_font, TEXT, TINT, TINT_A, TRACK_H,
};
use crate::icons;

/// How long the readout stays fully visible after the last change, then how long it fades.
const HOLD: Duration = Duration::from_secs(3);
const FADE: Duration = Duration::from_millis(400);

/// Tick cadence while it is up; only the fade repaints.
const TICK_MS: u32 = 33;
/// The fade timer's id, on whichever window [`Osd::new`] was told ticks go to.
pub(crate) const TIMER_ID: usize = 0x05D;

// Geometry, in DIPs. `PANEL_H` and `RADIUS` match the TAP's taskbar pill (`decorate`).
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
/// Track background and muted fill, as in the flyout's sliders.
const TRACK_A: f32 = 0.28;
const MUTED_FILL_A: f32 = 0.34;
const MUTED_VALUE_A: f32 = 0.5;

/// Warm tint on a muted glyph, matching the taskbar buttons beside it rather than the flyout's
/// accent. Keep in step with `MUTED_TINT` in the TAP's `decorate` module.
const MUTED_GLYPH: [u8; 3] = [0xE8, 0x83, 0x6A];

/// The readout's window, pixels and fade state. The window is created on first use, then hidden, never destroyed.
pub(crate) struct Osd {
    hwnd: HWND,
    /// Where the fade timer's `WM_TIMER` goes: the tray's message window, or our own (`None`).
    ticks_to: Option<HWND>,
    /// Display scale the current geometry and buffer were built for.
    scale: f32,
    width: i32,
    height: i32,
    buf: Vec<u8>,
    x: i32,
    y: i32,
    shown: bool,
    /// When the level last changed; the hold and fade count from here.
    changed: Instant,
}

impl Osd {
    pub(crate) fn new(ticks_to: Option<HWND>) -> Self {
        Osd {
            hwnd: HWND(std::ptr::null_mut()),
            ticks_to,
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

    /// Whether `hwnd` is this readout's window (for [`preview`]'s own loop).
    #[cfg(feature = "dev")]
    fn owns(&self, hwnd: HWND) -> bool {
        !self.hwnd.0.is_null() && self.hwnd.0 == hwnd.0
    }

    fn timer_window(&self) -> HWND {
        self.ticks_to.unwrap_or(self.hwnd)
    }

    /// Show or refresh one endpoint's level and restart the hold. `anchor` is the tray icon's
    /// screen rect; without one the pointer is used.
    pub(crate) fn show(&mut self, flow: Flow, level: f32, muted: bool, anchor: Option<RECT>) {
        let at = match anchor {
            Some(slot) => POINT { x: (slot.left + slot.right) / 2, y: (slot.top + slot.bottom) / 2 },
            None => {
                let mut cursor = POINT::default();
                let _ = unsafe { GetCursorPos(&mut cursor) };
                cursor
            }
        };
        if let Err(e) = self.ensure_window(crate::win::monitor_at(at).0) {
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
        // Re-asserted every time: the taskbar is topmost too.
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
        // Replaces an armed timer.
        unsafe { SetTimer(Some(self.timer_window()), TIMER_ID, TICK_MS, None) };
    }

    /// One frame of the hold-then-fade, on each [`TIMER_ID`] tick.
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
        // Ease-in.
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

    /// Take the readout away now (the tray does before opening the flyout, which covers it).
    pub(crate) fn hide(&mut self) {
        if !self.shown {
            return;
        }
        unsafe {
            let _ = KillTimer(Some(self.timer_window()), TIMER_ID);
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
        self.shown = false;
    }

    /// Size the geometry for `scale` (the target monitor's DPI) and create the window if needed.
    fn ensure_window(&mut self, scale: f32) -> windows::core::Result<()> {
        if (scale - self.scale).abs() > 0.01 {
            self.scale = scale;
            self.width = (PANEL_W * scale).round() as i32;
            self.height = (PANEL_H * scale).round() as i32;
            self.buf = vec![0u8; (self.width * self.height * 4) as usize];
        }
        if !self.hwnd.0.is_null() {
            return Ok(());
        }

        // Click-through and never activated.
        let hwnd = crate::win::create_popup(
            w!("AudioTrayVolumeOsd"),
            w!("Audio volume"),
            Some(wndproc),
            WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT,
            (0, 0, self.width, self.height),
        )?;
        self.hwnd = hwnd;
        // The small corner: the flyout's radius over-curves a 32 DIP panel.
        unsafe { crate::layered::style_panel(hwnd, crate::layered::CORNER_ROUND_SMALL) };
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

        let glyph = endpoint_glyph(flow, muted);
        let glyph_col = if muted { MUTED_GLYPH } else { TEXT };
        let gpx = (GLYPH_PX * scale).round() as u32;
        if let Ok((rgba, gw, gh)) = icons::render_glyph(glyph, gpx, glyph_col) {
            let gx = (GLYPH_CX * scale).round() as i32 - gw as i32 / 2;
            let gy = cy as i32 - gh as i32 / 2;
            cv.blit(gx, gy, &rgba, gw, gh, 1.0);
            if flow == Flow::Input && crate::audio::mic::in_use() {
                recording_dot(&mut cv, gx, gy, gpx);
            }
        }

        // Track, then the fill; no thumb, since nothing here can be dragged.
        let x0 = TRACK_X0 * scale;
        let x1 = w as f32 - TRACK_RIGHT * scale;
        let th = TRACK_H * scale;
        cv.fill_round_rect(Rect::new(x0, cy - th / 2.0, x1, cy + th / 2.0), th / 2.0, TEXT, TRACK_A);
        let fx = x0 + (x1 - x0) * level;
        if fx > x0 {
            let (col, alpha) = if muted { (TEXT, MUTED_FILL_A) } else { (accent, 1.0) };
            cv.fill_round_rect(Rect::new(x0, cy - th / 2.0, fx, cy + th / 2.0), th / 2.0, col, alpha);
        }

        // The level as a number, right-aligned, no percent sign (as in the flyout).
        if let Some(font) = ui_font() {
            let vpx = VALUE_PX * scale;
            let text = (level * 100.0).round().to_string();
            let vx = w as f32 - VALUE_RIGHT * scale - measure(font, vpx, &text);
            let alpha = if muted { MUTED_VALUE_A } else { 1.0 };
            cv.draw_text(font, vpx, (vx, cy + vpx * 0.34), TEXT, alpha, &text);
        }
    }

    /// Where the readout goes: right of the icon slot, centred on it, or left if there is no room.
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
            let _ = KillTimer(Some(self.timer_window()), TIMER_ID);
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

/// Dev preview (`--osd`): put the readout up beside the cursor and pump until it has faded.
/// `level` overrides the drawn level; nothing is written to the device.
#[cfg(feature = "dev")]
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

    let mut osd = Osd::new(None);
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

/// Bounds of the display `rect` sits on, else the primary screen.
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

/// A pass-through: the readout is click-through and its timer is handled by whoever pumps it.
unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    DefWindowProcW(hwnd, msg, wp, lp)
}
