//! Painting the settings window with Direct2D and DirectWrite, in DIPs: Windows 11's settings look
//! (cards on a plain background, a navigation pane with an accent marker), in the system's light
//! or dark theme. One render target for the window, rebuilt when the device is lost.

use anyhow::{Context, Result};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_IGNORE, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_BEGIN_HOLLOW,
    D2D1_FIGURE_END_CLOSED, D2D1_FIGURE_END_OPEN, D2D1_PIXEL_FORMAT, D2D_RECT_F, D2D_SIZE_U,
};
use windows::Win32::Graphics::Direct2D::{
    ID2D1PathGeometry, D2D1_CAP_STYLE_FLAT, D2D1_CAP_STYLE_ROUND, D2D1_DASH_STYLE_SOLID, D2D1_LINE_JOIN_ROUND,
    D2D1_STROKE_STYLE_PROPERTIES,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Bitmap, ID2D1Factory, ID2D1HwndRenderTarget, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, D2D1_BITMAP_PROPERTIES, ID2D1SolidColorBrush, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
    D2D1_DRAW_TEXT_OPTIONS_CLIP, D2D1_ELLIPSE, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_HWND_RENDER_TARGET_PROPERTIES,
    D2D1_PRESENT_OPTIONS_NONE, D2D1_RENDER_TARGET_PROPERTIES, D2D1_ROUNDED_RECT,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat, DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL,
    DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD,
    DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT, DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING,
    DWRITE_TEXT_ALIGNMENT_TRAILING, DWRITE_TEXT_METRICS, DWRITE_TRIMMING, DWRITE_TRIMMING_GRANULARITY_CHARACTER,
    DWRITE_WORD_WRAPPING_NO_WRAP,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows_numerics::{Matrix3x2, Vector2};

use super::layout::{self, Item, Kind, Laid, Target, R};
use super::model::{Model, Tone};
use std::cell::RefCell;

use crate::config::PillAlign;
use crate::icons::IconId;

/// An sRGB colour with straight alpha.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) struct Color(f32, f32, f32, f32);

impl Color {
    const fn hex(rgb: u32, a: f32) -> Self {
        Color(((rgb >> 16) & 0xff) as f32 / 255.0, ((rgb >> 8) & 0xff) as f32 / 255.0, (rgb & 0xff) as f32 / 255.0, a)
    }

    fn rgb([r, g, b]: [u8; 3]) -> Self {
        Color(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0)
    }

    fn alpha(self, a: f32) -> Self {
        Color(self.0, self.1, self.2, self.3 * a)
    }
}

/// Windows 11's theme colours, as its own settings use them.
pub(super) struct Palette {
    bg: Color,
    card: Color,
    card_hover: Color,
    card_stroke: Color,
    text: Color,
    text2: Color,
    text3: Color,
    subtle_hover: Color,
    subtle_pressed: Color,
    control: Color,
    control_hover: Color,
    control_stroke: Color,
    /// The selected tab of a segmented choice, raised from its well.
    raised: Color,
    accent: Color,
    on_accent: Color,
    good: Color,
    warn: Color,
}

impl Palette {
    pub(super) fn current() -> Self {
        let light = crate::win::hkcu_dword(w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"), w!("AppsUseLightTheme")) == Some(1);
        // Dev: screenshots in either theme without switching Windows' (`AUDIO_TRAY_THEME=light|dark`).
        #[cfg(feature = "dev")]
        let light = std::env::var("AUDIO_TRAY_THEME").map_or(light, |t| t == "light");
        if light {
            Palette {
                bg: Color::hex(0xF3F3F3, 1.0),
                card: Color::hex(0xFBFBFB, 1.0),
                card_hover: Color::hex(0xF6F6F6, 1.0),
                card_stroke: Color::hex(0xE5E5E5, 1.0),
                text: Color::hex(0x000000, 0.896),
                text2: Color::hex(0x000000, 0.606),
                text3: Color::hex(0x000000, 0.361),
                subtle_hover: Color::hex(0x000000, 0.037),
                subtle_pressed: Color::hex(0x000000, 0.024),
                control: Color::hex(0xFFFFFF, 0.7),
                control_hover: Color::hex(0xF9F9F9, 0.5),
                control_stroke: Color::hex(0x000000, 0.0578),
                raised: Color::hex(0xFFFFFF, 1.0),
                accent: Color::rgb(crate::flyout::theme::accent_shade(4).unwrap_or([0x00, 0x5F, 0xB8])),
                on_accent: Color::hex(0xFFFFFF, 1.0),
                good: Color::hex(0x0F7B0F, 1.0),
                warn: Color::hex(0x9D5D00, 1.0),
            }
        } else {
            Palette {
                bg: Color::hex(0x202020, 1.0),
                card: Color::hex(0x2B2B2B, 1.0),
                card_hover: Color::hex(0x323232, 1.0),
                card_stroke: Color::hex(0x1D1D1D, 1.0),
                text: Color::hex(0xFFFFFF, 1.0),
                text2: Color::hex(0xFFFFFF, 0.786),
                text3: Color::hex(0xFFFFFF, 0.362),
                subtle_hover: Color::hex(0xFFFFFF, 0.061),
                subtle_pressed: Color::hex(0xFFFFFF, 0.042),
                control: Color::hex(0xFFFFFF, 0.061),
                control_hover: Color::hex(0xFFFFFF, 0.084),
                control_stroke: Color::hex(0xFFFFFF, 0.093),
                raised: Color::hex(0xFFFFFF, 0.12),
                accent: Color::rgb(crate::flyout::theme::accent_rgb()),
                on_accent: Color::hex(0x000000, 1.0),
                good: Color::hex(0x6CCB5F, 1.0),
                warn: Color::hex(0xF7B32B, 1.0),
            }
        }
    }

    /// Whether the window frame should be dark too.
    pub(super) fn dark(&self) -> bool {
        self.bg.0 < 0.5
    }

    fn tone(&self, tone: Tone) -> Color {
        match tone {
            Tone::Plain => self.text2,
            Tone::Good => self.good,
            Tone::Warn => self.warn,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum Font {
    Title,
    Body,
    BodyStrong,
    Caption,
    Icon,
    IconLarge,
    IconSmall,
    Number,
}

#[derive(Clone, Copy)]
enum Align {
    Left,
    Center,
    Right,
}

/// A drawn device icon: which, its pixel size and colour.
type IconKey = (IconId, u32, [u8; 3]);

pub(super) struct Gfx {
    factory: ID2D1Factory,
    dwrite: IDWriteFactory,
    formats: Vec<IDWriteTextFormat>,
    target: Option<ID2D1HwndRenderTarget>,
    /// Device icons drawn for this target.
    icons: RefCell<Vec<(IconKey, ID2D1Bitmap)>>,
    /// The app's logo, by its pixel size, drawn for this target.
    logo: RefCell<Option<(u32, ID2D1Bitmap)>>,
}

/// What the pointer is doing, for hover and press states.
#[derive(Clone, Copy, Default)]
pub(super) struct Pointer {
    pub hover: Option<Target>,
    pub pressed: Option<Target>,
}

impl Gfx {
    pub(super) fn new() -> Result<Self> {
        unsafe {
            let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).context("D2D1CreateFactory")?;
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).context("DWriteCreateFactory")?;
            let mut gfx = Gfx { factory, dwrite, formats: Vec::new(), target: None, icons: RefCell::default(), logo: RefCell::default() };
            let text = w!("Segoe UI Variable Text");
            let display = w!("Segoe UI Variable Display");
            let icons = w!("Segoe Fluent Icons");
            gfx.formats = vec![
                gfx.format(display, DWRITE_FONT_WEIGHT_SEMI_BOLD, 28.0)?,
                gfx.format(text, DWRITE_FONT_WEIGHT_NORMAL, 14.0)?,
                gfx.format(text, DWRITE_FONT_WEIGHT_SEMI_BOLD, 14.0)?,
                gfx.format(text, DWRITE_FONT_WEIGHT_NORMAL, 12.0)?,
                gfx.format(icons, DWRITE_FONT_WEIGHT_NORMAL, 16.0)?,
                gfx.format(icons, DWRITE_FONT_WEIGHT_NORMAL, 20.0)?,
                gfx.format(icons, DWRITE_FONT_WEIGHT_NORMAL, 12.0)?,
                gfx.format(display, DWRITE_FONT_WEIGHT_SEMI_BOLD, 20.0)?,
            ];
            Ok(gfx)
        }
    }

    fn format(&self, family: PCWSTR, weight: DWRITE_FONT_WEIGHT, size: f32) -> Result<IDWriteTextFormat> {
        unsafe {
            let format = self.dwrite.CreateTextFormat(family, None, weight, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, size, w!("en-us"))?;
            format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            let sign = self.dwrite.CreateEllipsisTrimmingSign(&format)?;
            let trimming = DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 };
            format.SetTrimming(&trimming, &sign)?;
            Ok(format)
        }
    }

    /// The width of `text` in `font`, in DIPs.
    pub(super) fn measure(&self, font: Font, text: &str) -> f32 {
        let wide: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            let Ok(layout) = self.dwrite.CreateTextLayout(&wide, &self.formats[font as usize], 4096.0, 100.0) else {
                return 0.0;
            };
            let mut metrics = DWRITE_TEXT_METRICS::default();
            if layout.GetMetrics(&mut metrics).is_err() {
                return 0.0;
            }
            metrics.widthIncludingTrailingWhitespace
        }
    }

    /// The client area changed size (pixels).
    pub(super) fn resize(&self, width: u32, height: u32) {
        if let Some(target) = &self.target {
            let _ = unsafe { target.Resize(&D2D_SIZE_U { width, height }) };
        }
    }

    /// Draw a frame into `hwnd`'s client area (`size` in pixels) at `scale` pixels per DIP. A lost
    /// device drops the target, built again on the next frame.
    pub(super) fn frame(&mut self, hwnd: HWND, size: (u32, u32), scale: f32, draw: impl FnOnce(&mut Pen)) -> Result<()> {
        unsafe {
            if self.target.is_none() {
                let props = D2D1_RENDER_TARGET_PROPERTIES {
                    pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_IGNORE },
                    ..Default::default()
                };
                let hwnd_props = D2D1_HWND_RENDER_TARGET_PROPERTIES {
                    hwnd,
                    pixelSize: D2D_SIZE_U { width: size.0.max(1), height: size.1.max(1) },
                    presentOptions: D2D1_PRESENT_OPTIONS_NONE,
                };
                self.target = Some(self.factory.CreateHwndRenderTarget(&props, &hwnd_props).context("CreateHwndRenderTarget")?);
                self.icons.borrow_mut().clear();
                *self.logo.borrow_mut() = None;
            }
            let target = self.target.clone().expect("just built");
            target.SetDpi(96.0 * scale, 96.0 * scale);
            target.BeginDraw();
            let mut pen = Pen { gfx: self, target: target.clone(), scale };
            draw(&mut pen);
            if let Err(e) = target.EndDraw(None, None) {
                // D2DERR_RECREATE_TARGET: the device went away; the next frame builds anew.
                self.target = None;
                if e.code().0 as u32 != 0x8899_000C {
                    return Err(e).context("EndDraw");
                }
            }
        }
        Ok(())
    }
}

pub(super) struct Pen<'a> {
    gfx: &'a Gfx,
    target: ID2D1HwndRenderTarget,
    /// Pixels per DIP.
    scale: f32,
}

impl Pen<'_> {
    fn brush(&self, c: Color) -> Option<ID2D1SolidColorBrush> {
        unsafe { self.target.CreateSolidColorBrush(&D2D1_COLOR_F { r: c.0, g: c.1, b: c.2, a: c.3 }, None) }.ok()
    }

    fn clear(&self, c: Color) {
        unsafe { self.target.Clear(Some(&D2D1_COLOR_F { r: c.0, g: c.1, b: c.2, a: 1.0 })) };
    }

    fn fill(&self, r: R, radius: f32, c: Color) {
        if let Some(brush) = self.brush(c) {
            unsafe { self.target.FillRoundedRectangle(&rounded(r, radius), &brush) };
        }
    }

    /// A stroke inside `r` (a hairline sits on the pixel grid, not across it).
    fn stroke(&self, r: R, radius: f32, width: f32, c: Color) {
        if let Some(brush) = self.brush(c) {
            let half = width / 2.0;
            let inner = R::new(r.x + half, r.y + half, r.w - width, r.h - width);
            unsafe { self.target.DrawRoundedRectangle(&rounded(inner, radius), &brush, width, None) };
        }
    }

    fn circle(&self, cx: f32, cy: f32, radius: f32, c: Color) {
        if let Some(brush) = self.brush(c) {
            let e = D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: radius, radiusY: radius };
            unsafe { self.target.FillEllipse(&e, &brush) };
        }
    }

    fn text(&self, text: &str, font: Font, r: R, c: Color, align: Align) {
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

    fn glyph(&self, glyph: char, font: Font, r: R, c: Color) {
        self.text(&glyph.to_string(), font, r, c, Align::Center);
    }

    /// A device icon centred in `r`, `size` DIPs square, rendered as the flyout and the strip do.
    fn icon(&self, icon: IconId, size: f32, r: R, c: Color) {
        let px = (size * self.scale).round() as u32;
        let rgb = [(c.0 * 255.0) as u8, (c.1 * 255.0) as u8, (c.2 * 255.0) as u8];
        let key = (icon, px, rgb);
        let cached = self.gfx.icons.borrow().iter().find(|(k, _)| *k == key).map(|(_, b)| b.clone());
        let bitmap = match cached {
            Some(b) => b,
            None => {
                let Some(b) = self.bitmap(icon, px, rgb) else { return };
                self.gfx.icons.borrow_mut().push((key, b.clone()));
                b
            }
        };
        let at = R::new(r.x + (r.w - size) / 2.0, r.y + (r.h - size) / 2.0, size, size);
        unsafe { self.target.DrawBitmap(&bitmap, Some(&rect(at)), c.3, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None) };
    }

    fn bitmap(&self, icon: IconId, px: u32, rgb: [u8; 3]) -> Option<ID2D1Bitmap> {
        let (rgba, w, h) = icon.render(px, rgb).ok()?;
        self.rgba_bitmap(&rgba, w, h)
    }

    /// Audio Tray's own logo (the exe's icon, as the title bar shows it), filling `r`.
    fn logo(&self, r: R) {
        let px = (r.w * self.scale).round() as u32;
        let cached = self.gfx.logo.borrow().as_ref().filter(|(at, _)| *at == px).map(|(_, b)| b.clone());
        let bitmap = match cached {
            Some(b) => b,
            None => {
                let made = (|| {
                    let exe = std::env::current_exe().ok()?;
                    let png = crate::image::exe_icon_png(&exe.to_string_lossy(), px).ok()?;
                    let rgba = crate::image::sprite(&png, px, 0.0).ok()?;
                    self.rgba_bitmap(&rgba, px, px)
                })();
                let Some(b) = made else { return };
                *self.gfx.logo.borrow_mut() = Some((px, b.clone()));
                b
            }
        };
        unsafe { self.target.DrawBitmap(&bitmap, Some(&rect(r)), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None) };
    }

    fn rgba_bitmap(&self, rgba: &[u8], w: u32, h: u32) -> Option<ID2D1Bitmap> {
        // Straight RGBA to premultiplied BGRA.
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

    /// A line through `points`, `width` wide with round caps and joins.
    fn polyline(&self, points: &[Vector2], width: f32, c: Color) {
        if let (Some(path), Some(brush)) = (self.path(points, false), self.brush(c)) {
            unsafe {
                let round = D2D1_STROKE_STYLE_PROPERTIES {
                    startCap: D2D1_CAP_STYLE_ROUND,
                    endCap: D2D1_CAP_STYLE_ROUND,
                    dashCap: D2D1_CAP_STYLE_FLAT,
                    lineJoin: D2D1_LINE_JOIN_ROUND,
                    miterLimit: 1.0,
                    dashStyle: D2D1_DASH_STYLE_SOLID,
                    dashOffset: 0.0,
                };
                let style = self.gfx.factory.CreateStrokeStyle(&round, None).ok();
                self.target.DrawGeometry(&path, &brush, width, style.as_ref());
            }
        }
    }

    /// The shape inside `points`, filled.
    fn fill_shape(&self, points: &[Vector2], c: Color) {
        if let (Some(path), Some(brush)) = (self.path(points, true), self.brush(c)) {
            unsafe { self.target.FillGeometry(&path, &brush, None) };
        }
    }

    fn path(&self, points: &[Vector2], closed: bool) -> Option<ID2D1PathGeometry> {
        let (first, rest) = points.split_first()?;
        // A NaN point loses the whole shape, silently: leave it out instead.
        if points.iter().any(|p| !p.X.is_finite() || !p.Y.is_finite()) {
            return None;
        }
        unsafe {
            let path = self.gfx.factory.CreatePathGeometry().ok()?;
            let sink = path.Open().ok()?;
            sink.BeginFigure(*first, if closed { D2D1_FIGURE_BEGIN_FILLED } else { D2D1_FIGURE_BEGIN_HOLLOW });
            sink.AddLines(rest);
            sink.EndFigure(if closed { D2D1_FIGURE_END_CLOSED } else { D2D1_FIGURE_END_OPEN });
            sink.Close().ok()?;
            Some(path)
        }
    }

    fn clip(&self, r: R) {
        unsafe { self.target.PushAxisAlignedClip(&rect(r), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE) };
    }

    fn unclip(&self) {
        unsafe { self.target.PopAxisAlignedClip() };
    }

    fn translate(&self, dx: f32, dy: f32) {
        unsafe { self.target.SetTransform(&Matrix3x2::translation(dx, dy)) };
    }
}

fn rect(r: R) -> D2D_RECT_F {
    D2D_RECT_F { left: r.x, top: r.y, right: r.x + r.w, bottom: r.y + r.h }
}

fn rounded(r: R, radius: f32) -> D2D1_ROUNDED_RECT {
    D2D1_ROUNDED_RECT { rect: rect(r), radiusX: radius, radiusY: radius }
}

/// Everything one frame needs.
pub(super) struct Scene<'a> {
    pub model: &'a Model,
    pub page: &'a Laid,
    pub pointer: Pointer,
    pub palette: &'a Palette,
    /// The client area, in DIPs.
    pub size: (f32, f32),
    pub scroll: f32,
    /// Seconds on the window's animation clock: the pictures of moving bars move with it.
    pub time: f32,
}

pub(super) fn draw(pen: &mut Pen, s: &Scene) {
    let p = s.palette;
    let (w, h) = s.size;
    pen.translate(0.0, 0.0);
    pen.clear(p.bg);
    draw_nav(pen, s);
    // The page scrolls under a clip beside the nav.
    let area = R::new(layout::NAV_W, 0.0, w - layout::NAV_W, h);
    pen.clip(area);
    pen.translate(layout::NAV_W, -s.scroll);
    for (i, item) in s.page.items.iter().enumerate() {
        draw_item(pen, s, &s.page.items, i, item);
    }
    pen.translate(0.0, 0.0);
    pen.unclip();
    // A thin scroll indicator when the page is taller than the window.
    if s.page.height > h {
        let len = (h * h / s.page.height).max(32.0);
        let at = s.scroll / (s.page.height - h) * (h - len);
        pen.fill(R::new(w - 6.0, at + 2.0, 3.0, len - 4.0), 1.5, p.text3);
    }
}

fn draw_nav(pen: &Pen, s: &Scene) {
    let p = s.palette;
    // The app, by name and version, over its pages.
    let head = layout::nav_header();
    let logo = R::new(head.x, head.y + (head.h - 36.0) / 2.0, 36.0, 36.0);
    pen.logo(logo);
    let x = logo.right() + 12.0;
    pen.text("Audio Tray", Font::BodyStrong, R::new(x, logo.y - 1.0, head.right() - x, 20.0), p.text, Align::Left);
    let version = concat!("Version ", env!("CARGO_PKG_VERSION"));
    pen.text(version, Font::Caption, R::new(x, logo.y + 19.0, head.right() - x, 18.0), p.text2, Align::Left);
    let (headings, pages) = layout::nav_entries(s.size.1);
    for (heading, r) in headings {
        pen.text(heading, Font::Caption, R::new(r.x, r.y + 8.0, r.w, r.h - 8.0), p.text2, Align::Left);
    }
    for (page, r) in pages {
        let target = Some(Target::Nav(page));
        let selected = s.model.page == page;
        if selected || s.pointer.hover == target {
            let fill = if s.pointer.pressed == target { p.subtle_pressed } else { p.subtle_hover };
            pen.fill(r, 4.0, fill);
        }
        if selected {
            pen.fill(R::new(r.x, r.y + (r.h - 16.0) / 2.0, 3.0, 16.0), 1.5, p.accent);
        }
        pen.glyph(page.glyph(), Font::Icon, R::new(r.x + 12.0, r.y, 24.0, r.h), p.text);
        let needs = layout::attention(s.model, page);
        let badge_w = if needs > 0 { 28.0 } else { 0.0 };
        pen.text(page.title(), Font::Body, R::new(r.x + 48.0, r.y, r.w - 56.0 - badge_w, r.h), p.text, Align::Left);
        // What wants attention there: a count in the warning colour.
        if needs > 0 {
            let badge = R::new(r.right() - 30.0, r.y + (r.h - 18.0) / 2.0, 20.0, 18.0);
            pen.fill(badge, 9.0, p.warn);
            pen.text(&needs.to_string(), Font::Caption, R::new(badge.x, badge.y - 1.0, badge.w, badge.h), Color::hex(0x1A1A1A, 1.0), Align::Center);
        }
    }
}

/// Where the text of the card at `index` must stop: before the first control inside it.
fn text_limit(items: &[Item], index: usize, card: R) -> f32 {
    items[index + 1..]
        .iter()
        .take_while(|i| !matches!(i.kind, Kind::Card { .. } | Kind::Section(_) | Kind::Title(_) | Kind::Map { .. }))
        .filter(|i| i.r.x > card.x + layout::CARD_TEXT_X && i.r.y >= card.y && i.r.bottom() <= card.bottom() + 0.5)
        // A switch's On or Off sits before it.
        .map(|i| i.r.x - 16.0 - if matches!(i.kind, Kind::Toggle { .. }) { layout::TOGGLE_LABEL_W + 8.0 } else { 0.0 })
        .fold(card.right() - layout::CARD_PAD, f32::min)
}

fn draw_item(pen: &Pen, s: &Scene, items: &[Item], index: usize, item: &Item) {
    let p = s.palette;
    let r = item.r;
    let hover = |t: Target| s.pointer.hover == Some(t);
    let pressed = |t: Target| s.pointer.pressed == Some(t) && s.pointer.hover == Some(t);
    match &item.kind {
        Kind::Title(text) => pen.text(text, Font::Title, R::new(r.x, r.y + 16.0, r.w, 44.0), p.text, Align::Left),
        Kind::Section(text) => pen.text(text, Font::BodyStrong, R::new(r.x + 2.0, r.y + 12.0, r.w, r.h - 16.0), p.text, Align::Left),
        Kind::Card { glyph, title, desc, dim, target } => {
            let fill = match target {
                Some(t) if pressed(*t) && !dim => p.card,
                Some(t) if hover(*t) && !dim => p.card_hover,
                _ => p.card,
            };
            pen.fill(r, 4.0, fill);
            pen.stroke(r, 4.0, 1.0, p.card_stroke);
            let a = if *dim { 0.45 } else { 1.0 };
            if let Some(g) = glyph {
                pen.glyph(*g, Font::IconLarge, R::new(r.x + 16.0, r.y, 24.0, r.h), p.text.alpha(a));
            }
            let x = r.x + layout::CARD_TEXT_X;
            let limit = text_limit(items, index, r);
            match desc {
                Some((desc, tone)) => {
                    pen.text(title, Font::Body, R::new(x, r.y + 13.0, limit - x, 22.0), p.text.alpha(a), Align::Left);
                    status(pen, p, desc, *tone, R::new(x, r.y + 34.0, limit - x, 20.0), a);
                }
                None => pen.text(title, Font::Body, R::new(x, r.y, limit - x, r.h), p.text.alpha(a), Align::Left),
            }
        }
        Kind::Trailing(g) => pen.glyph(*g, Font::IconSmall, r, p.text2),
        Kind::Divider => pen.fill(r, 0.0, p.card_stroke),
        Kind::Badge { n, done } => {
            let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
            let ring = R::new(cx - 12.0, cy - 12.0, 24.0, 24.0);
            if *done {
                pen.fill(ring, 12.0, p.accent);
                pen.glyph('\u{E73E}', Font::IconSmall, ring, p.on_accent);
            } else {
                pen.stroke(ring, 12.0, 1.0, p.text2);
                pen.text(&n.to_string(), Font::Caption, ring, p.text, Align::Center);
            }
        }
        Kind::Label { title, desc, dim } => {
            let a = if *dim { 0.45 } else { 1.0 };
            match desc {
                Some((desc, tone)) => {
                    let top = r.y + (r.h - 42.0) / 2.0;
                    pen.text(title, Font::Body, R::new(r.x, top, r.w, 22.0), p.text.alpha(a), Align::Left);
                    status(pen, p, desc, *tone, R::new(r.x, top + 21.0, r.w, 20.0), a);
                }
                None => pen.text(title, Font::Body, r, p.text.alpha(a), Align::Left),
            }
        }
        Kind::Value(text) => pen.text(text, Font::Body, r, p.text2, Align::Right),
        Kind::Toggle { on, dim, toggle } => {
            let a = if *dim { 0.4 } else { 1.0 };
            let t = Target::Toggle(*toggle);
            pen.text(if *on { "On" } else { "Off" }, Font::Body, R::new(r.x - layout::TOGGLE_LABEL_W - 8.0, r.y - 6.0, layout::TOGGLE_LABEL_W, r.h + 12.0), p.text.alpha(a), Align::Right);
            let knob = if hover(t) && !dim { 7.0 } else { 6.0 };
            let cy = r.y + r.h / 2.0;
            match (*on, *dim) {
                // Disabled is neutral, as Windows draws it: a faded accent reads as another colour.
                (true, true) => {
                    pen.fill(r, r.h / 2.0, p.text.alpha(0.16));
                    pen.circle(r.right() - r.h / 2.0, cy, knob, p.text.alpha(0.5));
                }
                (true, false) => {
                    pen.fill(r, r.h / 2.0, if hover(t) { p.accent.alpha(0.9) } else { p.accent });
                    pen.circle(r.right() - r.h / 2.0, cy, knob, p.on_accent);
                }
                (false, _) => {
                    if hover(t) && !dim {
                        pen.fill(r, r.h / 2.0, p.subtle_hover);
                    }
                    let edge = if *dim { p.text3 } else { p.text2 };
                    pen.stroke(r, r.h / 2.0, 1.0, edge);
                    pen.circle(r.x + r.h / 2.0, cy, knob, edge);
                }
            }
        }
        Kind::Segment { label, selected, first, dim, target, .. } => {
            let a = if *dim { 0.4 } else { 1.0 };
            if *first {
                // The group's well, under every segment of it.
                let end = items[index..]
                    .iter()
                    .take_while(|i| matches!(i.kind, Kind::Segment { .. }))
                    .position(|i| matches!(i.kind, Kind::Segment { last: true, .. }))
                    .map_or(r.right(), |k| items[index + k].r.right());
                let well = R::new(r.x, r.y, end - r.x, r.h);
                pen.fill(well, 6.0, p.control.alpha(a));
                pen.stroke(well, 6.0, 1.0, p.control_stroke.alpha(a));
            }
            let inner = R::new(r.x + 3.0, r.y + 3.0, r.w - 6.0, r.h - 6.0);
            if *selected {
                // A raised tab, its accent a short bar under the label (Windows' selector look).
                pen.fill(inner, 4.0, p.raised.alpha(a));
                pen.stroke(inner, 4.0, 1.0, p.control_stroke.alpha(a));
                let bar = R::new(inner.x + (inner.w - 16.0) / 2.0, inner.bottom() - 2.5, 16.0, 3.0);
                pen.fill(bar, 1.5, if *dim { p.text3 } else { p.accent });
            } else if hover(*target) {
                pen.fill(inner, 4.0, if pressed(*target) { p.subtle_pressed } else { p.subtle_hover });
            }
            let color = if *selected { p.text } else { p.text2 };
            pen.text(label, Font::Body, R::new(r.x, r.y - 2.0, r.w, r.h), color.alpha(a), Align::Center);
        }
        Kind::Button { label, accent, glyph, target } => {
            let (fill, text) = if *accent {
                (if hover(*target) { p.accent.alpha(0.9) } else { p.accent }, p.on_accent)
            } else {
                (if hover(*target) { p.control_hover } else { p.control }, if pressed(*target) { p.text2 } else { p.text })
            };
            pen.fill(r, 4.0, fill);
            if !accent {
                pen.stroke(r, 4.0, 1.0, p.control_stroke);
            }
            match glyph {
                // A menu's chevron trails its label.
                Some(g @ ('\u{E70D}' | '\u{E70E}')) => {
                    pen.text(label, Font::Body, R::new(r.x + 12.0, r.y, r.w - 40.0, r.h), text, Align::Left);
                    pen.glyph(*g, Font::IconSmall, R::new(r.right() - 28.0, r.y, 16.0, r.h), text);
                }
                Some(g) => {
                    let tw = pen.gfx.measure(Font::Body, label);
                    let x = r.x + (r.w - tw - 24.0) / 2.0;
                    pen.glyph(*g, Font::IconSmall, R::new(x, r.y, 16.0, r.h), text);
                    pen.text(label, Font::Body, R::new(x + 24.0, r.y, tw + 2.0, r.h), text, Align::Left);
                }
                None => pen.text(label, Font::Body, r, text, Align::Center),
            }
        }
        Kind::Map { dim } => draw_map(pen, s, r, *dim),
        Kind::Text(text) => pen.text(text, Font::Caption, r, p.text2, Align::Left),
        Kind::Glance { title, head, line, tone, target } => {
            pen.fill(r, 6.0, if hover(*target) { p.card_hover } else { p.card });
            pen.stroke(r, 6.0, 1.0, p.card_stroke);
            let inner = R::new(r.x + 16.0, r.y + 14.0, r.w - 32.0, r.h - 28.0);
            status(pen, p, title, *tone, R::new(inner.x, inner.y, inner.w, 18.0), 1.0);
            pen.text(head, Font::BodyStrong, R::new(inner.x, inner.y + 24.0, inner.w, 22.0), p.text, Align::Left);
            pen.text(line, Font::Caption, R::new(inner.x, inner.y + 48.0, inner.w, 20.0), p.text2, Align::Left);
        }
        Kind::Tile { picture, title, desc, selected, dim, target } => {
            let a = if *dim { 0.45 } else { 1.0 };
            pen.fill(r, 6.0, if hover(*target) && !dim { p.card_hover } else { p.card });
            if *selected {
                pen.stroke(r, 6.0, 2.0, if *dim { p.text3 } else { p.accent });
            } else {
                pen.stroke(r, 6.0, 1.0, p.card_stroke);
            }
            let art = R::new(r.x + 12.0, r.y + 12.0, r.w - 24.0, r.h - 12.0 - 58.0);
            picture_of(pen, s, *picture, art, a);
            // A radio, its name, and a line on what it does.
            let (cx, cy) = (r.x + 21.0, r.bottom() - 37.0);
            if *selected {
                pen.circle(cx, cy, 8.0, if *dim { p.text3 } else { p.accent });
                pen.circle(cx, cy, 3.5, p.on_accent);
            } else {
                pen.circle(cx, cy, 8.0, p.text2.alpha(a));
                pen.circle(cx, cy, 7.0, p.card);
            }
            pen.text(title, Font::BodyStrong, R::new(r.x + 36.0, cy - 11.0, r.w - 48.0, 22.0), p.text.alpha(a), Align::Left);
            pen.text(desc, Font::Caption, R::new(r.x + 12.0, r.bottom() - 26.0, r.w - 24.0, 18.0), p.text2.alpha(a), Align::Left);
        }
        Kind::PillPreview { on } => pill_preview(pen, s, r, *on),
        Kind::Outline => pen.stroke(r, 4.0, 2.0, p.accent),
        Kind::Level { level, low } => {
            // The number, then a bar filled to it: amber when low.
            let bar = R::new(r.x, r.y + (r.h - 6.0) / 2.0, r.w - 56.0, 6.0);
            pen.fill(bar, 3.0, p.text.alpha(0.16));
            let fill = if *low { p.warn } else { p.good };
            pen.fill(R::new(bar.x, bar.y, (bar.w * *level as f32 / 100.0).max(6.0), bar.h), 3.0, fill);
            pen.text(&format!("{level}%"), Font::Body, R::new(bar.right() + 8.0, r.y, 48.0, r.h), p.text, Align::Right);
        }
        Kind::Pick { label, selected, dim, target } => {
            let a = if *dim { 0.45 } else { 1.0 };
            let (fill, text) = if *selected {
                (if *dim { p.text3 } else { p.accent }, p.on_accent)
            } else if hover(*target) && !dim {
                (p.control_hover, p.text)
            } else {
                (p.control, p.text)
            };
            pen.fill(r, r.h / 2.0, fill.alpha(a));
            if !selected {
                pen.stroke(r, r.h / 2.0, 1.0, p.control_stroke.alpha(a));
            }
            pen.text(label, Font::Body, R::new(r.x + 8.0, r.y - 1.0, r.w - 16.0, r.h), text.alpha(a), Align::Center);
        }
        Kind::Logo => pen.logo(r),
        Kind::Chip { icon, selected, target } => {
            let fill = if hover(*target) { p.control_hover } else { p.control };
            pen.fill(r, 6.0, fill);
            if *selected {
                pen.stroke(r, 6.0, 2.0, p.accent);
            } else {
                pen.stroke(r, 6.0, 1.0, p.control_stroke);
            }
            pen.icon(*icon, 22.0, r, p.text);
        }
        Kind::Icon(icon) => pen.icon(*icon, 20.0, r, p.text),
        Kind::Field(text) => {
            // Windows' focused text box: a control well with an accent line along its bottom.
            pen.fill(r, 4.0, p.control);
            pen.stroke(r, 4.0, 1.0, p.control_stroke);
            pen.fill(R::new(r.x + 1.0, r.bottom() - 2.0, r.w - 2.0, 2.0), 1.0, p.accent);
            let inner = R::new(r.x + 10.0, r.y, r.w - 20.0, r.h);
            pen.clip(inner);
            pen.text(text, Font::Body, inner, p.text, Align::Left);
            let caret = inner.x + pen.gfx.measure(Font::Body, text).min(inner.w - 1.0);
            pen.fill(R::new(caret, r.y + 8.0, 1.0, r.h - 16.0), 0.0, p.text);
            pen.unclip();
        }
    }
}

/// The displays as the desktop arranges them, the one the pill is on outlined, and the pill's
/// nine places on it (on the hovered display too) to click.
fn draw_map(pen: &Pen, s: &Scene, r: R, dim: bool) {
    let p = s.palette;
    let a = if dim { 0.45 } else { 1.0 };
    pen.fill(r, 4.0, p.card);
    pen.stroke(r, 4.0, 1.0, p.card_stroke);
    let pill = &s.model.config.pill;
    let chosen = crate::pill::display_for(pill.edge, pill.monitor.as_deref(), &s.model.monitors);
    let hovered = match s.pointer.hover {
        Some(Target::Monitor(i) | Target::Slot(i, ..)) => Some(i),
        _ => None,
    };
    for (i, screen) in layout::map_rects(s.model, r).into_iter().enumerate() {
        let on = Some(i) == chosen;
        let fill = if hovered == Some(i) && !dim { p.control_hover } else { p.control };
        pen.fill(screen, 6.0, fill.alpha(a));
        if on {
            pen.stroke(screen, 6.0, 2.0, p.accent.alpha(a));
        } else {
            pen.stroke(screen, 6.0, 1.0, p.control_stroke.alpha(a));
        }
        let label = R::new(screen.x, screen.y + screen.h / 2.0 - 18.0, screen.w, 28.0);
        pen.text(&(i + 1).to_string(), Font::Number, label, p.text.alpha(a), Align::Center);
        if s.model.monitors.get(i).is_some_and(|m| m.primary) {
            pen.text("Main", Font::Caption, R::new(screen.x, label.bottom(), screen.w, 18.0), p.text2.alpha(a), Align::Center);
        }
        if !(on || hovered == Some(i)) {
            continue;
        }
        for edge in layout::EDGES {
            for align in layout::ALIGNS {
                let slot = layout::slot_rect(screen, edge, align);
                let target = Target::Slot(i, edge, align);
                let placed = on && pill.edge == edge && pill.align == align;
                let color = if placed {
                    p.accent
                } else if s.pointer.hover == Some(target) {
                    p.text2
                } else {
                    p.text3.alpha(0.6)
                };
                pen.fill(slot, slot.w.min(slot.h) / 2.0, color.alpha(a));
            }
        }
    }
    // Which side says it in words too: the map is small.
    if !dim {
        let labels = layout::align_labels(pill.edge);
        let at = layout::ALIGNS.iter().position(|x| *x == pill.align).unwrap_or(1);
        let edge = match pill.edge {
            crate::config::PillEdge::Left => "Left edge",
            crate::config::PillEdge::Right => "Right edge",
            crate::config::PillEdge::Top => "Top edge",
        };
        let place = if pill.align == PillAlign::Center { "middle".to_string() } else { labels[at].to_lowercase() };
        pen.text(&format!("{edge}, {place}"), Font::Caption, R::new(r.x + 16.0, r.bottom() - 26.0, r.w - 32.0, 20.0), p.text2, Align::Left);
    }
}

/// A card's second line: plain, or after a dot in its tone's colour (the sentence itself stays
/// neutral; a whole line in green or amber reads as noise).
fn status(pen: &Pen, p: &Palette, text: &str, tone: Tone, r: R, a: f32) {
    if tone == Tone::Plain {
        pen.text(text, Font::Caption, r, p.text2.alpha(a), Align::Left);
        return;
    }
    pen.circle(r.x + 4.0, r.y + r.h / 2.0 + 0.5, 4.0, p.tone(tone).alpha(a));
    pen.text(text, Font::Caption, R::new(r.x + 14.0, r.y, r.w - 14.0, r.h), p.text2.alpha(a), Align::Left);
}

// ---- Pictures: what a choice looks like, drawn small and moving with the window's clock. ----

/// The desktop behind a picture of the pill: the same in both themes, as a screen would be.
const SCREEN: Color = Color::hex(0x26303F, 1.0);
const COVER: Color = Color::hex(0xE2683C, 1.0);
/// A sample cover's own colour, for the pictures when the waves take the cover's.
const COVER_TINT: Color = Color::hex(0xFF8A5C, 1.0);
const PILL_BG: Color = Color::hex(0x1C1C1C, 0.9);
const DEVICE_DISC: Color = Color::hex(0x3A3A3A, 1.0);

fn mix(a: Color, b: Color, t: f32) -> Color {
    Color(a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t, a.2 + (b.2 - a.2) * t, a.3)
}

/// The waves' colour: the accent, or the sample cover's.
fn wave_tint(s: &Scene) -> Color {
    if s.model.config.motion.colour == crate::config::WaveColour::Cover {
        COVER_TINT
    } else {
        s.palette.accent
    }
}

/// A point `radius` from `(cx, cy)` at `angle`, clockwise from the top.
fn at(cx: f32, cy: f32, radius: f32, angle: f32) -> Vector2 {
    Vector2 { X: cx + radius * angle.sin(), Y: cy - radius * angle.cos() }
}

/// One of Samsung's layers: its length (a share of the played part), height (a share of the
/// room), speed in periods a second, phase, shade (towards white when positive) and opacity.
#[derive(Clone, Copy)]
struct Layer {
    share: f32,
    height: f32,
    speed: f32,
    phase: f32,
    shade: f32,
    alpha: f32,
}

const LAYERS: [Layer; 3] = [
    Layer { share: 0.55, height: 1.0, speed: 0.35, phase: 0.0, shade: 0.0, alpha: 0.7 },
    Layer { share: 0.3, height: 0.8, speed: -0.55, phase: 1.7, shade: -0.2, alpha: 0.6 },
    Layer { share: 0.2, height: 0.62, speed: 0.8, phase: 3.1, shade: 0.45, alpha: 0.55 },
];

/// A layer's height `u` along a played part `span` long, at most `room`, `t` seconds in.
fn layer_height(layer: Layer, u: f32, span: f32, room: f32, t: f32) -> f32 {
    let length = (span * layer.share).max(8.0);
    let hump = 0.5 + 0.5 * (std::f32::consts::TAU * (u / length - layer.speed * t) + layer.phase).sin();
    let ends = (std::f32::consts::PI * (u / span).clamp(0.0, 1.0)).sin().max(0.0).powf(0.7);
    room * layer.height * hump * ends
}

fn shade(c: Color, by: f32) -> Color {
    if by >= 0.0 {
        mix(c, Color::hex(0xFFFFFF, c.3), by)
    } else {
        mix(c, Color::hex(0x000000, c.3), -by)
    }
}

/// A glowing knob.
fn knob(pen: &Pen, x: f32, y: f32, r: f32, c: Color) {
    pen.circle(x, y, r * 1.9, c.alpha(0.16));
    pen.circle(x, y, r * 1.4, c.alpha(0.3));
    pen.circle(x, y, r, c);
    pen.circle(x, y, r * 0.45, mix(c, Color::hex(0xFFFFFF, c.3), 0.5));
}

fn picture_of(pen: &Pen, s: &Scene, picture: layout::Picture, r: R, a: f32) {
    use layout::Picture;
    match picture {
        Picture::Size(size) => size_picture(pen, size, r, a),
        Picture::Click(click) => click_picture(pen, s, click, r, a),
        Picture::PlayerButton(style) => player_picture(pen, s, style, r, a),
        Picture::Ring(style) => {
            let radius = (r.w.min(r.h) / 2.0 - 10.0).max(10.0);
            ring_picture(pen, s, style, (r.x + r.w / 2.0, r.y + r.h / 2.0), radius, a);
        }
        Picture::Bar(style) => bar_picture(pen, s, style, r, a),
        Picture::Volume(style) => volume_picture(pen, s, style, r, a),
        Picture::Colour(colour) => {
            let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
            match colour {
                crate::config::WaveColour::Accent => pen.circle(cx, cy, 18.0, s.palette.accent.alpha(a)),
                crate::config::WaveColour::Cover => {
                    pen.fill(R::new(cx - 46.0, cy - 18.0, 36.0, 36.0), 4.0, Color::hex(0xF0A34A, a));
                    pen.fill(R::new(cx - 46.0, cy, 36.0, 18.0), 4.0, Color::hex(0xC2366B, a));
                    pen.circle(cx + 20.0, cy, 18.0, COVER_TINT.alpha(a));
                }
            }
        }
    }
}

/// A cover with its ring around it, in `style`, filled to two thirds.
fn ring_picture(pen: &Pen, s: &Scene, style: crate::config::MotionStyle, (cx, cy): (f32, f32), r: f32, a: f32) {
    use crate::config::MotionStyle;
    use std::f32::consts::TAU;
    let p = s.palette;
    let tint = wave_tint(s).alpha(a);
    let track = p.text.alpha(0.22 * a);
    let sweep = 0.64 * TAU;
    let sw = (r * 0.11).clamp(2.0, 4.0);
    pen.circle(cx, cy, r * 0.76, COVER.alpha(a));
    let arc = |from: f32, to: f32, radius: &dyn Fn(f32) -> f32| -> Vec<Vector2> {
        let steps = (((to - from) * r * 2.0).ceil() as usize).max(2);
        (0..=steps).map(|i| from + (to - from) * i as f32 / steps as f32).map(|ang| at(cx, cy, radius(ang), ang)).collect()
    };
    match style {
        MotionStyle::None => {
            pen.polyline(&arc(0.0, TAU, &|_| r), sw, track);
            pen.polyline(&arc(0.0, sweep, &|_| r), sw, tint);
        }
        MotionStyle::Google => {
            let gap = (sw + 2.5) / r;
            pen.polyline(&arc(sweep + gap, TAU - gap, &|_| r), sw, track);
            let periods = (TAU * r / 12.0).round().max(5.0);
            let amp = r * 0.06;
            let span = sweep * r;
            let ease = TAU * r / periods / 2.0;
            let smooth = |v: f32| {
                let v = v.clamp(0.0, 1.0);
                v * v * (3.0 - 2.0 * v)
            };
            let phase = TAU * s.time;
            let rippled = |ang: f32| {
                let u = ang * r;
                r + amp * smooth(u / ease) * smooth((span - u) / ease) * (periods * ang - phase).sin()
            };
            pen.polyline(&arc(0.0, sweep, &rippled), sw, tint);
        }
        MotionStyle::Samsung => {
            let gap = (sw + 3.0) / r;
            pen.polyline(&arc(sweep + gap, TAU - gap * 0.5, &|_| r), sw * 0.6, track);
            let span = sweep * r;
            let base = r + sw / 2.0;
            let room = r * 0.3;
            for layer in LAYERS {
                let mut shape = arc(0.0, sweep, &|ang| base + layer_height(layer, ang * r, span, room, s.time));
                shape.extend(arc(0.0, sweep, &|_| base - 0.3).into_iter().rev());
                pen.fill_shape(&shape, shade(tint, layer.shade).alpha(layer.alpha));
            }
            pen.polyline(&arc(0.0, sweep, &|_| r), sw * 0.8, tint);
            let head = at(cx, cy, r, sweep);
            knob(pen, head.X, head.Y, sw * 1.2, tint);
        }
    }
}

/// A card's track bar in `style`, filled to three fifths.
fn bar_picture(pen: &Pen, s: &Scene, style: crate::config::MotionStyle, r: R, a: f32) {
    use crate::config::MotionStyle;
    use std::f32::consts::TAU;
    let p = s.palette;
    let tint = wave_tint(s).alpha(a);
    let track = p.text.alpha(0.22 * a);
    let (x0, x1) = (r.x + 10.0, r.right() - 10.0);
    let cy = r.y + r.h * 0.62;
    let head = x0 + (x1 - x0) * 0.6;
    let line = |from: f32, to: f32| [Vector2 { X: from, Y: cy }, Vector2 { X: to, Y: cy }];
    match style {
        MotionStyle::None => {
            pen.polyline(&line(x0, x1), 3.0, track);
            pen.polyline(&line(x0, head), 3.0, tint);
        }
        MotionStyle::Google => {
            pen.polyline(&line(head + 7.0, x1), 3.0, track);
            let length = 20.0;
            let end = head - 5.0;
            let steps = ((end - x0) * 2.0).max(2.0) as usize;
            let wave: Vec<Vector2> = (0..=steps)
                .map(|i| {
                    let x = x0 + (end - x0) * i as f32 / steps as f32;
                    Vector2 { X: x, Y: cy + 2.5 * (TAU * ((x - x0) / length - s.time)).sin() }
                })
                .collect();
            pen.polyline(&wave, 3.0, tint);
            pen.fill(R::new(head - 2.0, cy - 7.0, 4.0, 14.0), 2.0, tint);
        }
        MotionStyle::Samsung => {
            pen.polyline(&line(head + 10.0, x1), 3.0, track);
            let span = head - x0;
            let steps = span.max(2.0) as usize;
            for layer in LAYERS {
                let mut shape: Vec<Vector2> = (0..=steps)
                    .map(|i| {
                        let u = span * i as f32 / steps as f32;
                        Vector2 { X: x0 + u, Y: cy - 1.5 - layer_height(layer, u, span, 14.0, s.time) }
                    })
                    .collect();
                shape.push(Vector2 { X: head, Y: cy });
                shape.push(Vector2 { X: x0, Y: cy });
                pen.fill_shape(&shape, shade(tint, layer.shade).alpha(layer.alpha));
            }
            pen.polyline(&line(x0, head), 3.0, tint);
            knob(pen, head, cy, 4.5, tint);
        }
    }
}

/// A volume slider in `style`, its level stepping between two values and getting there as the
/// style does.
fn volume_picture(pen: &Pen, s: &Scene, style: crate::config::MotionStyle, r: R, a: f32) {
    use crate::config::MotionStyle;
    let p = s.palette;
    let accent = p.accent.alpha(a);
    const STEP: f32 = 1.4;
    let n = (s.time / STEP).floor();
    let tau = s.time - n * STEP;
    let (from, to) = if (n as i64) % 2 == 0 { (0.8, 0.35) } else { (0.35, 0.8) };
    let level = match style {
        MotionStyle::None => to,
        MotionStyle::Google => to + (from - to) * (-5.0 * tau).exp() * (9.0 * tau).cos(),
        MotionStyle::Samsung => to + (from - to) * (-3.5 * tau).exp() * (8.0 * tau).cos(),
    };
    let (x0, x1) = (r.x + 14.0, r.right() - 14.0);
    let cy = r.y + r.h / 2.0;
    match style {
        MotionStyle::None => {
            pen.fill(R::new(x0, cy - 2.0, x1 - x0, 4.0), 2.0, p.text.alpha(0.28 * a));
            let fx = x0 + (x1 - x0) * level;
            pen.fill(R::new(x0, cy - 2.0, fx - x0, 4.0), 2.0, accent);
            pen.circle(fx, cy, 7.0, accent);
        }
        MotionStyle::Google => {
            let (th, gap) = (10.0, 4.0);
            let fx = x0 + (x1 - x0) * level.clamp(0.0, 1.0);
            if x1 - (fx + 2.0 + gap) > 0.0 {
                pen.fill(R::new(fx + 2.0 + gap, cy - th / 2.0, x1 - fx - 2.0 - gap, th), th / 2.0, p.text.alpha(0.18 * a));
                pen.circle(x1 - th / 2.0, cy, 2.0, accent);
            }
            if fx - 2.0 - gap > x0 {
                pen.fill(R::new(x0, cy - th / 2.0, fx - 2.0 - gap - x0, th), th / 2.0, accent);
            }
            pen.fill(R::new(fx - 2.0, cy - 13.0, 4.0, 26.0), 2.0, accent);
        }
        MotionStyle::Samsung => {
            let th = 22.0;
            pen.fill(R::new(x0, cy - th / 2.0, x1 - x0, th), th / 2.0, p.text.alpha(0.12 * a));
            let fx = x0 + th + (x1 - x0 - th) * level.clamp(0.0, 1.02);
            pen.fill(R::new(x0, cy - th / 2.0, fx - x0, th), th / 2.0, accent);
        }
    }
}

/// The pill at rest in `size`, on the left edge of a small screen.
fn size_picture(pen: &Pen, size: crate::config::PillSize, r: R, a: f32) {
    use crate::config::PillSize;
    pen.fill(r, 6.0, SCREEN.alpha(a));
    pen.clip(r);
    let cy = r.y + r.h / 2.0;
    match size {
        PillSize::Hidden => pen.fill(R::new(r.x - 3.0, cy - 18.0, 7.0, 36.0), 3.0, Color::hex(0x9A9A9A, 0.8 * a)),
        PillSize::Small => {
            pen.fill(R::new(r.x - 14.0, cy - 28.0, 36.0, 56.0), 14.0, PILL_BG.alpha(a));
            pen.circle(r.x + 11.0, cy - 12.0, 7.0, COVER.alpha(a));
            pen.circle(r.x + 11.0, cy + 13.0, 7.0, DEVICE_DISC.alpha(a));
        }
        PillSize::Big => {
            pen.fill(R::new(r.x - 24.0, cy - 44.0, 64.0, 88.0), 24.0, PILL_BG.alpha(a));
            pen.circle(r.x + 18.0, cy - 18.0, 14.0, COVER.alpha(a));
            pen.circle(r.x + 18.0, cy + 22.0, 12.0, DEVICE_DISC.alpha(a));
        }
    }
    pen.unclip();
}

/// A strip of the taskbar along the bottom of `r`, and its top.
fn taskbar_strip(pen: &Pen, r: R, a: f32) -> f32 {
    pen.fill(r, 6.0, SCREEN.alpha(a));
    let bar = R::new(r.x, r.bottom() - 30.0, r.w, 30.0);
    pen.fill(bar, 0.0, Color::hex(0x1C1C1C, a));
    pen.fill(R::new(bar.x, bar.bottom() - 6.0, bar.w, 6.0), 6.0, Color::hex(0x1C1C1C, a));
    bar.y
}

/// What a left click on the output button does: the button on the taskbar, and above it what
/// happens.
fn click_picture(pen: &Pen, s: &Scene, click: crate::config::LeftClick, r: R, a: f32) {
    use crate::config::LeftClick;
    let p = s.palette;
    let white = Color::hex(0xFFFFFF, a);
    pen.clip(r);
    let top = taskbar_strip(pen, r, a);
    let cx = r.x + r.w / 2.0;
    // The output button, under the pointer.
    let button = R::new(cx - 16.0, top + 2.0, 32.0, 26.0);
    pen.fill(button, 4.0, Color::hex(0xFFFFFF, 0.12 * a));
    pen.glyph('\u{E767}', Font::Icon, button, white);
    let row = R::new(r.x, r.y + 8.0, r.w, top - r.y - 16.0);
    match click {
        LeftClick::Switch => {
            pen.glyph('\u{E7F6}', Font::IconLarge, R::new(cx - 64.0, row.y, 40.0, row.h), white.alpha(0.6));
            pen.glyph('\u{E72A}', Font::Icon, R::new(cx - 12.0, row.y, 24.0, row.h), p.accent.alpha(a));
            pen.glyph('\u{E7F5}', Font::IconLarge, R::new(cx + 24.0, row.y, 40.0, row.h), white);
        }
        LeftClick::Mute => {
            pen.glyph('\u{E767}', Font::IconLarge, R::new(cx - 64.0, row.y, 40.0, row.h), white.alpha(0.6));
            pen.glyph('\u{E72A}', Font::Icon, R::new(cx - 12.0, row.y, 24.0, row.h), p.accent.alpha(a));
            pen.glyph('\u{E74F}', Font::IconLarge, R::new(cx + 24.0, row.y, 40.0, row.h), white);
        }
        LeftClick::Panel => {
            // The panel, risen over the taskbar, its two sliders.
            let panel = R::new(cx - 52.0, row.y + 4.0, 104.0, row.h - 4.0);
            pen.fill(panel, 6.0, Color::hex(0x2C2C2C, a));
            pen.stroke(panel, 6.0, 1.0, Color::hex(0xFFFFFF, 0.1 * a));
            for (i, level) in [0.62, 0.4].into_iter().enumerate() {
                let y = panel.y + panel.h * (0.35 + 0.33 * i as f32);
                let (x0, x1) = (panel.x + 12.0, panel.right() - 12.0);
                pen.fill(R::new(x0, y - 1.5, x1 - x0, 3.0), 1.5, Color::hex(0xFFFFFF, 0.25 * a));
                pen.fill(R::new(x0, y - 1.5, (x1 - x0) * level, 3.0), 1.5, p.accent.alpha(a));
            }
        }
    }
    pen.unclip();
}

/// What the music player's taskbar button shows: its own icon, the cover, or the cover and title,
/// with the song's progress along its bottom.
fn player_picture(pen: &Pen, s: &Scene, style: crate::config::TileStyle, r: R, a: f32) {
    use crate::config::TileStyle;
    let p = s.palette;
    pen.clip(r);
    let top = taskbar_strip(pen, r, a);
    let cx = r.x + r.w / 2.0;
    let wide = style == TileStyle::Full;
    let w = if wide { 120.0 } else { 40.0 };
    let button = R::new(cx - w / 2.0, top + 2.0, w, 26.0);
    pen.fill(button, 4.0, Color::hex(0xFFFFFF, 0.1 * a));
    let icon = R::new(button.x + 10.0, button.y + 4.0, 18.0, 18.0);
    match style {
        TileStyle::Off => pen.glyph('\u{EC4F}', Font::Icon, R::new(button.x, button.y, button.w, button.h), Color::hex(0xFF4D4D, a)),
        TileStyle::Cover | TileStyle::Full => {
            pen.fill(icon, 3.0, COVER.alpha(a));
            if wide {
                pen.fill(R::new(icon.right() + 8.0, icon.y + 3.0, 70.0, 4.0), 2.0, Color::hex(0xFFFFFF, 0.85 * a));
                pen.fill(R::new(icon.right() + 8.0, icon.y + 11.0, 46.0, 4.0), 2.0, Color::hex(0xFFFFFF, 0.5 * a));
            }
        }
    }
    // Its progress, as Windows draws a taskbar button's.
    pen.fill(R::new(button.x + 4.0, button.bottom() - 3.0, (button.w - 8.0) * 0.55, 2.0), 1.0, p.accent.alpha(a));
    // Above it, the song: what the button stands for.
    pen.glyph('\u{EC4F}', Font::IconLarge, R::new(r.x, r.y + 8.0, r.w, top - r.y - 16.0), Color::hex(0xFFFFFF, 0.35 * a));
    pen.unclip();
}

/// Home's picture: the pill as it is, its ring moving as its style does, on the left edge.
fn pill_preview(pen: &Pen, s: &Scene, r: R, on: bool) {
    let a = if on { 1.0 } else { 0.45 };
    pen.fill(r, 6.0, SCREEN);
    pen.clip(r);
    let cy = r.y + r.h / 2.0;
    pen.fill(R::new(r.x - 34.0, cy - 64.0, 100.0, 128.0), 33.0, PILL_BG.alpha(a));
    ring_picture(pen, s, s.model.config.motion.ring, (r.x + 32.0, cy - 26.0), 22.0, a);
    pen.circle(r.x + 32.0, cy + 30.0, 16.0, DEVICE_DISC.alpha(a));
    pen.glyph('\u{E767}', Font::IconSmall, R::new(r.x + 24.0, cy + 22.0, 16.0, 16.0), Color::hex(0xFFFFFF, a));
    pen.unclip();
    pen.text("Main display", Font::Caption, R::new(r.x, r.bottom() - 24.0, r.w - 12.0, 18.0), Color::hex(0xD6D6D6, 1.0), Align::Right);
}
