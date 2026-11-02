//! Decode PNG or JPEG bytes (cover art, avatars) into a square straight-alpha RGBA sprite for
//! the software canvas: centre-cropped, scaled by WIC, its corners masked. And the reverse: an
//! icon or a file's thumbnail as PNG, for the surfaces that take covers as bytes.

use anyhow::{Context, Result};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppRGBA, IWICImagingFactory, WICBitmapDitherTypeNone,
    WICBitmapInterpolationModeHighQualityCubic, WICBitmapPaletteTypeMedianCut, WICDecodeMetadataCacheOnDemand,
    WICRect,
};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};

/// `bytes` as a `size`×`size` sprite whose corners are rounded to `radius` px (`size / 2` for a
/// circle).
pub fn sprite(bytes: &[u8], size: u32, radius: f32) -> Result<Vec<u8>> {
    unsafe {
        let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).context("WIC factory")?;
        let stream = wic.CreateStream()?;
        stream.InitializeFromMemory(bytes)?;
        let decoder = wic.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand).context("not an image")?;
        let frame = decoder.GetFrame(0)?;
        let (mut w, mut h) = (0u32, 0u32);
        frame.GetSize(&mut w, &mut h)?;
        let side = w.min(h).max(1);
        let clipper = wic.CreateBitmapClipper()?;
        let crop = WICRect { X: ((w - side) / 2) as i32, Y: ((h - side) / 2) as i32, Width: side as i32, Height: side as i32 };
        clipper.Initialize(&frame, &crop)?;
        let scaler = wic.CreateBitmapScaler()?;
        scaler.Initialize(&clipper, size, size, WICBitmapInterpolationModeHighQualityCubic)?;
        let converter = wic.CreateFormatConverter()?;
        converter.Initialize(&scaler, &GUID_WICPixelFormat32bppRGBA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeMedianCut)?;
        let mut rgba = vec![0u8; (size * size * 4) as usize];
        converter.CopyPixels(std::ptr::null(), size * 4, &mut rgba)?;
        mask_corners(&mut rgba, size, radius);
        Ok(rgba)
    }
}

/// The icon of the program at `exe`, `px` pixels square, as PNG (what the pill decodes).
pub fn exe_icon_png(exe: &str, px: u32) -> Result<Vec<u8>> {
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::SHDefExtractIconW;
    use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, HICON};

    let path = crate::win::wide(exe);
    let mut icon = HICON::default();
    unsafe { SHDefExtractIconW(PCWSTR(path.as_ptr()), 0, 0, Some(&mut icon), None, px) }.ok().context("no icon")?;
    let png = (|| unsafe {
        let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).context("WIC factory")?;
        let bitmap = wic.CreateBitmapFromHICON(icon)?;
        encode_png(&wic, &bitmap)
    })();
    unsafe {
        let _ = DestroyIcon(icon);
    }
    png
}

/// The shell's thumbnail of the file at `path` (a video's frame, as Explorer shows it), at most
/// `px` pixels on a side, as PNG. Never an icon: `Err` when there is no thumbnail.
pub fn file_thumbnail_png(path: &std::path::Path, px: u32) -> Result<Vec<u8>> {
    use windows::Win32::Foundation::SIZE;
    use windows::Win32::Graphics::Gdi::DeleteObject;
    use windows::Win32::Graphics::Imaging::WICBitmapIgnoreAlpha;
    use windows::Win32::UI::Shell::{IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_BIGGERSIZEOK, SIIGBF_THUMBNAILONLY};

    let wide = windows_core::HSTRING::from(path.as_os_str());
    unsafe {
        let item: IShellItemImageFactory = SHCreateItemFromParsingName(&wide, None).context("no such file")?;
        let size = SIZE { cx: px as i32, cy: px as i32 };
        let hbitmap = item.GetImage(size, SIIGBF_THUMBNAILONLY | SIIGBF_BIGGERSIZEOK).context("no thumbnail")?;
        let png = (|| {
            let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).context("WIC factory")?;
            // A video frame is opaque; the shell often leaves its alpha at 0.
            let bitmap = wic.CreateBitmapFromHBITMAP(hbitmap, windows::Win32::Graphics::Gdi::HPALETTE::default(), WICBitmapIgnoreAlpha)?;
            encode_png(&wic, &bitmap)
        })();
        let _ = DeleteObject(hbitmap.into());
        png
    }
}

/// `bitmap` as PNG bytes.
fn encode_png(wic: &IWICImagingFactory, bitmap: &windows::Win32::Graphics::Imaging::IWICBitmap) -> Result<Vec<u8>> {
    use windows::Win32::Graphics::Imaging::{GUID_ContainerFormatPng, WICBitmapEncoderNoCache};
    use windows::Win32::System::Com::StructuredStorage::{CreateStreamOnHGlobal, GetHGlobalFromStream};
    use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
    unsafe {
        let stream = CreateStreamOnHGlobal(Default::default(), true)?;
        let encoder = wic.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null())?;
        encoder.Initialize(&stream, WICBitmapEncoderNoCache)?;
        let mut frame = None;
        encoder.CreateNewFrame(&mut frame, std::ptr::null_mut())?;
        let frame = frame.context("no PNG frame")?;
        frame.Initialize(None)?;
        frame.WriteSource(bitmap, std::ptr::null())?;
        frame.Commit()?;
        encoder.Commit()?;
        let global = GetHGlobalFromStream(&stream)?;
        let len = GlobalSize(global);
        let ptr = GlobalLock(global) as *const u8;
        anyhow::ensure!(!ptr.is_null(), "GlobalLock");
        let bytes = std::slice::from_raw_parts(ptr, len).to_vec();
        let _ = GlobalUnlock(global);
        Ok(bytes)
    }
}

/// Fade the alpha outside a rounded square, anti-aliased over one pixel.
fn mask_corners(rgba: &mut [u8], size: u32, radius: f32) {
    let half = size as f32 / 2.0;
    let r = radius.min(half);
    for y in 0..size {
        for x in 0..size {
            let qx = ((x as f32 + 0.5) - half).abs() - (half - r);
            let qy = ((y as f32 + 0.5) - half).abs() - (half - r);
            let sd = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r;
            let coverage = (0.5 - sd).clamp(0.0, 1.0);
            let a = &mut rgba[((y * size + x) * 4 + 3) as usize];
            *a = (*a as f32 * coverage).round() as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_circle_mask_clears_the_corners_and_keeps_the_middle() {
        let size = 16;
        let mut rgba = vec![255u8; (size * size * 4) as usize];
        mask_corners(&mut rgba, size, size as f32 / 2.0);
        assert_eq!(rgba[3], 0, "the corner is outside the circle");
        let centre = ((8 * size + 8) * 4 + 3) as usize;
        assert_eq!(rgba[centre], 255);
    }
}
