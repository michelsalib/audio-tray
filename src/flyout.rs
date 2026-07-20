//! A custom Windows-11-style flyout, modelled on the system sound-output flyout: an
//! acrylic-blurred, rounded, dark surface with an accent "pill" on the selected row.
//!
//! A classic Win32 `TrackPopupMenu` can't look like this — that flyout is a WinUI/XAML
//! surface. So we build our own: a per-pixel-alpha *layered* window whose contents we
//! paint into an ARGB buffer (`UpdateLayeredWindow`), with the acrylic blur supplied by
//! the compositor (`SetWindowCompositionAttribute`) and rounded corners by DWM. Rows are
//! laid out and hit-tested by hand; the flyout is modal via mouse capture, like a menu.

use std::sync::OnceLock;

use ab_glyph::{Font, FontVec, PxScale, ScaleFont};
use windows::core::{s, w};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE,
};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject,
    AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS,
    HGDIOBJ,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::UI::HiDpi::GetDpiForSystem;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetCursorPos, GetMessageW,
    LoadCursorW, RegisterClassW, SetCursor, SetForegroundWindow, ShowWindow, SystemParametersInfoW,
    TranslateMessage, UpdateLayeredWindow, IDC_ARROW, MSG, SPI_GETWORKAREA, SW_SHOWNA,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, ULW_ALPHA, WM_CAPTURECHANGED, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MOUSEMOVE, WM_RBUTTONDOWN, WNDCLASSW, WS_EX_LAYERED, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP,
};

use crate::audio::{Device, DeviceId};
use crate::config::Config;
use crate::icons::{self, IconId};

/// What the user chose in the flyout.
pub enum FlyoutAction {
    Switch(DeviceId),
    SetIcon(String, IconId),
    Quit,
}

/// Where to open the flyout: horizontally centred on the tray icon (`cx`), sitting just
/// above it (`bottom` = the icon's top), like the native tray flyouts.
#[derive(Clone, Copy)]
pub struct Anchor {
    pub cx: i32,
    pub bottom: i32,
}

// Layout, in DIPs (scaled by the monitor DPI at show time). Tuned to the native Win11
// "Sortie son" (sound-output) flyout: roomy 40-DIP rows, semibold section headers, an
// accent selection pill, and generous side padding.
const CORNER: f32 = 8.0;
const PAD_V: f32 = 6.0; // top/bottom padding inside the panel
const ITEM_H: f32 = 44.0; // clickable row height (native list-item pitch ≈ 44 DIP)
const HEADER_H: f32 = 40.0; // section-header block (top gap + label)
const SEP_H: f32 = 9.0;
const ICON_X: f32 = 14.0; // left inset of a row's leading icon
const ICON_PX: f32 = 20.0; // leading icon glyph size
const TEXT_X: f32 = 48.0; // left inset of a row's label
const HEADER_X: f32 = 15.0; // left inset of a section-header label
const RIGHT_PAD: f32 = 20.0;
const MIN_W: f32 = 340.0;
const MAX_W: f32 = 460.0;
const ROW_MARGIN: f32 = 4.0; // side margin of the row highlight/pill
const ROW_RADIUS: f32 = 4.0; // corner radius of the row highlight
const PILL_W: f32 = 3.0; // accent selection-indicator pill width
const PILL_H: f32 = 16.0; // accent selection-indicator pill height

// Colours (RGB); alpha applied at blend time.
const TINT: [u8; 3] = [0x2C, 0x2C, 0x2C]; // panel base (semi-transparent, acrylic shows through)
const TINT_A: f32 = 0.82;
const TEXT: [u8; 3] = [0xFF, 0xFF, 0xFF]; // primary text (body + section headers, differ by weight)
const HOVER_A: f32 = 0.06; // white overlay for hover
const SEL_A: f32 = 0.09; // white overlay for the selected row

#[derive(Clone)]
enum RowKind {
    Item { icon: Option<IconId>, glyph: Option<char>, label: String, selected: bool },
    Header(String),
    Separator,
}

struct Row {
    kind: RowKind,
    top: i32,
    height: i32,
    action: Option<FlyoutAction>,
}

struct Flyout {
    scale: f32,
    width: i32,
    height: i32,
    accent: [u8; 3],
    rows: Vec<Row>,
    /// index into `rows` of the currently hovered actionable row
    hover: Option<usize>,
    base: Vec<u8>, // static content (panel + text + icons + selection), rendered once
    buf: Vec<u8>,  // base + hover overlay, straight-alpha RGBA, presented each frame
}

/// Show the flyout near the tray and block until the user picks an item or dismisses it.
pub fn show(
    devices: &[Device],
    current: Option<&DeviceId>,
    config: &Config,
    anchor: Option<Anchor>,
) -> Option<FlyoutAction> {
    unsafe { show_inner(devices, current, config, anchor) }
}

unsafe fn show_inner(
    devices: &[Device],
    current: Option<&DeviceId>,
    config: &Config,
    anchor: Option<Anchor>,
) -> Option<FlyoutAction> {
    let scale = (GetDpiForSystem() as f32 / 96.0).max(1.0);
    let accent = accent_rgb();

    // The device whose icon the "Icon" section edits: the current default (else the first).
    let target = current
        .and_then(|c| devices.iter().find(|d| &d.id == c))
        .or_else(|| devices.first());

    let mut fly = Flyout {
        scale,
        width: 0,
        height: 0,
        accent,
        rows: Vec::new(),
        hover: None,
        base: Vec::new(),
        buf: Vec::new(),
    };
    fly.build_rows(devices, current, target, config);
    fly.layout();

    // Position: bottom-right anchored to the tray icon (so the flyout opens above it),
    // clamped to the work area. Falls back to the cursor when there's no anchor.
    let mut wa = RECT::default();
    let _ = SystemParametersInfoW(
        SPI_GETWORKAREA,
        0,
        Some(&mut wa as *mut _ as *mut std::ffi::c_void),
        SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
    );
    let margin = (8.0 * scale) as i32;
    let gap = (8.0 * scale) as i32;
    let (cx, bottom) = match anchor {
        Some(a) => (a.cx, a.bottom - gap),
        None => {
            let mut cur = POINT::default();
            let _ = GetCursorPos(&mut cur);
            (cur.x, cur.y)
        }
    };
    let bottom = bottom.min(wa.bottom - margin);
    // Centre horizontally on the icon, clamped to the work area.
    let x = (cx - fly.width / 2)
        .min(wa.right - margin - fly.width)
        .max(wa.left + margin);
    let y = (bottom - fly.height).max(wa.top + margin);

    let hwnd = match fly.create_window(x, y) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("flyout: create_window failed: {e:?}");
            return None;
        }
    };

    // Paint the static layer once (expensive: text + icons); hover only re-composites.
    fly.render_base();
    fly.compose();
    let _ = ShowWindow(hwnd, SW_SHOWNA);
    fly.animate_in(hwnd, x, y);
    // Foreground + capture so the flyout behaves like a menu: it receives every mouse
    // move/click (for hover + selection) and an outside click reliably dismisses it.
    let _ = SetForegroundWindow(hwnd);
    SetCapture(hwnd);
    // While the mouse is captured Windows stops sending WM_SETCURSOR, so the class cursor
    // isn't consulted — force the arrow explicitly (kills the leftover "busy" spinner).
    let _ = SetCursor(LoadCursorW(None, IDC_ARROW).ok());

    // Modal loop: mouse is captured to us, so we see every move/click (coords relative to
    // our client). A click inside a row selects it; a click outside dismisses.
    let mut result: Option<FlyoutAction> = None;
    let mut msg = MSG::default();
    'pump: while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
        match msg.message {
            WM_MOUSEMOVE => {
                let (mx, my) = mouse_xy(msg.lParam);
                let inside = (0..fly.width).contains(&mx) && (0..fly.height).contains(&my);
                let hover = if inside { fly.row_at(my) } else { None };
                if hover != fly.hover {
                    fly.hover = hover;
                    fly.compose();
                    fly.present(hwnd, x, y, 255);
                }
            }
            WM_LBUTTONUP => {
                let (mx, my) = mouse_xy(msg.lParam);
                let inside = (0..fly.width).contains(&mx) && (0..fly.height).contains(&my);
                if inside {
                    if let Some(i) = fly.row_at(my) {
                        result = fly.rows[i].action.take();
                        break 'pump;
                    }
                } else {
                    break 'pump; // click outside → dismiss
                }
            }
            WM_LBUTTONDOWN | WM_RBUTTONDOWN => {
                let (mx, my) = mouse_xy(msg.lParam);
                let inside = (0..fly.width).contains(&mx) && (0..fly.height).contains(&my);
                if !inside {
                    break 'pump;
                }
            }
            WM_CAPTURECHANGED => break 'pump, // lost capture (e.g. clicked another app)
            _ => {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    let _ = ReleaseCapture();
    let _ = DestroyWindow(hwnd);
    result
}

impl Flyout {
    fn build_rows(
        &mut self,
        devices: &[Device],
        current: Option<&DeviceId>,
        target: Option<&Device>,
        config: &Config,
    ) {
        // Devices (click to switch; current is selected), under a section header like the
        // native flyout's "Périphérique de sortie".
        self.rows.push(Row {
            kind: RowKind::Header("Output device".to_string()),
            top: 0,
            height: 0,
            action: None,
        });
        for dev in devices {
            let icon = config
                .icon_for(&dev.id.0)
                .unwrap_or_else(|| icons::default_icon(dev.form_factor, &dev.friendly_name));
            let selected = current == Some(&dev.id);
            self.rows.push(Row {
                kind: RowKind::Item {
                    icon: Some(icon),
                    glyph: None,
                    label: dev.friendly_name.clone(),
                    selected,
                },
                top: 0,
                height: 0,
                action: Some(FlyoutAction::Switch(dev.id.clone())),
            });
        }

        // Icon picker for the current default device (its own section header separates it
        // from the device list — no rule, matching the native flyout).
        if let Some(dev) = target {
            self.rows.push(Row {
                kind: RowKind::Header("Icon".to_string()),
                top: 0,
                height: 0,
                action: None,
            });
            let sel = config
                .icon_for(&dev.id.0)
                .unwrap_or_else(|| icons::default_icon(dev.form_factor, &dev.friendly_name));
            for icon in IconId::ALL {
                self.rows.push(Row {
                    kind: RowKind::Item {
                        icon: Some(icon),
                        glyph: None,
                        label: icon.label().to_string(),
                        selected: icon == sel,
                    },
                    top: 0,
                    height: 0,
                    action: Some(FlyoutAction::SetIcon(dev.id.0.clone(), icon)),
                });
            }
        }

        // Quit.
        self.rows.push(Row { kind: RowKind::Separator, top: 0, height: 0, action: None });
        self.rows.push(Row {
            kind: RowKind::Item {
                icon: None,
                glyph: Some('\u{E711}'), // Cancel
                label: "Quit".to_string(),
                selected: false,
            },
            top: 0,
            height: 0,
            action: Some(FlyoutAction::Quit),
        });
    }

    /// Assign each row a vertical slot and compute the panel size.
    fn layout(&mut self) {
        let scale = self.scale;
        let d = |v: f32| (v * scale).round() as i32;

        // Width from the widest content: row labels (indented past the icon) and section
        // headers (indented to HEADER_X), whichever needs more room.
        let font = ui_font();
        let font_sb = ui_font_semibold();
        let text_px = 14.0 * scale;
        let hdr_px = 14.0 * scale;
        let mut max_w = 0.0f32;
        for r in &self.rows {
            match &r.kind {
                RowKind::Item { label, .. } => {
                    if let Some(f) = font {
                        max_w = max_w.max(d(TEXT_X) as f32 + measure(f, text_px, label));
                    }
                }
                RowKind::Header(h) => {
                    if let Some(f) = font_sb.or(font) {
                        max_w = max_w.max(d(HEADER_X) as f32 + measure(f, hdr_px, h));
                    }
                }
                RowKind::Separator => {}
            }
        }
        let want = max_w + d(RIGHT_PAD) as f32;
        self.width = want.clamp(d(MIN_W) as f32, d(MAX_W) as f32).round() as i32;

        let mut y = d(PAD_V);
        for r in &mut self.rows {
            let h = match &r.kind {
                RowKind::Item { .. } => d(ITEM_H),
                RowKind::Header(_) => d(HEADER_H),
                RowKind::Separator => d(SEP_H),
            };
            r.top = y;
            r.height = h;
            y += h;
        }
        self.height = y + d(PAD_V);
        let bytes = (self.width * self.height * 4) as usize;
        self.base = vec![0u8; bytes];
        self.buf = vec![0u8; bytes];
    }

    fn row_at(&self, y: i32) -> Option<usize> {
        self.rows
            .iter()
            .position(|r| r.action.is_some() && y >= r.top && y < r.top + r.height)
    }

    /// Render the static layer (panel + separators + text + icons + selection pill) into
    /// `base`. Expensive (glyph rasterization) — done once; hover updates only `compose`.
    fn render_base(&mut self) {
        let scale = self.scale;
        let accent = self.accent;
        let w = self.width;
        let h = self.height;
        let d = |v: f32| (v * scale).round() as i32;
        // Disjoint field borrows: rows shared, base mutable.
        let rows = &self.rows;
        let buf = self.base.as_mut_slice();

        for p in buf.iter_mut() {
            *p = 0;
        }
        // Panel base: rounded, semi-transparent (acrylic shows through the rest).
        fill_round_rect(buf, w, h, 0.0, 0.0, w as f32, h as f32, d(CORNER) as f32, TINT, TINT_A);

        let font = ui_font();
        let font_sb = ui_font_semibold().or(font);
        let text_px = 14.0 * scale;
        let hdr_px = 14.0 * scale;
        let icon_px = (ICON_PX * scale).round() as u32;
        let mx = d(ROW_MARGIN) as f32;

        for r in rows.iter() {
            match &r.kind {
                RowKind::Separator => {
                    let sy = r.top as f32 + r.height as f32 / 2.0;
                    fill_rect(buf, w, h, mx + d(6.0) as f32, sy, w as f32 - mx - d(6.0) as f32, sy + 1.0, [0xFF, 0xFF, 0xFF], 0.07);
                }
                RowKind::Header(text) => {
                    // Semibold, full white — matches the native flyout's section captions
                    // (body text below is the same colour but regular weight).
                    if let Some(f) = font_sb {
                        let base = r.top as f32 + r.height as f32 - hdr_px * 0.62;
                        draw_text(buf, w, h, f, hdr_px, d(HEADER_X) as f32, base, TEXT, 1.0, text);
                    }
                }
                RowKind::Item { icon, glyph, label, selected } => {
                    let ry0 = r.top as f32 + 1.0;
                    let ry1 = (r.top + r.height) as f32 - 1.0;
                    if *selected {
                        fill_round_rect(buf, w, h, mx, ry0, w as f32 - mx, ry1, d(ROW_RADIUS) as f32, [0xFF, 0xFF, 0xFF], SEL_A);
                        // Accent selection-indicator pill on the selected row (3×16 DIP, like
                        // the native flyout / WinUI NavigationView).
                        let ph = d(PILL_H) as f32;
                        let pw = d(PILL_W) as f32;
                        let py0 = (ry0 + ry1) / 2.0 - ph / 2.0;
                        let px0 = mx + d(2.0) as f32;
                        fill_round_rect(buf, w, h, px0, py0, px0 + pw, py0 + ph, pw / 2.0, accent, 1.0);
                    }

                    let cy = r.top + r.height / 2;
                    // Leading glyph: neutral white (only the accent pill carries the accent,
                    // as in the native flyout).
                    if let Some(id) = icon {
                        if let Ok((rgba, gw, gh)) = id.render(icon_px, TEXT) {
                            blit(buf, w, h, d(ICON_X), cy - gh as i32 / 2, &rgba, gw, gh);
                        }
                    } else if let Some(ch) = glyph {
                        if let Some(ff) = icons::fluent_font() {
                            let gpx = ICON_PX * scale;
                            draw_text(buf, w, h, ff, gpx, d(ICON_X) as f32, cy as f32 + gpx * 0.36, TEXT, 1.0, &ch.to_string());
                        }
                    }
                    // Label.
                    if let Some(f) = font {
                        let base = cy as f32 + text_px * 0.34;
                        draw_text(buf, w, h, f, text_px, d(TEXT_X) as f32, base, TEXT, 1.0, label);
                    }
                }
            }
        }
    }

    /// Cheap per-frame composite: copy the static base, then draw the hover highlight over
    /// the hovered row. Keeps hover updates snappy (no glyph rasterization).
    fn compose(&mut self) {
        self.buf.copy_from_slice(&self.base);
        let Some(i) = self.hover else { return };
        let scale = self.scale;
        let (w, h) = (self.width, self.height);
        let d = |v: f32| (v * scale).round() as i32;
        let (top, height) = (self.rows[i].top, self.rows[i].height);
        let mx = d(ROW_MARGIN) as f32;
        let ry0 = top as f32 + 1.0;
        let ry1 = (top + height) as f32 - 1.0;
        fill_round_rect(self.buf.as_mut_slice(), w, h, mx, ry0, w as f32 - mx, ry1, d(ROW_RADIUS) as f32, [0xFF, 0xFF, 0xFF], HOVER_A);
    }

    unsafe fn create_window(&self, x: i32, y: i32) -> windows::core::Result<HWND> {
        static REGISTERED: OnceLock<()> = OnceLock::new();
        let hinstance = HINSTANCE(GetModuleHandleW(None)?.0);
        REGISTERED.get_or_init(|| {
            // A class cursor is required, or the window inherits whatever the cursor last
            // was — often the "app starting" spinner left over while we rasterize/animate
            // before pumping messages — and never resets it to a plain arrow.
            let cursor = LoadCursorW(None, IDC_ARROW).unwrap_or_default();
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: hinstance,
                hCursor: cursor,
                lpszClassName: w!("AudioTrayFlyout"),
                ..Default::default()
            };
            RegisterClassW(&wc);
        });

        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            w!("AudioTrayFlyout"),
            w!("Audio output"),
            WS_POPUP,
            x,
            y,
            self.width,
            self.height,
            None,
            None,
            Some(hinstance),
            None,
        )?;

        // Win11 dark + rounded corners for the frame around our acrylic content.
        let dark: i32 = 1;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &dark as *const _ as *const std::ffi::c_void,
            4,
        );
        let round: i32 = 2; // DWMWCP_ROUND
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &round as *const _ as *const std::ffi::c_void,
            4,
        );
        enable_acrylic(hwnd);
        Ok(hwnd)
    }

    /// Slide up + fade in, like the native tray flyouts. Runs before the modal loop.
    unsafe fn animate_in(&self, hwnd: HWND, x: i32, y: i32) {
        let slide = (14.0 * self.scale) as i32;
        let frames = 9;
        for i in 1..=frames {
            let t = i as f32 / frames as f32;
            let ease = 1.0 - (1.0 - t) * (1.0 - t); // ease-out quad
            let yy = y + (slide as f32 * (1.0 - ease)) as i32;
            self.present(hwnd, x, yy, (255.0 * ease) as u8);
            std::thread::sleep(std::time::Duration::from_millis(9));
        }
        self.present(hwnd, x, y, 255);
    }

    /// Push the rendered ARGB buffer to the layered window (premultiplied BGRA), scaled by
    /// a global `alpha` (for fade animations).
    unsafe fn present(&self, hwnd: HWND, x: i32, y: i32, alpha: u8) {
        let (w, h) = (self.width, self.height);
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));

        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let hbm = CreateDIBSection(Some(mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0);
        let Ok(hbm) = hbm else {
            let _ = DeleteDC(mem);
            ReleaseDC(None, screen);
            return;
        };

        // straight-alpha RGBA -> premultiplied BGRA
        let px = (w * h) as usize;
        let dst = std::slice::from_raw_parts_mut(bits as *mut u8, px * 4);
        for i in 0..px {
            let r = self.buf[i * 4] as u32;
            let g = self.buf[i * 4 + 1] as u32;
            let b = self.buf[i * 4 + 2] as u32;
            let a = self.buf[i * 4 + 3] as u32;
            dst[i * 4] = ((b * a) / 255) as u8;
            dst[i * 4 + 1] = ((g * a) / 255) as u8;
            dst[i * 4 + 2] = ((r * a) / 255) as u8;
            dst[i * 4 + 3] = a as u8;
        }

        let old = SelectObject(mem, HGDIOBJ(hbm.0));
        let mut src = POINT { x: 0, y: 0 };
        let mut dpos = POINT { x, y };
        let mut size = SIZE { cx: w, cy: h };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: alpha,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let _ = UpdateLayeredWindow(
            hwnd,
            Some(screen),
            Some(&mut dpos),
            Some(&mut size),
            Some(mem),
            Some(&mut src),
            COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        );

        SelectObject(mem, old);
        let _ = DeleteObject(HGDIOBJ(hbm.0));
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    DefWindowProcW(hwnd, msg, wp, lp)
}

fn mouse_xy(lp: LPARAM) -> (i32, i32) {
    let x = (lp.0 & 0xFFFF) as u16 as i16 as i32;
    let y = ((lp.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
    (x, y)
}

// --- acrylic + accent -------------------------------------------------------

/// Enable the acrylic blur-behind via the undocumented (but ubiquitous)
/// `SetWindowCompositionAttribute`. Best-effort — if it no-ops, the panel is still a
/// legible semi-transparent dark surface.
unsafe fn enable_acrylic(hwnd: HWND) {
    #[repr(C)]
    struct AccentPolicy {
        accent_state: u32,
        accent_flags: u32,
        gradient_color: u32,
        animation_id: u32,
    }
    #[repr(C)]
    struct WindowCompositionAttributeData {
        attrib: u32,
        pv_data: *mut std::ffi::c_void,
        cb_data: usize,
    }
    type SetWca = unsafe extern "system" fn(HWND, *mut WindowCompositionAttributeData) -> i32;

    let Ok(user32) = GetModuleHandleW(w!("user32.dll")) else {
        return;
    };
    let Some(p) = GetProcAddress(user32, s!("SetWindowCompositionAttribute")) else {
        return;
    };
    let set_wca: SetWca = std::mem::transmute(p);

    const ACCENT_ENABLE_ACRYLICBLURBEHIND: u32 = 4;
    const WCA_ACCENT_POLICY: u32 = 19;
    let mut policy = AccentPolicy {
        accent_state: ACCENT_ENABLE_ACRYLICBLURBEHIND,
        accent_flags: 0,
        // AABBGGRR — a light dark tint; our own buffer supplies most of the tint.
        gradient_color: 0x0020_2020,
        animation_id: 0,
    };
    let mut data = WindowCompositionAttributeData {
        attrib: WCA_ACCENT_POLICY,
        pv_data: &mut policy as *mut _ as *mut std::ffi::c_void,
        cb_data: std::mem::size_of::<AccentPolicy>(),
    };
    set_wca(hwnd, &mut data);
}

/// The user's Windows accent colour (registry `DWM\AccentColor`, stored `AABBGGRR`).
fn accent_rgb() -> [u8; 3] {
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
    let mut v: u32 = 0;
    let mut size = 4u32;
    let ok = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\DWM"),
            w!("AccentColor"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut v as *mut u32 as *mut core::ffi::c_void),
            Some(&mut size),
        )
    };
    if ok.0 == 0 {
        [(v & 0xFF) as u8, ((v >> 8) & 0xFF) as u8, ((v >> 16) & 0xFF) as u8]
    } else {
        [0x60, 0xCD, 0xFF] // fallback Win11 accent
    }
}

// --- software rendering -----------------------------------------------------

fn ui_font() -> Option<&'static FontVec> {
    static FONT: OnceLock<Option<FontVec>> = OnceLock::new();
    FONT.get_or_init(|| {
        let bytes = std::fs::read(r"C:\Windows\Fonts\segoeui.ttf").ok()?;
        FontVec::try_from_vec(bytes).ok()
    })
    .as_ref()
}

/// Segoe UI Semibold — the weight Windows uses for the flyout's section captions
/// ("BodyStrong"). Falls back to the regular UI font at the call site if absent.
fn ui_font_semibold() -> Option<&'static FontVec> {
    static FONT: OnceLock<Option<FontVec>> = OnceLock::new();
    FONT.get_or_init(|| {
        let bytes = std::fs::read(r"C:\Windows\Fonts\seguisb.ttf").ok()?;
        FontVec::try_from_vec(bytes).ok()
    })
    .as_ref()
}

/// Convert a desired **em size** (in px) into the ab_glyph `PxScale` that actually yields
/// it. ab_glyph scales a font by its *height* (ascent − descent + line-gap), so a plain
/// `PxScale::from(px)` renders an em of only ~0.75·px for Segoe UI — leaving text ~25%
/// smaller than the same nominal size in Windows' own em-based layout (the native flyout).
/// Sizing by em keeps our text matched to Windows' point sizes.
fn em_scale(font: &FontVec, em_px: f32) -> PxScale {
    match font.units_per_em() {
        Some(upem) => PxScale::from(em_px * font.height_unscaled() / upem),
        None => PxScale::from(em_px),
    }
}

fn measure(font: &FontVec, px: f32, text: &str) -> f32 {
    let sf = font.as_scaled(em_scale(font, px));
    text.chars().map(|c| sf.h_advance(font.glyph_id(c))).sum()
}

#[allow(clippy::too_many_arguments)]
fn draw_text(buf: &mut [u8], w: i32, h: i32, font: &FontVec, px: f32, mut pen: f32, baseline: f32, col: [u8; 3], alpha: f32, text: &str) {
    let scale = em_scale(font, px);
    let sf = font.as_scaled(scale);
    for ch in text.chars() {
        let gid = font.glyph_id(ch);
        let glyph = ab_glyph::Glyph { id: gid, scale, position: ab_glyph::point(pen, baseline) };
        if let Some(outline) = font.outline_glyph(glyph) {
            let bb = outline.px_bounds();
            outline.draw(|gx, gy, cov| {
                let x = bb.min.x as i32 + gx as i32;
                let y = bb.min.y as i32 + gy as i32;
                blend(buf, w, h, x, y, col, cov * alpha);
            });
        }
        pen += sf.h_advance(gid);
    }
}

/// Blit a straight-alpha RGBA sprite (its own colour) onto the buffer.
fn blit(buf: &mut [u8], w: i32, h: i32, x0: i32, y0: i32, rgba: &[u8], sw: u32, sh: u32) {
    for sy in 0..sh as i32 {
        for sx in 0..sw as i32 {
            let i = ((sy as u32 * sw + sx as u32) * 4) as usize;
            let a = rgba[i + 3] as f32 / 255.0;
            if a <= 0.0 {
                continue;
            }
            blend(buf, w, h, x0 + sx, y0 + sy, [rgba[i], rgba[i + 1], rgba[i + 2]], a);
        }
    }
}

fn fill_rect(buf: &mut [u8], w: i32, h: i32, x0: f32, y0: f32, x1: f32, y1: f32, col: [u8; 3], alpha: f32) {
    for y in y0.floor() as i32..y1.ceil() as i32 {
        for x in x0.floor() as i32..x1.ceil() as i32 {
            blend(buf, w, h, x, y, col, alpha);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn fill_round_rect(buf: &mut [u8], w: i32, h: i32, x0: f32, y0: f32, x1: f32, y1: f32, r: f32, col: [u8; 3], alpha: f32) {
    let cx = (x0 + x1) / 2.0;
    let cy = (y0 + y1) / 2.0;
    let hx = (x1 - x0) / 2.0;
    let hy = (y1 - y0) / 2.0;
    let r = r.min(hx).min(hy);
    for y in y0.floor() as i32..y1.ceil() as i32 {
        for x in x0.floor() as i32..x1.ceil() as i32 {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let qx = (px - cx).abs() - (hx - r);
            let qy = (py - cy).abs() - (hy - r);
            let outside = qx.max(0.0).hypot(qy.max(0.0));
            let inside = qx.max(qy).min(0.0);
            let sd = outside + inside - r;
            let cov = (0.5 - sd).clamp(0.0, 1.0);
            if cov > 0.0 {
                blend(buf, w, h, x, y, col, alpha * cov);
            }
        }
    }
}

/// Source-over blend of a straight-alpha colour into the straight-alpha RGBA buffer.
fn blend(buf: &mut [u8], w: i32, h: i32, x: i32, y: i32, col: [u8; 3], a: f32) {
    if x < 0 || y < 0 || x >= w || y >= h {
        return;
    }
    let sa = a.clamp(0.0, 1.0);
    if sa <= 0.0 {
        return;
    }
    let idx = ((y * w + x) * 4) as usize;
    let da = buf[idx + 3] as f32 / 255.0;
    let out_a = sa + da * (1.0 - sa);
    if out_a <= 0.0 {
        return;
    }
    for c in 0..3 {
        let s = col[c] as f32 / 255.0;
        let d = buf[idx + c] as f32 / 255.0;
        let o = (s * sa + d * da * (1.0 - sa)) / out_a;
        buf[idx + c] = (o * 255.0).round().clamp(0.0, 255.0) as u8;
    }
    buf[idx + 3] = (out_a * 255.0).round().clamp(0.0, 255.0) as u8;
}
