//! Audio Tray's own battery icon (a PC without a battery): one picture for every device, drawn at
//! the tray's exact pixel size so each edge lands on the pixel grid. Either each device (up to
//! three, the lowest first) is a bar, a ring or a bubble filled to its level in its tone's colour
//! over a faint track, or the lowest one alone is a battery or its figures, with a dot per device
//! under it.

use crate::batteries::Tone;

/// How the devices are drawn (Settings' choice, `[batteries] icon_style`).
pub use crate::config::BatteryIconStyle as Style;

/// At most this many devices are drawn (the lowest ones).
pub const MAX: usize = 3;

/// The picture, RGBA `size`×`size`: `levels` (lowest first) with their tones, on a light taskbar
/// or a dark one (the tracks take the taskbar's ink).
pub fn render(style: Style, levels: &[(u8, Tone)], size: u32, light: bool) -> Vec<u8> {
    let mut canvas = Canvas::new(size);
    let ink = if light { [0x20, 0x20, 0x20] } else { [0xff, 0xff, 0xff] };
    let shown = &levels[..levels.len().min(MAX)];
    match style {
        Style::Bars => bars(&mut canvas, shown, ink, light),
        Style::Rings => rings(&mut canvas, shown, ink, light),
        Style::Bubbles => bubbles(&mut canvas, shown, ink, light),
        Style::Battery => battery(&mut canvas, shown, ink, light),
        Style::Number => {
            if !number(&mut canvas, shown, light) {
                // No font to write it with: the battery says the same.
                battery(&mut canvas, shown, ink, light);
            }
        }
    }
    canvas.pixels
}

/// A level as a fraction, never quite empty: an empty bar still shows its colour.
fn fraction(level: u8) -> f32 {
    (level.min(100) as f32 / 100.0).max(crate::batteries::MIN_SHOWN)
}

/// A colour and its opacity.
type Paint = ([u8; 3], f32);

/// The track's opacity under a level.
const TRACK: f32 = 0.32;

fn bars(c: &mut Canvas, levels: &[(u8, Tone)], ink: [u8; 3], light: bool) {
    let u = c.size as f32 / 24.0;
    let n = levels.len().max(1) as f32;
    // Thicker bars when fewer: one fills like a battery, three still read apart.
    let (h, gap) = match levels.len() {
        0 | 1 => (7.0 * u, 0.0),
        2 => (5.0 * u, 3.0 * u),
        _ => (4.0 * u, 2.5 * u),
    };
    let total = n * h + (n - 1.0) * gap;
    // Whole pixels for each bar's top, so its edges are sharp.
    let mut y = ((c.size as f32 - total) / 2.0).round();
    let (x0, w) = (2.0 * u, c.size as f32 - 4.0 * u);
    for &(level, tone) in levels {
        let top = y.round();
        c.round_rect(x0, top, w, h, h / 2.0, (ink, TRACK));
        c.round_rect(x0, top, w * fraction(level), h, h / 2.0, (tone.rgb(light), 1.0));
        y += h + gap;
    }
}

fn rings(c: &mut Canvas, levels: &[(u8, Tone)], ink: [u8; 3], light: bool) {
    let u = c.size as f32 / 24.0;
    let centre = c.size as f32 / 2.0;
    let (stroke, gap) = match levels.len() {
        0 | 1 => (3.5 * u, 0.0),
        2 => (3.0 * u, 1.5 * u),
        _ => (2.5 * u, 1.25 * u),
    };
    let mut radius = 10.75 * u - stroke / 2.0;
    for &(level, tone) in levels {
        c.arc(centre, centre, radius, stroke, 1.0, (ink, TRACK));
        c.arc(centre, centre, radius, stroke, fraction(level), (tone.rgb(light), 1.0));
        radius -= stroke + gap;
    }
}

/// One circle per device, filled from the bottom to its level like a glass: one large, two side by
/// side, three in a cluster.
fn bubbles(c: &mut Canvas, levels: &[(u8, Tone)], ink: [u8; 3], light: bool) {
    let u = c.size as f32 / 24.0;
    // Centre and radius, on the 24-unit grid.
    let places: &[(f32, f32, f32)] = match levels.len() {
        0 | 1 => &[(12.0, 12.0, 9.5)],
        2 => &[(6.0, 12.0, 5.0), (18.0, 12.0, 5.0)],
        _ => &[(12.0, 6.25, 5.25), (6.25, 16.75, 5.25), (17.75, 16.75, 5.25)],
    };
    let rim = (1.25 * u).round().max(1.0);
    for (&(level, tone), &(x, y, r)) in levels.iter().zip(places) {
        let (cx, cy, r) = (x * u, y * u, r * u);
        let inner = r - rim;
        c.disc(cx, cy, inner, (ink, TRACK * 0.5));
        let top = cy + inner - 2.0 * inner * fraction(level);
        let rgb = tone.rgb(light);
        c.fill(rgb, 1.0, |px, py| {
            let disc = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt() - inner;
            disc.max(top - py)
        });
        c.fill(ink, 0.6, |px, py| (((px - cx).powi(2) + (py - cy).powi(2)).sqrt() - (r - rim / 2.0)).abs() - rim / 2.0);
    }
}

/// The dots' diameter and the gap between them, in whole pixels.
fn dot_size(u: f32) -> (f32, f32) {
    ((3.5 * u).round().max(2.0), (1.5 * u).round().max(1.0))
}

/// How tall the dots' row is with the space above it (nothing for one device: no dots).
fn dots_room(levels: &[(u8, Tone)], u: f32) -> f32 {
    if levels.len() < 2 {
        return 0.0;
    }
    dot_size(u).0 + (2.0 * u).round().max(1.0)
}

/// A dot per device in its tone, a centred row whose top is `top`; none for one device.
fn dots(c: &mut Canvas, levels: &[(u8, Tone)], top: f32, light: bool) {
    if levels.len() < 2 {
        return;
    }
    let u = c.size as f32 / 24.0;
    let (d, gap) = dot_size(u);
    let n = levels.len() as f32;
    let x0 = ((c.size as f32 - (n * d + (n - 1.0) * gap)) / 2.0).round();
    for (i, &(_, tone)) in levels.iter().enumerate() {
        c.disc(x0 + i as f32 * (d + gap) + d / 2.0, top + d / 2.0, d / 2.0, (tone.rgb(light), 1.0));
    }
}

/// One battery, outlined in the taskbar's ink and filled to the lowest level in its tone, the dots
/// under it. Every edge on whole pixels.
fn battery(c: &mut Canvas, levels: &[(u8, Tone)], ink: [u8; 3], light: bool) {
    let Some(&(level, tone)) = levels.first() else { return };
    let s = c.size as f32;
    let u = s / 24.0;
    let line = (1.25 * u).round().max(1.0);
    let nub = (2.0 * u).round().max(1.0);
    let (x, h) = ((2.0 * u).round(), (10.0 * u).round());
    let w = s - (1.0 * u).round() - nub - x;
    let below = dots_room(levels, u);
    let y = ((s - h - below) / 2.0).round();
    c.ring_rect([x, y, w, h], 2.5 * u, line, (ink, 1.0));
    let inset = (h * 0.3).round();
    c.round_rect(x + w - 1.0, y + inset, nub + 1.0, h - 2.0 * inset, 0.75 * u, (ink, 1.0));
    let pad = line + (1.0 * u).round().max(1.0);
    let (fw, fh) = (w - 2.0 * pad, h - 2.0 * pad);
    c.round_rect(x + pad, y + pad, fw * fraction(level), fh, (0.75 * u).min(fh / 2.0), (tone.rgb(light), 1.0));
    dots(c, levels, y + h + below - dot_size(u).0, light);
}

/// The lowest level in figures, in its tone, the dots under it; false when no font could write it.
fn number(c: &mut Canvas, levels: &[(u8, Tone)], light: bool) -> bool {
    let Some(&(level, tone)) = levels.first() else { return true };
    let s = c.size as f32;
    let u = s / 24.0;
    let text = level.min(100).to_string();
    let below = dots_room(levels, u);
    // As large as fits, a pixel clear of each side.
    let (room_w, room_h) = (s - 2.0, s - 2.0 - below);
    let mut em = s * if below > 0.0 { 0.7 } else { 0.85 };
    let fit = loop {
        if em < 5.0 {
            return false;
        }
        match crate::icons::text_coverage(&text, em) {
            Some((cov, w, h)) if w as f32 <= room_w && h as f32 <= room_h => break (cov, w, h),
            Some(_) => em -= 1.0,
            None => return false,
        }
    };
    let (cov, w, h) = fit;
    let x0 = ((s - w as f32) / 2.0).round() as u32;
    let y0 = ((s - h as f32 - below) / 2.0).round() as u32;
    let rgb = tone.rgb(light);
    for y in 0..h {
        for x in 0..w {
            let a = cov[(y * w + x) as usize];
            if a > 0 && x0 + x < c.size && y0 + y < c.size {
                c.blend(x0 + x, y0 + y, rgb, 1.0, a as f32 / 255.0);
            }
        }
    }
    dots(c, levels, y0 as f32 + h as f32 + below - dot_size(u).0, light);
    true
}

/// A square RGBA picture, drawn into with coverage (anti-aliased) and blended over.
struct Canvas {
    size: u32,
    pixels: Vec<u8>,
}

impl Canvas {
    fn new(size: u32) -> Self {
        Canvas { size, pixels: vec![0; (size * size * 4) as usize] }
    }

    /// `rgb` at `opacity` over pixel `(x, y)`, `cover` of it covered.
    fn blend(&mut self, x: u32, y: u32, rgb: [u8; 3], opacity: f32, cover: f32) {
        let a = (cover * opacity).clamp(0.0, 1.0);
        if a <= 0.0 {
            return;
        }
        let i = ((y * self.size + x) * 4) as usize;
        let below = self.pixels[i + 3] as f32 / 255.0;
        let out = a + below * (1.0 - a);
        for (channel, value) in self.pixels[i..i + 3].iter_mut().zip(rgb) {
            *channel = ((value as f32 * a + *channel as f32 * below * (1.0 - a)) / out).round() as u8;
        }
        self.pixels[i + 3] = (out * 255.0).round() as u8;
    }

    /// Each pixel's coverage by `inside(px, py) -> signed distance` (negative inside), sampled at
    /// its centre and smoothed over one pixel.
    fn fill(&mut self, rgb: [u8; 3], opacity: f32, distance: impl Fn(f32, f32) -> f32) {
        for y in 0..self.size {
            for x in 0..self.size {
                let d = distance(x as f32 + 0.5, y as f32 + 0.5);
                let cover = (0.5 - d).clamp(0.0, 1.0);
                if cover > 0.0 {
                    self.blend(x, y, rgb, opacity, cover);
                }
            }
        }
    }

    fn round_rect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, (rgb, opacity): Paint) {
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        self.fill(rgb, opacity, |px, py| rect_distance(px, py, x, y, w, h, r));
    }

    /// A rounded rectangle's outline (`[x, y, w, h]`), `line` wide inside its edge.
    fn ring_rect(&mut self, [x, y, w, h]: [f32; 4], r: f32, line: f32, (rgb, opacity): Paint) {
        if w <= 2.0 * line || h <= 2.0 * line {
            return;
        }
        let inner = (r - line).max(0.0);
        self.fill(rgb, opacity, |px, py| {
            let outside = rect_distance(px, py, x, y, w, h, r);
            let hole = rect_distance(px, py, x + line, y + line, w - 2.0 * line, h - 2.0 * line, inner);
            outside.max(-hole)
        });
    }

    fn disc(&mut self, cx: f32, cy: f32, r: f32, (rgb, opacity): Paint) {
        self.fill(rgb, opacity, |px, py| ((px - cx).powi(2) + (py - cy).powi(2)).sqrt() - r);
    }

    /// A ring of `radius` (its stroke's middle), `stroke` wide, drawn clockwise from the top for
    /// `portion` of a turn, with round ends.
    fn arc(&mut self, cx: f32, cy: f32, radius: f32, stroke: f32, portion: f32, (rgb, opacity): Paint) {
        use std::f32::consts::TAU;
        let sweep = portion.clamp(0.0, 1.0) * TAU;
        let half = stroke / 2.0;
        // The ends' centres (the start at the top).
        let at = |angle: f32| (cx + radius * angle.sin(), cy - radius * angle.cos());
        let (sx, sy) = at(0.0);
        let (ex, ey) = at(sweep);
        self.fill(rgb, opacity, |px, py| {
            let (dx, dy) = (px - cx, py - cy);
            let ring = ((dx * dx + dy * dy).sqrt() - radius).abs() - half;
            if sweep >= TAU - 1e-3 {
                return ring;
            }
            // Clockwise from the top: 0 at 12 o'clock.
            let mut angle = dx.atan2(-dy);
            if angle < 0.0 {
                angle += TAU;
            }
            if angle <= sweep {
                return ring;
            }
            // Past the sweep: only the round ends.
            let to_start = ((px - sx).powi(2) + (py - sy).powi(2)).sqrt() - half;
            let to_end = ((px - ex).powi(2) + (py - ey).powi(2)).sqrt() - half;
            to_start.min(to_end)
        });
    }
}

/// `(px, py)`'s signed distance to a rectangle with corners of radius `r` (negative inside).
fn rect_distance(px: f32, py: f32, x: f32, y: f32, w: f32, h: f32, r: f32) -> f32 {
    let r = r.min(w / 2.0).min(h / 2.0);
    let qx = (px - x - w / 2.0).abs() - (w / 2.0 - r);
    let qy = (py - y - h / 2.0).abs() - (h / 2.0 - r);
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    outside + qx.max(qy).min(0.0) - r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha(p: &[u8], size: u32, x: u32, y: u32) -> u8 {
        p[((y * size + x) * 4 + 3) as usize]
    }

    fn rgb(p: &[u8], size: u32, x: u32, y: u32) -> [u8; 3] {
        let i = ((y * size + x) * 4) as usize;
        [p[i], p[i + 1], p[i + 2]]
    }

    /// A bar is filled in its tone up to its level, its track faint past it, nothing outside.
    #[test]
    fn a_bar_fills_to_its_level() {
        let p = render(Style::Bars, &[(50, Tone::Good)], 24, false);
        assert_eq!(rgb(&p, 24, 6, 12), Tone::Good.rgb(false));
        assert_eq!(alpha(&p, 24, 6, 12), 255);
        let track = alpha(&p, 24, 18, 12);
        assert!(track > 40 && track < 120, "track {track}");
        assert_eq!(alpha(&p, 24, 12, 2), 0);
    }

    /// Three devices are three bars, apart; a fourth is not drawn.
    #[test]
    fn several_devices_are_several_bars() {
        let levels = [(10, Tone::Low), (35, Tone::Fair), (80, Tone::Good), (90, Tone::Good)];
        let p = render(Style::Bars, &levels, 24, false);
        let column: Vec<bool> = (0..24).map(|y| alpha(&p, 24, 3, y) > 128).collect();
        let runs = column.windows(2).filter(|w| !w[0] && w[1]).count() + usize::from(column[0]);
        assert_eq!(runs, 3);
    }

    /// A ring is coloured clockwise from the top up to its level: its right side at 50 %, not its
    /// left.
    #[test]
    fn a_ring_fills_clockwise_from_the_top() {
        let p = render(Style::Rings, &[(50, Tone::Fair)], 24, false);
        let right = rgb(&p, 24, 21, 12);
        assert_eq!(right, Tone::Fair.rgb(false));
        assert!(alpha(&p, 24, 2, 12) < 160, "left {}", alpha(&p, 24, 2, 12));
        assert_eq!(alpha(&p, 24, 12, 12), 0);
    }

    const THREE: [(u8, Tone); 3] = [(10, Tone::Low), (35, Tone::Fair), (80, Tone::Good)];

    /// The pixels of exactly `rgb`, fully opaque, in rows `ys`.
    fn count(p: &[u8], size: u32, ys: std::ops::Range<u32>, rgb: [u8; 3]) -> usize {
        ys.flat_map(|y| (0..size).map(move |x| (x, y))).filter(|&(x, y)| alpha(p, size, x, y) == 255 && self::rgb(p, size, x, y) == rgb).count()
    }

    /// The battery is the lowest device's, in its tone; under it, a dot per device in each one's.
    #[test]
    fn the_battery_shows_the_lowest_and_a_dot_each() {
        let p = render(Style::Battery, &THREE, 24, false);
        // The body (rows 4 to 13) holds only the lowest's colour.
        assert!(count(&p, 24, 4..14, Tone::Low.rgb(false)) > 0);
        assert_eq!(count(&p, 24, 4..14, Tone::Good.rgb(false)), 0);
        // The dots' row holds all three.
        for tone in [Tone::Low, Tone::Fair, Tone::Good] {
            assert!(count(&p, 24, 14..24, tone.rgb(false)) > 0, "{tone:?}");
        }
        // One device: no dots.
        let one = render(Style::Battery, &[(80, Tone::Good)], 24, false);
        assert_eq!(count(&one, 24, 18..24, Tone::Good.rgb(false)), 0);
    }

    /// The figures are written in the lowest's tone, at every tray size.
    #[test]
    fn the_number_is_written_in_its_tone() {
        // Written, not the battery it falls back to.
        assert!(crate::icons::text_coverage("10", 16.0).is_some());
        for size in [16, 20, 24, 32] {
            let p = render(Style::Number, &THREE, size, true);
            let ink = (0..size * size).filter(|i| p[(*i * 4 + 3) as usize] > 128 && p[(*i * 4) as usize..(*i * 4 + 3) as usize] == Tone::Low.rgb(true)).count();
            assert!(ink > 6, "size {size}: {ink} pixels");
        }
    }

    /// Bubbles: one per device, each filled from the bottom (the low one barely, the full one high).
    #[test]
    fn a_bubble_per_device_fills_from_the_bottom() {
        let p = render(Style::Bubbles, &[(30, Tone::Low), (35, Tone::Fair), (80, Tone::Good)], 24, false);
        // The top bubble (centre 12, 6.25) is the lowest: its bottom is coloured, its top is not.
        assert_eq!(rgb(&p, 24, 12, 9), Tone::Low.rgb(false));
        assert_ne!(rgb(&p, 24, 12, 5), Tone::Low.rgb(false));
        // The right one (80%) is coloured high up.
        assert_eq!(rgb(&p, 24, 17, 15), Tone::Good.rgb(false));
    }
}
