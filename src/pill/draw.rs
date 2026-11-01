//! Lays out and paints each [`View`] into one composition surface, in DIPs. The progress ring's
//! moving part is not painted here: [`Scene::rings`] says where they go and [`super`] builds it
//! from two half-rings the compositor rotates.

use anyhow::Result;

use super::gfx::{Font, Gfx, Painter, Rgba};
use super::view::{CallPill, Device, Devices, Level, View};
use crate::audio::Flow;
use crate::hub::Member;
use crate::music::smtc::Command;

/// Clickable parts of a scene.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hit {
    /// A transport button on the card's row for session `.0`.
    Media(usize, Command),
    /// A session's cover: bring its app or tab to the front.
    Focus(usize),
    /// The call: toggle the user's mute (in the voice app, else the microphone).
    CallMute,
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
/// The quiet column's dimensions at one pill size: [`BIG`], or [`SMALL`] at about half.
#[derive(Clone, Copy)]
struct Metrics {
    /// The column's width.
    col: f32,
    disc_step: f32,
    /// A cover's diameter, its ring's radius and stroke.
    art: f32,
    ring_r: f32,
    ring_stroke: f32,
    /// A device button's step down the column, radius and glyph font.
    dev_step: f32,
    dev_r: f32,
    dev_font: Font,
    /// A face's diameter and step in the call column.
    face: f32,
    face_step: f32,
    /// Room for words: the LIVE badge, a speaker's name tag, mute badges on faces.
    words: bool,
}

const BIG: Metrics = Metrics {
    col: 52.0,
    disc_step: 44.0,
    art: 30.0,
    ring_r: 19.5,
    ring_stroke: 3.0,
    dev_step: 40.0,
    dev_r: 16.0,
    dev_font: Font::Icons,
    face: 34.0,
    face_step: 26.0,
    words: true,
};

const SMALL: Metrics = Metrics {
    col: 28.0,
    disc_step: 25.0,
    art: 16.0,
    ring_r: 10.5,
    ring_stroke: 2.0,
    dev_step: 24.0,
    dev_r: 10.0,
    dev_font: Font::IconsSmall,
    face: 18.0,
    face_step: 14.0,
    words: false,
};

/// Paint `view`, the quiet states `small` or not. `None` for [`View::Hidden`].
pub fn scene(gfx: &Gfx, view: &View, small: bool) -> Result<Option<Scene>> {
    let m = if small { SMALL } else { BIG };
    Ok(Some(match view {
        View::Hidden => return Ok(None),
        View::Idle { call: None, devices: None } => {
            let (w, h) = (6.0, 56.0);
            let (content, _) = gfx.painted(w, h, |p| p.round_rect(-TUCK, 0.0, w + TUCK, h, 3.0, SLIVER))?;
            Scene { w, h, content, rings: Vec::new(), pulses: Vec::new(), hits: Vec::new() }
        }
        View::Idle { call, devices } => stack(gfx, m, &[], call.as_ref(), devices.as_ref())?,
        View::Music { media, card: false, call, devices } => stack(gfx, m, media, call.as_ref(), devices.as_ref())?,
        View::Music { media, card: true, call, devices } => {
            // The call's row, one row per session with its own controls, a row of devices.
            const CALL_ROW: f32 = 60.0;
            const ROW: f32 = 92.0;
            const DEVICES_ROW: f32 = 48.0;
            let w = 300.0;
            let call_h = if call.is_some() { CALL_ROW } else { 0.0 };
            let rows_h = call_h + ROW * media.len() as f32 + 8.0;
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
                    (call_h + 4.0 + i as f32 * ROW, buttons)
                })
                .collect();
            let mut hits: Vec<([f32; 4], Hit)> = rows
                .iter()
                .enumerate()
                .flat_map(|(i, (top, buttons))| {
                    let art = std::iter::once(([14.0, top + 12.0, 60.0, 60.0], Hit::Focus(i)));
                    art.chain(
                        buttons
                            .iter()
                            .enumerate()
                            .map(move |(b, (_, command))| ([text_x - 6.0 + b as f32 * 40.0, top + 58.0, 32.0, 32.0], Hit::Media(i, *command))),
                    )
                })
                .collect();
            if call.is_some() {
                hits.push(([w - 54.0, 12.0, 40.0, 40.0], Hit::CallMute));
            }
            let (content, _) = gfx.painted(w, h, |p| {
                p.round_rect(-TUCK, 0.0, w + TUCK, h, 18.0, NORMAL_BG)?;
                if let Some(call) = call {
                    call_row(p, call, w)?;
                }
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
        }        View::Banner { level, glyph, text, .. } => {
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

const GLYPH_VOLUME_MUTED: char = '\u{E74F}';

fn device_count(devices: &Devices) -> usize {
    usize::from(devices.output.is_some()) + usize::from(devices.input.is_some())
}

/// The output then the input, as round buttons down the pill's column from `top`.
fn device_column(p: &mut Painter, devices: &Devices, top: f32, hits: &mut Vec<([f32; 4], Hit)>, m: Metrics) -> Result<()> {
    let mut y = top;
    for (flow, device) in [(Flow::Output, &devices.output), (Flow::Input, &devices.input)] {
        let Some(device) = device else { continue };
        device_button(p, flow, device, m.col / 2.0, y + m.dev_step / 2.0, m)?;
        hits.push(([0.0, y, m.col, m.dev_step], Hit::Device(flow)));
        y += m.dev_step;
    }
    Ok(())
}

/// The output and input side by side, each with its name, along the card's bottom.
fn device_row(p: &mut Painter, devices: &Devices, top: f32, w: f32, hits: &mut Vec<([f32; 4], Hit)>) -> Result<()> {
    let half = (w - 16.0) / 2.0;
    for (i, (flow, device)) in [(Flow::Output, &devices.output), (Flow::Input, &devices.input)].into_iter().enumerate() {
        let Some(device) = device else { continue };
        let x = 8.0 + i as f32 * half;
        device_button(p, flow, device, x + 20.0, top + 22.0, BIG)?;
        p.text(&device.name, Font::Small, (x + 42.0, top + 8.0, half - 48.0, 28.0), DIM_WHITE)?;
        hits.push(([x, top, half, 44.0], Hit::Device(flow)));
    }
    Ok(())
}

/// One device: its glyph on a disc (the mute glyph when muted), and the recording dot.
fn device_button(p: &mut Painter, flow: Flow, device: &Device, cx: f32, cy: f32, m: Metrics) -> Result<()> {
    let r = m.dev_r;
    p.circle(cx, cy, r, if device.muted { Rgba::hex(0x5A5A5A, 1.0) } else { Rgba::hex(0x3A3A3A, 1.0) })?;
    let glyph = match (flow, device.muted) {
        (Flow::Output, true) => GLYPH_VOLUME_MUTED,
        (Flow::Input, true) => GLYPH_MIC_OFF,
        _ => device.glyph,
    };
    p.text(&glyph.to_string(), m.dev_font, (cx - r, cy - r, 2.0 * r, 2.0 * r), WHITE)?;
    if device.recording {
        p.circle(cx + r * 0.7, cy - r * 0.7, r * 0.28, LIVE_DOT)?;
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

/// The gap between two pills of the column.
const PILL_GAP: f32 = 6.0;

/// The quiet column: separate pills for the playing sessions, the call and the devices, each
/// only when there is one, top to bottom with a gap between.
fn stack(gfx: &Gfx, m: Metrics, media: &[crate::hub::Media], call: Option<&CallPill>, devices: Option<&Devices>) -> Result<Scene> {
    let col = m.col;
    let cx = col / 2.0;
    let music_h = if media.is_empty() { 0.0 } else { col + m.disc_step * (media.len() as f32 - 1.0) };
    let call_layout = call.map(|c| CallLayout::of(gfx, c, m));
    let pad = (col - 2.0 * m.dev_r) / 4.0;
    let devices_h = devices.map_or(0.0, |d| 2.0 * pad + m.dev_step * device_count(d) as f32);
    let heights: Vec<f32> = [music_h, call_layout.as_ref().map_or(0.0, |l| l.h), devices_h].into_iter().collect();
    let tops: Vec<f32> = heights
        .iter()
        .scan(0.0, |y, &h| {
            let top = *y;
            if h > 0.0 {
                *y += h + PILL_GAP;
            }
            Some(top)
        })
        .collect();
    let h = heights.iter().filter(|h| **h > 0.0).map(|h| h + PILL_GAP).sum::<f32>() - PILL_GAP;
    let w = col + call_layout.as_ref().map_or(0.0, |l| if l.tag_w > 0.0 { l.tag_w + 6.0 } else { 0.0 });
    let mut hits = Vec::new();
    let mut pulses = Vec::new();
    let (content, _) = gfx.painted(w, h, |p| {
        if music_h > 0.0 {
            let top = tops[0];
            p.round_rect(-TUCK, top, col + TUCK, music_h, col / 2.0, QUIET_BG)?;
            for (i, media) in media.iter().enumerate() {
                let cy = top + cx + i as f32 * m.disc_step;
                let half = m.art / 2.0;
                // Clickable only when the pill is; quiet, clicks pass through.
                hits.push(([0.0, cy - m.disc_step / 2.0, col, m.disc_step], Hit::Focus(i)));
                art(p, media.cover.as_deref().map(|c| c.as_slice()), cx - half, cy - half, m.art, None)?;
                if media.length_seconds().is_some() {
                    p.ring(cx, cy, m.ring_r, m.ring_stroke, TRACK)?;
                } else if m.words {
                    // A stream has no end to fill a ring towards: a badge on the art instead.
                    p.round_rect(cx - 12.0, cy + 9.0, 24.0, 11.0, 3.0, LIVE_DOT)?;
                    p.text("LIVE", Font::Badge, (cx - 12.0, cy + 9.0, 24.0, 11.0), WHITE)?;
                } else {
                    // Small: a red dot says live.
                    let (dx, dy) = (cx + half - 1.0, cy + half - 1.0);
                    p.circle(dx, dy, 4.0, QUIET_BG)?;
                    p.circle(dx, dy, 3.0, LIVE_DOT)?;
                }
            }
        }
        if let (Some(call), Some(layout)) = (call, &call_layout) {
            pulses.extend(paint_call(p, call, layout, m, tops[1]));
            hits.push(([0.0, tops[1], col, layout.h], Hit::CallMute));
        }
        if let Some(devices) = devices {
            p.round_rect(-TUCK, tops[2], col + TUCK, devices_h, col / 2.0, QUIET_BG)?;
            device_column(p, devices, tops[2] + pad, &mut hits, m)?;
        }
        Ok(())
    })?;
    let rings = media
        .iter()
        .enumerate()
        .filter(|(_, media)| media.length_seconds().is_some())
        .map(|(i, media)| Ring {
            cx,
            cy: tops[0] + cx + i as f32 * m.disc_step,
            r: m.ring_r,
            stroke: m.ring_stroke,
            fraction: media.fraction_now().unwrap_or(0.0) as f32,
            rate: media.length_seconds().filter(|_| media.playing).map_or(0.0, |len| (1.0 / len) as f32),
        })
        .collect();
    Ok(Scene { w, h, content, rings, pulses, hits })
}

/// The call pill's geometry, measured before painting.
struct CallLayout {
    h: f32,
    shown: usize,
    speaker: Option<usize>,
    /// The speaker's name tag, beside the column (0 for none).
    tag_w: f32,
}

impl CallLayout {
    const MAX_FACES: usize = 5;

    fn of(gfx: &Gfx, call: &CallPill, m: Metrics) -> Self {
        let shown = call.members.len().min(Self::MAX_FACES);
        // No faces: the app's icon says whose call it is, else its name's first letters.
        let faces_h = match shown {
            0 if call.icon.is_some() => m.face + 8.0,
            0 => 0.0,
            _ => m.face + m.face_step * (shown as f32 - 1.0) + 8.0,
        };
        let label_h = if shown == 0 && call.icon.is_none() && m.words { 16.0 } else { 0.0 };
        let speaker = call.members.iter().take(shown).position(|member| member.speaking);
        // Small has no room for the name: the lit face and its pulse say who.
        let tag_w = speaker.filter(|_| m.words).map_or(0.0, |i| gfx.measure(Font::Small, &call.members[i].name).min(160.0) + 20.0);
        Self { h: 8.0 + faces_h + label_h + 2.0 * m.dev_r + 8.0, shown, speaker, tag_w }
    }
}

/// The call pill at `top`: a column of faces (the speaking one lit green, its name beside it),
/// then the microphone with its live dot. Returns the speaker's pulse.
fn paint_call(p: &mut Painter, call: &CallPill, layout: &CallLayout, m: Metrics, top: f32) -> Option<Pulse> {
    let col = m.col;
    let cx = col / 2.0;
    let mut paint = || -> Result<()> {
        p.round_rect(-TUCK, top, col + TUCK, layout.h, col / 2.0, NORMAL_BG)?;
        for (i, member) in call.members.iter().take(layout.shown).enumerate() {
            let y = top + 8.0 + i as f32 * m.face_step;
            let lit = Some(i) == layout.speaker;
            face(p, member, cx - m.face / 2.0, y, m.face, lit, m.words)?;
            if lit && layout.tag_w > 0.0 {
                let ty = y + m.face / 2.0 - 11.0;
                p.round_rect(col + 6.0, ty, layout.tag_w, 22.0, 11.0, NORMAL_BG)?;
                p.text(&member.name, Font::Small, (col + 16.0, ty, layout.tag_w - 20.0, 22.0), WHITE)?;
            }
        }
        let extra = call.members.len().saturating_sub(layout.shown);
        if extra > 0 && m.words {
            let y = top + 8.0 + m.face + m.face_step * (layout.shown as f32 - 1.0);
            p.text(&format!("+{extra}"), Font::Small, (0.0, y - 2.0, col, 12.0), DIM_WHITE)?;
        }
        if let (0, Some(icon)) = (layout.shown, &call.icon) {
            p.image(icon, cx - m.face / 2.0, top + 8.0, m.face, Some(0.0))?;
        } else if layout.shown == 0 && m.words {
            if let Some(app) = call.app.map(|a| a.label()).or(call.holder.as_deref()) {
                p.text(&app.chars().take(2).collect::<String>(), Font::Small, (0.0, top + 8.0, col, 16.0), DIM_WHITE)?;
            }
        }
        let (my, r) = (top + layout.h - m.dev_r - 6.0, m.dev_r);
        p.circle(cx, my, r, if call.muted { Rgba::hex(0x5A5A5A, 1.0) } else { Rgba::hex(0x3A3A3A, 1.0) })?;
        p.text(&(if call.muted { GLYPH_MIC_OFF } else { GLYPH_MIC }).to_string(), m.dev_font, (cx - r, my - r, 2.0 * r, 2.0 * r), WHITE)?;
        if !call.muted {
            p.circle(cx + r * 0.7, my - r * 0.7, r * 0.28, LIVE_DOT)?;
        }
        Ok(())
    };
    if let Err(e) = paint() {
        eprintln!("pill: could not paint the call ({e:#})");
    }
    layout.speaker.map(|i| Pulse { cx, cy: top + 8.0 + i as f32 * m.face_step + m.face / 2.0, r: m.face / 2.0 + 1.0 })
}

/// The card's call row: overlapping faces, the channel (or app) and who is talking, and the mute.
fn call_row(p: &mut Painter, call: &CallPill, w: f32) -> Result<()> {
    const FACE: f32 = 28.0;
    let mut x = 14.0;
    for member in call.members.iter().take(4) {
        face(p, member, x, 16.0, FACE, member.speaking, false)?;
        x += FACE * 0.7;
    }
    if call.members.is_empty() {
        match &call.icon {
            Some(icon) => {
                p.image(icon, 14.0, 16.0, FACE, Some(0.0))?;
            }
            None => p.text(&GLYPH_MIC.to_string(), Font::Icons, (14.0, 16.0, FACE, FACE), WHITE)?,
        }
        x += FACE;
    }
    let text_x = x + FACE * 0.3 + 8.0;
    let text_w = w - text_x - 62.0;
    let title = match (call.app, &call.channel) {
        (Some(app), Some(channel)) => format!("{} · {channel}", app.label()),
        (Some(app), None) => app.label().to_string(),
        (None, _) => call.holder.clone().unwrap_or_else(|| "Call".to_string()),
    };
    let speaking: Vec<&str> = call.members.iter().filter(|m| m.speaking).map(|m| m.name.as_str()).collect();
    let status = if !speaking.is_empty() {
        format!("{} talking", speaking.join(", "))
    } else if call.muted {
        "You are muted".to_string()
    } else {
        format!("{} in the call", call.members.len().max(1))
    };
    p.text(&title, Font::Title, (text_x, 12.0, text_w, 20.0), WHITE)?;
    p.text(&status, Font::Small, (text_x, 32.0, text_w, 16.0), DIM_WHITE)?;
    let (bx, by) = (w - 34.0, 32.0);
    p.circle(bx, by, 16.0, if call.muted { Rgba::hex(0x5A5A5A, 1.0) } else { Rgba::hex(0x3A3A3A, 1.0) })?;
    p.text(&(if call.muted { GLYPH_MIC_OFF } else { GLYPH_MIC }).to_string(), Font::Icons, (bx - 16.0, by - 16.0, 32.0, 32.0), WHITE)
}
/// One face: the avatar, or an initial on a colour from the name; dimmed unless speaking.
fn face(p: &mut Painter, member: &Member, x: f32, y: f32, size: f32, lit: bool, badges: bool) -> Result<()> {
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
        p.text(&initial, if badges { Font::Initial } else { Font::Badge }, (x, y, size, size), WHITE)?;
    }
    if lit {
        p.ring(cx, cy, r + 1.0, if badges { 2.5 } else { 1.5 }, SPEAKING)?;
    } else {
        p.circle(cx, cy, r, Rgba::hex(0x000000, 0.45))?;
    }
    if member.muted && badges {
        p.circle(cx + r * 0.7, cy + r * 0.7, 7.0, NORMAL_BG)?;
        p.text(&GLYPH_MIC_OFF.to_string(), Font::IconsSmall, (cx + r * 0.7 - 7.0, cy + r * 0.7 - 7.0, 14.0, 14.0), WHITE)?;
    }
    Ok(())
}
