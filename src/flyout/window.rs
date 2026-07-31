//! The [`Surface`]: the flyout's on-screen presence and pixel buffers, plus the Win32
//! plumbing that puts them on screen.
//!
//! It owns the layered `HWND`, the panel geometry (size, position, work-area, anchor), the
//! laid-out elements to draw, and the two RGBA buffers — `base` (the static layer) and `buf`
//! (base + dynamic overlays, presented each frame). It knows how to create the window,
//! reposition itself, and play the open animation — but nothing about the audio model, the
//! drawing itself (the controller fills the buffers via [`super::render`] and hands them
//! here to present), or the layered blend, which is [`crate::layered`]'s.

use std::sync::OnceLock;

use windows::core::w;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, LoadCursorW, PostMessageW, RegisterClassW, IDC_ARROW,
    WM_CAPTURECHANGED, WNDCLASSW, WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
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
        unsafe { crate::layered::enable_acrylic(hwnd) };
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

    /// Push a rendered ARGB buffer (`w`×`h`) to the layered window, scaled by a global
    /// `alpha` (for fade animations) — which also moves and resizes the window to
    /// `(x, y)`/`(w, h)`. See [`crate::layered::present`].
    pub(super) fn present_buf(&self, src_buf: &[u8], w: i32, h: i32, x: i32, y: i32, alpha: u8) {
        crate::layered::present(self.hwnd, src_buf, w, h, x, y, alpha);
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
