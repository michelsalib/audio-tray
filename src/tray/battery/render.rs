//! Audio Tray's own battery icon (a PC without a battery): one picture for every device, drawn at
//! the tray's exact pixel size so each edge lands on the pixel grid. Each device (up to three, the
//! lowest first) is a bar or a ring filled to its level in its tone's colour, over a faint track:
//! it reads as "the devices", not as this PC's battery.

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
        let r = r.min(w / 2.0).min(h / 2.0);
        let (cx, cy) = (x + w / 2.0, y + h / 2.0);
        let (hw, hh) = (w / 2.0 - r, h / 2.0 - r);
        self.fill(rgb, opacity, |px, py| {
            let qx = (px - cx).abs() - hw;
            let qy = (py - cy).abs() - hh;
            let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
            outside + qx.max(qy).min(0.0) - r
        });
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
}
