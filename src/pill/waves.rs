//! The rings and bars painted again on every frame while they move (see [`super::Live`]), for
//! what no compositor animation can do: ends that thin out.
//!
//! - The Google ring: a ripple that grows from nothing at both ends, so each sits on the ring.
//! - The Samsung ring and bar, after Samsung's media player: translucent sine waves of several
//!   lengths, speeds and shades overlap on the played part, thinning to nothing at both ends, and
//!   end at a glowing knob.

use std::f32::consts::{PI, TAU};

use anyhow::Result;
use windows_numerics::Vector2;

use super::gfx::{Painter, Rgba};

/// One wave: its length (DIPs along a bar; on a ring, a share of the circle, see [`RING_PERIODS`]),
/// height (a share of the room), speed (periods per second, backwards when negative), phase,
/// shade (towards white when positive, black when negative) and opacity. Painted in this order.
struct Layer {
    length: f32,
    height: f32,
    speed: f32,
    phase: f32,
    shade: f32,
    alpha: f32,
    /// How much it swells and falls back.
    swell: f32,
}

const LAYERS: [Layer; 4] = [
    Layer { length: 150.0, height: 1.0, speed: 0.35, phase: 0.0, shade: 0.0, alpha: 0.7, swell: 0.35 },
    Layer { length: 90.0, height: 0.8, speed: -0.55, phase: 1.7, shade: -0.15, alpha: 0.6, swell: 0.25 },
    Layer { length: 62.0, height: 0.65, speed: 0.8, phase: 3.1, shade: 0.4, alpha: 0.55, swell: 0.45 },
    Layer { length: 38.0, height: 0.4, speed: -1.25, phase: 4.4, shade: 0.75, alpha: 0.45, swell: 0.6 },
];

/// On a ring, each layer's periods to a turn: whole numbers, so each keeps its shape all round.
const RING_PERIODS: [f32; 4] = [3.0, 5.0, 8.0, 13.0];

/// The rest of the track.
const TRACK: Rgba = Rgba(1.0, 1.0, 1.0, 0.2);

/// Layer `layer`'s height `u` along a played part `span` long, `length` to a period, at most
/// `room`, `t` seconds in, raised by `amp` (0 paused, 1 playing).
fn height(layer: &Layer, u: f32, span: f32, length: f32, room: f32, t: f32, amp: f32) -> f32 {
    if span < 4.0 || amp <= 0.002 {
        return 0.0;
    }
    let hump = 0.5 + 0.5 * (TAU * u / length - layer.speed * TAU * t + layer.phase).sin();
    let swell = 1.0 - layer.swell + layer.swell * (0.4 + 0.6 * (0.5 + 0.5 * (t * 1.7 + layer.phase * 2.0).sin()));
    // At the very end, sin(π) is a hair below zero in f32: its power would be NaN, and one NaN
    // point loses the whole shape.
    let ends = (PI * (u / span).clamp(0.0, 1.0)).sin().max(0.0).powf(0.7);
    room * amp * layer.height * swell * hump * ends
}

fn shaded(color: Rgba, shade: f32) -> Rgba {
    if shade >= 0.0 {
        color.mix(Rgba(1.0, 1.0, 1.0, color.3), shade)
    } else {
        color.mix(Rgba(0.0, 0.0, 0.0, color.3), -shade)
    }
}

/// A ring `r` round `centre`, `stroke` wide, filled to `fraction` from the top, with the waves
/// rising outwards into `room` and fading as they go.
#[allow(clippy::too_many_arguments)]
pub fn ring(p: &mut Painter, centre: (f32, f32), r: f32, stroke: f32, room: f32, fraction: f32, (t, amp): (f32, f32), color: Rgba) -> Result<()> {
    let (cx, cy) = centre;
    let at = |angle: f32, radius: f32| Vector2 { X: cx + radius * angle.sin(), Y: cy - radius * angle.cos() };
    let sweep = fraction.clamp(0.0, 1.0) * TAU;
    // The rest of the track: thinner, after a gap either side.
    let gap = (stroke + 3.0) / r;
    if sweep + 1.5 * gap < TAU {
        let steps = (((TAU - sweep - 1.5 * gap) * r * 2.0).ceil() as usize).max(2);
        let points: Vec<Vector2> = (0..=steps).map(|i| at(sweep + gap + (TAU - sweep - 1.5 * gap) * i as f32 / steps as f32, r)).collect();
        p.polyline(&points, stroke * 0.6, TRACK, false)?;
    }
    let span = sweep * r;
    let base = r + stroke / 2.0;
    if span > 2.0 {
        let steps = ((span * 2.0).ceil() as usize).max(8);
        for (layer, periods) in LAYERS.iter().zip(RING_PERIODS) {
            let length = TAU * r / periods;
            let mut points: Vec<Vector2> = (0..=steps)
                .map(|i| {
                    let u = span * i as f32 / steps as f32;
                    at(u / r, base + height(layer, u, span, length, room, t, amp))
                })
                .collect();
            // Back along the ring itself.
            points.extend((0..=steps).rev().map(|i| at(sweep * i as f32 / steps as f32, base - 0.2)));
            let shade = shaded(color, layer.shade);
            // Little room on a pill: stronger than on the bar, fading only a little outwards.
            let alpha = (layer.alpha * 1.25).min(0.95);
            p.fill_polygon_radial(&points, centre, (base, base + room), shade, (alpha, alpha * 0.6))?;
        }
        let points: Vec<Vector2> = (0..=steps).map(|i| at(sweep * i as f32 / steps as f32, r)).collect();
        p.polyline(&points, stroke, color, false)?;
    }
    let knob = at(sweep, r);
    p.knob(knob.X, knob.Y, stroke * 1.3, color)
}

/// The Google style's ring: filled to `fraction` from the top with a line that ripples `amp`
/// either side of `r`, `periods` times a turn, travelling forwards one period every `period`
/// seconds. The ripple thins to nothing towards both ends, so each sits on the ring itself, round,
/// with a gap before the rest of the track.
#[allow(clippy::too_many_arguments)]
pub fn ripple_ring(p: &mut Painter, centre: (f32, f32), r: f32, stroke: f32, (amp, periods, period): (f32, f32, f32), fraction: f32, (t, rise): (f32, f32), color: Rgba, track: Rgba) -> Result<()> {
    let (cx, cy) = centre;
    let at = |angle: f32, radius: f32| Vector2 { X: cx + radius * angle.sin(), Y: cy - radius * angle.cos() };
    let sweep = fraction.clamp(0.0, 1.0) * TAU;
    // The gap either side of the rest of the track, round caps included.
    let gap = (stroke + 2.5) / r;
    let rest = TAU - sweep - 2.0 * gap;
    if rest > 0.0 {
        let steps = ((rest * r * 2.0).ceil() as usize).max(2);
        let points: Vec<Vector2> = (0..=steps).map(|i| at(sweep + gap + rest * i as f32 / steps as f32, r)).collect();
        p.polyline(&points, stroke, track, false)?;
    }
    let span = sweep * r;
    if span <= 0.5 {
        return p.circle(at(0.0, r).X, at(0.0, r).Y, stroke / 2.0, color);
    }
    // Over half a wavelength at each end, the ripple grows from nothing.
    let ease = (TAU * r / periods) / 2.0;
    let smooth = |v: f32| {
        let v = v.clamp(0.0, 1.0);
        v * v * (3.0 - 2.0 * v)
    };
    let phase = TAU * t / period;
    let steps = ((span * 3.0).ceil() as usize).max(8);
    let points: Vec<Vector2> = (0..=steps)
        .map(|i| {
            let a = sweep * i as f32 / steps as f32;
            let u = a * r;
            let taper = smooth(u / ease) * smooth((span - u) / ease);
            at(a, r + amp * rise * taper * (periods * a - phase).sin())
        })
        .collect();
    p.polyline(&points, stroke, color, false)
}

/// A bar from `x` along `w` on the line `cy`, `stroke` thick, filled to `fraction`, with the waves
/// rising above it into `room`.
#[allow(clippy::too_many_arguments)]
pub fn bar(p: &mut Painter, (x, cy): (f32, f32), w: f32, stroke: f32, room: f32, fraction: f32, (t, amp): (f32, f32), color: Rgba, knob: f32) -> Result<()> {
    let head = x + w * fraction.clamp(0.0, 1.0);
    let span = head - x;
    if x + w - head > knob + 4.0 {
        p.polyline(&[Vector2 { X: head + knob + 4.0, Y: cy }, Vector2 { X: x + w, Y: cy }], stroke, TRACK, false)?;
    }
    if span > 2.0 {
        let steps = (span.ceil() as usize).max(4);
        for layer in &LAYERS {
            let mut points: Vec<Vector2> = (0..=steps)
                .map(|i| {
                    let u = span * i as f32 / steps as f32;
                    Vector2 { X: x + u, Y: cy - stroke / 2.0 - height(layer, u, span, layer.length, room, t, amp) }
                })
                .collect();
            points.push(Vector2 { X: head, Y: cy });
            points.push(Vector2 { X: x, Y: cy });
            p.fill_polygon(&points, shaded(color, layer.shade).alpha(layer.alpha))?;
        }
        p.polyline(&[Vector2 { X: x, Y: cy }, Vector2 { X: head, Y: cy }], stroke, color, false)?;
    }
    p.knob(head, cy, knob, color)
}
