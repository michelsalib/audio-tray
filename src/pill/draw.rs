//! Lays out and paints each [`View`] into one composition surface, in DIPs. The progress ring's
//! moving part is not painted here: [`Scene::ring`] says where it goes and [`super`] builds it
//! from two half-rings the compositor rotates.

use anyhow::Result;

use super::gfx::{Font, Gfx, Painter, Rgba};
use super::view::{Level, View};
use crate::hub::Member;

/// Clickable parts of a scene.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hit {
    Previous,
    PlayPause,
    Next,
    /// Anywhere on the scene: the view's [`super::view::Action`], or the call's mute.
    Whole,
}

/// Where the progress ring sits, and how it moves.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Ring {
    pub cx: f32,
    pub cy: f32,
    pub r: f32,
    pub stroke: f32,
    /// How far through the track it is now, 0–1.
    pub fraction: f32,
    /// Fraction per second while playing; 0 when paused.
    pub rate: f32,
}

pub struct Scene {
    pub w: f32,
    pub h: f32,
    pub content: windows::Win32::Graphics::DirectComposition::IDCompositionVisual2,
    pub ring: Option<Ring>,
    pub hits: Vec<([f32; 4], Hit)>,
}

const WHITE: Rgba = Rgba::hex(0xFFFFFF, 1.0);
const DIM_WHITE: Rgba = Rgba::hex(0xFFFFFF, 0.72);
const QUIET_BG: Rgba = Rgba::hex(0x1C1C1C, 0.72);
const NORMAL_BG: Rgba = Rgba::hex(0x202020, 0.96);
/// Amber with near-black text: 10.9:1.
const WARN_BG: Rgba = Rgba::hex(0xF7B32B, 1.0);
const WARN_TEXT: Rgba = Rgba::hex(0x1A1A1A, 1.0);
/// Windows' error red with white text: 5.9:1.
const ALERT_BG: Rgba = Rgba::hex(0xC42B1C, 1.0);
/// A notice: white text on 0x6B3FD4 is 6.5:1.
const NOTICE_BG: Rgba = Rgba::hex(0x6B3FD4, 1.0);
const SLIVER: Rgba = Rgba::hex(0x9A9A9A, 0.6);
const SPEAKING: Rgba = Rgba::hex(0x3DD68C, 1.0);
const LIVE_DOT: Rgba = Rgba::hex(0xFF4D4D, 1.0);
const TRACK: Rgba = Rgba::hex(0xFFFFFF, 0.22);
pub const RING: Rgba = Rgba::hex(0xFFFFFF, 0.95);

/// The rounded end that sticks out from the left edge: shapes start left of 0 so only their
/// right side shows rounded.
const TUCK: f32 = 16.0;

const GLYPH_PREVIOUS: char = '\u{E892}';
const GLYPH_PLAY: char = '\u{E768}';
const GLYPH_PAUSE: char = '\u{E769}';
const GLYPH_NEXT: char = '\u{E893}';
const GLYPH_MIC: char = '\u{E720}';
const GLYPH_MIC_OFF: char = '\u{F781}';
const GLYPH_MUSIC: char = '\u{EC4F}';

/// Paint `view`. `None` for [`View::Hidden`].
pub fn scene(gfx: &Gfx, view: &View) -> Result<Option<Scene>> {
    Ok(Some(match view {
        View::Hidden => return Ok(None),
        View::Idle => {
            let (w, h) = (6.0, 56.0);
            let (content, _) = gfx.painted(w, h, |p| p.round_rect(-TUCK, 0.0, w + TUCK, h, 3.0, SLIVER))?;
            Scene { w, h, content, ring: None, hits: Vec::new() }
        }
        View::Music { media, card: false } => {
            let (w, h) = (52.0, 52.0);
            let (cx, cy) = (26.0, 26.0);
            let live = media.length_seconds().is_none();
            let (content, _) = gfx.painted(w, h, |p| {
                p.round_rect(-TUCK, 0.0, w + TUCK, h, h / 2.0, QUIET_BG)?;
                art(p, media.cover.as_deref().map(|c| c.as_slice()), cx - 15.0, cy - 15.0, 30.0, None)?;
                if live {
                    // A stream has no end to fill a ring towards: a badge on the art instead.
                    p.round_rect(cx - 12.0, cy + 9.0, 24.0, 11.0, 3.0, LIVE_DOT)?;
                    p.text("LIVE", Font::Badge, (cx - 12.0, cy + 9.0, 24.0, 11.0), WHITE)?;
                    Ok(())
                } else {
                    p.ring(cx, cy, 19.5, 3.0, TRACK)
                }
            })?;
            let ring = (!live).then(|| Ring {
                cx,
                cy,
                r: 19.5,
                stroke: 3.0,
                fraction: media.fraction_now().unwrap_or(0.0) as f32,
                rate: media.length_seconds().filter(|_| media.playing).map_or(0.0, |len| (1.0 / len) as f32),
            });
            Scene { w, h, content, ring, hits: Vec::new() }
        }
        View::Music { media, card: true } => {
            let (w, h) = (300.0, 100.0);
            let text_x = 84.0;
            let text_w = w - text_x - 16.0;
            let play = if media.playing { GLYPH_PAUSE } else { GLYPH_PLAY };
            // A live stream has nothing before or after it: play/pause only.
            let live = media.length_seconds().is_none();
            let buttons: Vec<(char, Hit)> = if live {
                vec![(play, Hit::PlayPause)]
            } else {
                vec![(GLYPH_PREVIOUS, Hit::Previous), (play, Hit::PlayPause), (GLYPH_NEXT, Hit::Next)]
            };
            let (content, _) = gfx.painted(w, h, |p| {
                p.round_rect(-TUCK, 0.0, w + TUCK, h, 18.0, NORMAL_BG)?;
                art(p, media.cover.as_deref().map(|c| c.as_slice()), 14.0, 14.0, 60.0, Some(8.0))?;
                p.text(&media.title, Font::Title, (text_x, 12.0, text_w, 20.0), WHITE)?;
                p.text(&media.artist, Font::Body, (text_x, 32.0, text_w, 18.0), DIM_WHITE)?;
                let app_w = gfx.measure(Font::Small, &media.app).min(text_w - 40.0);
                p.text(&media.app, Font::Small, (text_x, 50.0, app_w + 2.0, 14.0), Rgba::hex(0xFFFFFF, 0.55))?;
                if live {
                    let x = text_x + app_w + 8.0;
                    p.round_rect(x, 51.5, 26.0, 11.0, 3.0, LIVE_DOT)?;
                    p.text("LIVE", Font::Badge, (x, 51.5, 26.0, 11.0), WHITE)?;
                }
                for (i, (glyph, _)) in buttons.iter().enumerate() {
                    let x = text_x - 6.0 + i as f32 * 40.0;
                    p.text(&glyph.to_string(), Font::Icons, (x, 66.0, 32.0, 28.0), WHITE)?;
                }
                Ok(())
            })?;
            let hits = buttons
                .iter()
                .enumerate()
                .map(|(i, (_, hit))| ([text_x - 6.0 + i as f32 * 40.0, 64.0, 32.0, 32.0], *hit))
                .collect();
            Scene { w, h, content, ring: None, hits }
        }
        View::Call { app, holder, members, muted } => call(gfx, app.map(|a| a.label()).or(holder.as_deref()), members, *muted)?,
        View::Banner { level, glyph, text, .. } => {
            let (bg, fg) = match level {
                Level::Alert => (ALERT_BG, WHITE),
                Level::Warn => (WARN_BG, WARN_TEXT),
                _ => (NOTICE_BG, WHITE),
            };
            let text_w = gfx.measure(Font::Title, text).min(360.0);
            let (w, h) = (16.0 + 20.0 + 10.0 + text_w + 18.0, 44.0);
            let (content, _) = gfx.painted(w, h, |p| {
                p.round_rect(-TUCK, 0.0, w + TUCK, h, h / 2.0, bg)?;
                p.text(&glyph.to_string(), Font::Icons, (14.0, 0.0, 22.0, h), fg)?;
                p.text(text, Font::Title, (46.0, 0.0, text_w + 2.0, h), fg)
            })?;
            Scene { w, h, content, ring: None, hits: vec![([0.0, 0.0, w, h], Hit::Whole)] }
        }
    }))
}

/// Cover art, or a music glyph on a plain disc when there is none (or it will not decode).
fn art(p: &mut Painter, cover: Option<&[u8]>, x: f32, y: f32, size: f32, radius: Option<f32>) -> Result<()> {
    if let Some(bytes) = cover {
        if p.image(bytes, x, y, size, radius)? {
            return Ok(());
        }
    }
    let disc = Rgba::hex(0x3A3A3A, 1.0);
    match radius {
        Some(r) => p.round_rect(x, y, size, size, r, disc)?,
        None => p.circle(x + size / 2.0, y + size / 2.0, size / 2.0, disc)?,
    }
    p.text(&GLYPH_MUSIC.to_string(), Font::Icons, (x, y, size, size), DIM_WHITE)
}

/// The call pill: a column of faces (the speaking one lit green, its name beside it), then the
/// microphone with its live dot.
fn call(gfx: &Gfx, app: Option<&str>, members: &[Member], muted: bool) -> Result<Scene> {
    const FACE: f32 = 34.0;
    const STEP: f32 = 26.0;
    const MAX_FACES: usize = 5;
    let col = 48.0;
    let shown: Vec<&Member> = members.iter().take(MAX_FACES).collect();
    let extra = members.len().saturating_sub(MAX_FACES);
    let faces_h = if shown.is_empty() { 0.0 } else { FACE + STEP * (shown.len() as f32 - 1.0) + 8.0 };
    let label_h = if shown.is_empty() { 16.0 } else { 0.0 };
    let h = 8.0 + faces_h + label_h + 40.0;
    let speaker = shown.iter().position(|m| m.speaking);
    let tag_w = speaker.map_or(0.0, |i| gfx.measure(Font::Small, &shown[i].name).min(160.0) + 20.0);
    let w = col + if tag_w > 0.0 { tag_w + 6.0 } else { 0.0 };
    let (content, _) = gfx.painted(w, h, |p| {
        p.round_rect(-TUCK, 0.0, col + TUCK, h, col / 2.0, NORMAL_BG)?;
        let cx = col / 2.0;
        for (i, member) in shown.iter().enumerate() {
            let top = 8.0 + i as f32 * STEP;
            let lit = Some(i) == speaker;
            face(p, member, cx - FACE / 2.0, top, FACE, lit)?;
            if lit {
                let ty = top + FACE / 2.0 - 11.0;
                p.round_rect(col + 6.0, ty, tag_w, 22.0, 11.0, NORMAL_BG)?;
                p.text(&member.name, Font::Small, (col + 16.0, ty, tag_w - 20.0, 22.0), WHITE)?;
            }
        }
        if extra > 0 {
            p.text(&format!("+{extra}"), Font::Small, (0.0, faces_h - 6.0, col, 12.0), DIM_WHITE)?;
        }
        if shown.is_empty() {
            if let Some(app) = app {
                p.text(&app.chars().take(2).collect::<String>(), Font::Small, (0.0, 8.0, col, 16.0), DIM_WHITE)?;
            }
        }
        let my = h - 24.0;
        p.circle(cx, my, 16.0, if muted { Rgba::hex(0x5A5A5A, 1.0) } else { Rgba::hex(0x3A3A3A, 1.0) })?;
        p.text(&(if muted { GLYPH_MIC_OFF } else { GLYPH_MIC }).to_string(), Font::Icons, (cx - 16.0, my - 16.0, 32.0, 32.0), WHITE)?;
        if !muted {
            p.circle(cx + 11.0, my - 11.0, 4.5, LIVE_DOT)?;
        }
        Ok(())
    })?;
    Ok(Scene { w, h, content, ring: None, hits: vec![([0.0, 0.0, col, h], Hit::Whole)] })
}

/// One face: the avatar, or an initial on a colour from the name; dimmed unless speaking.
fn face(p: &mut Painter, member: &Member, x: f32, y: f32, size: f32, lit: bool) -> Result<()> {
    let (cx, cy, r) = (x + size / 2.0, y + size / 2.0, size / 2.0);
    // A dark rim separates overlapping faces.
    p.circle(cx, cy, r + 2.0, NORMAL_BG)?;
    let drawn = match &member.avatar {
        Some(bytes) => p.image(bytes, x, y, size, None)?,
        None => false,
    };
    if !drawn {
        let hue = member.name.bytes().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
        const PALETTE: [u32; 6] = [0x5865F2, 0x2D7D9A, 0x8E44AD, 0xC0392B, 0x16A085, 0xD35400];
        p.circle(cx, cy, r, Rgba::hex(PALETTE[hue as usize % PALETTE.len()], 1.0))?;
        let initial: String = member.name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
        p.text(&initial, Font::Initial, (x, y, size, size), WHITE)?;
    }
    if lit {
        p.ring(cx, cy, r + 1.0, 2.5, SPEAKING)?;
    } else {
        p.circle(cx, cy, r, Rgba::hex(0x000000, 0.45))?;
    }
    if member.muted {
        p.circle(cx + r * 0.7, cy + r * 0.7, 7.0, NORMAL_BG)?;
        p.text(&GLYPH_MIC_OFF.to_string(), Font::IconsSmall, (cx + r * 0.7 - 7.0, cy + r * 0.7 - 7.0, 14.0, 14.0), WHITE)?;
    }
    Ok(())
}
