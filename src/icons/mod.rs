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
    Headphones,
    HeadsetMic,
    Microphone,
    Speakers,
    LaptopSpeakers,
    Hdmi,
    Unknown,
}

impl IconId {
    /// Every icon, in the order shown in the picker.
    pub const ALL: [IconId; 8] = [
        IconId::WirelessEarbuds,
        IconId::Headphones,
        IconId::HeadsetMic,
        IconId::Microphone,
        IconId::Speakers,
        IconId::LaptopSpeakers,
        IconId::Hdmi,
        IconId::Unknown,
    ];

    /// Segoe Fluent Icons code point for this icon. (There is no earbuds glyph in the
    /// font — Windows itself shows headphones for earbuds — so we reuse it.)
    fn glyph(self) -> char {
        match self {
            IconId::WirelessEarbuds => '\u{E7F6}', // Headphone (no earbuds glyph exists)
            IconId::Headphones => '\u{E7F6}',      // Headphone
            IconId::HeadsetMic => '\u{E95B}',      // Headset (with mic)
            IconId::Microphone => '\u{E720}',      // Microphone
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

    /// Rasterize this icon's glyph into a `size`×`size` RGBA buffer, in colour `rgb`
    /// (with anti-aliased alpha). Rendering at the target size keeps it crisp.
    pub fn render(self, size: u32, rgb: [u8; 3]) -> Result<(Vec<u8>, u32, u32)> {
        render_glyph(self.glyph(), size, rgb)
    }
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
