//! Putting a hand-painted RGBA buffer on screen as a per-pixel-alpha layered window, in a
//! dark, rounded, acrylic frame. Shared by the flyout and the OSD.

use windows::core::{s, w};
use windows::Win32::Foundation::{COLORREF, HWND, POINT, SIZE};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject,
    AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS,
    HGDIOBJ,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::UI::WindowsAndMessaging::{UpdateLayeredWindow, ULW_ALPHA};

/// Push a straight-alpha RGBA buffer (`w`×`h`) to a layered window at `(x, y)`, scaled by a
/// global `alpha`. This also moves and resizes the window.
pub(crate) fn present(hwnd: HWND, src_buf: &[u8], w: i32, h: i32, x: i32, y: i32, alpha: u8) {
    present_with(hwnd, w, h, x, y, alpha, |dst| {
        // straight-alpha RGBA -> premultiplied BGRA
        for (d, s) in dst.as_chunks_mut::<4>().0.iter_mut().zip(src_buf.as_chunks::<4>().0) {
            let a = s[3] as u32;
            d[0] = ((s[2] as u32 * a) / 255) as u8;
            d[1] = ((s[1] as u32 * a) / 255) as u8;
            d[2] = ((s[0] as u32 * a) / 255) as u8;
            d[3] = a as u8;
        }
    });
}

/// Put `w`×`h` pixels on the layered window at `(x, y)`: `fill` writes them, premultiplied BGRA
/// (Direct2D's), into the bitmap.
fn present_with(hwnd: HWND, w: i32, h: i32, x: i32, y: i32, alpha: u8, fill: impl FnOnce(&mut [u8])) {
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

        let px = (w * h) as usize;
        fill(std::slice::from_raw_parts_mut(bits as *mut u8, px * 4));

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
            hwnd,
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

/// A layered window's pixels, kept between frames: the bitmap is made once per size, a frame is
/// written into it, and a window only moved or faded is not sent its pixels again.
pub(crate) struct Canvas {
    dc: windows::Win32::Graphics::Gdi::HDC,
    bitmap: HGDIOBJ,
    old: HGDIOBJ,
    bits: *mut u8,
    size: (i32, i32),
}

impl Canvas {
    pub(crate) const fn new() -> Self {
        Canvas { dc: windows::Win32::Graphics::Gdi::HDC(std::ptr::null_mut()), bitmap: HGDIOBJ(std::ptr::null_mut()), old: HGDIOBJ(std::ptr::null_mut()), bits: std::ptr::null_mut(), size: (0, 0) }
    }

    /// The pixels last written (premultiplied BGRA, `size` wide and high, row after row).
    pub(crate) fn pixels(&self) -> &[u8] {
        if self.bits.is_null() {
            return &[];
        }
        unsafe { std::slice::from_raw_parts(self.bits, (self.size.0 * self.size.1) as usize * 4) }
    }

    pub(crate) fn size(&self) -> (i32, i32) {
        self.size
    }

    fn free(&mut self) {
        unsafe {
            if !self.dc.is_invalid() {
                SelectObject(self.dc, self.old);
                let _ = DeleteObject(self.bitmap);
                let _ = DeleteDC(self.dc);
            }
        }
        // Field by field: assigning a whole Canvas would drop this one, and free it again.
        self.dc = windows::Win32::Graphics::Gdi::HDC(std::ptr::null_mut());
        self.bitmap = HGDIOBJ(std::ptr::null_mut());
        self.old = HGDIOBJ(std::ptr::null_mut());
        self.bits = std::ptr::null_mut();
        self.size = (0, 0);
    }

    /// `w`×`h` pixels, written by `fill`, on `hwnd` at `(x, y)` faded to `alpha`.
    pub(crate) fn present(&mut self, hwnd: HWND, (w, h): (i32, i32), (x, y): (i32, i32), alpha: u8, fill: impl FnOnce(&mut [u8])) {
        if w <= 0 || h <= 0 {
            return;
        }
        unsafe {
            if self.size != (w, h) || self.dc.is_invalid() {
                self.free();
                let dc = CreateCompatibleDC(None);
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
                let Ok(hbm) = CreateDIBSection(Some(dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) else {
                    let _ = DeleteDC(dc);
                    return;
                };
                let old = SelectObject(dc, HGDIOBJ(hbm.0));
                (self.dc, self.bitmap, self.old, self.bits, self.size) = (dc, HGDIOBJ(hbm.0), old, bits.cast(), (w, h));
            }
            fill(std::slice::from_raw_parts_mut(self.bits, (w * h) as usize * 4));
            let src = POINT { x: 0, y: 0 };
            let size = SIZE { cx: w, cy: h };
            let _ = UpdateLayeredWindow(hwnd, None, Some(&POINT { x, y }), Some(&size), Some(self.dc), Some(&src), COLORREF(0), Some(&blend(alpha)), ULW_ALPHA);
        }
    }

    /// Move `hwnd` to `(x, y)` and fade it to `alpha`, its pixels as they are.
    pub(crate) fn shift(hwnd: HWND, x: i32, y: i32, alpha: u8) {
        let _ = unsafe { UpdateLayeredWindow(hwnd, None, Some(&POINT { x, y }), None, None, None, COLORREF(0), Some(&blend(alpha)), ULW_ALPHA) };
    }
}

impl Drop for Canvas {
    fn drop(&mut self) {
        self.free();
    }
}

fn blend(alpha: u8) -> BLENDFUNCTION {
    BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: alpha, AlphaFormat: AC_SRC_ALPHA as u8 }
}

/// Corner radii DWM offers for a panel: `ROUND` for the flyout, `ROUNDSMALL` for the
/// 32-DIP readout, which the large radius would over-curve.
pub(crate) const CORNER_ROUND: i32 = 2;
pub(crate) const CORNER_ROUND_SMALL: i32 = 3;

/// Dress a layered window as one of our panels: dark frame, rounded corners (so the blur
/// follows what we paint) and acrylic.
///
/// # Safety
/// `hwnd` must be a live window this process owns.
pub(crate) unsafe fn style_panel(hwnd: HWND, corner: i32) {
    use windows::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE,
    };

    let attribute = |which, value: &i32| {
        let _ = DwmSetWindowAttribute(
            hwnd,
            which,
            value as *const i32 as *const std::ffi::c_void,
            std::mem::size_of::<i32>() as u32,
        );
    };
    attribute(DWMWA_USE_IMMERSIVE_DARK_MODE, &1);
    attribute(DWMWA_WINDOW_CORNER_PREFERENCE, &corner);
    enable_acrylic(hwnd);
}

/// Enable acrylic blur-behind via the undocumented `SetWindowCompositionAttribute`; best-effort.
///
/// # Safety
/// `hwnd` must be a live window this process owns.
pub(crate) unsafe fn enable_acrylic(hwnd: HWND) {
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
