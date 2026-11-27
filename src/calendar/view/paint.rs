//! Drawing the calendar with Direct2D and DirectWrite into an off-screen bitmap, whose
//! premultiplied pixels go to the layered window as they are ([`crate::layered`]): a translucent
//! panel over Windows' acrylic, like the audio flyout.

use anyhow::{Context, Result};
use windows::core::{w, PCWSTR};
use windows::Win32::Graphics::Direct2D::Common::{D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_RECT_F, D2D_SIZE_U};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Bitmap, ID2D1Factory, ID2D1RenderTarget, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, D2D1_BITMAP_PROPERTIES, ID2D1SolidColorBrush, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_ANTIALIAS_MODE_ALIASED,
    D2D1_DRAW_TEXT_OPTIONS_CLIP, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_ROUNDED_RECT, D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat, IDWriteTextLayout, DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_WEIGHT_NORMAL,
    DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_TEXT_ALIGNMENT, DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING,
    DWRITE_TEXT_ALIGNMENT_TRAILING,
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
    /// The footer's strip and its hairline: the audio panel's, so every panel ends alike.
    pub shade: Color,
    pub divider: Color,
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
                accent: Color::rgb(crate::theme::accent_shade(4).unwrap_or([0x00, 0x5F, 0xB8])),
                on_accent: Color::hex(0xFFFFFF, 1.0),
                tip: Color::hex(0xF9F9F9, 1.0),
                // A shade darker on the light ground too, the hairline as faint as on the dark.
                shade: Color::hex(0x000000, 0.04),
                divider: Color::hex(0x000000, 0.10),
            }
        } else {
            let [r, g, b] = crate::theme::DARK_TINT;
            Palette {
                bg: Color::rgb([r, g, b]).alpha(crate::theme::DARK_TINT_A),
                raised: Color::hex(0xFFFFFF, 0.05),
                text: Color::hex(0xFFFFFF, 1.0),
                text2: Color::hex(0xFFFFFF, 0.786),
                text3: Color::hex(0xFFFFFF, 0.362),
                hover: Color::hex(0xFFFFFF, 0.061),
                accent: Color::rgb(crate::theme::accent_rgb()),
                on_accent: Color::hex(0x000000, 1.0),
                tip: Color::hex(0x2C2C2C, 1.0),
                shade: Color::hex(0x000000, crate::theme::FOOTER_SHADE_A),
                divider: Color::hex(0xFFFFFF, crate::theme::DIVIDER_A),
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

impl Font {
    /// Every font, in its own order: [`Gfx`] makes their formats in this order and finds each by
    /// its value.
    const ALL: [Font; 12] = [
        Font::Body,
        Font::Strong,
        Font::Caption,
        Font::CaptionStrong,
        Font::Icon,
        Font::IconSmall,
        Font::Big,
        Font::Title,
        Font::Tiny,
        Font::TinyStrong,
        Font::Wrap,
        Font::TitleWrap,
    ];

    /// Its family, weight and size, and whether it wraps (from the top) or keeps to one line
    /// (centred).
    fn spec(self) -> crate::text::Spec {
        use crate::text::Flow::{Line, Wrap};
        let text = w!("Segoe UI Variable Text");
        let icons = w!("Segoe Fluent Icons");
        let (family, weight, size, flow) = match self {
            Font::Body => (text, DWRITE_FONT_WEIGHT_NORMAL, 14.0, Line),
            Font::Strong => (text, DWRITE_FONT_WEIGHT_SEMI_BOLD, 14.0, Line),
            Font::Caption => (text, DWRITE_FONT_WEIGHT_NORMAL, 12.0, Line),
            Font::CaptionStrong => (text, DWRITE_FONT_WEIGHT_SEMI_BOLD, 12.0, Line),
            Font::Icon => (icons, DWRITE_FONT_WEIGHT_NORMAL, 16.0, Line),
            Font::IconSmall => (icons, DWRITE_FONT_WEIGHT_NORMAL, 10.0, Line),
            Font::Big => (text, DWRITE_FONT_WEIGHT_SEMI_BOLD, 22.0, Line),
            Font::Title => (text, DWRITE_FONT_WEIGHT_SEMI_BOLD, 18.0, Line),
            Font::Tiny => (text, DWRITE_FONT_WEIGHT_NORMAL, 11.0, Line),
            Font::TinyStrong => (text, DWRITE_FONT_WEIGHT_SEMI_BOLD, 11.0, Line),
            Font::Wrap => (text, DWRITE_FONT_WEIGHT_NORMAL, 13.0, Wrap),
            Font::TitleWrap => (text, DWRITE_FONT_WEIGHT_SEMI_BOLD, 18.0, Wrap),
        };
        crate::text::Spec { family, weight, size, flow }
    }
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
    /// Images drawn for this surface, by key and pixel size, least recently drawn first.
    images: std::cell::RefCell<Vec<(String, u32, Option<ID2D1Bitmap>)>>,
    /// The size of the last frame (the surface can be taller).
    drawn: (u32, u32),
    /// Text laid out, kept while drawn ([`Layouts`]).
    layouts: std::cell::RefCell<Layouts>,
}

/// A text's layout by what makes it: the text, its font, its box's size (bits) and alignment.
type LayoutKey = (String, u8, u32, u32, u8);

/// Layouts made or used since the last full frame, and those of the frame before: one not used
/// for a whole frame goes. A scroll or a hover lays out nothing anew.
#[derive(Default)]
struct Layouts {
    now: std::collections::HashMap<LayoutKey, IDWriteTextLayout>,
    before: std::collections::HashMap<LayoutKey, IDWriteTextLayout>,
}

/// Layouts kept at most: past it, a full frame forgets what it did not draw.
const LAYOUTS_MAX: usize = 4000;

/// Images kept decoded at most (logos, photos and pictures of the notifications listed).
const IMAGES_MAX: usize = 64;

impl Gfx {
    /// Kept for later: the last frame's pixels freed, so the next frame is drawn whole.
    pub fn forget_frame(&mut self) {
        self.pixels = Vec::new();
        self.drawn = (0, 0);
    }

    pub fn new() -> Result<Self> {
        unsafe {
            let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).context("D2D1CreateFactory")?;
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).context("DWriteCreateFactory")?;
            let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).context("WIC")?;
            let formats = Font::ALL.iter().map(|font| crate::text::format(&dwrite, font.spec(), w!(""))).collect::<Result<_>>()?;
            Ok(Gfx { factory, dwrite, wic, formats, surface: None, pixels: Vec::new(), images: Default::default(), drawn: (0, 0), layouts: Default::default() })
        }
    }

    /// Draw a frame of `size` pixels at `scale` pixels per DIP into [`Gfx::pixels`].
    pub fn frame(&mut self, size: (u32, u32), scale: f32, draw: impl FnOnce(&Pen)) -> Result<()> {
        let mut draw = Some(draw);
        self.frame_in(size, scale, None, |pen, _| {
            if let Some(draw) = draw.take() {
                draw(pen);
            }
        })
        .map(|_| ())
    }

    /// Draw a frame of `size` pixels, only in `areas` (DIPs) where given and the last frame was the
    /// same size: `draw` is called for each area (`None`: the whole frame), clipped to it, and
    /// should skip what lies outside it. Returns whether the whole frame was drawn.
    pub fn frame_in(&mut self, size: (u32, u32), scale: f32, areas: Option<&[R]>, draw: impl FnMut(&Pen, Option<R>)) -> Result<bool> {
        self.frame_zoomed(size, scale, 1.0, areas, draw)
    }

    /// [`Gfx::frame_in`], everything drawn `zoom` times its size about the frame's middle (a banner
    /// shrinking into the bell): the frame keeps its size, so its surface and its decoded pictures
    /// stay, and so does the window's bitmap.
    pub fn frame_zoomed(&mut self, size: (u32, u32), scale: f32, zoom: f32, areas: Option<&[R]>, mut draw: impl FnMut(&Pen, Option<R>)) -> Result<bool> {
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
            // In parts only over a frame of the same size, whose other pixels stand (none after
            // [`Gfx::forget_frame`]).
            let areas = areas.filter(|_| fits && self.drawn == size);
            let full = areas.is_none();
            {
                let mut layouts = self.layouts.borrow_mut();
                if full {
                    // What the last full frame did not draw goes.
                    let now = std::mem::take(&mut layouts.now);
                    layouts.before = if now.len() > LAYOUTS_MAX { Default::default() } else { now };
                } else if layouts.now.len() > LAYOUTS_MAX {
                    // Parts only add (a long reply typed in): bounded all the same.
                    layouts.now.clear();
                    layouts.before.clear();
                }
            }
            let (_, bitmap, target) = self.surface.as_ref().context("no surface")?;
            target.SetDpi(96.0 * scale, 96.0 * scale);
            // Zoomed about the middle (in DIPs), or not at all: set each frame, never left over.
            let (cx, cy) = (size.0 as f32 / scale / 2.0, size.1 as f32 / scale / 2.0);
            let zoom = zoom.clamp(0.01, 1.0);
            target.SetTransform(&windows_numerics::Matrix3x2 { M11: zoom, M12: 0.0, M21: 0.0, M22: zoom, M31: cx * (1.0 - zoom), M32: cy * (1.0 - zoom) });
            let brush = target.CreateSolidColorBrush(&D2D1_COLOR_F::default(), None).context("CreateSolidColorBrush")?;
            target.BeginDraw();
            let pen = Pen { gfx: self, target: target.clone(), scale, brush };
            // The rows drawn, to read back (pixels).
            let mut rows = (0u32, size.1);
            match areas {
                None => draw(&pen, None),
                Some(areas) => {
                    rows = (size.1, 0);
                    for &area in areas {
                        target.PushAxisAlignedClip(&rect(area), D2D1_ANTIALIAS_MODE_ALIASED);
                        draw(&pen, Some(area));
                        target.PopAxisAlignedClip();
                        let top = ((area.y * scale).floor().max(0.0) as u32).min(size.1);
                        let bottom = ((area.bottom() * scale).ceil().max(0.0) as u32 + 1).min(size.1);
                        rows = (rows.0.min(top), rows.1.max(bottom));
                    }
                }
            }
            drop(pen);
            target.EndDraw(None, None).context("EndDraw")?;
            self.drawn = size;

            let row = size.0 as usize * 4;
            self.pixels.resize(row * size.1 as usize, 0);
            if rows.0 < rows.1 {
                let rect = WICRect { X: 0, Y: rows.0 as i32, Width: size.0 as i32, Height: (rows.1 - rows.0) as i32 };
                let lock = bitmap.Lock(&rect, WICBitmapLockRead.0 as u32).context("Lock")?;
                let stride = lock.GetStride()? as usize;
                let mut len = 0u32;
                let mut data = std::ptr::null_mut();
                lock.GetDataPointer(&mut len, &mut data)?;
                let src = std::slice::from_raw_parts(data, len as usize);
                let out = &mut self.pixels[rows.0 as usize * row..rows.1 as usize * row];
                for (y, out) in out.chunks_exact_mut(row).enumerate() {
                    out.copy_from_slice(&src[y * stride..y * stride + row]);
                }
            }
            Ok(full)
        }
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
    /// One brush for the frame, coloured for each use.
    brush: ID2D1SolidColorBrush,
}

fn rect(r: R) -> D2D_RECT_F {
    D2D_RECT_F { left: r.x, top: r.y, right: r.x + r.w, bottom: r.y + r.h }
}

impl Pen<'_> {
    /// The frame's brush, in colour c.
    fn brush(&self, c: Color) -> &ID2D1SolidColorBrush {
        unsafe { self.brush.SetColor(&D2D1_COLOR_F { r: c.0, g: c.1, b: c.2, a: c.3 }) };
        &self.brush
    }

    /// Fill everything with `c`, alpha included (the panel is translucent).
    pub fn clear(&self, c: Color) {
        unsafe { self.target.Clear(Some(&D2D1_COLOR_F { r: c.0 * c.3, g: c.1 * c.3, b: c.2 * c.3, a: c.3 })) };
    }

    pub fn fill(&self, r: R, radius: f32, c: Color) {
        let rounded = D2D1_ROUNDED_RECT { rect: rect(r), radiusX: radius, radiusY: radius };
        unsafe { self.target.FillRoundedRectangle(&rounded, self.brush(c)) };
    }

    /// A pie in circle `r`, `fraction` of it filled clockwise from the top (the focus timer's).
    pub fn pie(&self, r: R, fraction: f32, c: Color) {
        use windows::Win32::Graphics::Direct2D::Common::{D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_END_CLOSED, D2D_SIZE_F};
        use windows::Win32::Graphics::Direct2D::{D2D1_ARC_SEGMENT, D2D1_ARC_SIZE_LARGE, D2D1_ARC_SIZE_SMALL, D2D1_SWEEP_DIRECTION_CLOCKWISE};
        let fraction = fraction.clamp(0.0, 1.0);
        if fraction >= 0.999 {
            self.fill(r, r.w / 2.0, c);
            return;
        }
        if fraction <= 0.001 {
            return;
        }
        let brush = self.brush(c);
        let (cx, cy, radius) = (r.x + r.w / 2.0, r.y + r.h / 2.0, r.w / 2.0);
        let angle = fraction * std::f32::consts::TAU;
        let end = Vector2 { X: cx + radius * angle.sin(), Y: cy - radius * angle.cos() };
        let drawn = (|| -> windows::core::Result<()> {
            let path = unsafe { self.gfx.factory.CreatePathGeometry() }?;
            let sink = unsafe { path.Open() }?;
            unsafe {
                sink.BeginFigure(Vector2 { X: cx, Y: cy }, D2D1_FIGURE_BEGIN_FILLED);
                sink.AddLine(Vector2 { X: cx, Y: cy - radius });
                sink.AddArc(&D2D1_ARC_SEGMENT {
                    point: end,
                    size: D2D_SIZE_F { width: radius, height: radius },
                    rotationAngle: 0.0,
                    sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
                    arcSize: if fraction > 0.5 { D2D1_ARC_SIZE_LARGE } else { D2D1_ARC_SIZE_SMALL },
                });
                sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                sink.Close()?;
                self.target.FillGeometry(&path, brush, None);
            }
            Ok(())
        })();
        if let Err(e) = drawn {
            eprintln!("calendar: could not draw a pie ({e})");
        }
    }

    pub fn stroke(&self, r: R, radius: f32, width: f32, c: Color) {
        let half = width / 2.0;
        let inner = R::new(r.x + half, r.y + half, r.w - width, r.h - width);
        let rounded = D2D1_ROUNDED_RECT { rect: rect(inner), radiusX: radius, radiusY: radius };
        unsafe { self.target.DrawRoundedRectangle(&rounded, self.brush(c), width, None) };
    }

    pub fn text(&self, text: &str, font: Font, r: R, c: Color, align: Align) {
        if text.is_empty() || r.w <= 0.0 {
            return;
        }
        let Some(layout) = self.layout(text, font, r, align) else { return };
        // Colour fonts in colour: an emoji (a quick reaction) as the app shows it.
        let options = D2D1_DRAW_TEXT_OPTIONS_CLIP | windows::Win32::Graphics::Direct2D::D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT;
        unsafe { self.target.DrawTextLayout(Vector2 { X: r.x, Y: r.y }, &layout, self.brush(c), options) };
    }

    /// `c` washed in from the left edge of `r`, gone by `reach` (0..1) of its width: an app's
    /// colour on its banner.
    pub fn wash(&self, r: R, radius: f32, c: Color, reach: f32) {
        use windows::Win32::Graphics::Direct2D::Common::D2D1_GRADIENT_STOP;
        use windows::Win32::Graphics::Direct2D::{D2D1_EXTEND_MODE_CLAMP, D2D1_GAMMA_2_2, D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES};
        let stop = |position: f32, a: f32| D2D1_GRADIENT_STOP { position, color: D2D1_COLOR_F { r: c.0, g: c.1, b: c.2, a } };
        let stops = [stop(0.0, c.3), stop(1.0, 0.0)];
        let Ok(collection) = (unsafe { self.target.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP) }) else { return };
        let line = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES { startPoint: Vector2 { X: r.x, Y: r.y }, endPoint: Vector2 { X: r.x + r.w * reach.max(0.05), Y: r.y + r.h * 0.35 } };
        let Ok(brush) = (unsafe { self.target.CreateLinearGradientBrush(&line, None, &collection) }) else { return };
        let rounded = D2D1_ROUNDED_RECT { rect: rect(r), radiusX: radius, radiusY: radius };
        unsafe { self.target.FillRoundedRectangle(&rounded, &brush) };
    }

    /// A circle's outline inside `r`, `width` thick.
    pub fn ring(&self, r: R, width: f32, c: Color) {
        use windows::Win32::Graphics::Direct2D::D2D1_ELLIPSE;
        let ellipse = D2D1_ELLIPSE { point: Vector2 { X: r.x + r.w / 2.0, Y: r.y + r.h / 2.0 }, radiusX: (r.w - width) / 2.0, radiusY: (r.h - width) / 2.0 };
        unsafe { self.target.DrawEllipse(&ellipse, self.brush(c), width, None) };
    }

    /// `text` laid out in a box of `r`'s size: made once, then kept while it is drawn.
    fn layout(&self, text: &str, font: Font, r: R, align: Align) -> Option<IDWriteTextLayout> {
        let key: LayoutKey = (text.to_string(), font as u8, r.w.to_bits(), r.h.to_bits(), align as u8);
        let mut layouts = self.gfx.layouts.borrow_mut();
        if let Some(layout) = layouts.now.get(&key) {
            return Some(layout.clone());
        }
        let layout = match layouts.before.remove(&key) {
            Some(layout) => layout,
            None => unsafe {
                let wide: Vec<u16> = text.encode_utf16().collect();
                let layout = self.gfx.dwrite.CreateTextLayout(&wide, &self.gfx.formats[font as usize], r.w, r.h).ok()?;
                let a: DWRITE_TEXT_ALIGNMENT = match align {
                    Align::Left => DWRITE_TEXT_ALIGNMENT_LEADING,
                    Align::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
                    Align::Right => DWRITE_TEXT_ALIGNMENT_TRAILING,
                };
                let _ = layout.SetTextAlignment(a);
                layout
            },
        };
        layouts.now.insert(key, layout.clone());
        Some(layout)
    }

    /// A one-line text box's insides in `r`: its text on one line, what is selected on `selection`
    /// (in `selected` ink), and the caret (in `caret`, when drawn), placed where DirectWrite puts
    /// them. Scrolled so the caret shows: a long line slides left as it is typed.
    pub fn field(&self, editor: &super::editor::Editor, font: Font, r: R, [text, selection, selected]: [Color; 3], caret: Option<Color>) {
        use windows::Win32::Graphics::DirectWrite::{DWRITE_HIT_TEST_METRICS, DWRITE_WORD_WRAPPING_NO_WRAP};
        // A width no other text has: the layout is this box's alone (its wrapping is changed).
        const WIDE: f32 = 100_000.0;
        let Some(layout) = self.layout(editor.text(), font, R::new(0.0, 0.0, WIDE, r.h), Align::Left) else { return };
        let at = |position: usize| unsafe {
            let (mut x, mut y) = (0.0f32, 0.0f32);
            let mut m = DWRITE_HIT_TEST_METRICS::default();
            let _ = layout.HitTestTextPosition(position as u32, false, &mut x, &mut y, &mut m);
            (x, m.top, m.height)
        };
        unsafe {
            let _ = layout.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
        }
        let (cx, top, height) = at(editor.caret16());
        // Scrolled only as far as the caret needs, a pixel of room after it.
        let offset = (cx - (r.w - 2.0)).max(0.0);
        let x0 = r.x - offset;
        // Whole pixels: a 1-pixel caret stays sharp at any scale.
        let snap = |v: f32| (v * self.scale).round() / self.scale;
        self.clip(R::new(r.x - 1.0, r.y, r.w + 2.0, r.h));
        let band = editor.selection16().map(|(a, b)| {
            let (ax, bx) = (at(a).0, at(b).0);
            R::new(snap(x0 + ax), r.y + top, snap(bx - ax), height)
        });
        if let Some(band) = band {
            self.fill(band, 2.0, selection);
        }
        let options = D2D1_DRAW_TEXT_OPTIONS_CLIP | windows::Win32::Graphics::Direct2D::D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT;
        unsafe { self.target.DrawTextLayout(Vector2 { X: x0, Y: r.y }, &layout, self.brush(text), options) };
        // The selected part again, on its band, in the band's own ink.
        if let Some(band) = band {
            self.clip(band);
            unsafe { self.target.DrawTextLayout(Vector2 { X: x0, Y: r.y }, &layout, self.brush(selected), options) };
            self.unclip();
        }
        if let Some(c) = caret.filter(|_| band.is_none()) {
            let w = 1.0 / self.scale * self.scale.round().max(1.0);
            self.fill(R::new(snap(x0 + cx), r.y + top, w, height), 0.0, c);
        }
        self.unclip();
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
        {
            // Least recently drawn goes first: one used now moves to the back, so a frame never
            // evicts what it is drawing.
            let mut images = self.gfx.images.borrow_mut();
            if let Some(at) = images.iter().position(|(k, p, _)| k == key && *p == px) {
                let entry = images.remove(at);
                let bitmap = entry.2.clone();
                images.push(entry);
                return bitmap;
            }
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

#[cfg(test)]
mod tests {
    use super::Font;

    /// A font's format is found by its value: [`Font::ALL`] lists every font, each at its value.
    #[test]
    fn every_font_is_listed_at_its_own_place() {
        for (i, font) in Font::ALL.iter().enumerate() {
            assert_eq!(*font as usize, i, "{font:?}");
        }
        assert_eq!(Font::ALL.len(), Font::TitleWrap as usize + 1);
    }
}
