//! DirectWrite text shared by the Direct2D painters (the pill, the calendar's panels, Settings):
//! making a text format from a font's description, and measuring a line.

use anyhow::Result;
use windows::core::PCWSTR;
use windows::Win32::Graphics::DirectWrite::{
    IDWriteFactory, IDWriteTextFormat, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT,
    DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_PARAGRAPH_ALIGNMENT_NEAR, DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_METRICS,
    DWRITE_TRIMMING, DWRITE_TRIMMING_GRANULARITY_CHARACTER, DWRITE_WORD_WRAPPING_NO_WRAP, DWRITE_WORD_WRAPPING_WRAP,
};

/// How a format lays its text out in a box.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Flow {
    /// One line, centred top to bottom, cut with "…" when too long.
    Line,
    /// One line, centred both ways and never cut: a glyph, a letter, a badge.
    Centred,
    /// Lines wrapping from the top, the last cut with "…".
    Wrap,
}

/// A font as a painter describes it: its family, weight, size (DIPs) and flow.
#[derive(Clone, Copy)]
pub struct Spec {
    pub family: PCWSTR,
    pub weight: DWRITE_FONT_WEIGHT,
    pub size: f32,
    pub flow: Flow,
}

/// The text format for `spec`, in `locale` (empty: the user's).
pub fn format(dwrite: &IDWriteFactory, spec: Spec, locale: PCWSTR) -> Result<IDWriteTextFormat> {
    unsafe {
        let format = dwrite.CreateTextFormat(spec.family, None, spec.weight, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, spec.size, locale)?;
        if spec.flow == Flow::Wrap {
            format.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP)?;
            format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR)?;
        } else {
            format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
        }
        if spec.flow == Flow::Centred {
            format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
        } else {
            let sign = dwrite.CreateEllipsisTrimmingSign(&format)?;
            let trimming = DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 };
            format.SetTrimming(&trimming, &sign)?;
        }
        Ok(format)
    }
}

/// The width of `text` on one line in `format`, in DIPs (0 when it cannot be laid out).
pub fn width(dwrite: &IDWriteFactory, format: &IDWriteTextFormat, text: &str) -> f32 {
    let wide: Vec<u16> = text.encode_utf16().collect();
    unsafe {
        let Ok(layout) = dwrite.CreateTextLayout(&wide, format, 4096.0, 100.0) else {
            return 0.0;
        };
        let mut metrics = DWRITE_TEXT_METRICS::default();
        if layout.GetMetrics(&mut metrics).is_err() {
            return 0.0;
        }
        metrics.widthIncludingTrailingWhitespace
    }
}
