//! Drawing the calendar with Direct2D and DirectWrite into an off-screen bitmap, whose
//! premultiplied pixels go to the layered window as they are ([`crate::layered`]): a translucent
//! panel over Windows' acrylic, like the audio flyout.

use anyhow::{Context, Result};
use windows::core::{w, PCWSTR};
use windows::Win32::Graphics::Direct2D::Common::{D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_RECT_F, D2D_SIZE_U};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Bitmap, ID2D1Factory, ID2D1RenderTarget, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, D2D1_BITMAP_PROPERTIES, ID2D1SolidColorBrush, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
    D2D1_DRAW_TEXT_OPTIONS_CLIP, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_ROUNDED_RECT, D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat, DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL,
    DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD,
    DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_PARAGRAPH_ALIGNMENT_NEAR, DWRITE_TEXT_ALIGNMENT, DWRITE_TEXT_ALIGNMENT_CENTER,
    DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_ALIGNMENT_TRAILING, DWRITE_TRIMMING, DWRITE_TRIMMING_GRANULARITY_CHARACTER,
    DWRITE_WORD_WRAPPING_NO_WRAP, DWRITE_WORD_WRAPPING_WRAP,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICBitmap, IWICImagingFactory, WICBitmapCacheOnLoad, WICBitmapLockRead,
    WICRect,
};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows_numerics::Vector2;

use super::scene::R;

/// An sRGB colour with straight alpha.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Color(pub f32, pub f32, pub f32, pub f32);

impl Color {
    pub const fn hex(rgb: u32, a: f32) -> Self {
        Color(((rgb >> 16) & 0xff) as f32 / 255.0, ((rgb >> 8) & 0xff) as f32 / 255.0, (rgb & 0xff) as f32 / 255.0, a)
    }

    pub fn rgb([r, g, b]: [u8; 3]) -> Self {
        Color(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0)
    }

    pub fn alpha(self, a: f32) -> Self {
        Color(self.0, self.1, self.2, self.3 * a)
    }
}

/// The flyout's colours in the system's light or dark theme.
pub struct Palette {
    /// The panel, translucent over the acrylic.
    pub bg: Color,
    /// A raised surface (a notification, the event's card).
    pub raised: Color,
    pub text: Color,
    pub text2: Color,
    pub text3: Color,
    pub hover: Color,
    pub accent: Color,
    pub on_accent: Color,
    /// A tooltip's ground.
    pub tip: Color,
}

impl Palette {
    pub fn current() -> Self {
        let light = crate::win::hkcu_dword(w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"), w!("SystemUsesLightTheme")) == Some(1);
        #[cfg(feature = "dev")]
        let light = std::env::var("AUDIO_TRAY_THEME").map_or(light, |t| t == "light");
        if light {
            Palette {
                bg: Color::hex(0xEEEEEE, 0.86),
                raised: Color::hex(0xFFFFFF, 0.7),
                text: Color::hex(0x000000, 0.896),
                text2: Color::hex(0x000000, 0.606),
                text3: Color::hex(0x000000, 0.361),
                hover: Color::hex(0x000000, 0.045),
                accent: Color::rgb(crate::flyout::theme::accent_shade(4).unwrap_or([0x00, 0x5F, 0xB8])),
                on_accent: Color::hex(0xFFFFFF, 1.0),
                tip: Color::hex(0xF9F9F9, 1.0),
            }
        } else {
            let [r, g, b] = crate::flyout::theme::TINT;
            Palette {
                bg: Color::rgb([r, g, b]).alpha(crate::flyout::theme::TINT_A),
                raised: Color::hex(0xFFFFFF, 0.05),
                text: Color::hex(0xFFFFFF, 1.0),
                text2: Color::hex(0xFFFFFF, 0.786),
                text3: Color::hex(0xFFFFFF, 0.362),
                hover: Color::hex(0xFFFFFF, 0.061),
                accent: Color::rgb(crate::flyout::theme::accent_rgb()),
                on_accent: Color::hex(0x000000, 1.0),
                tip: Color::hex(0x2C2C2C, 1.0),
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Font {
    Body,
    Strong,
    Caption,
    CaptionStrong,
    Icon,
    IconSmall,
    /// The day's date (22, semibold).
    Big,
    /// An event's title in its details (18, semibold).
    Title,
    /// Small labels (11), and their emphasis.
    Tiny,
    TinyStrong,
    /// Wrapping text from the top (a notification's body, an event's notes).
    Wrap,
    /// [`Font::Title`], wrapping (an event's title in its details).
    TitleWrap,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

pub struct Gfx {
    factory: ID2D1Factory,
    dwrite: IDWriteFactory,
    wic: IWICImagingFactory,
    formats: Vec<IDWriteTextFormat>,
    /// The bitmap drawn into, its size in pixels, and the target on it.
    surface: Option<((u32, u32), IWICBitmap, ID2D1RenderTarget)>,
    /// The last frame, premultiplied BGRA.
    pub pixels: Vec<u8>,
    /// Images drawn for this surface, by key and pixel size, oldest first.
    images: std::cell::RefCell<Vec<(String, u32, Option<ID2D1Bitmap>)>>,
    /// The size of the last frame (the surface can be taller).
    drawn: (u32, u32),
}

/// Images kept decoded at most (logos, photos and pictures of the notifications listed).
const IMAGES_MAX: usize = 64;

impl Gfx {
    pub fn new() -> Result<Self> {
        unsafe {
            let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).context("D2D1CreateFactory")?;
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).context("DWriteCreateFactory")?;
            let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).context("WIC")?;
            let mut gfx = Gfx { factory, dwrite, wic, formats: Vec::new(), surface: None, pixels: Vec::new(), images: Default::default(), drawn: (0, 0) };
            let text = w!("Segoe UI Variable Text");
            let icons = w!("Segoe Fluent Icons");
            gfx.formats = vec![
                gfx.format(text, DWRITE_FONT_WEIGHT_NORMAL, 14.0, false)?,
                gfx.format(text, DWRITE_FONT_WEIGHT_SEMI_BOLD, 14.0, false)?,
                gfx.format(text, DWRITE_FONT_WEIGHT_NORMAL, 12.0, false)?,
                gfx.format(text, DWRITE_FONT_WEIGHT_SEMI_BOLD, 12.0, false)?,
                gfx.format(icons, DWRITE_FONT_WEIGHT_NORMAL, 16.0, false)?,
                gfx.format(icons, DWRITE_FONT_WEIGHT_NORMAL, 10.0, false)?,
                gfx.format(text, DWRITE_FONT_WEIGHT_SEMI_BOLD, 22.0, false)?,
                gfx.format(text, DWRITE_FONT_WEIGHT_SEMI_BOLD, 18.0, false)?,
                gfx.format(text, DWRITE_FONT_WEIGHT_NORMAL, 11.0, false)?,
                gfx.format(text, DWRITE_FONT_WEIGHT_SEMI_BOLD, 11.0, false)?,
                gfx.format(text, DWRITE_FONT_WEIGHT_NORMAL, 13.0, true)?,
                gfx.format(text, DWRITE_FONT_WEIGHT_SEMI_BOLD, 18.0, true)?,
            ];
            Ok(gfx)
        }
    }

    fn format(&self, family: PCWSTR, weight: DWRITE_FONT_WEIGHT, size: f32, wrap: bool) -> Result<IDWriteTextFormat> {
        unsafe {
            let format = self.dwrite.CreateTextFormat(family, None, weight, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, size, w!(""))?;
            if wrap {
                format.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP)?;
                format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR)?;
            } else {
                format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
                format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            }
            let sign = self.dwrite.CreateEllipsisTrimmingSign(&format)?;
            let trimming = DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 };
            format.SetTrimming(&trimming, &sign)?;
            Ok(format)
        }
    }

    /// Draw a frame of `size` pixels at `scale` pixels per DIP into [`Gfx::pixels`].
    pub fn frame(&mut self, size: (u32, u32), scale: f32, draw: impl FnOnce(&Pen)) -> Result<()> {
        unsafe {
            // The surface only grows (same width): a panel growing or shrinking keeps its target,
            // and so its decoded pictures. Only the top `size.1` rows are read back.
            let fits = self.surface.as_ref().is_some_and(|((w, h), ..)| *w == size.0 && *h >= size.1);
            if !fits {
                let bitmap = self.wic.CreateBitmap(size.0.max(1), size.1.max(1), &GUID_WICPixelFormat32bppPBGRA, WICBitmapCacheOnLoad).context("CreateBitmap")?;
                let props = D2D1_RENDER_TARGET_PROPERTIES {
                    r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                    pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                    ..Default::default()
                };
                let target = self.factory.CreateWicBitmapRenderTarget(&bitmap, &props).context("CreateWicBitmapRenderTarget")?;
                target.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
                self.surface = Some((size, bitmap, target));
                self.images.borrow_mut().clear();
            }
            let (_, bitmap, target) = self.surface.as_ref().context("no surface")?;
            target.SetDpi(96.0 * scale, 96.0 * scale);
            target.BeginDraw();
            draw(&Pen { gfx: self, target: target.clone(), scale });
            target.EndDraw(None, None).context("EndDraw")?;
            self.drawn = size;

            let rect = WICRect { X: 0, Y: 0, Width: size.0 as i32, Height: size.1.max(1) as i32 };
            let lock = bitmap.Lock(&rect, WICBitmapLockRead.0 as u32).context("Lock")?;
            let stride = lock.GetStride()? as usize;
            let mut len = 0u32;
            let mut data = std::ptr::null_mut();
            lock.GetDataPointer(&mut len, &mut data)?;
            let src = std::slice::from_raw_parts(data, len as usize);
            let row = size.0 as usize * 4;
            self.pixels.resize(row * size.1 as usize, 0);
            for (y, out) in self.pixels.chunks_exact_mut(row).enumerate() {
                out.copy_from_slice(&src[y * stride..y * stride + row]);
            }
        }
        Ok(())
    }

    /// The last frame, as a PNG file.
    pub fn save_png(&self, path: &std::path::Path) -> Result<()> {
        use windows::Win32::Foundation::GENERIC_WRITE;
        use windows::Win32::Graphics::Imaging::{GUID_ContainerFormatPng, WICBitmapEncoderNoCache};
        let (_, bitmap, _) = self.surface.as_ref().context("nothing drawn")?;
        let wide = crate::win::wide(&path.to_string_lossy());
        unsafe {
            let stream = self.wic.CreateStream()?;
            stream.InitializeFromFilename(PCWSTR(wide.as_ptr()), GENERIC_WRITE.0)?;
            let encoder = self.wic.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null())?;
            encoder.Initialize(&stream, WICBitmapEncoderNoCache)?;
            let mut frame = None;
            encoder.CreateNewFrame(&mut frame, std::ptr::null_mut())?;
            let frame = frame.context("no frame")?;
            frame.Initialize(None)?;
            // The frame drawn, not the whole (taller) surface.
            let (w, h) = self.drawn;
            let source = self.wic.CreateBitmapFromSourceRect(bitmap, 0, 0, w.max(1), h.max(1))?;
            frame.WriteSource(&source, std::ptr::null())?;
            frame.Commit()?;
            encoder.Commit()?;
        }
        Ok(())
    }
}

pub struct Pen<'a> {
    gfx: &'a Gfx,
    target: ID2D1RenderTarget,
    /// Pixels per DIP.
    scale: f32,
}

fn rect(r: R) -> D2D_RECT_F {
    D2D_RECT_F { left: r.x, top: r.y, right: r.x + r.w, bottom: r.y + r.h }
}

impl Pen<'_> {
    fn brush(&self, c: Color) -> Option<ID2D1SolidColorBrush> {
        unsafe { self.target.CreateSolidColorBrush(&D2D1_COLOR_F { r: c.0, g: c.1, b: c.2, a: c.3 }, None) }.ok()
    }

    /// Fill everything with `c`, alpha included (the panel is translucent).
    pub fn clear(&self, c: Color) {
        unsafe { self.target.Clear(Some(&D2D1_COLOR_F { r: c.0 * c.3, g: c.1 * c.3, b: c.2 * c.3, a: c.3 })) };
    }

    pub fn fill(&self, r: R, radius: f32, c: Color) {
        if let Some(brush) = self.brush(c) {
            let rounded = D2D1_ROUNDED_RECT { rect: rect(r), radiusX: radius, radiusY: radius };
            unsafe { self.target.FillRoundedRectangle(&rounded, &brush) };
        }
    }

    pub fn stroke(&self, r: R, radius: f32, width: f32, c: Color) {
        if let Some(brush) = self.brush(c) {
            let half = width / 2.0;
            let inner = R::new(r.x + half, r.y + half, r.w - width, r.h - width);
            let rounded = D2D1_ROUNDED_RECT { rect: rect(inner), radiusX: radius, radiusY: radius };
            unsafe { self.target.DrawRoundedRectangle(&rounded, &brush, width, None) };
        }
    }

    pub fn text(&self, text: &str, font: Font, r: R, c: Color, align: Align) {
        if text.is_empty() || r.w <= 0.0 {
            return;
        }
        let wide: Vec<u16> = text.encode_utf16().collect();
        let Some(brush) = self.brush(c) else { return };
        unsafe {
            let Ok(layout) = self.gfx.dwrite.CreateTextLayout(&wide, &self.gfx.formats[font as usize], r.w, r.h) else {
                return;
            };
            let a: DWRITE_TEXT_ALIGNMENT = match align {
                Align::Left => DWRITE_TEXT_ALIGNMENT_LEADING,
                Align::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
                Align::Right => DWRITE_TEXT_ALIGNMENT_TRAILING,
            };
            let _ = layout.SetTextAlignment(a);
            self.target.DrawTextLayout(Vector2 { X: r.x, Y: r.y }, &layout, &brush, D2D1_DRAW_TEXT_OPTIONS_CLIP);
        }
    }

    pub fn glyph(&self, glyph: char, font: Font, r: R, c: Color) {
        self.text(&glyph.to_string(), font, r, c, Align::Center);
    }

    /// An image file (`bytes`, kept by `key`) drawn square into `r`.
    pub fn image(&self, key: &str, bytes: &[u8], r: R) {
        let px = (r.w * self.scale).round().max(1.0) as u32;
        let made = self.cached(key, px, || crate::image::sprite(bytes, px, 0.0).ok().map(|rgba| (rgba, px, px)));
        self.place(made, r);
    }

    /// A photo cut to a circle in `r` (square).
    pub fn avatar(&self, key: &str, bytes: &[u8], r: R) {
        let px = (r.w * self.scale).round().max(1.0) as u32;
        let made = self.cached(&format!("round:{key}"), px, || crate::image::sprite(bytes, px, px as f32 / 2.0).ok().map(|rgba| (rgba, px, px)));
        self.place(made, r);
    }

    /// A picture whole, fitted and centred in `r`, its corners slightly rounded.
    pub fn picture(&self, key: &str, bytes: &[u8], r: R) {
        let (max_w, max_h) = ((r.w * self.scale).round().max(1.0) as u32, (r.h * self.scale).round().max(1.0) as u32);
        let made = self.cached(&format!("whole:{key}"), max_w * 10_000 + max_h, || {
            let (mut rgba, w, h) = crate::image::fitted(bytes, max_w, max_h).ok()?;
            crate::image::round_corners(&mut rgba, w, h, 6.0 * self.scale);
            Some((rgba, w, h))
        });
        if let Some(bitmap) = &made {
            let size = unsafe { bitmap.GetSize() };
            let at = R::new(r.x + (r.w - size.width) / 2.0, r.y + (r.h - size.height) / 2.0, size.width, size.height);
            self.place(made, at);
        }
    }

    /// The bitmap for `key` at `px`, made once.
    fn cached(&self, key: &str, px: u32, make: impl FnOnce() -> Option<(Vec<u8>, u32, u32)>) -> Option<ID2D1Bitmap> {
        if let Some(b) = self.gfx.images.borrow().iter().find(|(k, p, _)| k == key && *p == px).map(|(_, _, b)| b.clone()) {
            return b;
        }
        let made = make().and_then(|(rgba, w, h)| self.bitmap(&rgba, w, h));
        let mut images = self.gfx.images.borrow_mut();
        if images.len() >= IMAGES_MAX {
            images.remove(0);
        }
        images.push((key.to_string(), px, made.clone()));
        made
    }

    fn place(&self, bitmap: Option<ID2D1Bitmap>, r: R) {
        if let Some(bitmap) = bitmap {
            unsafe { self.target.DrawBitmap(&bitmap, Some(&rect(r)), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None) };
        }
    }

    fn bitmap(&self, rgba: &[u8], w: u32, h: u32) -> Option<ID2D1Bitmap> {
        let bgra: Vec<u8> = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| {
                let a = p[3] as u32;
                let m = |c: u8| ((c as u32 * a + 127) / 255) as u8;
                [m(p[2]), m(p[1]), m(p[0]), p[3]]
            })
            .collect();
        let props = D2D1_BITMAP_PROPERTIES {
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: 96.0 * self.scale,
            dpiY: 96.0 * self.scale,
        };
        unsafe { self.target.CreateBitmap(D2D_SIZE_U { width: w, height: h }, Some(bgra.as_ptr() as *const _), w * 4, &props) }.ok()
    }

    pub fn clip(&self, r: R) {
        unsafe { self.target.PushAxisAlignedClip(&rect(r), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE) };
    }

    pub fn unclip(&self) {
        unsafe { self.target.PopAxisAlignedClip() };
    }
}
