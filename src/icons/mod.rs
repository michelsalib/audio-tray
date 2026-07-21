//! Fixed built-in icon set (plan §3), rendered from **Segoe Fluent Icons** — the same
//! font Windows uses for its own shell icons — so the tray glyph looks native and stays
//! crisp when rendered at the exact display size.
//!
//! Users pick one icon per device in Settings; `default_icon` only chooses the starting
//! glyph until then.

use std::sync::OnceLock;

use ab_glyph::{Font, FontVec};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::audio::FormFactor;

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

    /// Segoe Fluent Icons code point for this icon. The two earbud variants have no
    /// entry: the font ships no earbuds glyph, so [`render`](Self::render) hand-draws
    /// them instead (this returns the headphone glyph only as a harmless fallback).
    fn glyph(self) -> char {
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
    pub fn parse(s: &str) -> Option<IconId> {
        IconId::ALL
            .into_iter()
            .find(|i| format!("{i:?}").eq_ignore_ascii_case(s))
    }

    /// Rasterize this icon into a `size`×`size` RGBA buffer, in colour `rgb` (with
    /// anti-aliased alpha). Rendering at the target size keeps it crisp.
    ///
    /// Every icon is a Segoe Fluent glyph except the two earbud variants, which the font
    /// doesn't provide — those are drawn by [`render_earbuds`] (AirPods-style, with a
    /// stem) and [`render_round_earbuds`] (Sony-style, round and stemless), so each reads
    /// as a distinct pair of buds rather than reusing the headphones glyph.
    pub fn render(self, size: u32, rgb: [u8; 3]) -> Result<(Vec<u8>, u32, u32)> {
        match self {
            IconId::WirelessEarbuds => Ok(render_earbuds(size, rgb)),
            IconId::RoundEarbuds => Ok(render_round_earbuds(size, rgb)),
            _ => render_glyph(self.glyph(), size, rgb),
        }
    }
}

/// A primitive for the hand-drawn earbud icons: a circle, or a capsule (a line segment
/// with a radius — i.e. a stem). Its signed distance is negative inside the shape and ~0
/// on the boundary, which is what [`render_outline`] strokes.
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

/// Stroke half-width for the hand-drawn icons, in normalised units. Tuned to sit at the
/// line weight of the surrounding Segoe Fluent glyphs (≈1 px at the 16–20 px tray/picker
/// sizes) while staying legible without the hinting a real font gets.
const OUTLINE_HW: f32 = 0.030;

/// Rasterize `shapes` as line art into a `size`×`size` RGBA buffer, colour `rgb`: an
/// anti-aliased stroke ([`OUTLINE_HW`] wide) is traced along *each* shape's outline, so
/// overlapping shapes keep the seam between them (a bud reads as sitting on its stem,
/// an ear-tip as fused to its bud). This mirrors the outline look of the font glyphs, and
/// like them is drawn at the target size to stay crisp. 4×4 supersampled for smooth edges.
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

/// Draw a pair of AirPods-style wireless earbuds — a round bud on a slim stem — as line
/// art. Segoe Fluent Icons has no earbuds glyph, so the shape is defined here in
/// normalised coordinates (origin top-left, y down) and stroked by [`render_outline`].
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

/// Draw a pair of round, stemless earbuds (the Sony WF / Galaxy Buds silhouette) as line
/// art: a rounded bud body with a smaller ear-tip fused at its inner-lower edge, the two
/// buds mirrored so the ear-tips face each other. Companion to [`render_earbuds`].
///
/// The `y` values are chosen so the shape's bounding box is vertically centred in the box
/// (body top ≈0.32, ear-tip bottom ≈0.68), matching the vertical placement of the font
/// glyphs and the stem earbuds beside it.
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

/// Rasterize an arbitrary Segoe Fluent glyph into a `size`×`size` RGBA buffer, colour
/// `rgb`, its bounding box centred in the box. Used for the built-in [`IconId`] set and
/// for the flyout's control glyphs (speaker, mic, gear…) so they all align identically.
pub fn render_glyph(glyph: char, size: u32, rgb: [u8; 3]) -> Result<(Vec<u8>, u32, u32)> {
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
        let bytes = std::fs::read(r"C:\Windows\Fonts\SegoeIcons.ttf").ok()?;
        FontVec::try_from_vec(bytes).ok()
    })
    .as_ref()
}

/// Starting glyph for a device before the user assigns one in Settings.
///
/// Form factor is the base (it can't tell earbuds from headphones — plan §2.2), then a
/// friendly-name heuristic refines toward wireless earbuds when the name gives it away.
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
