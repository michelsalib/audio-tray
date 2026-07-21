//! The [`Surface`]: the flyout's on-screen presence and pixel buffers, plus the Win32
//! plumbing that puts them on screen.
//!
//! It owns the layered `HWND`, the panel geometry (size, position, work-area, anchor), the
//! laid-out elements to draw, and the two RGBA buffers — `base` (the static layer) and `buf`
//! (base + dynamic overlays, presented each frame). It knows how to create the window, blend
//! a buffer onto it via `UpdateLayeredWindow`, reposition itself, and play the open
//! animation — but nothing about the audio model or the drawing itself (the controller fills
//! the buffers via [`super::render`] and hands them here to present).

use std::sync::OnceLock;

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
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, LoadCursorW, PostMessageW, RegisterClassW, UpdateLayeredWindow,
    IDC_ARROW, ULW_ALPHA, WM_CAPTURECHANGED, WNDCLASSW, WS_EX_LAYERED, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP,
};

use super::layout::LaidElem;

/// The flyout's window + geometry + pixel buffers.
pub(super) struct Surface {
    pub hwnd: HWND,
    pub width: i32,
    pub height: i32,
    pub elems: Vec<LaidElem>,
    pub base: Vec<u8>, // static content, re-rendered on model changes
    pub buf: Vec<u8>,  // base + dynamic overlays (sliders, hover), presented each frame
    pub x: i32,
    pub y: i32,
    pub base_cx: i32,     // horizontal anchor (icon centre / cursor)
    pub base_bottom: i32, // bottom edge to sit above
    pub wa: RECT,         // work area
    pub margin: i32,
}

impl Surface {
    pub(super) fn new(margin: i32) -> Self {
        Surface {
            hwnd: HWND(std::ptr::null_mut()),
            width: 0,
            height: 0,
            elems: Vec::new(),
            base: Vec::new(),
            buf: Vec::new(),
            x: 0,
            y: 0,
            base_cx: 0,
            base_bottom: 0,
            wa: RECT::default(),
            margin,
        }
    }

    /// Position the panel: centred on the anchor, sitting above it, clamped to the work
    /// area. Recomputed whenever the size changes so it keeps its bottom edge.
    pub(super) fn reposition(&mut self) {
        self.x = (self.base_cx - self.width / 2)
            .min(self.wa.right - self.margin - self.width)
            .max(self.wa.left + self.margin);
        self.y = (self.base_bottom - self.height).max(self.wa.top + self.margin);
    }

    pub(super) fn create_window(&mut self) -> windows::core::Result<()> {
        static REGISTERED: OnceLock<()> = OnceLock::new();
        let hinstance = HINSTANCE(unsafe { GetModuleHandleW(None) }?.0);
        REGISTERED.get_or_init(|| {
            let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default();
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: hinstance,
                hCursor: cursor,
                lpszClassName: w!("AudioTrayFlyout"),
                ..Default::default()
            };
            unsafe { RegisterClassW(&wc) };
        });

        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                w!("AudioTrayFlyout"),
                w!("Audio"),
                WS_POPUP,
                self.x,
                self.y,
                self.width,
                self.height,
                None,
                None,
                Some(hinstance),
                None,
            )
        }?;
        self.hwnd = hwnd;

        let dark: i32 = 1;
        let _ = unsafe {
            DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const _ as *const std::ffi::c_void, 4)
        };
        let round: i32 = 2; // DWMWCP_ROUND
        let _ = unsafe {
            DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &round as *const _ as *const std::ffi::c_void, 4)
        };
        unsafe { enable_acrylic(hwnd) };
        Ok(())
    }

    /// Slide up + fade in, like the native tray flyouts. Runs before the modal loop.
    pub(super) fn animate_in(&self, scale: f32) {
        let slide = (14.0 * scale) as i32;
        let frames = 9;
        for i in 1..=frames {
            let t = i as f32 / frames as f32;
            let ease = 1.0 - (1.0 - t) * (1.0 - t); // ease-out quad
            let yy = self.y + (slide as f32 * (1.0 - ease)) as i32;
            self.present(self.x, yy, (255.0 * ease) as u8);
            std::thread::sleep(std::time::Duration::from_millis(9));
        }
        self.present(self.x, self.y, 255);
    }

    /// Present the current `buf` at the resting position, fully opaque.
    pub(super) fn flush(&self) {
        self.present(self.x, self.y, 255);
    }

    /// Push `self.buf` (the current screen) to the layered window.
    pub(super) fn present(&self, x: i32, y: i32, alpha: u8) {
        self.present_buf(&self.buf, self.width, self.height, x, y, alpha);
    }

    /// Push a rendered ARGB buffer (`w`×`h`) to the layered window (premultiplied BGRA),
    /// scaled by a global `alpha` (for fade animations). `UpdateLayeredWindow` also moves +
    /// resizes the window to `(x, y)` and `(w, h)`.
    pub(super) fn present_buf(&self, src_buf: &[u8], w: i32, h: i32, x: i32, y: i32, alpha: u8) {
        unsafe {
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
                let r = src_buf[i * 4] as u32;
                let g = src_buf[i * 4 + 1] as u32;
                let b = src_buf[i * 4 + 2] as u32;
                let a = src_buf[i * 4 + 3] as u32;
                dst[i * 4] = ((b * a) / 255) as u8;
                dst[i * 4 + 1] = ((g * a) / 255) as u8;
                dst[i * 4 + 2] = ((r * a) / 255) as u8;
                dst[i * 4 + 3] = a as u8;
            }

            let old = SelectObject(mem, HGDIOBJ(hbm.0));
            let src = POINT { x: 0, y: 0 };
            let dpos = POINT { x, y };
            let size = SIZE { cx: w, cy: h };
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: alpha,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let _ = UpdateLayeredWindow(
                self.hwnd,
                Some(screen),
                Some(&dpos),
                Some(&size),
                Some(mem),
                Some(&src),
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
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // WM_CAPTURECHANGED is *sent* straight to the proc (it never reaches the modal loop's
    // GetMessage), so losing capture — Start menu, Alt-Tab, another app grabbing focus —
    // would otherwise orphan the flyout. Re-post it as a queued message the loop dismisses
    // on. (Our own ReleaseCapture at teardown also lands here, harmlessly.)
    if msg == WM_CAPTURECHANGED {
        let _ = PostMessageW(Some(hwnd), super::WM_FLYOUT_CLOSE, WPARAM(0), LPARAM(0));
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

/// Enable the acrylic blur-behind via the undocumented (but ubiquitous)
/// `SetWindowCompositionAttribute`. Best-effort — if it no-ops, the panel is still a legible
/// semi-transparent dark surface.
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
