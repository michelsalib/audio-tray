//! Lays out and paints each [`View`] into one composition surface, in DIPs. The progress ring's
//! moving part is not painted here: [`Scene::rings`] says where they go and [`super`] builds it
//! from two half-rings the compositor rotates.

use anyhow::Result;

use super::gfx::{Font, Gfx, Painter, Rgba};
use super::view::{Device, Devices, Level, View};
use crate::audio::Flow;
use crate::hub::Member;
use crate::music::smtc::Command;

/// Clickable parts of a scene.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hit {
    /// A transport button on the card's row for session `.0`.
    Media(usize, Command),
    /// A device button: switch that flow to its next device.
    Device(crate::audio::Flow),
    /// Anywhere on the scene: the view's [`super::view::Action`], or the call's mute.
    Whole,
}

/// A ring that pulses out from a face while its owner speaks, animated on repeat by DWM.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Pulse {
    pub cx: f32,
    pub cy: f32,
    pub r: f32,
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
    pub rings: Vec<Ring>,
    pub pulses: Vec<Pulse>,
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
/// The speaking pulse: the speaking ring's green.
pub const PULSE: Rgba = SPEAKING;

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
        View::Idle { devices: None } => {
            let (w, h) = (6.0, 56.0);
            let (content, _) = gfx.painted(w, h, |p| p.round_rect(-TUCK, 0.0, w + TUCK, h, 3.0, SLIVER))?;
            Scene { w, h, content, rings: Vec::new(), pulses: Vec::new(), hits: Vec::new() }
        }
        View::Idle { devices: Some(devices) } => {
            // Just the devices, in a small pill of their own.
            let w = 52.0;
            let h = 8.0 + DEVICE_STEP * device_count(devices) as f32;
            let mut hits = Vec::new();
            let (content, _) = gfx.painted(w, h, |p| {
                p.round_rect(-TUCK, 0.0, w + TUCK, h, w / 2.0, QUIET_BG)?;
                device_column(p, devices, 4.0, &mut hits)
            })?;
            Scene { w, h, content, rings: Vec::new(), pulses: Vec::new(), hits }
        }
        View::Music { media, card: false, devices } => {
            // One disc per playing session, stacked in the same pill, the devices under them.
            const STEP: f32 = 44.0;
            let w = 52.0;
            let discs_h = 52.0 + STEP * (media.len() as f32 - 1.0);
            let devices_h = devices.as_ref().map_or(0.0, |d| 6.0 + DEVICE_STEP * device_count(d) as f32);
            let h = discs_h + devices_h;
            let mut hits = Vec::new();
            let (content, _) = gfx.painted(w, h, |p| {
                p.round_rect(-TUCK, 0.0, w + TUCK, h, w / 2.0, QUIET_BG)?;
                for (i, m) in media.iter().enumerate() {
                    let (cx, cy) = (26.0, 26.0 + i as f32 * STEP);
                    art(p, m.cover.as_deref().map(|c| c.as_slice()), cx - 15.0, cy - 15.0, 30.0, None)?;
                    if m.length_seconds().is_none() {
                        // A stream has no end to fill a ring towards: a badge on the art instead.
                        p.round_rect(cx - 12.0, cy + 9.0, 24.0, 11.0, 3.0, LIVE_DOT)?;
                        p.text("LIVE", Font::Badge, (cx - 12.0, cy + 9.0, 24.0, 11.0), WHITE)?;
                    } else {
                        p.ring(cx, cy, 19.5, 3.0, TRACK)?;
                    }
                }
                if let Some(devices) = devices {
                    p.round_rect(14.0, discs_h + 1.0, w - 28.0, 1.0, 0.0, Rgba::hex(0xFFFFFF, 0.14))?;
                    device_column(p, devices, discs_h + 4.0, &mut hits)?;
                }
                Ok(())
            })?;
            let rings = media
                .iter()
                .enumerate()
                .filter(|(_, m)| m.length_seconds().is_some())
                .map(|(i, m)| Ring {
                    cx: 26.0,
                    cy: 26.0 + i as f32 * STEP,
                    r: 19.5,
                    stroke: 3.0,
                    fraction: m.fraction_now().unwrap_or(0.0) as f32,
                    rate: m.length_seconds().filter(|_| m.playing).map_or(0.0, |len| (1.0 / len) as f32),
                })
                .collect();
            Scene { w, h, content, rings, pulses: Vec::new(), hits }
        }
        View::Music { media, card: true, devices } => {
            // One row per session, each with its own controls; a row of devices closes the card.
            const ROW: f32 = 92.0;
            const DEVICES_ROW: f32 = 48.0;
            let w = 300.0;
            let rows_h = ROW * media.len() as f32 + 8.0;
            let h = rows_h + devices.as_ref().map_or(0.0, |_| DEVICES_ROW);
            let text_x = 84.0;
            let text_w = w - text_x - 16.0;
            let rows: Vec<(f32, Vec<(char, Command)>)> = media
                .iter()
                .enumerate()
                .map(|(i, m)| {
                    let play = if m.playing { GLYPH_PAUSE } else { GLYPH_PLAY };
                    // Previous/next only where they reach the player (not live, not a browser tab).
                    let buttons = if !m.skip {
                        vec![(play, Command::TogglePlayPause)]
                    } else {
                        vec![(GLYPH_PREVIOUS, Command::Previous), (play, Command::TogglePlayPause), (GLYPH_NEXT, Command::Next)]
                    };
                    (4.0 + i as f32 * ROW, buttons)
                })
                .collect();
            let mut hits: Vec<([f32; 4], Hit)> = rows
                .iter()
                .enumerate()
                .flat_map(|(i, (top, buttons))| {
                    buttons
                        .iter()
                        .enumerate()
                        .map(move |(b, (_, command))| ([text_x - 6.0 + b as f32 * 40.0, top + 58.0, 32.0, 32.0], Hit::Media(i, *command)))
                })
                .collect();
            let (content, _) = gfx.painted(w, h, |p| {
                p.round_rect(-TUCK, 0.0, w + TUCK, h, 18.0, NORMAL_BG)?;
                for (m, (top, buttons)) in media.iter().zip(&rows) {
                    let top = *top;
                    if top > 4.0 {
                        p.round_rect(14.0, top - 1.0, w - 28.0, 1.0, 0.0, Rgba::hex(0xFFFFFF, 0.10))?;
                    }
                    art(p, m.cover.as_deref().map(|c| c.as_slice()), 14.0, top + 12.0, 60.0, Some(8.0))?;
                    p.text(&m.title, Font::Title, (text_x, top + 8.0, text_w, 20.0), WHITE)?;
                    p.text(&m.artist, Font::Body, (text_x, top + 28.0, text_w, 18.0), DIM_WHITE)?;
                    let app_w = gfx.measure(Font::Small, &m.app).min(text_w - 40.0);
                    p.text(&m.app, Font::Small, (text_x, top + 46.0, app_w + 2.0, 14.0), Rgba::hex(0xFFFFFF, 0.55))?;
                    if m.length_seconds().is_none() {
                        let x = text_x + app_w + 8.0;
                        p.round_rect(x, top + 47.5, 26.0, 11.0, 3.0, LIVE_DOT)?;
                        p.text("LIVE", Font::Badge, (x, top + 47.5, 26.0, 11.0), WHITE)?;
                    }
                    for (b, (glyph, _)) in buttons.iter().enumerate() {
                        let x = text_x - 6.0 + b as f32 * 40.0;
                        p.text(&glyph.to_string(), Font::Icons, (x, top + 60.0, 32.0, 28.0), WHITE)?;
                    }
                }
                if let Some(devices) = devices {
                    p.round_rect(14.0, rows_h - 3.0, w - 28.0, 1.0, 0.0, Rgba::hex(0xFFFFFF, 0.10))?;
                    device_row(p, devices, rows_h, w, &mut hits)?;
                }
                Ok(())
            })?;
            Scene { w, h, content, rings: Vec::new(), pulses: Vec::new(), hits }
        }        View::Call { app, holder, members, muted } => call(gfx, app.map(|a| a.label()).or(holder.as_deref()), members, *muted)?,
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
            Scene { w, h, content, rings: Vec::new(), pulses: Vec::new(), hits: vec![([0.0, 0.0, w, h], Hit::Whole)] }
        }
    }))
}

/// Height of one device button in the pill's column.
const DEVICE_STEP: f32 = 40.0;
const GLYPH_VOLUME_MUTED: char = '\u{E74F}';

fn device_count(devices: &Devices) -> usize {
    usize::from(devices.output.is_some()) + usize::from(devices.input.is_some())
}

/// The output then the input, as round buttons down the pill's column from `top`.
fn device_column(p: &mut Painter, devices: &Devices, top: f32, hits: &mut Vec<([f32; 4], Hit)>) -> Result<()> {
    let mut y = top;
    for (flow, device) in [(Flow::Output, &devices.output), (Flow::Input, &devices.input)] {
        let Some(device) = device else { continue };
        let (cx, cy) = (26.0, y + DEVICE_STEP / 2.0);
        device_button(p, flow, device, cx, cy)?;
        hits.push(([4.0, y, 44.0, DEVICE_STEP], Hit::Device(flow)));
        y += DEVICE_STEP;
    }
    Ok(())
}

/// The output and input side by side, each with its name, along the card's bottom.
fn device_row(p: &mut Painter, devices: &Devices, top: f32, w: f32, hits: &mut Vec<([f32; 4], Hit)>) -> Result<()> {
    let half = (w - 16.0) / 2.0;
    for (i, (flow, device)) in [(Flow::Output, &devices.output), (Flow::Input, &devices.input)].into_iter().enumerate() {
        let Some(device) = device else { continue };
        let x = 8.0 + i as f32 * half;
        device_button(p, flow, device, x + 20.0, top + 22.0)?;
        p.text(&device.name, Font::Small, (x + 42.0, top + 8.0, half - 48.0, 28.0), DIM_WHITE)?;
        hits.push(([x, top, half, 44.0], Hit::Device(flow)));
    }
    Ok(())
}

/// One device: its glyph on a disc (the mute glyph when muted), and the recording dot.
fn device_button(p: &mut Painter, flow: Flow, device: &Device, cx: f32, cy: f32) -> Result<()> {
    p.circle(cx, cy, 16.0, if device.muted { Rgba::hex(0x5A5A5A, 1.0) } else { Rgba::hex(0x3A3A3A, 1.0) })?;
    let glyph = match (flow, device.muted) {
        (Flow::Output, true) => GLYPH_VOLUME_MUTED,
        (Flow::Input, true) => GLYPH_MIC_OFF,
        _ => device.glyph,
    };
    p.text(&glyph.to_string(), Font::Icons, (cx - 16.0, cy - 16.0, 32.0, 32.0), WHITE)?;
    if device.recording {
        p.circle(cx + 11.0, cy - 11.0, 4.5, LIVE_DOT)?;
    }
    Ok(())
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
    // The speaker's face pulses (see [`Pulse`]).
    let pulses = speaker
        .map(|i| Pulse { cx: col / 2.0, cy: 8.0 + i as f32 * STEP + FACE / 2.0, r: FACE / 2.0 + 1.0 })
        .into_iter()
        .collect();
    Ok(Scene { w, h, content, rings: Vec::new(), pulses, hits: vec![([0.0, 0.0, col, h], Hit::Whole)] })
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
