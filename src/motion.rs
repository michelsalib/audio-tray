//! How the volume sliders move and look in each [`MotionStyle`]: the flyout's and the scroll
//! readout's. Both are painted in software ([`Canvas`]) and repainted on their own timers, so a
//! slider keeps a [`SliderMotion`] whose springs those timers advance until they settle.
//!
//! - **None**: a thin track and a round thumb, at the level the moment it changes.
//! - **Google** (Android's volume panel): a thick track, a slim upright handle with a gap either
//!   side, a stop dot at the end, the live peak as a lighter band inside the fill; the level springs
//!   to each change with a slight overshoot.
//! - **Samsung**: one thick capsule whose fill keeps at least a circle's width, springing softer and
//!   bouncier; dragged past either end it stretches like elastic and snaps back on release, and a
//!   wheel notch at an end bounces it.

use crate::canvas::{lerp3, Canvas, Rect};
use crate::config::MotionStyle;

const TEXT: [u8; 3] = [0xFF, 0xFF, 0xFF];
const GREY: [u8; 3] = [0x6A, 0x6A, 0x6A];

/// A damped spring's position and speed.
#[derive(Clone, Copy, Default, Debug)]
pub struct Spring {
    pub x: f32,
    v: f32,
}

impl Spring {
    pub fn at(x: f32) -> Self {
        Spring { x, v: 0.0 }
    }

    /// Move towards `target` for `dt` seconds with stiffness `k` and damping `c`; whether it is
    /// still moving. Settled, it sits exactly on the target.
    fn step(&mut self, target: f32, (k, c): (f32, f32), dt: f32) -> bool {
        let steps = 4;
        for _ in 0..steps {
            let pull = k * (target - self.x) - c * self.v;
            self.v += pull * dt / steps as f32;
            self.x += self.v * dt / steps as f32;
        }
        if (target - self.x).abs() < 0.0015 && self.v.abs() < 0.01 {
            *self = Spring::at(target);
            return false;
        }
        true
    }

    fn kick(&mut self, speed: f32) {
        self.v += speed;
    }
}

/// A slider's moving parts.
#[derive(Clone, Copy, Default, Debug)]
pub struct SliderMotion {
    /// The level drawn, springing to the real one.
    pub level: Spring,
    /// The fill's share left by muting: 1 unmuted, 0 muted.
    pub unmuted: Spring,
    /// How far an end is pulled, in DIPs: negative past 0, positive past 100.
    pub stretch: Spring,
    stretch_to: f32,
    /// Whether it has been drawn yet: the first draw starts on the level, without a spring.
    started: bool,
}

/// How hard each style's springs pull, and how much they are damped.
fn springs(style: MotionStyle) -> (f32, f32) {
    match style {
        MotionStyle::Samsung => (300.0, 20.0),
        _ => (380.0, 24.0),
    }
}

/// Elastic: further pulls stretch less and less, to at most this many DIPs.
const STRETCH_MOST: f32 = 14.0;

impl SliderMotion {
    /// At `level`, still: for a slider drawn once, without springs.
    pub fn still(level: f32, muted: bool) -> Self {
        SliderMotion { level: Spring::at(level), unmuted: Spring::at(if muted { 0.0 } else { 1.0 }), started: true, ..Default::default() }
    }

    /// Advance by `dt` seconds towards `level` (and muted or not) in `style`; whether anything is
    /// still moving and needs another frame.
    pub fn step(&mut self, style: MotionStyle, level: f32, muted: bool, dt: f32) -> bool {
        let to_unmuted = if muted { 0.0 } else { 1.0 };
        if !self.started || style == MotionStyle::None {
            self.started = true;
            self.level = Spring::at(level);
            self.unmuted = Spring::at(to_unmuted);
            self.stretch = Spring::at(0.0);
            return false;
        }
        let dt = dt.min(0.05);
        let a = self.level.step(level, springs(style), dt);
        let b = self.unmuted.step(to_unmuted, (260.0, 30.0), dt);
        let c = style == MotionStyle::Samsung && self.stretch.step(self.stretch_to, (420.0, 18.0), dt);
        a || b || c
    }

    /// Being dragged `over` DIPs past an end (0 within it): the Samsung capsule stretches.
    pub fn pull(&mut self, over: f32) {
        self.stretch_to = over.signum() * STRETCH_MOST * (1.0 - (-over.abs() / 40.0).exp());
    }

    /// A wheel notch that could not move the level, `up` or down: a small bounce at that end.
    pub fn bump(&mut self, up: bool) {
        self.stretch.kick(if up { 140.0 } else { -140.0 });
    }

    /// The level to show as a number: where the fill is drawn.
    pub fn shown(&self) -> f32 {
        self.level.x.clamp(0.0, 1.0)
    }
}

/// Where a slider goes on the canvas, in pixels, and how big a DIP is.
#[derive(Clone, Copy)]
pub struct Track {
    pub x0: f32,
    pub x1: f32,
    pub cy: f32,
    pub scale: f32,
}

/// Paint a slider in `style`: `motion` as it is now, `muted`, the live `peak` (0–1), in `accent`.
/// `compact` is the scroll readout's smaller size.
#[allow(clippy::too_many_arguments)]
pub fn draw(cv: &mut Canvas, style: MotionStyle, track: Track, motion: &SliderMotion, muted: bool, peak: f32, accent: [u8; 3], compact: bool) {
    let Track { x0, x1, cy, scale } = track;
    let d = |v: f32| v * scale;
    let level = motion.level.x.clamp(0.0, 1.02);
    let unmuted = motion.unmuted.x.clamp(0.0, 1.0);
    let fill_col = lerp3(GREY, accent, unmuted);
    let peak = peak.clamp(0.0, 1.0).powf(0.55) * unmuted;
    match style {
        MotionStyle::None => {
            let (th, tr) = (d(4.0), d(if compact { 6.0 } else { 7.0 }));
            let fx = x0 + (x1 - x0) * level.min(1.0);
            cv.fill_round_rect(Rect::new(x0, cy - th / 2.0, x1, cy + th / 2.0), th / 2.0, TEXT, 0.28);
            let (col, a) = if muted { (TEXT, 0.34) } else { (accent, 1.0) };
            if fx > x0 {
                cv.fill_round_rect(Rect::new(x0, cy - th / 2.0, fx, cy + th / 2.0), th / 2.0, col, a);
            }
            cv.fill_round_rect(Rect::new(fx - tr, cy - tr, fx + tr, cy + tr), tr, col, if muted { 0.5 } else { 1.0 });
        }
        MotionStyle::Google => {
            let th = d(if compact { 8.0 } else { 12.0 });
            let (hw, hh) = (d(4.0), d(if compact { 18.0 } else { 26.0 }));
            let gap = d(4.0);
            let fx = x0 + (x1 - x0) * (level * unmuted).min(1.0);
            let (fill_end, rest_start) = (fx - hw / 2.0 - gap, fx + hw / 2.0 + gap);
            if x1 > rest_start {
                cv.fill_round_rect(Rect::new(rest_start, cy - th / 2.0, x1, cy + th / 2.0), th / 2.0, TEXT, 0.18);
                // The stop dot, while there is room before it.
                if x1 - rest_start > th * 1.5 {
                    let r = d(2.0);
                    cv.fill_round_rect(Rect::new(x1 - th / 2.0 - r, cy - r, x1 - th / 2.0 + r, cy + r), r, fill_col, 1.0);
                }
            }
            if fill_end > x0 {
                cv.fill_round_rect(Rect::new(x0, cy - th / 2.0, fill_end, cy + th / 2.0), th / 2.0, fill_col, 1.0);
                if peak > 0.01 {
                    let px = x0 + (fill_end - x0) * peak;
                    cv.fill_round_rect(Rect::new(x0, cy - th / 2.0, px.max(x0 + th), cy + th / 2.0), th / 2.0, TEXT, 0.28);
                }
            }
            cv.fill_round_rect(Rect::new(fx - hw / 2.0, cy - hh / 2.0, fx + hw / 2.0, cy + hh / 2.0), hw / 2.0, fill_col, 1.0);
        }
        MotionStyle::Samsung => {
            let stretch = d(motion.stretch.x);
            let (x0, x1) = if stretch > 0.0 { (x0, x1 + stretch) } else { (x0 + stretch, x1) };
            let th = d(if compact { 16.0 } else { 22.0 }) * (1.0 - (stretch.abs() / d(70.0)).min(0.22));
            cv.fill_round_rect(Rect::new(x0, cy - th / 2.0, x1, cy + th / 2.0), th / 2.0, TEXT, 0.12);
            // The fill is never shorter than a circle.
            let fx = x0 + th + (x1 - x0 - th) * (level * unmuted).clamp(0.0, 1.0);
            cv.fill_round_rect(Rect::new(x0, cy - th / 2.0, fx, cy + th / 2.0), th / 2.0, fill_col, 1.0);
            if peak > 0.01 {
                cv.fill_round_rect(Rect::new(x0, cy - th / 2.0, x0 + th + (fx - x0 - th) * peak, cy + th / 2.0), th / 2.0, TEXT, 0.16);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_level_springs_to_its_target_and_settles_on_it() {
        let mut m = SliderMotion::default();
        assert!(!m.step(MotionStyle::Google, 0.2, false, 0.016), "the first step starts on the level");
        let mut frames = 0;
        while m.step(MotionStyle::Google, 0.8, false, 0.016) {
            frames += 1;
            assert!(frames < 200, "never settled");
        }
        assert_eq!(m.level.x, 0.8);
        assert!(frames > 5, "it moved over several frames");
    }

    #[test]
    fn no_motion_jumps_straight_to_the_level() {
        let mut m = SliderMotion::default();
        m.step(MotionStyle::None, 0.2, false, 0.016);
        assert!(!m.step(MotionStyle::None, 0.9, false, 0.016));
        assert_eq!(m.level.x, 0.9);
    }

    #[test]
    fn a_pull_past_the_end_stretches_less_and_less_and_lets_go() {
        let mut m = SliderMotion::default();
        m.step(MotionStyle::Samsung, 1.0, false, 0.016);
        m.pull(40.0);
        let near = m.stretch_to;
        m.pull(400.0);
        assert!(m.stretch_to > near && m.stretch_to <= STRETCH_MOST);
        m.pull(0.0);
        while m.step(MotionStyle::Samsung, 1.0, false, 0.016) {}
        assert_eq!(m.stretch.x, 0.0);
    }
}
