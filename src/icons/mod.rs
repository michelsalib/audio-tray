//! The built-in icon set, rendered from Segoe Fluent Icons at the exact display size. Users
//! pick one per device; `default_icon` only chooses the starting glyph.

use std::cell::OnceCell;
use std::mem::ManuallyDrop;
use std::sync::OnceLock;

use ab_glyph::{Font, FontVec};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use windows::core::PCWSTR;
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory2, IDWriteFontFace, DWRITE_FACTORY_TYPE_SHARED,
    DWRITE_FONT_FACE_TYPE_TRUETYPE, DWRITE_FONT_SIMULATIONS_NONE, DWRITE_GLYPH_OFFSET,
    DWRITE_GLYPH_RUN, DWRITE_GRID_FIT_MODE_ENABLED, DWRITE_MEASURING_MODE_GDI_NATURAL,
    DWRITE_RENDERING_MODE_GDI_NATURAL, DWRITE_TEXTURE_ALIASED_1x1,
    DWRITE_TEXT_ANTIALIAS_MODE_GRAYSCALE,
};

use crate::audio::FormFactor;

/// Segoe Fluent Icons, the font Windows renders its own shell/tray glyphs from.
const FONT_PATH: &str = r"C:\Windows\Fonts\SegoeIcons.ttf";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum IconId {
    WirelessEarbuds,
    RoundEarbuds,
    Headphones,
    HeadsetMic,
    Microphone,
    Webcam,
    Speakers,
    LaptopSpeakers,
    Hdmi,
    Unknown,
}

impl IconId {
    /// Every icon, in the order shown in the picker.
    pub const ALL: [IconId; 10] = [
        IconId::WirelessEarbuds,
        IconId::RoundEarbuds,
        IconId::Headphones,
        IconId::HeadsetMic,
        IconId::Microphone,
        IconId::Webcam,
        IconId::Speakers,
        IconId::LaptopSpeakers,
        IconId::Hdmi,
        IconId::Unknown,
    ];

    /// Segoe Fluent code point; the earbud variants are hand-drawn and return headphones here.
    pub fn glyph(self) -> char {
        match self {
            IconId::WirelessEarbuds => '\u{E7F6}', // Headphone (no earbuds glyph; see render)
            IconId::RoundEarbuds => '\u{E7F6}',    // Headphone (no earbuds glyph; see render)
            IconId::Headphones => '\u{E7F6}',      // Headphone
            IconId::HeadsetMic => '\u{E95B}',      // Headset (with mic)
            IconId::Microphone => '\u{E720}',      // Microphone
            IconId::Webcam => '\u{E960}',          // Webcam2 (lens in a body on a stand)
            IconId::Speakers => '\u{E767}',        // Volume (speaker + waves)
            IconId::LaptopSpeakers => '\u{E7F8}',  // DeviceLaptopNoPic
            IconId::Hdmi => '\u{E7F4}',            // TVMonitor
            IconId::Unknown => '\u{E9CE}',         // Unknown (? in a circle)
        }
    }

    /// Parse a variant name case-insensitively (e.g. "speakers" -> `Speakers`).
    #[cfg(feature = "dev")]
    pub fn parse(s: &str) -> Option<IconId> {
        IconId::ALL
            .into_iter()
            .find(|i| format!("{i:?}").eq_ignore_ascii_case(s))
    }

    /// Rasterize this icon into a `size`×`size` RGBA buffer in colour `rgb`. The earbud
    /// variants are drawn by [`render_earbuds`] and [`render_round_earbuds`].
    pub fn render(self, size: u32, rgb: [u8; 3]) -> Result<(Vec<u8>, u32, u32)> {
        match self {
            IconId::WirelessEarbuds => Ok(render_earbuds(size, rgb)),
            IconId::RoundEarbuds => Ok(render_round_earbuds(size, rgb)),
            _ => render_glyph(self.glyph(), size, rgb),
        }
    }
}

/// A primitive for the hand-drawn earbuds: a circle or a capsule (a stem).
enum Shape {
    Circle { cx: f32, cy: f32, r: f32 },
    Capsule { ax: f32, ay: f32, bx: f32, by: f32, r: f32 },
}

impl Shape {
    /// Signed distance from `(x, y)` to this shape's boundary (negative inside).
    fn signed_distance(&self, x: f32, y: f32) -> f32 {
        match *self {
            Shape::Circle { cx, cy, r } => ((x - cx).powi(2) + (y - cy).powi(2)).sqrt() - r,
            Shape::Capsule { ax, ay, bx, by, r } => dist_to_segment(x, y, ax, ay, bx, by) - r,
        }
    }
}

/// Stroke half-width for the hand-drawn icons (normalised), matched to Segoe Fluent's weight.
const OUTLINE_HW: f32 = 0.030;

/// Rasterize `shapes` as line art (4×4 supersampled), stroking each outline separately so
/// overlapping shapes keep their seam.
fn render_outline(size: u32, rgb: [u8; 3], shapes: &[Shape]) -> (Vec<u8>, u32, u32) {
    const SS: u32 = 4;
    let s = size as f32;
    let mut buf = vec![0u8; (size * size * 4) as usize];
    for py in 0..size {
        for px in 0..size {
            let mut hits = 0u32;
            for sy in 0..SS {
                for sx in 0..SS {
                    let x = (px as f32 + (sx as f32 + 0.5) / SS as f32) / s;
                    let y = (py as f32 + (sy as f32 + 0.5) / SS as f32) / s;
                    if shapes.iter().any(|sh| sh.signed_distance(x, y).abs() <= OUTLINE_HW) {
                        hits += 1;
                    }
                }
            }
            if hits > 0 {
                let i = ((py * size + px) * 4) as usize;
                buf[i] = rgb[0];
                buf[i + 1] = rgb[1];
                buf[i + 2] = rgb[2];
                buf[i + 3] = (hits as f32 / (SS * SS) as f32 * 255.0).round() as u8;
            }
        }
    }
    (buf, size, size)
}

/// Draw stemmed wireless earbuds as line art (normalised coordinates, y down).
fn render_earbuds(size: u32, rgb: [u8; 3]) -> (Vec<u8>, u32, u32) {
    render_outline(
        size,
        rgb,
        &[
            Shape::Circle { cx: 0.31, cy: 0.27, r: 0.135 },
            Shape::Capsule { ax: 0.31, ay: 0.30, bx: 0.28, by: 0.78, r: 0.055 },
            Shape::Circle { cx: 0.69, cy: 0.27, r: 0.135 },
            Shape::Capsule { ax: 0.69, ay: 0.30, bx: 0.72, by: 0.78, r: 0.055 },
        ],
    )
}

/// Draw round, stemless earbuds (body plus a fused ear-tip, mirrored) as line art, vertically
/// centred like the font glyphs.
fn render_round_earbuds(size: u32, rgb: [u8; 3]) -> (Vec<u8>, u32, u32) {
    render_outline(
        size,
        rgb,
        &[
            Shape::Circle { cx: 0.28, cy: 0.475, r: 0.155 },
            Shape::Circle { cx: 0.37, cy: 0.595, r: 0.085 },
            Shape::Circle { cx: 0.72, cy: 0.475, r: 0.155 },
            Shape::Circle { cx: 0.63, cy: 0.595, r: 0.085 },
        ],
    )
}

/// Euclidean distance from point `(px, py)` to the line segment `(ax, ay)`–`(bx, by)`.
fn dist_to_segment(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    let (abx, aby) = (bx - ax, by - ay);
    let (apx, apy) = (px - ax, py - ay);
    let denom = abx * abx + aby * aby;
    let t = if denom > 0.0 {
        ((apx * abx + apy * aby) / denom).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (cx, cy) = (ax + t * abx, ay + t * aby);
    ((px - cx).powi(2) + (py - cy).powi(2)).sqrt()
}

/// Rasterize a Segoe Fluent glyph into a `size`×`size` RGBA buffer, centred. Uses grid-fitted
/// DirectWrite for crisp tray-size strokes, else [`render_glyph_ab`].
pub fn render_glyph(glyph: char, size: u32, rgb: [u8; 3]) -> Result<(Vec<u8>, u32, u32)> {
    DWRITE.with(|cell| match cell.get_or_init(Dwrite::new) {
        Some(dw) => dw.render(glyph, size, rgb),
        None => render_glyph_ab(glyph, size, rgb),
    })
}

/// A DirectWrite factory and Segoe Fluent face, cached per thread.
struct Dwrite {
    factory: IDWriteFactory2,
    face: IDWriteFontFace,
}

thread_local! {
    static DWRITE: OnceCell<Option<Dwrite>> = const { OnceCell::new() };
}

impl Dwrite {
    fn new() -> Option<Dwrite> {
        unsafe {
            let factory: IDWriteFactory2 = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).ok()?;
            let path = crate::win::wide(FONT_PATH);
            let file = factory.CreateFontFileReference(PCWSTR(path.as_ptr()), None).ok()?;
            let files = [Some(file)];
            let face = factory
                .CreateFontFace(DWRITE_FONT_FACE_TYPE_TRUETYPE, &files, 0, DWRITE_FONT_SIMULATIONS_NONE)
                .ok()?;
            Some(Dwrite { factory, face })
        }
    }

    fn render(&self, glyph: char, size: u32, rgb: [u8; 3]) -> Result<(Vec<u8>, u32, u32)> {
        // Em fraction of the box; leaves a little padding, matching the previous look.
        const FILL: f32 = 0.84;
        let mut buf = vec![0u8; (size * size * 4) as usize];
        unsafe {
            let cp = glyph as u32;
            let mut gi: u16 = 0;
            self.face.GetGlyphIndices(&cp, 1, &mut gi)?;

            let advance = 0.0f32;
            let offset = DWRITE_GLYPH_OFFSET { advanceOffset: 0.0, ascenderOffset: 0.0 };
            let run = DWRITE_GLYPH_RUN {
                // Borrow the cached face without touching its refcount: transmute_copy
                // duplicates the pointer and `ManuallyDrop` never releases it, so the one
                // reference stays owned by `self.face` (which outlives this call).
                fontFace: ManuallyDrop::new(Some(std::mem::transmute_copy(&self.face))),
                fontEmSize: size as f32 * FILL,
                glyphCount: 1,
                glyphIndices: &gi,
                glyphAdvances: &advance,
                glyphOffsets: &offset,
                isSideways: false.into(),
                bidiLevel: 0,
            };

            // GDI-natural + grid-fit + greyscale = the hinted, pixel-snapped rendering the
            // shell uses; greyscale (not ClearType) keeps the alpha texture 1 byte/pixel.
            let analysis = self.factory.CreateGlyphRunAnalysis(
                &run,
                None,
                DWRITE_RENDERING_MODE_GDI_NATURAL,
                DWRITE_MEASURING_MODE_GDI_NATURAL,
                DWRITE_GRID_FIT_MODE_ENABLED,
                DWRITE_TEXT_ANTIALIAS_MODE_GRAYSCALE,
                0.0,
                size as f32 / 2.0,
            )?;
            let bounds = analysis.GetAlphaTextureBounds(DWRITE_TEXTURE_ALIASED_1x1)?;
            let bw = (bounds.right - bounds.left).max(0) as u32;
            let bh = (bounds.bottom - bounds.top).max(0) as u32;
            if bw > 0 && bh > 0 {
                let mut cov = vec![0u8; (bw * bh) as usize];
                analysis.CreateAlphaTexture(DWRITE_TEXTURE_ALIASED_1x1, &bounds, &mut cov)?;
                // Centre the coverage box in the icon; an integer offset keeps the grid-fit
                // alignment (and therefore the crispness) intact.
                let dx = ((size as i32 - bw as i32) / 2).max(0) as u32;
                let dy = ((size as i32 - bh as i32) / 2).max(0) as u32;
                for y in 0..bh {
                    for x in 0..bw {
                        let a = cov[(y * bw + x) as usize];
                        if a == 0 {
                            continue;
                        }
                        let (px, py) = (dx + x, dy + y);
                        if px < size && py < size {
                            let i = ((py * size + px) * 4) as usize;
                            buf[i] = rgb[0];
                            buf[i + 1] = rgb[1];
                            buf[i + 2] = rgb[2];
                            buf[i + 3] = a;
                        }
                    }
                }
            }
        }
        Ok((buf, size, size))
    }
}

/// Fallback glyph rasteriser (unhinted `ab_glyph`) for when DirectWrite cannot initialise.
fn render_glyph_ab(glyph: char, size: u32, rgb: [u8; 3]) -> Result<(Vec<u8>, u32, u32)> {
    let font = fluent_font().context("Segoe Fluent Icons font not found")?;
    let mut buf = vec![0u8; (size * size * 4) as usize];

    // Leave a little padding so the glyph doesn't touch the edges.
    let px = size as f32 * 0.84;
    let g = font.glyph_id(glyph).with_scale(px);

    if let Some(outline) = font.outline_glyph(g) {
        let b = outline.px_bounds();
        let ox = ((size as f32 - b.width()) / 2.0).round() as i32;
        let oy = ((size as f32 - b.height()) / 2.0).round() as i32;
        outline.draw(|gx, gy, coverage| {
            let x = gx as i32 + ox;
            let y = gy as i32 + oy;
            if x >= 0 && y >= 0 && (x as u32) < size && (y as u32) < size {
                let i = ((y as u32 * size + x as u32) * 4) as usize;
                buf[i] = rgb[0];
                buf[i + 1] = rgb[1];
                buf[i + 2] = rgb[2];
                buf[i + 3] = (coverage * 255.0).round().clamp(0.0, 255.0) as u8;
            }
        });
    }
    Ok((buf, size, size))
}

/// Load & cache the Segoe Fluent Icons font (present on Windows 10 1903+/11).
pub(crate) fn fluent_font() -> Option<&'static FontVec> {
    static FONT: OnceLock<Option<FontVec>> = OnceLock::new();
    FONT.get_or_init(|| {
        let bytes = std::fs::read(FONT_PATH).ok()?;
        FontVec::try_from_vec(bytes).ok()
    })
    .as_ref()
}

/// Starting glyph for a device: from the form factor, refined toward earbuds by its name.
pub fn default_icon(form_factor: FormFactor, name_hint: &str) -> IconId {
    let base = match form_factor {
        FormFactor::Speakers => IconId::Speakers,
        FormFactor::Headphones => IconId::Headphones,
        FormFactor::Headset => IconId::HeadsetMic,
        FormFactor::Microphone => IconId::Microphone,
        FormFactor::DigitalDisplay => IconId::Hdmi,
        FormFactor::Spdif => IconId::Speakers, // digital passthrough, usually to speakers
        FormFactor::Unknown => IconId::Unknown,
    };

    let name = name_hint.to_lowercase();
    const EARBUD_HINTS: [&str; 6] = ["buds", "earbud", "airpod", "wf-", "freebuds", "pixel buds"];
    if EARBUD_HINTS.iter().any(|k| name.contains(k)) {
        return IconId::WirelessEarbuds;
    }
    base
}
