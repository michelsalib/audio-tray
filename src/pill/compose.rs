//! The pill's DirectComposition pieces: the progress rings and card bars in each motion style, the
//! speaking pulse, and the animations they are made of. Each is a compositor animation set once
//! per change, but for the [`Live`] rings and bars, painted again on every frame while they move.

use anyhow::Result;
use windows::Win32::Graphics::DirectComposition::{
    IDCompositionAnimation, IDCompositionRectangleClip, IDCompositionSurface, IDCompositionVisual2, DCOMPOSITION_BORDER_MODE_SOFT,
};

use super::draw::{self, Ring};
use super::gfx::Gfx;
use super::waves;
use crate::config::MotionStyle;

/// One beat of the speaking pulse.
const PULSE_SECONDS: f64 = 1.1;
/// A playing track's progress ripples, as Android's media player does: the wave travels one
/// wavelength per this many seconds.
const WAVE_SECONDS: f64 = 1.0;
/// The ring's wavelength along its circle, and its amplitude per DIP of radius.
const RING_WAVELENGTH: f32 = 12.0;
const RING_AMP: f32 = 0.05;
/// The card bar's wavelength and amplitude.
const BAR_WAVELENGTH: f32 = 20.0;
const BAR_AMP: f32 = 2.5;
/// The card bar's playhead: a short upright bar, and the gap either side of it.
const THUMB_W: f32 = 4.0;
const THUMB_H: f32 = 14.0;
const THUMB_GAP: f32 = 3.0;
/// Pausing settles the ripple flat, and playing raises it again, over this long.
const SETTLE_SECONDS: f64 = 0.4;

/// Fade `visual` from opacity `from` to `to` over `seconds`, easing out.
pub(super) fn fade(gfx: &Gfx, visual: &IDCompositionVisual2, from: f32, to: f32, seconds: f64) -> Result<()> {
    unsafe {
        let opacity = gfx.dcomp.CreateAnimation()?;
        ease_out(&opacity, from, to, seconds)?;
        let effect = gfx.dcomp.CreateEffectGroup()?;
        effect.SetOpacity(&opacity)?;
        visual.SetEffect(&effect)?;
    }
    Ok(())
}

/// Scale `visual` about `anchor` (pixels) from `from` to `to` (x and y) over `seconds`, easing out.
pub(super) fn anchored_scale(gfx: &Gfx, visual: &IDCompositionVisual2, (ax, ay): (f32, f32), from: (f32, f32), to: (f32, f32), seconds: f64) -> Result<()> {
    unsafe {
        let scale = gfx.dcomp.CreateScaleTransform()?;
        scale.SetCenterX2(ax)?;
        scale.SetCenterY2(ay)?;
        let sx = gfx.dcomp.CreateAnimation()?;
        ease_out(&sx, from.0, to.0, seconds)?;
        let sy = gfx.dcomp.CreateAnimation()?;
        ease_out(&sy, from.1, to.1, seconds)?;
        scale.SetScaleX(&sx)?;
        scale.SetScaleY(&sy)?;
        visual.SetTransform(&scale)?;
    }
    Ok(())
}

/// The None style's ring, in two halves: a right-half clip for 0–180° and a left-half one for
/// 180–360°. In each, the filled arc is a whole ring behind a half-plane clip that turns with the
/// progress, so it shows from the top to the playhead; the rest of the track is a whole ring
/// behind the other half-plane, turned with it. Each angle is a compositor animation from now to
/// the end of the track, so a playing ring costs nothing between changes.
pub(super) fn ring_visuals(gfx: &Gfx, holder: &IDCompositionVisual2, ring: Ring) -> Result<Vec<IDCompositionVisual2>> {
    let reach = ring.r + ring.stroke;
    let size = 2.0 * reach;
    let scale = gfx.scale;
    let (_, flat) = gfx.painted(size, size, |p| p.ring(reach, reach, ring.r, ring.stroke, ring.color))?;
    let (_, track) = gfx.painted(size, size, |p| p.ring(reach, reach, ring.r, ring.stroke, draw::TRACK))?;
    // Degrees now, and degrees per second.
    let now = ring.fraction.clamp(0.0, 1.0) * 360.0;
    let rate = ring.rate * 360.0;
    // The left or right half of the square, in pixels.
    let half = |left: bool| -> Result<IDCompositionRectangleClip> {
        unsafe {
            let clip = gfx.dcomp.CreateRectangleClip()?;
            let (l, r) = if left { (0.0, reach * scale) } else { (reach * scale, size * scale) };
            clip.SetLeft2(l)?;
            clip.SetRight2(r)?;
            clip.SetTop2(0.0)?;
            clip.SetBottom2(size * scale)?;
            Ok(clip)
        }
    };
    let mut keep = Vec::new();
    for clip_left in [false, true] {
        unsafe {
            let container = gfx.dcomp.CreateVisual()?;
            container.SetOffsetX2((ring.cx - reach) * scale)?;
            container.SetOffsetY2((ring.cy - reach) * scale)?;
            container.SetClip(&half(clip_left)?)?;
            // The right clip shows angles 0–180 (the first half), the left 180–360: an angle in
            // this half that holds until the playhead reaches it, then follows it to 180.
            let base = if clip_left { 180.0 } else { 0.0 };
            let angle = gfx.dcomp.CreateAnimation()?;
            piecewise(&angle, (now - base).clamp(0.0, 180.0), rate, if clip_left { (180.0 - now).max(0.0) } else { 0.0 })?;
            let rotate = gfx.dcomp.CreateRotateTransform()?;
            rotate.SetCenterX2(reach * scale)?;
            rotate.SetCenterY2(reach * scale)?;
            rotate.SetAngle(&angle)?;
            // The rest of the track behind this clip's own half-plane, the filled arc behind the
            // other, both turned to the playhead.
            for (content, own_half) in [(&track, true), (&flat, false)] {
                let part = gfx.dcomp.CreateVisual()?;
                part.SetContent(content)?;
                part.SetClip(&half(if own_half { clip_left } else { !clip_left })?)?;
                part.SetTransform(&rotate)?;
                part.SetBorderMode(DCOMPOSITION_BORDER_MODE_SOFT)?;
                container.AddVisual(&part, false, None)?;
                keep.push(part);
            }
            // `false` with no reference visual puts it on top of its siblings, over the content.
            holder.AddVisual(&container, false, None)?;
            keep.push(container);
        }
    }
    Ok(keep)
}

/// Which fills to show, rippling or not, each with the opacities it fades between: the one, or
/// both cross-fading when the ripple starts or stops.
fn fills(waving: bool, was: Option<bool>) -> Vec<(bool, Option<(f32, f32)>)> {
    match was {
        Some(was) if was != waving => vec![(was, Some((1.0, 0.0))), (waving, Some((0.0, 1.0)))],
        _ => vec![(waving, None)],
    }
}

/// A card bar's moving parts in the Google style: the filled part up to just before the playhead,
/// the playhead, and the rest of the track from just after it, each moved by DWM from now to the
/// end of the track. While playing, the filled part is a wave strip a wavelength longer than the
/// bar, slid by one wavelength on repeat; paused, it is a flat bar. `was` is whether the bar shown
/// before was rippling (`None`: no bar before): a change cross-fades the ripple in or out.
pub(super) fn bar_visuals(gfx: &Gfx, holder: &IDCompositionVisual2, bar: draw::Bar, was: Option<bool>) -> Result<Vec<IDCompositionVisual2>> {
    let scale = gfx.scale;
    let band = THUMB_H.max(2.0 * BAR_AMP + bar.h + 2.0);
    let (top, mid) = (bar.y + bar.h / 2.0 - band / 2.0, band / 2.0);
    let w = bar.w;
    // The playhead, in pixels from the bar's start: now, and how fast it moves.
    let (now, speed, end) = (bar.fraction.clamp(0.0, 1.0) * w * scale, bar.rate * w * scale, w * scale);
    // A position that keeps `by` pixels from the playhead.
    let follow = |by: f32| -> Result<IDCompositionAnimation> {
        let at = unsafe { gfx.dcomp.CreateAnimation()? };
        glide(&at, now + by, end + by, speed)?;
        Ok(at)
    };
    // The playhead, and the gaps either side of it: none on a live stream.
    let beside = if bar.knob { (THUMB_W / 2.0 + THUMB_GAP) * scale } else { 0.0 };
    let mut keep = Vec::new();
    unsafe {
        // The rest of the track, from just after the playhead.
        let (rest, _) = gfx.painted(w, band, |p| p.round_rect(0.0, mid - bar.h / 2.0, w, bar.h, bar.h / 2.0, draw::TRACK))?;
        rest.SetOffsetX2(bar.x * scale)?;
        rest.SetOffsetY2(top * scale)?;
        let clip = gfx.dcomp.CreateRectangleClip()?;
        clip.SetLeft(&follow(beside)?)?;
        clip.SetRight2(end)?;
        clip.SetTop2(0.0)?;
        clip.SetBottom2(band * scale)?;
        rest.SetClip(&clip)?;
        holder.AddVisual(&rest, false, None)?;
        keep.push(rest);
        // The filled part, up to just before it.
        let filled = gfx.dcomp.CreateVisual()?;
        filled.SetOffsetX2(bar.x * scale)?;
        filled.SetOffsetY2(top * scale)?;
        let clip = gfx.dcomp.CreateRectangleClip()?;
        clip.SetLeft2(0.0)?;
        clip.SetRight(&follow(-beside)?)?;
        clip.SetTop2(0.0)?;
        clip.SetBottom2(band * scale)?;
        filled.SetClip(&clip)?;
        for (rippling, settle) in fills(bar.playing, was) {
            let fill = if rippling {
                let length = w + BAR_WAVELENGTH;
                let (strip, _) = gfx.painted(length, band, |p| p.wave_line((0.0, length), mid, BAR_AMP, BAR_WAVELENGTH, bar.h, bar.color))?;
                // From a wavelength back to its place, on repeat: the wave travels forwards.
                let slide = gfx.dcomp.CreateAnimation()?;
                cycle(&slide, -BAR_WAVELENGTH * scale, BAR_WAVELENGTH * scale, WAVE_SECONDS)?;
                strip.SetOffsetX(&slide)?;
                strip
            } else {
                gfx.painted(w, band, |p| p.round_rect(0.0, mid - bar.h / 2.0, w, bar.h, bar.h / 2.0, bar.color))?.0
            };
            if let Some((from, to)) = settle {
                fade(gfx, &fill, from, to, SETTLE_SECONDS)?;
            }
            filled.AddVisual(&fill, false, None)?;
            keep.push(fill);
        }
        holder.AddVisual(&filled, false, None)?;
        keep.push(filled);
        // The playhead.
        if bar.knob {
            let (head, _) = gfx.painted(THUMB_W, THUMB_H, |p| p.round_rect(0.0, 0.0, THUMB_W, THUMB_H, THUMB_W / 2.0, bar.color))?;
            head.SetOffsetX(&follow((bar.x - THUMB_W / 2.0) * scale)?)?;
            head.SetOffsetY2((top + mid - THUMB_H / 2.0) * scale)?;
            holder.AddVisual(&head, false, None)?;
            keep.push(head);
        }
    }
    Ok(keep)
}

/// A ring in the Google or the Samsung style, or a Samsung card bar (see [`waves`]): a surface
/// painted again on every frame while it moves, then left as it is.
pub(super) struct Live {
    pub(super) visual: IDCompositionVisual2,
    surface: IDCompositionSurface,
    shape: LiveShape,
    /// When it was shown: its progress moves on from there.
    shown: std::time::Instant,
    last: std::time::Instant,
    /// The waves' height, springing to 1 while playing and 0 paused, and its speed.
    amp: f32,
    speed: f32,
}

enum LiveShape {
    /// The ring, in a square `side` DIPs wide centred on it, in the Google or the Samsung style.
    Ring(Ring, f32, MotionStyle),
    /// The Samsung bar, in a band from `(x, y)` DIPs.
    Bar(draw::Bar, (f32, f32)),
}

/// The Samsung style's dimensions: the room the waves rise into, the line's width, the knob.
const LIVE_RING_ROOM: f32 = 0.22;
/// The Samsung ring's line, as a share of the other styles' stroke: thinner, so the waves lead.
const LIVE_RING_STROKE: f32 = 0.7;
const LIVE_BAR_ROOM: f32 = 7.0;
const LIVE_BAR_KNOB: f32 = 5.0;
/// How far the knob's glow reaches, in knob radii.
const KNOB_GLOW: f32 = 2.1;

impl Live {
    pub(super) fn ring(gfx: &Gfx, holder: &IDCompositionVisual2, ring: Ring, was: Option<bool>, style: MotionStyle) -> Result<Live> {
        let reach = if style == MotionStyle::Samsung {
            let stroke = ring.stroke * LIVE_RING_STROKE;
            ring.r + (ring.r * LIVE_RING_ROOM + stroke).max(stroke * 1.3 * KNOB_GLOW) + 1.0
        } else {
            ring.r + ring.stroke + ring.r * RING_AMP + 1.0
        };
        let side = 2.0 * reach;
        Live::new(gfx, holder, (ring.cx - reach, ring.cy - reach), (side, side), LiveShape::Ring(ring, side, style), ring.playing, was)
    }

    pub(super) fn bar(gfx: &Gfx, holder: &IDCompositionVisual2, bar: draw::Bar, was: Option<bool>) -> Result<Live> {
        let pad = LIVE_BAR_KNOB * KNOB_GLOW;
        let cy = bar.y + bar.h / 2.0;
        let top = cy - bar.h / 2.0 - LIVE_BAR_ROOM - 1.0;
        let (x, y) = (bar.x - pad, top.min(cy - pad));
        let size = (bar.w + 2.0 * pad, (cy + pad) - y);
        Live::new(gfx, holder, (x, y), size, LiveShape::Bar(bar, (x, y)), bar.playing, was)
    }

    fn new(gfx: &Gfx, holder: &IDCompositionVisual2, (x, y): (f32, f32), (w, h): (f32, f32), shape: LiveShape, playing: bool, was: Option<bool>) -> Result<Live> {
        let (visual, surface) = gfx.painted(w, h, |_| Ok(()))?;
        unsafe {
            visual.SetOffsetX2(x * gfx.scale)?;
            visual.SetOffsetY2(y * gfx.scale)?;
            holder.AddVisual(&visual, false, None)?;
        }
        let now = std::time::Instant::now();
        let amp = if was.unwrap_or(playing) { 1.0 } else { 0.0 };
        let mut live = Live { visual, surface, shape, shown: now, last: now, amp, speed: 0.0 };
        live.paint(gfx, now)?;
        Ok(live)
    }

    fn playing(&self) -> bool {
        match &self.shape {
            LiveShape::Ring(ring, ..) => ring.playing,
            LiveShape::Bar(bar, _) => bar.playing,
        }
    }

    /// Whether it still has to be painted again: playing, or its waves still settling.
    pub(super) fn moving(&self) -> bool {
        self.playing() || self.amp > 0.002 || self.speed.abs() > 0.002
    }

    /// Paint it as it is at `now`.
    pub(super) fn paint(&mut self, gfx: &Gfx, now: std::time::Instant) -> Result<()> {
        // The waves' height springs to where it goes, without overshoot.
        let target = if self.playing() { 1.0 } else { 0.0 };
        let dt = now.duration_since(self.last).as_secs_f32().min(0.05);
        self.last = now;
        for _ in 0..4 {
            let pull = 120.0 * (target - self.amp) - 22.0 * self.speed;
            self.speed += pull * dt / 4.0;
            self.amp += self.speed * dt / 4.0;
        }
        if (target - self.amp).abs() < 0.002 && self.speed.abs() < 0.01 {
            (self.amp, self.speed) = (target, 0.0);
        }
        let elapsed = now.duration_since(self.shown).as_secs_f32();
        let t = clock() as f32;
        let amp = self.amp.max(0.0);
        match &self.shape {
            LiveShape::Ring(ring, side, style) => {
                let fraction = ring.fraction + ring.rate * elapsed;
                let centre = (side / 2.0, side / 2.0);
                if *style == MotionStyle::Samsung {
                    let stroke = ring.stroke * LIVE_RING_STROKE;
                    gfx.repaint(&self.surface, |p| waves::ring(p, centre, ring.r, stroke, ring.r * LIVE_RING_ROOM, fraction, (t, amp), ring.color, ring.knob))
                } else {
                    let periods = (std::f32::consts::TAU * ring.r / RING_WAVELENGTH).round().max(5.0);
                    let wave = (ring.r * RING_AMP, periods, WAVE_SECONDS as f32);
                    gfx.repaint(&self.surface, |p| waves::ripple_ring(p, centre, ring.r, ring.stroke, wave, fraction, (t, amp), ring.color, draw::TRACK))
                }
            }
            LiveShape::Bar(bar, (x, y)) => {
                let fraction = bar.fraction + bar.rate * elapsed;
                let at = (bar.x - x, bar.y + bar.h / 2.0 - y);
                gfx.repaint(&self.surface, |p| waves::bar(p, at, bar.w, bar.h, LIVE_BAR_ROOM, fraction, (t, amp), bar.color, if bar.knob { LIVE_BAR_KNOB } else { 0.0 }))
            }
        }
    }
}

/// Seconds since the app started, on one clock shared by every repeating animation (see
/// [`cycle`]).
fn clock() -> f64 {
    static EPOCH: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    EPOCH.get_or_init(std::time::Instant::now).elapsed().as_secs_f64()
}

/// A progress bar as it always was: its track, and over it the whole filled bar, painted once,
/// behind a clip whose right side DWM moves from now to the end of the track.
pub(super) fn plain_bar_visuals(gfx: &Gfx, holder: &IDCompositionVisual2, bar: draw::Bar) -> Result<Vec<IDCompositionVisual2>> {
    let (track, _) = gfx.painted(bar.w, bar.h, |p| p.round_rect(0.0, 0.0, bar.w, bar.h, bar.h / 2.0, draw::TRACK))?;
    let (filled, _) = gfx.painted(bar.w, bar.h, |p| p.round_rect(0.0, 0.0, bar.w, bar.h, bar.h / 2.0, bar.color))?;
    let scale = gfx.scale;
    let (w, now) = (bar.w * scale, bar.fraction.clamp(0.0, 1.0) * bar.w * scale);
    unsafe {
        for v in [&track, &filled] {
            v.SetOffsetX2(bar.x * scale)?;
            v.SetOffsetY2(bar.y * scale)?;
            holder.AddVisual(v, false, None)?;
        }
        let clip = gfx.dcomp.CreateRectangleClip()?;
        clip.SetLeft2(0.0)?;
        clip.SetTop2(0.0)?;
        clip.SetBottom2(bar.h * scale)?;
        let right = gfx.dcomp.CreateAnimation()?;
        glide(&right, now, w, bar.rate * w)?;
        clip.SetRight(&right)?;
        filled.SetClip(&clip)?;
    }
    Ok(vec![track, filled])
}

/// From `from` to `to` at `speed` per second, then still; `speed` 0 holds at `from`.
fn glide(animation: &IDCompositionAnimation, from: f32, to: f32, speed: f32) -> Result<()> {
    unsafe {
        if speed > 0.0 && from < to {
            animation.AddCubic(0.0, from, speed, 0.0, 0.0)?;
            animation.End(((to - from) / speed) as f64, to)?;
        } else {
            animation.AddCubic(0.0, from, 0.0, 0.0, 0.0)?;
        }
    }
    Ok(())
}

/// A green ring around a speaking face that swells and fades, over and over: one animation set
/// once, repeated by DWM forever.
pub(super) fn pulse_visual(gfx: &Gfx, holder: &IDCompositionVisual2, pulse: draw::Pulse) -> Result<IDCompositionVisual2> {
    const SWELL: f32 = 1.45;
    let reach = pulse.r * SWELL + 2.0;
    let size = 2.0 * reach;
    let (visual, _) = gfx.painted(size, size, |p| p.ring(reach, reach, pulse.r, 2.0, draw::PULSE))?;
    let scale = gfx.scale;
    unsafe {
        visual.SetOffsetX2((pulse.cx - reach) * scale)?;
        visual.SetOffsetY2((pulse.cy - reach) * scale)?;
        let grow = gfx.dcomp.CreateScaleTransform()?;
        grow.SetCenterX2(reach * scale)?;
        grow.SetCenterY2(reach * scale)?;
        let swell = gfx.dcomp.CreateAnimation()?;
        beat(&swell, 1.0, SWELL - 1.0)?;
        grow.SetScaleX(&swell)?;
        grow.SetScaleY(&swell)?;
        visual.SetTransform(&grow)?;
        let fade = gfx.dcomp.CreateAnimation()?;
        beat(&fade, 0.9, -0.9)?;
        let effect = gfx.dcomp.CreateEffectGroup()?;
        effect.SetOpacity(&fade)?;
        visual.SetEffect(&effect)?;
        // On top: the call pill's background is opaque.
        holder.AddVisual(&visual, false, None)?;
    }
    Ok(visual)
}

/// A ramp from `from` by `by` over each [`PULSE_SECONDS`], repeated forever (see [`cycle`]).
fn beat(animation: &IDCompositionAnimation, from: f32, by: f32) -> Result<()> {
    cycle(animation, from, by, PULSE_SECONDS)
}

/// A ramp from `from` by `by` over each `period`, repeated forever, in step with one clock shared
/// by every animation since the app started: a repaint (another face speaking, a hover) picks the
/// pulses and waves up where they were instead of restarting them all from the first frame.
fn cycle(animation: &IDCompositionAnimation, from: f32, by: f32, period: f64) -> Result<()> {
    let phase = clock() % period;
    let slope = (by as f64 / period) as f32;
    // What is left of the current cycle, then whole cycles from the next one on.
    let next = period - phase;
    unsafe {
        animation.AddCubic(0.0, from + slope * phase as f32, slope, 0.0, 0.0)?;
        animation.AddCubic(next, from, slope, 0.0, 0.0)?;
        animation.AddRepeat(next + period, period)?;
    }
    Ok(())
}

/// An angle that holds at `start` for `wait` seconds, then grows at `rate` per second until 180,
/// where it stays. `rate` 0 holds forever.
fn piecewise(animation: &IDCompositionAnimation, start: f32, rate: f32, wait: f32) -> Result<()> {
    unsafe {
        if rate <= 0.0 || start >= 180.0 {
            animation.AddCubic(0.0, start, 0.0, 0.0, 0.0)?;
            return Ok(());
        }
        let wait = if start > 0.0 { 0.0 } else { (wait / rate) as f64 };
        if wait > 0.0 {
            animation.AddCubic(0.0, start, 0.0, 0.0, 0.0)?;
        }
        animation.AddCubic(wait, start, rate, 0.0, 0.0)?;
        let full = wait + ((180.0 - start) / rate) as f64;
        animation.End(full, 180.0)?;
    }
    Ok(())
}

/// `from` to `to` over `seconds`, decelerating (a cubic that lands with zero speed).
pub(super) fn ease_out(animation: &IDCompositionAnimation, from: f32, to: f32, seconds: f64) -> Result<()> {
    let d = (to - from) as f64;
    let t = seconds;
    // x(t) = from + d * (3s - 3s² + s³), s = t / T, written in t.
    unsafe {
        animation.AddCubic(0.0, from, (3.0 * d / t) as f32, (-3.0 * d / (t * t)) as f32, (d / (t * t * t)) as f32)?;
        animation.End(seconds, to)?;
    }
    Ok(())
}
