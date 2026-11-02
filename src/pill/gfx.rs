//! The pill's graphics: one D3D11 device feeding Direct2D, a DirectComposition desktop device
//! bound to the pill's window, DirectWrite text and WIC image decoding. Everything is drawn in
//! DIPs into composition surfaces; DWM composes and animates them, so a still pill costs nothing.

use anyhow::{Context, Result};
use windows::core::{w, Interface, PCWSTR};
use windows::Win32::Foundation::{HMODULE, HWND, POINT};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_RECT_F, D2D_SIZE_U,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Bitmap1, ID2D1Device, ID2D1DeviceContext, ID2D1Factory1, ID2D1RenderTarget,
    D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, D2D1_BITMAP_PROPERTIES,
    ID2D1SolidColorBrush, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_BITMAP_BRUSH_PROPERTIES1,
    D2D1_DRAW_TEXT_OPTIONS_CLIP, D2D1_ELLIPSE, D2D1_EXTEND_MODE_CLAMP, D2D1_FACTORY_OPTIONS,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC, D2D1_ROUNDED_RECT,
};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION,
};
use windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice2, IDCompositionDesktopDevice, IDCompositionSurface, IDCompositionTarget,
    IDCompositionVisual2,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat, DWRITE_FACTORY_TYPE_SHARED,
    DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT, DWRITE_FONT_WEIGHT_BOLD, DWRITE_FONT_WEIGHT_NORMAL,
    DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_MEASURING_MODE_NATURAL, DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
    DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_METRICS, DWRITE_TRIMMING, DWRITE_TRIMMING_GRANULARITY_CHARACTER,
    DWRITE_WORD_WRAPPING_NO_WRAP,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICBitmap, IWICImagingFactory, WICBitmapCacheOnLoad,
    WICBitmapDitherTypeNone, WICBitmapInterpolationModeHighQualityCubic, WICBitmapPaletteTypeMedianCut,
    WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows_numerics::{Matrix3x2, Vector2};

/// An sRGB colour with straight alpha.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Rgba(pub f32, pub f32, pub f32, pub f32);

impl Rgba {
    pub const fn hex(rgb: u32, a: f32) -> Self {
        Rgba(((rgb >> 16) & 0xff) as f32 / 255.0, ((rgb >> 8) & 0xff) as f32 / 255.0, (rgb & 0xff) as f32 / 255.0, a)
    }

    fn d2d(self) -> D2D1_COLOR_F {
        D2D1_COLOR_F { r: self.0, g: self.1, b: self.2, a: self.3 }
    }
}

/// Which text style.
#[derive(Clone, Copy)]
pub enum Font {
    Title,
    Body,
    Small,
    /// Segoe Fluent Icons, for glyphs.
    Icons,
    IconsSmall,
    /// One centred letter in a face without a picture.
    Initial,
    /// The LIVE badge: tiny, bold, centred.
    Badge,
}

pub struct Gfx {
    // Held for the device's lifetime; DirectComposition draws through them.
    _d2d_factory: ID2D1Factory1,
    _d2d_device: ID2D1Device,
    pub dcomp: IDCompositionDesktopDevice,
    _target: IDCompositionTarget,
    pub root: IDCompositionVisual2,
    dwrite: IDWriteFactory,
    wic: IWICImagingFactory,
    formats: Vec<IDWriteTextFormat>,
    /// Pixels per DIP.
    pub scale: f32,
    /// Images decoded and scaled down once, by a hash of their bytes: a repaint (a speaker
    /// change, a tab update) uploads these instead of decoding every full-size cover again.
    decoded: std::cell::RefCell<Vec<(u64, IWICBitmap)>>,
}

/// How many decoded images are kept: the covers and faces on screen, with room to spare.
const DECODED_KEPT: usize = 24;
/// The largest an image is ever drawn, in DIPs (the hover card's cover): decoded no bigger.
const LARGEST_IMAGE: f32 = 64.0;

impl Gfx {
    pub fn new(hwnd: HWND, scale: f32) -> Result<Self> {
        unsafe {
            let mut d3d: Option<ID3D11Device> = None;
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut d3d),
                None,
                None,
            )
            .context("D3D11CreateDevice")?;
            let dxgi: IDXGIDevice = d3d.context("no D3D11 device")?.cast()?;
            let d2d_factory: ID2D1Factory1 =
                D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, Some(&D2D1_FACTORY_OPTIONS::default()))
                    .context("D2D1CreateFactory")?;
            let d2d_device = d2d_factory.CreateDevice(&dxgi).context("ID2D1Factory1::CreateDevice")?;
            let dcomp: IDCompositionDesktopDevice =
                DCompositionCreateDevice2(&d2d_device).context("DCompositionCreateDevice2")?;
            let target = dcomp.CreateTargetForHwnd(hwnd, true).context("CreateTargetForHwnd")?;
            let root = dcomp.CreateVisual().context("CreateVisual")?;
            target.SetRoot(&root)?;
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).context("DWriteCreateFactory")?;
            let wic: IWICImagingFactory =
                CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).context("WIC factory")?;
            let mut gfx = Self { _d2d_factory: d2d_factory, _d2d_device: d2d_device, dcomp, _target: target, root, dwrite, wic, formats: Vec::new(), scale, decoded: Default::default() };
            gfx.formats = vec![
                gfx.format(w!("Segoe UI Variable Text"), DWRITE_FONT_WEIGHT_SEMI_BOLD, 13.0, false)?,
                gfx.format(w!("Segoe UI Variable Text"), DWRITE_FONT_WEIGHT_NORMAL, 12.0, false)?,
                gfx.format(w!("Segoe UI Variable Text"), DWRITE_FONT_WEIGHT_SEMI_BOLD, 11.0, false)?,
                gfx.format(w!("Segoe Fluent Icons"), DWRITE_FONT_WEIGHT_NORMAL, 16.0, true)?,
                gfx.format(w!("Segoe Fluent Icons"), DWRITE_FONT_WEIGHT_NORMAL, 12.0, true)?,
                gfx.format(w!("Segoe UI Variable Text"), DWRITE_FONT_WEIGHT_SEMI_BOLD, 15.0, true)?,
                gfx.format(w!("Segoe UI Variable Text"), DWRITE_FONT_WEIGHT_BOLD, 8.0, true)?,
            ];
            Ok(gfx)
        }
    }

    fn format(&self, family: PCWSTR, weight: DWRITE_FONT_WEIGHT, size: f32, centred: bool) -> Result<IDWriteTextFormat> {
        unsafe {
            let format = self.dwrite.CreateTextFormat(
                family,
                None,
                weight,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size,
                w!("en-us"),
            )?;
            format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            if centred {
                format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
            } else {
                let sign = self.dwrite.CreateEllipsisTrimmingSign(&format)?;
                let trimming = DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 };
                format.SetTrimming(&trimming, &sign)?;
            }
            Ok(format)
        }
    }

    fn text_format(&self, font: Font) -> &IDWriteTextFormat {
        &self.formats[font as usize]
    }

    /// The width of `text` in `font`, in DIPs.
    pub fn measure(&self, font: Font, text: &str) -> f32 {
        let wide: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            let Ok(layout) = self.dwrite.CreateTextLayout(&wide, self.text_format(font), 4096.0, 100.0) else {
                return 0.0;
            };
            let mut metrics = DWRITE_TEXT_METRICS::default();
            if layout.GetMetrics(&mut metrics).is_err() {
                return 0.0;
            }
            metrics.widthIncludingTrailingWhitespace
        }
    }

    /// A visual showing a fresh surface of `w` × `h` DIPs drawn by `draw`, which works in DIPs.
    pub fn painted(&self, w: f32, h: f32, draw: impl FnOnce(&mut Painter) -> Result<()>) -> Result<(IDCompositionVisual2, IDCompositionSurface)> {
        unsafe {
            let px = |v: f32| ((v * self.scale).ceil() as u32).max(1);
            let surface = self
                .dcomp
                .CreateSurface(px(w), px(h), DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_ALPHA_MODE_PREMULTIPLIED)
                .context("CreateSurface")?;
            let mut offset = POINT::default();
            let dc: ID2D1DeviceContext = surface.BeginDraw(None, &mut offset).context("BeginDraw")?;
            let dpi = 96.0 * self.scale;
            dc.SetDpi(dpi, dpi);
            dc.SetTransform(&Matrix3x2::translation(offset.x as f32 / self.scale, offset.y as f32 / self.scale));
            dc.Clear(Some(&Rgba(0.0, 0.0, 0.0, 0.0).d2d()));
            let mut painter = Painter { gfx: self, dc: dc.clone() };
            let drawn = draw(&mut painter);
            surface.EndDraw().context("EndDraw")?;
            drawn?;
            let visual = self.dcomp.CreateVisual()?;
            visual.SetContent(&surface)?;
            Ok((visual, surface))
        }
    }

    /// PNG or JPEG bytes as a bitmap for `dc`, decoded once (see [`Gfx::decoded`]).
    fn bitmap(&self, dc: &ID2D1DeviceContext, bytes: &[u8]) -> Result<ID2D1Bitmap1> {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        bytes.hash(&mut hasher);
        let key = hasher.finish();
        let cached = self.decoded.borrow().iter().find(|(k, _)| *k == key).map(|(_, b)| b.clone());
        let decoded = match cached {
            Some(decoded) => decoded,
            None => {
                let decoded = self.decode(bytes)?;
                let mut kept = self.decoded.borrow_mut();
                if kept.len() >= DECODED_KEPT {
                    kept.remove(0);
                }
                kept.push((key, decoded.clone()));
                decoded
            }
        };
        unsafe { Ok(dc.CreateBitmapFromWicBitmap(&decoded, None)?) }
    }

    /// Decode, scaled down to the largest size the pill draws, in premultiplied BGRA.
    fn decode(&self, bytes: &[u8]) -> Result<IWICBitmap> {
        unsafe {
            let stream = self.wic.CreateStream()?;
            stream.InitializeFromMemory(bytes)?;
            let decoder = self.wic.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand)?;
            let frame = decoder.GetFrame(0)?;
            let (mut w, mut h) = (0u32, 0u32);
            frame.GetSize(&mut w, &mut h)?;
            let most = (LARGEST_IMAGE * self.scale).ceil() as u32;
            let shrink = (most as f32 / w.min(h).max(1) as f32).min(1.0);
            let converter = self.wic.CreateFormatConverter()?;
            if shrink < 1.0 {
                let scaler = self.wic.CreateBitmapScaler()?;
                let (sw, sh) = (((w as f32 * shrink).round() as u32).max(1), ((h as f32 * shrink).round() as u32).max(1));
                scaler.Initialize(&frame, sw, sh, WICBitmapInterpolationModeHighQualityCubic)?;
                converter.Initialize(&scaler, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeMedianCut)?;
            } else {
                converter.Initialize(&frame, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeMedianCut)?;
            }
            Ok(self.wic.CreateBitmapFromSource(&converter, WICBitmapCacheOnLoad)?)
        }
    }

    pub fn commit(&self) -> Result<()> {
        unsafe { self.dcomp.Commit() }.context("Commit")
    }
}

/// Draws into one surface, in DIPs.
pub struct Painter<'a> {
    gfx: &'a Gfx,
    dc: ID2D1DeviceContext,
}

impl Painter<'_> {
    fn brush(&self, color: Rgba) -> Result<ID2D1SolidColorBrush> {
        Ok(unsafe { self.dc.CreateSolidColorBrush(&color.d2d(), None) }?)
    }

    pub fn round_rect(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32, color: Rgba) -> Result<()> {
        let rect = D2D1_ROUNDED_RECT { rect: rect(x, y, w, h), radiusX: radius, radiusY: radius };
        unsafe { self.dc.FillRoundedRectangle(&rect, &self.brush(color)?) };
        Ok(())
    }

    /// The outline of a rounded rectangle, `stroke` wide, inside the box.
    pub fn round_rect_outline(&mut self, (x, y, w, h): (f32, f32, f32, f32), radius: f32, stroke: f32, color: Rgba) -> Result<()> {
        let half = stroke / 2.0;
        let rect = D2D1_ROUNDED_RECT { rect: rect(x + half, y + half, w - stroke, h - stroke), radiusX: radius, radiusY: radius };
        unsafe { self.dc.DrawRoundedRectangle(&rect, &self.brush(color)?, stroke, None) };
        Ok(())
    }

    pub fn circle(&mut self, cx: f32, cy: f32, r: f32, color: Rgba) -> Result<()> {
        unsafe { self.dc.FillEllipse(&ellipse(cx, cy, r), &self.brush(color)?) };
        Ok(())
    }

    pub fn ring(&mut self, cx: f32, cy: f32, r: f32, stroke: f32, color: Rgba) -> Result<()> {
        unsafe { self.dc.DrawEllipse(&ellipse(cx, cy, r), &self.brush(color)?, stroke, None) };
        Ok(())
    }

    /// Only the half of a ring left (`left`) or right of its centre.
    ///
    /// The ends are cut a little short of the centre, so their anti-aliased edges never spill into
    /// the other half's side, where they showed as ticks at the top and bottom. The left half's top
    /// end is the exception and runs a little past instead: turned half a circle it meets the other
    /// half at the bottom, where a short cut showed as a seam. The visual's own clip makes the exact
    /// cut at the centre in both cases.
    pub fn half_ring(&mut self, cx: f32, cy: f32, r: f32, stroke: f32, color: Rgba, left: bool) -> Result<()> {
        const PAST: f32 = 1.5;
        const SHORT: f32 = 0.75;
        let reach = r + stroke;
        let clip = if left { rect(cx - reach, cy - reach, reach - SHORT, 2.0 * reach) } else { rect(cx + SHORT, cy - reach, reach - SHORT, 2.0 * reach) };
        unsafe {
            let brush = self.brush(color)?;
            self.dc.PushAxisAlignedClip(&clip, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            self.dc.DrawEllipse(&ellipse(cx, cy, r), &brush, stroke, None);
            self.dc.PopAxisAlignedClip();
            if left {
                self.dc.PushAxisAlignedClip(&rect(cx - SHORT - 0.5, cy - reach, SHORT + PAST + 0.5, reach), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
                self.dc.DrawEllipse(&ellipse(cx, cy, r), &brush, stroke, None);
                self.dc.PopAxisAlignedClip();
            }
        }
        Ok(())
    }

    /// `bytes` decoded and fitted (cover, centred) into a circle, or into a rounded square when
    /// `radius` is given. `false` when the image would not decode.
    pub fn image(&mut self, bytes: &[u8], x: f32, y: f32, size: f32, radius: Option<f32>) -> Result<bool> {
        let Ok(bitmap) = self.gfx.bitmap(&self.dc, bytes) else {
            return Ok(false);
        };
        unsafe {
            // In DIPs at the bitmap's own DPI, which is the image file's (an icon rendered to PNG
            // is not at the screen's): what the brush below is laid out in.
            let dips = bitmap.GetSize();
            let (bw, bh) = (dips.width.max(0.01), dips.height.max(0.01));
            let fit = size / bw.min(bh);
            let props = D2D1_BITMAP_BRUSH_PROPERTIES1 {
                extendModeX: D2D1_EXTEND_MODE_CLAMP,
                extendModeY: D2D1_EXTEND_MODE_CLAMP,
                interpolationMode: D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC,
            };
            let brush = self.dc.CreateBitmapBrush(&bitmap, Some(&props), None)?;
            let (dx, dy) = (x + (size - bw * fit) / 2.0, y + (size - bh * fit) / 2.0);
            brush.SetTransform(&(Matrix3x2 { M11: fit, M12: 0.0, M21: 0.0, M22: fit, M31: dx, M32: dy }));
            match radius {
                Some(r) => self.dc.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: rect(x, y, size, size), radiusX: r, radiusY: r }, &brush),
                None => self.dc.FillEllipse(&ellipse(x + size / 2.0, y + size / 2.0, size / 2.0), &brush),
            }
        }
        Ok(true)
    }

    /// A device icon in white, `size` DIPs square, centred on `(cx, cy)`: rendered as the tray
    /// renders it, for the icons no font has.
    pub fn icon(&mut self, icon: crate::icons::IconId, cx: f32, cy: f32, size: f32) -> Result<()> {
        let px = (size * self.gfx.scale).round() as u32;
        let (rgba, w, h) = icon.render(px, [0xFF, 0xFF, 0xFF])?;
        // Straight RGBA to premultiplied BGRA.
        let bgra: Vec<u8> = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| {
                let m = |c: u8| ((c as u32 * p[3] as u32 + 127) / 255) as u8;
                [m(p[2]), m(p[1]), m(p[0]), p[3]]
            })
            .collect();
        let dpi = 96.0 * self.gfx.scale;
        let props = D2D1_BITMAP_PROPERTIES {
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: dpi,
            dpiY: dpi,
        };
        unsafe {
            let target: ID2D1RenderTarget = self.dc.cast()?;
            let bitmap = target.CreateBitmap(D2D_SIZE_U { width: w, height: h }, Some(bgra.as_ptr() as *const _), w * 4, &props)?;
            let (bw, bh) = (w as f32 / self.gfx.scale, h as f32 / self.gfx.scale);
            let at = rect(cx - bw / 2.0, cy - bh / 2.0, bw, bh);
            target.DrawBitmap(&bitmap, Some(&at), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
        }
        Ok(())
    }

    /// `text` in `font`, left-aligned in the box `(x, y, w, h)` (or centred, for the centred
    /// fonts), vertically centred, cut with an ellipsis.
    pub fn text(&mut self, text: &str, font: Font, (x, y, w, h): (f32, f32, f32, f32), color: Rgba) -> Result<()> {
        let wide: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            self.dc.DrawText(
                &wide,
                self.gfx.text_format(font),
                &rect(x, y, w, h),
                &self.brush(color)?,
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
        Ok(())
    }
}

fn rect(x: f32, y: f32, w: f32, h: f32) -> D2D_RECT_F {
    D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h }
}

fn ellipse(cx: f32, cy: f32, r: f32) -> D2D1_ELLIPSE {
    D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: r, radiusY: r }
}
