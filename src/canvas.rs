//! A tiny straight-alpha RGBA software canvas over a borrowed buffer, used by the flyout and
//! the OSD. No Win32 here; [`crate::layered`] puts the buffer on screen.

use ab_glyph::{Font, FontVec, PxScale, ScaleFont};

/// An axis-aligned rectangle in pixel space (`x0,y0` top-left → `x1,y1` bottom-right).
#[derive(Clone, Copy)]
pub(crate) struct Rect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Rect {
    pub(crate) fn new(x0: f32, y0: f32, x1: f32, y1: f32) -> Self {
        Rect { x0, y0, x1, y1 }
    }
}

/// A mutable view over a straight-alpha RGBA pixel buffer, `w`×`h`, row-major, 4 bytes per
/// pixel (R, G, B, A). All drawing clips to these bounds.
pub(crate) struct Canvas<'a> {
    buf: &'a mut [u8],
    w: i32,
    h: i32,
}

impl<'a> Canvas<'a> {
    pub(crate) fn new(buf: &'a mut [u8], w: i32, h: i32) -> Self {
        Canvas { buf, w, h }
    }

    /// Reset every pixel to transparent black.
    pub(crate) fn clear(&mut self) {
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
    pub(crate) fn fill_round_rect(&mut self, rect: Rect, r: f32, col: [u8; 3], alpha: f32) {
        self.fill_rounded(rect, r, col, alpha, false);
    }

    /// Fill a rectangle with only its bottom corners rounded (a strip closing a rounded panel).
    pub(crate) fn fill_round_rect_bottom(&mut self, rect: Rect, r: f32, col: [u8; 3], alpha: f32) {
        self.fill_rounded(rect, r, col, alpha, true);
    }

    /// The rounded-rectangle SDF both fills share. `bottom_only` drops the vertical mirror and
    /// intersects with the half-plane below the top edge to restore that boundary.
    fn fill_rounded(&mut self, rect: Rect, r: f32, col: [u8; 3], alpha: f32, bottom_only: bool) {
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
                let dy = py - cy;
                let qy = if bottom_only { dy } else { dy.abs() } - (hy - r);
                let outside = qx.max(0.0).hypot(qy.max(0.0));
                let inside = qx.max(qy).min(0.0);
                let mut sd = outside + inside - r;
                if bottom_only {
                    sd = sd.max(y0 - py);
                }
                let cov = (0.5 - sd).clamp(0.0, 1.0);
                if cov > 0.0 {
                    self.blend(x, y, col, alpha * cov);
                }
            }
        }
    }

    /// Blit a straight-alpha RGBA sprite (its own colour) at `(x0, y0)`, scaling its alpha
    /// by `alpha` (pass `1.0` for an opaque blit, `<1.0` to dim it).
    pub(crate) fn blit(&mut self, x0: i32, y0: i32, rgba: &[u8], sw: u32, sh: u32, alpha: f32) {
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

    /// Draw a text run in `col` at `alpha`, sized to em `px`, starting at pen position
    /// `at` = (pen-x, baseline-y) in px. Advances glyph by glyph.
    /// Characters `font` lacks come from a fallback ([`resolve`]).
    pub(crate) fn draw_text(&mut self, font: &FontVec, px: f32, at: (f32, f32), col: [u8; 3], alpha: f32, text: &str) {
        let (mut pen, baseline) = at;
        for ch in text.chars() {
            let Some((face, gid)) = resolve(font, ch) else { continue };
            let scale = em_scale(face, px);
            let glyph = ab_glyph::Glyph { id: gid, scale, position: ab_glyph::point(pen, baseline) };
            if let Some(outline) = face.outline_glyph(glyph) {
                let bb = outline.px_bounds();
                outline.draw(|gx, gy, cov| {
                    let x = bb.min.x as i32 + gx as i32;
                    let y = bb.min.y as i32 + gy as i32;
                    self.blend(x, y, col, cov * alpha);
                });
            }
            pen += face.as_scaled(scale).h_advance(gid);
        }
    }
}

/// The face and glyph to draw `ch` with: `font` when it has it, else the first fallback for the
/// character's script that does. `None` for a character no font has, and for the invisible ones
/// that only compose emoji (a joiner, a variation selector, a skin tone), which drawn one glyph
/// at a time would show as stray pieces: skipped, never a box.
fn resolve(font: &FontVec, ch: char) -> Option<(&FontVec, ab_glyph::GlyphId)> {
    if composes_emoji(ch) {
        return None;
    }
    let gid = font.glyph_id(ch);
    if gid.0 != 0 {
        return Some((font, gid));
    }
    fallbacks_for(ch).iter().filter_map(|&file| fallback(file)).find_map(|face| {
        let gid = face.glyph_id(ch);
        (gid.0 != 0).then_some((face, gid))
    })
}

fn composes_emoji(ch: char) -> bool {
    matches!(ch,
        '\u{200D}' // zero-width joiner
        | '\u{FE0E}' | '\u{FE0F}' // text / emoji presentation
        | '\u{1F3FB}'..='\u{1F3FF}' // skin tones
        | '\u{E0020}'..='\u{E007F}' // tag characters (subdivision flags)
        | '\u{20E3}' // combining keycap
    )
}

/// The system fonts to try for `ch`, by script, so a title's emoji never loads a 20 MB CJK font.
/// Latin, Greek, Cyrillic, Hebrew and Arabic are Segoe UI's own (Arabic unjoined: no shaping).
fn fallbacks_for(ch: char) -> &'static [&'static str] {
    match ch as u32 {
        0x1100..=0x11FF | 0x3130..=0x318F | 0xA960..=0xA97F | 0xAC00..=0xD7FF => &["malgun.ttf"],
        0x3040..=0x30FF | 0x31F0..=0x31FF | 0xFF65..=0xFF9F => &["YuGothM.ttc", "msyh.ttc"],
        0x2E80..=0x2FDF | 0x3000..=0x303F | 0x3100..=0x312F | 0x3190..=0x31EF | 0x3200..=0x9FFF | 0xF900..=0xFAFF
        | 0xFE30..=0xFE4F | 0xFF00..=0xFFEF | 0x20000..=0x3FFFF => &["msyh.ttc", "YuGothM.ttc"],
        0x0900..=0x0DFF => &["Nirmala.ttc"],
        // Emoji (Segoe UI Emoji's one-colour outlines, in the text colour), then other symbols.
        _ => &["seguiemj.ttf", "seguisym.ttf"],
    }
}

/// A fallback font from the Windows fonts folder (the first face of a collection), loaded on
/// first use and kept; `None` if it is missing.
fn fallback(file: &'static str) -> Option<&'static FontVec> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static FONTS: OnceLock<Mutex<HashMap<&'static str, Option<&'static FontVec>>>> = OnceLock::new();
    let mut fonts = FONTS.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    *fonts.entry(file).or_insert_with(|| {
        let bytes = std::fs::read(format!(r"C:\Windows\Fonts\{file}")).ok()?;
        // Leaked on purpose: loaded once per process, at most a handful, and drawn from any thread.
        FontVec::try_from_vec_and_index(bytes, 0).ok().map(|f| &*Box::leak(Box::new(f)))
    })
}

/// The advance of `ch` at em `px`, from the face [`resolve`] draws it with (0 if skipped).
fn advance(font: &FontVec, px: f32, ch: char) -> f32 {
    resolve(font, ch).map_or(0.0, |(face, gid)| face.as_scaled(em_scale(face, px)).h_advance(gid))
}

/// Convert an em size (px) into the ab_glyph `PxScale` that yields it; ab_glyph scales by
/// height, so `PxScale::from(px)` would give Segoe UI an em of ~0.75·px.
pub(crate) fn em_scale(font: &FontVec, em_px: f32) -> PxScale {
    match font.units_per_em() {
        Some(upem) => PxScale::from(em_px * font.height_unscaled() / upem),
        None => PxScale::from(em_px),
    }
}

/// Total advance width (px) of `text` at em size `px`.
pub(crate) fn measure(font: &FontVec, px: f32, text: &str) -> f32 {
    text.chars().map(|c| advance(font, px, c)).sum()
}

/// Truncate `text` with a trailing ellipsis so it fits within `max_w` px. Returned as-is
/// when it already fits.
pub(crate) fn fit_label(font: &FontVec, px: f32, text: &str, max_w: f32) -> String {
    let advance = |c: char| advance(font, px, c);
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

/// Linear interpolation between two RGB colours (`t` clamped to 0..=1).
pub(crate) fn lerp3(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    [l(a[0], b[0]), l(a[1], b[1]), l(a[2], b[2])]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segoe() -> FontVec {
        FontVec::try_from_vec(std::fs::read(r"C:\Windows\Fonts\segoeui.ttf").expect("Segoe UI")).unwrap()
    }

    /// What Segoe UI lacks is drawn from a fallback, not as a box.
    #[test]
    fn emoji_and_cjk_come_from_a_fallback() {
        let font = segoe();
        let segoe_ptr = &font as *const FontVec;
        for ch in ['🔥', '♫', '漢', 'の', '한'] {
            let (face, gid) = resolve(&font, ch).unwrap_or_else(|| panic!("{ch} has no font"));
            assert_ne!(face as *const FontVec, segoe_ptr, "{ch} is Segoe UI's own?");
            let glyph = gid.with_scale_and_position(em_scale(face, 14.0), ab_glyph::point(0.0, 14.0));
            assert!(face.outline_glyph(glyph).is_some(), "{ch} has no outline to draw");
        }
        assert!(std::ptr::eq(resolve(&font, 'é').unwrap().0, &font), "Segoe UI's own stay its own");
    }

    /// The pieces that only compose an emoji, and characters no font has, take no room.
    #[test]
    fn invisible_and_unknown_characters_are_skipped() {
        let font = segoe();
        for ch in ['\u{200D}', '\u{FE0F}', '\u{1F3FD}', '\u{10FFFD}'] {
            assert!(resolve(&font, ch).is_none(), "{:X} drawn", ch as u32);
        }
        let family = "👨\u{200D}👩\u{200D}👧";
        assert_eq!(measure(&font, 14.0, family), 3.0 * measure(&font, 14.0, "👨"));
        assert!(measure(&font, 14.0, "🔥 SPEEDRUN") > measure(&font, 14.0, " SPEEDRUN"));
    }
}
