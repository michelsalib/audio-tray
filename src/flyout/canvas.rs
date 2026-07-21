//! A tiny straight-alpha RGBA software canvas: the flyout's whole 2D drawing surface.
//!
//! It owns nothing — it borrows a `width`×`height` RGBA byte slice and paints into it —
//! and knows nothing about Win32, the audio model, or the layout. Every primitive is a
//! method on [`Canvas`], so callers no longer thread `(buf, w, h)` through every draw call
//! (which is what forced the old `#[allow(clippy::too_many_arguments)]`s). Because it's
//! pure, its geometry and blending are unit-testable in isolation.

use ab_glyph::{Font, FontVec, PxScale, ScaleFont};

/// An axis-aligned rectangle in pixel space (`x0,y0` top-left → `x1,y1` bottom-right). Lets
/// [`Canvas::fill_round_rect`] take one geometry argument instead of four loose floats.
#[derive(Clone, Copy)]
pub(super) struct Rect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Rect {
    pub(super) fn new(x0: f32, y0: f32, x1: f32, y1: f32) -> Self {
        Rect { x0, y0, x1, y1 }
    }
}

/// A mutable view over a straight-alpha RGBA pixel buffer, `w`×`h`, row-major, 4 bytes per
/// pixel (R, G, B, A). All drawing clips to these bounds.
pub(super) struct Canvas<'a> {
    buf: &'a mut [u8],
    w: i32,
    h: i32,
}

impl<'a> Canvas<'a> {
    pub(super) fn new(buf: &'a mut [u8], w: i32, h: i32) -> Self {
        Canvas { buf, w, h }
    }

    /// Reset every pixel to transparent black.
    pub(super) fn clear(&mut self) {
        for p in self.buf.iter_mut() {
            *p = 0;
        }
    }

    /// Source-over blend of a straight-alpha colour into the buffer.
    fn blend(&mut self, x: i32, y: i32, col: [u8; 3], a: f32) {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return;
        }
        let sa = a.clamp(0.0, 1.0);
        if sa <= 0.0 {
            return;
        }
        let idx = ((y * self.w + x) * 4) as usize;
        let da = self.buf[idx + 3] as f32 / 255.0;
        let out_a = sa + da * (1.0 - sa);
        if out_a <= 0.0 {
            return;
        }
        for (c, &sc) in col.iter().enumerate() {
            let s = sc as f32 / 255.0;
            let d = self.buf[idx + c] as f32 / 255.0;
            let o = (s * sa + d * da * (1.0 - sa)) / out_a;
            self.buf[idx + c] = (o * 255.0).round().clamp(0.0, 255.0) as u8;
        }
        self.buf[idx + 3] = (out_a * 255.0).round().clamp(0.0, 255.0) as u8;
    }

    /// Fill a rounded rectangle (SDF-based, anti-aliased at the edge) with a straight-alpha
    /// colour. `r` is clamped to half the smaller side.
    pub(super) fn fill_round_rect(&mut self, rect: Rect, r: f32, col: [u8; 3], alpha: f32) {
        let Rect { x0, y0, x1, y1 } = rect;
        let cx = (x0 + x1) / 2.0;
        let cy = (y0 + y1) / 2.0;
        let hx = (x1 - x0) / 2.0;
        let hy = (y1 - y0) / 2.0;
        let r = r.min(hx).min(hy);
        for y in y0.floor() as i32..y1.ceil() as i32 {
            for x in x0.floor() as i32..x1.ceil() as i32 {
                let px = x as f32 + 0.5;
                let py = y as f32 + 0.5;
                let qx = (px - cx).abs() - (hx - r);
                let qy = (py - cy).abs() - (hy - r);
                let outside = qx.max(0.0).hypot(qy.max(0.0));
                let inside = qx.max(qy).min(0.0);
                let sd = outside + inside - r;
                let cov = (0.5 - sd).clamp(0.0, 1.0);
                if cov > 0.0 {
                    self.blend(x, y, col, alpha * cov);
                }
            }
        }
    }

    /// Blit a straight-alpha RGBA sprite (its own colour) at `(x0, y0)`, scaling its alpha
    /// by `alpha` (pass `1.0` for an opaque blit, `<1.0` to dim it).
    pub(super) fn blit(&mut self, x0: i32, y0: i32, rgba: &[u8], sw: u32, sh: u32, alpha: f32) {
        for sy in 0..sh as i32 {
            for sx in 0..sw as i32 {
                let i = ((sy as u32 * sw + sx as u32) * 4) as usize;
                let a = rgba[i + 3] as f32 / 255.0 * alpha;
                if a <= 0.0 {
                    continue;
                }
                self.blend(x0 + sx, y0 + sy, [rgba[i], rgba[i + 1], rgba[i + 2]], a);
            }
        }
    }

    /// Copy a same-size (`w`×`h`) page buffer into this canvas shifted horizontally by `dx`
    /// (opaque copy, no blending), clipping to bounds. Used to slide two pre-rendered
    /// screens across each other during a navigation transition.
    pub(super) fn blit_shift(&mut self, page: &[u8], dx: i32) {
        let (w, h) = (self.w, self.h);
        let x_lo = dx.max(0);
        let x_hi = (w + dx).min(w);
        if x_lo >= x_hi {
            return;
        }
        for y in 0..h {
            let row = (y * w) as usize * 4;
            for x in x_lo..x_hi {
                let di = row + x as usize * 4;
                let si = row + (x - dx) as usize * 4;
                self.buf[di..di + 4].copy_from_slice(&page[si..si + 4]);
            }
        }
    }

    /// Draw a text run in `col` at `alpha`, sized to em `px`, starting at pen position
    /// `at` = (pen-x, baseline-y) in px. Advances glyph by glyph.
    pub(super) fn draw_text(&mut self, font: &FontVec, px: f32, at: (f32, f32), col: [u8; 3], alpha: f32, text: &str) {
        let (mut pen, baseline) = at;
        let scale = em_scale(font, px);
        let sf = font.as_scaled(scale);
        for ch in text.chars() {
            let gid = font.glyph_id(ch);
            let glyph = ab_glyph::Glyph { id: gid, scale, position: ab_glyph::point(pen, baseline) };
            if let Some(outline) = font.outline_glyph(glyph) {
                let bb = outline.px_bounds();
                outline.draw(|gx, gy, cov| {
                    let x = bb.min.x as i32 + gx as i32;
                    let y = bb.min.y as i32 + gy as i32;
                    self.blend(x, y, col, cov * alpha);
                });
            }
            pen += sf.h_advance(gid);
        }
    }
}

/// Convert a desired **em size** (in px) into the ab_glyph `PxScale` that actually yields
/// it. ab_glyph scales a font by its *height*, so a plain `PxScale::from(px)` renders an
/// em of only ~0.75·px for Segoe UI. Sizing by em keeps our text matched to Windows.
pub(super) fn em_scale(font: &FontVec, em_px: f32) -> PxScale {
    match font.units_per_em() {
        Some(upem) => PxScale::from(em_px * font.height_unscaled() / upem),
        None => PxScale::from(em_px),
    }
}

/// Total advance width (px) of `text` at em size `px`.
pub(super) fn measure(font: &FontVec, px: f32, text: &str) -> f32 {
    let sf = font.as_scaled(em_scale(font, px));
    text.chars().map(|c| sf.h_advance(font.glyph_id(c))).sum()
}

/// Truncate `text` with a trailing ellipsis so it fits within `max_w` px. Returned as-is
/// when it already fits.
pub(super) fn fit_label(font: &FontVec, px: f32, text: &str, max_w: f32) -> String {
    let sf = font.as_scaled(em_scale(font, px));
    let advance = |c: char| sf.h_advance(font.glyph_id(c));
    if text.chars().map(advance).sum::<f32>() <= max_w {
        return text.to_string();
    }
    let budget = max_w - advance('…');
    let mut out = String::new();
    let mut acc = 0.0;
    for ch in text.chars() {
        let cw = advance(ch);
        if acc + cw > budget {
            break;
        }
        acc += cw;
        out.push(ch);
    }
    out.push('…');
    out
}

/// Linear interpolation between two RGB colours (`t` clamped to 0..=1). Used to lighten
/// the slider fill toward white as live audio activity rises.
pub(super) fn lerp3(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    [l(a[0], b[0]), l(a[1], b[1]), l(a[2], b[2])]
}
