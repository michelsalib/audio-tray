//! Drawing the calendar with Direct2D and DirectWrite, in Windows 11's flyout colours.

use anyhow::{Context, Result};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct2D::Common::{D2D1_ALPHA_MODE_IGNORE, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_RECT_F, D2D_SIZE_U};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Factory, ID2D1HwndRenderTarget, ID2D1SolidColorBrush, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
    D2D1_DRAW_TEXT_OPTIONS_CLIP, D2D1_ELLIPSE, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_HWND_RENDER_TARGET_PROPERTIES,
    D2D1_PRESENT_OPTIONS_NONE, D2D1_RENDER_TARGET_PROPERTIES, D2D1_ROUNDED_RECT,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat, DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL,
    DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD,
    DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT, DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING,
    DWRITE_TRIMMING, DWRITE_TRIMMING_GRANULARITY_CHARACTER, DWRITE_WORD_WRAPPING_NO_WRAP,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows_numerics::Vector2;

use super::layout::R;

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
    pub bg: Color,
    pub rule: Color,
    pub text: Color,
    pub text2: Color,
    pub text3: Color,
    pub hover: Color,
    pub pressed: Color,
    pub accent: Color,
    pub on_accent: Color,
    pub warn: Color,
}

impl Palette {
    pub fn current() -> Self {
        let light = crate::win::hkcu_dword(w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"), w!("SystemUsesLightTheme")) == Some(1);
        #[cfg(feature = "dev")]
        let light = std::env::var("AUDIO_TRAY_THEME").map_or(light, |t| t == "light");
        if light {
            Palette {
                bg: Color::hex(0xEEEEEE, 1.0),
                rule: Color::hex(0x000000, 0.08),
                text: Color::hex(0x000000, 0.896),
                text2: Color::hex(0x000000, 0.606),
                text3: Color::hex(0x000000, 0.361),
                hover: Color::hex(0x000000, 0.045),
                pressed: Color::hex(0x000000, 0.03),
                accent: Color::rgb(crate::flyout::theme::accent_shade(4).unwrap_or([0x00, 0x5F, 0xB8])),
                on_accent: Color::hex(0xFFFFFF, 1.0),
                warn: Color::hex(0x9D5D00, 1.0),
            }
        } else {
            Palette {
                bg: Color::hex(0x2B2B2B, 1.0),
                rule: Color::hex(0x000000, 0.25),
                text: Color::hex(0xFFFFFF, 1.0),
                text2: Color::hex(0xFFFFFF, 0.786),
                text3: Color::hex(0xFFFFFF, 0.362),
                hover: Color::hex(0xFFFFFF, 0.061),
                pressed: Color::hex(0xFFFFFF, 0.042),
                accent: Color::rgb(crate::flyout::theme::accent_rgb()),
                on_accent: Color::hex(0x000000, 1.0),
                warn: Color::hex(0xF7B32B, 1.0),
            }
        }
    }

    pub fn dark(&self) -> bool {
        self.bg.0 < 0.5
    }
}

#[derive(Clone, Copy)]
pub enum Font {
    /// The day's date at the top.
    Header,
    Body,
    Strong,
    Caption,
    CaptionStrong,
    Icon,
    IconSmall,
}

#[derive(Clone, Copy)]
pub enum Align {
    Left,
    Center,
}

pub struct Gfx {
    factory: ID2D1Factory,
    dwrite: IDWriteFactory,
    formats: Vec<IDWriteTextFormat>,
    target: Option<ID2D1HwndRenderTarget>,
}

impl Gfx {
    pub fn new() -> Result<Self> {
        unsafe {
            let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).context("D2D1CreateFactory")?;
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).context("DWriteCreateFactory")?;
            let mut gfx = Gfx { factory, dwrite, formats: Vec::new(), target: None };
            let text = w!("Segoe UI Variable Text");
            let icons = w!("Segoe Fluent Icons");
            gfx.formats = vec![
                gfx.format(text, DWRITE_FONT_WEIGHT_NORMAL, 15.0)?,
                gfx.format(text, DWRITE_FONT_WEIGHT_NORMAL, 14.0)?,
                gfx.format(text, DWRITE_FONT_WEIGHT_SEMI_BOLD, 14.0)?,
                gfx.format(text, DWRITE_FONT_WEIGHT_NORMAL, 12.0)?,
                gfx.format(text, DWRITE_FONT_WEIGHT_SEMI_BOLD, 12.0)?,
                gfx.format(icons, DWRITE_FONT_WEIGHT_NORMAL, 16.0)?,
                gfx.format(icons, DWRITE_FONT_WEIGHT_NORMAL, 10.0)?,
            ];
            Ok(gfx)
        }
    }

    fn format(&self, family: PCWSTR, weight: DWRITE_FONT_WEIGHT, size: f32) -> Result<IDWriteTextFormat> {
        unsafe {
            let format = self.dwrite.CreateTextFormat(family, None, weight, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, size, w!(""))?;
            format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            let sign = self.dwrite.CreateEllipsisTrimmingSign(&format)?;
            let trimming = DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 };
            format.SetTrimming(&trimming, &sign)?;
            Ok(format)
        }
    }

    /// Draw a frame into `hwnd` (`size` in pixels) at `scale` pixels per DIP.
    pub fn frame(&mut self, hwnd: HWND, size: (u32, u32), scale: f32, draw: impl FnOnce(&Pen)) -> Result<()> {
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
            }
            let target = self.target.clone().expect("just built");
            target.SetDpi(96.0 * scale, 96.0 * scale);
            target.BeginDraw();
            draw(&Pen { gfx: self, target: target.clone() });
            if let Err(e) = target.EndDraw(None, None) {
                self.target = None;
                // D2DERR_RECREATE_TARGET: the next frame builds a new one.
                if e.code().0 as u32 != 0x8899_000C {
                    return Err(e).context("EndDraw");
                }
            }
        }
        Ok(())
    }
}

pub struct Pen<'a> {
    gfx: &'a Gfx,
    target: ID2D1HwndRenderTarget,
}

fn rect(r: R) -> D2D_RECT_F {
    D2D_RECT_F { left: r.x, top: r.y, right: r.x + r.w, bottom: r.y + r.h }
}

impl Pen<'_> {
    fn brush(&self, c: Color) -> Option<ID2D1SolidColorBrush> {
        unsafe { self.target.CreateSolidColorBrush(&D2D1_COLOR_F { r: c.0, g: c.1, b: c.2, a: c.3 }, None) }.ok()
    }

    pub fn clear(&self, c: Color) {
        unsafe { self.target.Clear(Some(&D2D1_COLOR_F { r: c.0, g: c.1, b: c.2, a: 1.0 })) };
    }

    pub fn fill(&self, r: R, radius: f32, c: Color) {
        if let Some(brush) = self.brush(c) {
            let rounded = D2D1_ROUNDED_RECT { rect: rect(r), radiusX: radius, radiusY: radius };
            unsafe { self.target.FillRoundedRectangle(&rounded, &brush) };
        }
    }

    pub fn circle(&self, (cx, cy): (f32, f32), radius: f32, c: Color) {
        if let Some(brush) = self.brush(c) {
            let e = D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: radius, radiusY: radius };
            unsafe { self.target.FillEllipse(&e, &brush) };
        }
    }

    pub fn ring(&self, (cx, cy): (f32, f32), radius: f32, width: f32, c: Color) {
        if let Some(brush) = self.brush(c) {
            let e = D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: radius - width / 2.0, radiusY: radius - width / 2.0 };
            unsafe { self.target.DrawEllipse(&e, &brush, width, None) };
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
            };
            let _ = layout.SetTextAlignment(a);
            self.target.DrawTextLayout(Vector2 { X: r.x, Y: r.y }, &layout, &brush, D2D1_DRAW_TEXT_OPTIONS_CLIP);
        }
    }

    pub fn glyph(&self, glyph: char, font: Font, r: R, c: Color) {
        self.text(&glyph.to_string(), font, r, c, Align::Center);
    }

    pub fn clip(&self, r: R) {
        unsafe { self.target.PushAxisAlignedClip(&rect(r), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE) };
    }

    pub fn unclip(&self) {
        unsafe { self.target.PopAxisAlignedClip() };
    }
}
