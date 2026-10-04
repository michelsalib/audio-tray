//! The [`Surface`]: the flyout's layered `HWND`, geometry, laid-out elements and two RGBA
//! buffers (`base`, the static layer; `buf`, base plus overlays, presented each frame).
//! Knows nothing of the audio model or of drawing ([`super::render`] fills the buffers).

use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, PostMessageW, WM_CAPTURECHANGED, WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
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

    /// The panel's size, which every render pass needs and nothing changes while it is open.
    pub(super) fn size(&self) -> (i32, i32) {
        (self.width, self.height)
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
        let hwnd = crate::win::create_popup(
            w!("AudioTrayFlyout"),
            w!("Audio"),
            Some(wndproc),
            WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            (self.x, self.y, self.width, self.height),
        )?;
        self.hwnd = hwnd;
        unsafe { crate::layered::style_panel(hwnd, crate::layered::CORNER_ROUND) };
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
            self.present(&self.buf, self.x, yy, (255.0 * ease) as u8);
            std::thread::sleep(std::time::Duration::from_millis(9));
        }
        self.flush();
    }

    /// Present the current `buf` at the resting position, fully opaque.
    pub(super) fn flush(&self) {
        self.present(&self.buf, self.x, self.y, 255);
    }

    /// Present a panel-sized buffer at `(x, y)` with a global `alpha` (this also moves the window).
    pub(super) fn present(&self, buf: &[u8], x: i32, y: i32, alpha: u8) {
        crate::layered::present(self.hwnd, buf, self.width, self.height, x, y, alpha);
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // WM_CAPTURECHANGED is sent, never reaching the modal loop; re-post it so the loop
    // dismisses on capture loss (our own ReleaseCapture lands here harmlessly).
    if msg == WM_CAPTURECHANGED {
        let _ = PostMessageW(Some(hwnd), super::WM_FLYOUT_CLOSE, WPARAM(0), LPARAM(0));
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}
