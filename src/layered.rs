//! Putting a hand-painted RGBA buffer on the screen, as a per-pixel-alpha *layered*
//! window.
//!
//! Two surfaces are drawn this way — the control flyout ([`crate::flyout`]) and the
//! scroll readout ([`crate::osd`]) — and both need the same two things from Win32: an
//! `UpdateLayeredWindow` blend of a straight-alpha buffer, and the compositor's acrylic
//! blur behind it. Neither is obvious enough to keep two copies of, so both live here and
//! the two surfaces bring their own geometry and pixels.

use windows::core::{s, w};
use windows::Win32::Foundation::{COLORREF, HWND, POINT, SIZE};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject,
    AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS,
    HGDIOBJ,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::UI::WindowsAndMessaging::{UpdateLayeredWindow, ULW_ALPHA};

/// Push a rendered straight-alpha RGBA buffer (`w`×`h`) to a layered window as
/// premultiplied BGRA, scaled by a global `alpha` (for fade animations).
/// `UpdateLayeredWindow` also moves + resizes the window to `(x, y)` and `(w, h)`, so this
/// is how a layered surface is positioned as well as painted.
pub(crate) fn present(hwnd: HWND, src_buf: &[u8], w: i32, h: i32, x: i32, y: i32, alpha: u8) {
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

/// Enable the acrylic blur-behind via the undocumented (but ubiquitous)
/// `SetWindowCompositionAttribute`. Best-effort — if it no-ops, the surface is still a
/// legible semi-transparent dark panel.
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
