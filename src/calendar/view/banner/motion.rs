//! A banner's motion: springing in, gliding to its place in the stack, sliding away or flying into
//! the bell, each moved by its own frames.

use super::*;

/// How a banner leaves.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Exit {
    /// Sideways, fading (dismissed, gone from the list, the oldest pushed out).
    Slide,
    /// Shrinking into the bell, which rings (its time ran out: it is in the list now).
    ToBell,
    /// At once (the panel opened over them).
    Now,
}

/// A banner on its way in or out, moved by its own frames (never in a loop: the others keep
/// moving meanwhile, and so does everything else on the tray thread).
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Motion {
    /// Springing in from the side.
    Enter,
    /// Sliding away, fading.
    Slide,
    /// Shrinking into the bell from its middle `from`: the bell, at `bell`, rings as it lands.
    ToBell { from: (i32, i32), bell: (i32, i32) },
}

impl Motion {
    pub(super) fn length(self) -> Duration {
        Duration::from_millis(match self {
            Motion::Enter => 520,
            Motion::Slide => 180,
            Motion::ToBell { .. } => 380,
        })
    }
}

/// How long the others take to glide into the room a banner left (or out of a new one's way).
pub(super) const GLIDE: Duration = Duration::from_millis(320);

/// Leaving: out of the stack already (the others close up), its window still moving.
pub(super) fn leaving(s: &Shown) -> bool {
    matches!(s.motion, Some((Motion::Slide | Motion::ToBell { .. }, _)))
}

/// Springing in, `t` (0..1) of the way: how far from its place it is (pixels, toward the screen's
/// edge) and how faded.
pub(super) fn enter_at(t: f32, scale: f32, edge: Edge) -> (i32, u8) {
    let toward = if edge == Edge::Left { -1.0 } else { 1.0 };
    let slide = crate::win::FLYOUT_SLIDE * scale * 1.6;
    let offset = (toward * slide * (1.0 - spring(t))).round() as i32;
    (offset, ((t * 4.0).min(1.0) * 255.0) as u8)
}

/// A spring settling at 1 from 0, past it a little first.
pub(super) fn spring(t: f32) -> f32 {
    1.0 - (-7.0 * t).exp() * (10.0 * t).cos()
}

/// Banner `hwnd` goes as `exit` says; the others close up. The keyboard goes back to where it was
/// if its reply box had it.
pub(super) fn leave(b: &mut Banners, hwnd: HWND, exit: Exit) {
    let Some(at) = b.shown.iter().position(|s| s.hwnd == hwnd) else { return };
    if let Some(draft) = &b.shown[at].draft {
        // A reply half typed is kept for the panel's box, however the banner goes.
        drafts::keep(b.shown[at].toast.id, draft.text());
        give_keys_back(b, hwnd);
    }
    let exit = if animated() { exit } else { Exit::Now };
    // It goes from where it is: still springing in, from where the spring has it.
    let (scale, edge) = (b.scale, b.edge);
    if let Some((Motion::Enter, since)) = b.shown[at].motion {
        let (dx, _) = enter_at((since.elapsed().as_secs_f32() / Motion::Enter.length().as_secs_f32()).min(1.0), scale, edge);
        b.shown[at].pos.0 += dx;
    }
    let motion = match exit {
        Exit::Now => {
            finish(b, hwnd);
            return;
        }
        Exit::Slide => Motion::Slide,
        Exit::ToBell => {
            let s = &b.shown[at];
            let from = (s.pos.0 + b.px(s.scene.w) / 2, s.pos.1 + b.px(s.scene.h) / 2);
            // The bell where measured, else about where it sits by the taskbar's corner: in from
            // the work area's right, and below its foot (on the taskbar).
            const BELL_IN: f32 = 40.0;
            const BELL_BELOW: f32 = 24.0;
            let bell = crate::tray::bell_point().map_or((b.work.right - b.px(BELL_IN), b.work.bottom + b.px(BELL_BELOW)), |p| (p.x, p.y));
            Motion::ToBell { from, bell }
        }
    };
    // Shrinking within a window of its full size: the blur behind would stay that size.
    if matches!(motion, Motion::ToBell { .. }) {
        unsafe { crate::layered::set_acrylic(hwnd, false) };
    }
    // Going: the pointer passes through it to what is under it.
    unsafe {
        use windows::Win32::UI::WindowsAndMessaging::{GetWindowLongPtrW, SetWindowLongPtrW, GWL_EXSTYLE, WS_EX_TRANSPARENT};
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style | WS_EX_TRANSPARENT.0 as isize);
    }
    // Out of the stack at once (the others glide into its room) while it goes on its way.
    let s = &mut b.shown[at];
    s.motion = Some((motion, Instant::now()));
    s.glide = None;
    s.hover = None;
    unsafe {
        let _ = KillTimer(Some(hwnd), LIFE_TIMER);
    }
    b.place();
    b.pace(hwnd);
}

/// Banner `hwnd` gone for good: its window, and from the stack.
pub(super) fn finish(b: &mut Banners, hwnd: HWND) {
    let Some(at) = b.shown.iter().position(|s| s.hwnd == hwnd) else { return };
    let gone = b.shown.remove(at);
    unsafe {
        let _ = KillTimer(Some(gone.hwnd), LIFE_TIMER);
        let _ = KillTimer(Some(gone.hwnd), FRAME_TIMER);
        let _ = DestroyWindow(gone.hwnd);
    }
    let landed = matches!(gone.motion, Some((Motion::ToBell { .. }, _)));
    // Its drawing kit kept for the next (making one is the slow part of a banner's start).
    if b.spare.len() < MAX {
        b.spare.push(std::mem::ManuallyDrop::new(super::super::spare(gone.gfx)));
    }
    b.place();
    if landed {
        crate::taskbar::ring_bell();
    }
}

/// One frame of banner `hwnd`'s way in or out (once over, leaving, it is gone).
pub(super) fn step_motion(b: &mut Banners, hwnd: HWND) {
    let Some(s) = b.shown.iter_mut().find(|s| s.hwnd == hwnd) else { return };
    let Some((motion, since)) = s.motion else { return };
    let t = (since.elapsed().as_secs_f32() / motion.length().as_secs_f32()).min(1.0);
    let toward = if b.edge == Edge::Left { -1.0 } else { 1.0 };
    match motion {
        Motion::Enter => {
            // Still springing in when the stack moves (another arrived): it glides meanwhile too,
            // the spring on top, so it is never left to jump there once the spring ends.
            advance_glide(s);
            let (offset, alpha) = enter_at(t, b.scale, b.edge);
            crate::layered::Canvas::shift(hwnd, s.pos.0 + offset, s.pos.1, alpha);
            if t >= 1.0 {
                s.motion = None;
                crate::layered::Canvas::shift(hwnd, s.pos.0, s.pos.1, 255);
            }
        }
        Motion::Slide => {
            let slide = crate::win::FLYOUT_SLIDE * b.scale;
            let e = t * t;
            crate::layered::Canvas::shift(hwnd, s.pos.0 + (toward * slide * e).round() as i32, s.pos.1, ((1.0 - e) * 255.0) as u8);
            if t >= 1.0 {
                finish(b, hwnd);
            }
        }
        Motion::ToBell { from, bell } => {
            let k = t * t;
            let shrink = 1.0 - 0.85 * k;
            let centre = ((from.0 as f32 + (bell.0 - from.0) as f32 * k).round() as i32, (from.1 as f32 + (bell.1 - from.1) as f32 * k).round() as i32);
            b.render_zoomed(hwnd, shrink, centre, ((1.0 - k) * 255.0) as u8);
            if t >= 1.0 {
                finish(b, hwnd);
            }
        }
    }
}

/// One frame of banner `hwnd`'s glide to its place in the stack (eased out), only moved.
pub(super) fn step_glide(s: &mut Shown) {
    if advance_glide(s) {
        crate::layered::Canvas::shift(s.hwnd, s.pos.0, s.pos.1, 255);
    }
}

/// Banner `s`'s place on its glide this frame (`pos`), not yet shown; `false`: not gliding. Eased
/// in and out (a glide that starts at full speed shows its first frames as a jump), its clock
/// started at its first frame.
pub(super) fn advance_glide(s: &mut Shown) -> bool {
    let Some((since, from)) = s.glide else { return false };
    let since = since.unwrap_or_else(Instant::now);
    s.glide = Some((Some(since), from));
    let t = (since.elapsed().as_secs_f32() / GLIDE.as_secs_f32()).min(1.0);
    let e = glide_ease(t);
    s.pos = ((from.0 as f32 + (s.at.0 - from.0) as f32 * e).round() as i32, (from.1 as f32 + (s.at.1 - from.1) as f32 * e).round() as i32);
    if t >= 1.0 {
        s.pos = s.at;
        s.glide = None;
    }
    true
}

/// A glide's ease: slow out of its start, slow into its end, at most 1.5 times its mean speed
/// (smoothstep: a cubic in-and-out peaks at 3 times, a jump at a frame that comes late).
pub(super) fn glide_ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
