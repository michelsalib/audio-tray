//! Lays out and paints each [`View`] into one composition surface, in DIPs. The progress ring's
//! moving part is not painted here: [`Scene::rings`] says where they go and [`super`] builds it
//! from two half-rings the compositor rotates.

use anyhow::Result;

use super::gfx::{Font, Gfx, Painter, Rgba};
use super::view::{CallPill, Device, Devices, Level, SongNote, View};
use crate::audio::Flow;
use crate::config::PillEdge;
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
    /// A new song's note: bring the player to the front.
    Song,
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
    /// The accent while playing, yellow while paused (as the taskbar's progress bars).
    pub color: Rgba,
}

/// A card row's progress bar: the filled part grows from `fraction` of `w` to all of it, at
/// `rate` (fraction per second; 0 when paused), animated by DWM like the ring.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Bar {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub fraction: f32,
    pub rate: f32,
    pub color: Rgba,
}

pub struct Scene {
    pub w: f32,
    pub h: f32,
    /// How much of the start, along the edge, comes before what must not move (a song's note
    /// before the pills): the window is centred on the rest.
    pub lead: f32,
    pub content: windows::Win32::Graphics::DirectComposition::IDCompositionVisual2,
    pub rings: Vec<Ring>,
    /// The card's progress bars' moving parts (their tracks are in `content`).
    pub bars: Vec<Bar>,
    pub pulses: Vec<Pulse>,
    pub hits: Vec<([f32; 4], Hit)>,
    /// Where a hit's hover highlight goes when that is not the hit itself, by the hit's index: the
    /// whole call pill mutes, but the highlight is on its microphone.
    pub spots: Vec<(usize, [f32; 4])>,
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
/// A paused track's ring: the yellow of a paused progress bar on the taskbar.
const RING_PAUSED: Rgba = Rgba::hex(0xFCE100, 1.0);

thread_local! {
    /// The accent, read once until [`accent_changed`].
    static ACCENT: std::cell::Cell<Option<Rgba>> = const { std::cell::Cell::new(None) };
}

/// A playing track's ring: the Windows accent, as a playing progress bar on the taskbar.
fn ring_playing() -> Rgba {
    ACCENT.with(|accent| {
        accent.get().unwrap_or_else(|| {
            let [r, g, b] = crate::flyout::theme::accent_rgb();
            let read = Rgba::hex(u32::from_be_bytes([0, r, g, b]), 1.0);
            accent.set(Some(read));
            read
        })
    })
}

/// Windows' accent changed: read it again at the next paint.
pub fn accent_changed() {
    ACCENT.with(|accent| accent.set(None));
}
/// The speaking pulse: the speaking ring's green.
pub const PULSE: Rgba = SPEAKING;

/// The rounded end that sticks out from the edge: shapes reach past the scene on the edge's side
/// so only the other side shows rounded.
const TUCK: f32 = 16.0;

/// What a scene is drawn for: the edge it hugs, and whether a song's note goes after the pills
/// (at the start of an edge, where there is no room before them).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Place {
    pub edge: PillEdge,
    pub note_after: bool,
}

/// The box `(x, y, w, h)` filled with its corners on the edge's side tucked past it.
fn tucked(p: &mut Painter, edge: PillEdge, at: [f32; 4], radius: f32, color: Rgba) -> Result<()> {
    let (x, y, w, h) = tuck(edge, at);
    p.round_rect(x, y, w, h, radius, color)
}

/// The box `(x, y, w, h)` reaching past the edge on the edge's side (see [`tucked`]).
fn tuck(edge: PillEdge, [x, y, w, h]: [f32; 4]) -> (f32, f32, f32, f32) {
    match edge {
        PillEdge::Left => (x - TUCK, y, w + TUCK, h),
        PillEdge::Right => (x, y, w + TUCK, h),
        PillEdge::Top => (x, y - TUCK, w, h + TUCK),
    }
}

/// A session's section of the card, `(top, bottom)`, behind its own cover: blurred and darkened
/// (dark enough for white text on the brightest cover), inside the card's rounded `card` box.
/// Nothing without a cover, the card's plain background shows.
fn section_backdrop(p: &mut Painter, edge: PillEdge, media: &crate::hub::Media, (top, bottom): (f32, f32), card: [f32; 4], radius: f32) -> Result<()> {
    let Some(cover) = media.cover.as_deref() else { return Ok(()) };
    let shape = tuck(edge, card);
    // Across the card's whole width, past the edge too: the section is cut by the card's shape.
    let area = (shape.0, top, shape.2, bottom - top);
    p.blurred(cover, area, (shape, radius), 24.0, Rgba::hex(0x101010, 0.62))?;
    Ok(())
}

/// The quiet pills' coordinates: `along` the edge (down a side, across the top) and `across`,
/// away from it. Only positions turn; what is drawn at them (a face, a badge, text) stays upright.
struct Frame {
    edge: PillEdge,
    /// The scene's width, which the right edge measures from.
    w: f32,
}

impl Frame {
    fn pt(&self, along: f32, across: f32) -> (f32, f32) {
        match self.edge {
            PillEdge::Left => (across, along),
            PillEdge::Right => (self.w - across, along),
            PillEdge::Top => (along, across),
        }
    }

    /// The box `len` long along the edge and `depth` deep, from `(along, across)`, as `(x, y, w, h)`.
    fn rect(&self, along: f32, across: f32, len: f32, depth: f32) -> [f32; 4] {
        match self.edge {
            PillEdge::Left => [across, along, depth, len],
            PillEdge::Right => [self.w - across - depth, along, depth, len],
            PillEdge::Top => [along, across, len, depth],
        }
    }
}

/// A speaker's name tag's height.
const TAG_H: f32 = 22.0;

const GLYPH_PREVIOUS: char = '\u{E892}';
const GLYPH_PLAY: char = '\u{E768}';
const GLYPH_PAUSE: char = '\u{E769}';
const GLYPH_NEXT: char = '\u{E893}';
const GLYPH_MIC: char = '\u{E720}';
const GLYPH_MIC_OFF: char = '\u{F781}';
const GLYPH_MUSIC: char = '\u{EC4F}';
const GLYPH_VIDEO: char = '\u{E714}';

/// A transport button on the card: its glyph, its command, and whether it reaches the player.
type Transport = (char, Command, bool);

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
    /// A drawn device icon's size, matching `dev_font`'s glyphs.
    dev_px: f32,
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
    dev_px: 16.0,
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
    dev_px: 12.0,
    face: 18.0,
    face_step: 14.0,
    words: false,
};

/// Paint `view` for `place`, the quiet states `small` or not. `None` for [`View::Hidden`].
pub fn scene(gfx: &Gfx, view: &View, small: bool, place: Place) -> Result<Option<Scene>> {
    let m = if small { SMALL } else { BIG };
    let edge = place.edge;
    Ok(Some(match view {
        View::Hidden => return Ok(None),
        View::Idle { call: None, devices: None } => {
            let (w, h) = if edge.vertical() { (6.0, 56.0) } else { (56.0, 6.0) };
            let (content, _) = gfx.painted(w, h, |p| tucked(p, edge, [0.0, 0.0, w, h], 3.0, SLIVER))?;
            Scene { w, h, lead: 0.0, content, rings: Vec::new(), bars: Vec::new(), pulses: Vec::new(), hits: Vec::new(), spots: Vec::new() }
        }
        View::Idle { call, devices } => stack(gfx, m, &[], call.as_ref(), devices.as_ref(), None, place)?,
        View::Music { media, card: false, call, devices, song } => stack(gfx, m, media, call.as_ref(), devices.as_ref(), song.as_ref(), place)?,
        View::Music { media, card: true, call, devices, .. } => {
            // The call's row, one row per session with its own controls, a row of devices.
            const CALL_ROW: f32 = 60.0;
            /// A session's cover, and its margin all round: the row is the cover and its margins,
            /// the text, buttons and progress bar beside it.
            const ART: f32 = 80.0;
            const ART_MARGIN: f32 = 14.0;
            const ROW: f32 = ART + 2.0 * ART_MARGIN;
            const DEVICES_ROW: f32 = 48.0;
            let w = 300.0;
            // In the column's order: the sessions, the call, the devices.
            let call_h = call.as_ref().map_or(0.0, |call| CALL_ROW + CALL_LINE * (face_lines(call) - 1) as f32);
            // Each session's row's top, just under its separator (the first, the card's top).
            let tops: Vec<f32> = (0..media.len()).map(|i| i as f32 * ROW).collect();
            let call_top = ROW * media.len() as f32;
            let row_end = |i: usize| tops.get(i + 1).copied().unwrap_or(call_top);
            // After a call, its own bottom padding; right after a session, the devices' separator
            // where the sessions' are (on the row's last line), or nothing, the card's end.
            let rows_h = match (call.is_some(), devices.is_some()) {
                (true, _) => call_top + call_h + 4.0,
                (false, true) => call_top + 2.0,
                (false, false) => call_top,
            };
            let h = rows_h + devices.as_ref().map_or(0.0, |_| DEVICES_ROW);
            let text_x = ART_MARGIN + ART + 12.0;
            let text_w = w - text_x - 16.0;
            // The three transport buttons always, in their places; previous/next greyed out where they
            // do not reach the player (a live stream, a browser tab), as Windows' own media flyout does.
            let rows: Vec<(f32, Vec<Transport>)> = media
                .iter()
                .enumerate()
                .map(|(i, m)| {
                    let play = if m.playing { GLYPH_PAUSE } else { GLYPH_PLAY };
                    let buttons = vec![(GLYPH_PREVIOUS, Command::Previous, m.skip), (play, Command::TogglePlayPause, true), (GLYPH_NEXT, Command::Next, m.skip)];
                    (tops[i], buttons)
                })
                .collect();
            // Under the buttons, its bottom on the cover's.
            let bars: Vec<Bar> = media
                .iter()
                .zip(&tops)
                .filter(|(m, _)| m.length_seconds().is_some())
                .map(|(m, top)| Bar {
                    x: text_x,
                    y: top + ART_MARGIN + ART - 3.0,
                    w: text_w,
                    h: 3.0,
                    fraction: m.fraction_now().unwrap_or(0.0) as f32,
                    rate: m.length_seconds().filter(|_| m.playing).map_or(0.0, |len| (1.0 / len) as f32),
                    color: if m.playing { ring_playing() } else { RING_PAUSED },
                })
                .collect();
            let mut hits: Vec<([f32; 4], Hit)> = rows
                .iter()
                .enumerate()
                .flat_map(|(i, (top, buttons))| {
                    let art = std::iter::once(([ART_MARGIN, top + ART_MARGIN, ART, ART], Hit::Focus(i)));
                    art.chain(
                        buttons
                            .iter()
                            .enumerate()
                            .filter(|(_, (_, _, enabled))| *enabled)
                            .map(move |(b, (_, command, _))| ([text_x - 6.0 + b as f32 * 40.0, top + 58.0, 32.0, 32.0], Hit::Media(i, *command))),
                    )
                })
                .collect();
            if call.is_some() {
                hits.push(([w - 54.0, call_top + 8.0, 40.0, 40.0], Hit::CallMute));
            }
            let (content, _) = gfx.painted(w, h, |p| {
                tucked(p, edge, [0.0, 0.0, w, h], 18.0, NORMAL_BG)?;
                // Each session behind its own cover (a video's thumbnail is not the song's art),
                // from separator to separator; the first from the card's top, the last to its
                // bottom when nothing follows.
                // Each section ends on the separator line below it, wherever that is drawn.
                for (i, m) in media.iter().enumerate() {
                    let top = if i == 0 { 0.0 } else { tops[i] - 1.0 };
                    let bottom = match (i + 1 == media.len(), call.is_some(), devices.is_some()) {
                        (false, ..) | (true, true, _) => row_end(i) - 1.0,
                        (true, false, true) => rows_h - 3.0,
                        (true, false, false) => h,
                    };
                    section_backdrop(p, edge, m, (top, bottom), [0.0, 0.0, w, h], 18.0)?;
                }
                // The bars' tracks; their filled parts are visuals of their own (see `Scene::bars`).
                for bar in &bars {
                    p.round_rect(bar.x, bar.y, bar.w, bar.h, bar.h / 2.0, TRACK)?;
                }
                if let Some(call) = call {
                    if !media.is_empty() {
                        p.round_rect(0.0, call_top - 1.0, w, 1.0, 0.0, Rgba::hex(0xFFFFFF, 0.10))?;
                    }
                    call_row(p, call, w, call_top)?;
                }
                for (m, (top, buttons)) in media.iter().zip(&rows) {
                    let top = *top;
                    if top > 0.0 {
                        p.round_rect(0.0, top - 1.0, w, 1.0, 0.0, Rgba::hex(0xFFFFFF, 0.10))?;
                    }
                    art(p, m.cover.as_deref().map(|c| c.as_slice()), m.video, ART_MARGIN, top + ART_MARGIN, ART, Some(8.0))?;
                    p.text(&m.title, Font::Title, (text_x, top + 10.0, text_w, 20.0), WHITE)?;
                    p.text(&m.artist, Font::Body, (text_x, top + 30.0, text_w, 18.0), DIM_WHITE)?;
                    let app_w = gfx.measure(Font::Small, &m.app).min(text_w - 40.0);
                    p.text(&m.app, Font::Small, (text_x, top + 48.0, app_w + 2.0, 14.0), Rgba::hex(0xFFFFFF, 0.55))?;
                    if m.length_seconds().is_none() {
                        let x = text_x + app_w + 8.0;
                        p.round_rect(x, top + 49.5, 26.0, 11.0, 3.0, LIVE_DOT)?;
                        p.text("LIVE", Font::Badge, (x, top + 49.5, 26.0, 11.0), WHITE)?;
                    }
                    for (b, (glyph, _, enabled)) in buttons.iter().enumerate() {
                        let x = text_x - 6.0 + b as f32 * 40.0;
                        let color = if *enabled { WHITE } else { Rgba::hex(0xFFFFFF, 0.3) };
                        p.text(&glyph.to_string(), Font::Icons, (x, top + 60.0, 32.0, 28.0), color)?;
                    }
                }
                if let Some(devices) = devices {
                    p.round_rect(0.0, rows_h - 3.0, w, 1.0, 0.0, Rgba::hex(0xFFFFFF, 0.10))?;
                    device_row(p, devices, rows_h, w, &mut hits)?;
                }
                Ok(())
            })?;
            Scene { w, h, lead: 0.0, content, rings: Vec::new(), bars, pulses: Vec::new(), hits, spots: Vec::new() }
        }
        View::Banner { level, glyph, text, .. } => {
            let (bg, fg) = match level {
                Level::Alert => (ALERT_BG, WHITE),
                Level::Warn => (WARN_BG, WARN_TEXT),
                _ => (NOTICE_BG, WHITE),
            };
            let text_w = gfx.measure(Font::Title, text).min(360.0);
            let (w, h) = (16.0 + 20.0 + 10.0 + text_w + 18.0, 44.0);
            let (content, _) = gfx.painted(w, h, |p| {
                tucked(p, edge, [0.0, 0.0, w, h], h / 2.0, bg)?;
                p.text(&glyph.to_string(), Font::Icons, (14.0, 0.0, 22.0, h), fg)?;
                p.text(text, Font::Title, (46.0, 0.0, text_w + 2.0, h), fg)
            })?;
            Scene { w, h, lead: 0.0, content, rings: Vec::new(), bars: Vec::new(), pulses: Vec::new(), hits: vec![([0.0, 0.0, w, h], Hit::Whole)], spots: Vec::new() }
        }
    }))
}

const GLYPH_VOLUME_MUTED: char = '\u{E74F}';

fn device_count(devices: &Devices) -> usize {
    usize::from(devices.output.is_some()) + usize::from(devices.input.is_some())
}

/// The output then the input, as round buttons along the pill from `start`.
fn device_column(p: &mut Painter, frame: &Frame, devices: &Devices, start: f32, hits: &mut Vec<([f32; 4], Hit)>, m: Metrics) -> Result<()> {
    let mut along = start;
    for (flow, device) in [(Flow::Output, &devices.output), (Flow::Input, &devices.input)] {
        let Some(device) = device else { continue };
        let (cx, cy) = frame.pt(along + m.dev_step / 2.0, m.col / 2.0);
        device_button(p, flow, device, cx, cy, m)?;
        hits.push((frame.rect(along, 0.0, m.dev_step, m.col), Hit::Device(flow)));
        along += m.dev_step;
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
        (Flow::Output, true) => Some(GLYPH_VOLUME_MUTED),
        (Flow::Input, true) => Some(GLYPH_MIC_OFF),
        // No font has the earbuds: drawn as the tray and the strip draw them.
        _ if device.icon.drawn() => None,
        _ => Some(device.icon.glyph()),
    };
    match glyph {
        Some(glyph) => p.text(&glyph.to_string(), m.dev_font, (cx - r, cy - r, 2.0 * r, 2.0 * r), WHITE)?,
        None => p.icon(device.icon, cx, cy, m.dev_px)?,
    }
    if device.recording {
        p.circle(cx + r * 0.7, cy - r * 0.7, r * 0.28, LIVE_DOT)?;
    }
    Ok(())
}
/// Cover art, or a music (or `video`) glyph on a plain disc when there is none (or it will not decode).
fn art(p: &mut Painter, cover: Option<&[u8]>, video: bool, x: f32, y: f32, size: f32, radius: Option<f32>) -> Result<()> {
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
    let glyph = if video { GLYPH_VIDEO } else { GLYPH_MUSIC };
    p.text(&glyph.to_string(), Font::Icons, (x, y, size, size), DIM_WHITE)
}

/// The gap between two pills of the column.
const PILL_GAP: f32 = 6.0;

/// The quiet column: separate pills for the playing sessions, the call and the devices, each
/// only when there is one, along the edge with a gap between (a row on the top edge).
fn stack(gfx: &Gfx, m: Metrics, media: &[crate::hub::Media], call: Option<&CallPill>, devices: Option<&Devices>, song: Option<&SongNote>, place: Place) -> Result<Scene> {
    let col = m.col;
    let mid = col / 2.0;
    let vertical = place.edge.vertical();
    let music_len = if media.is_empty() { 0.0 } else { col + m.disc_step * (media.len() as f32 - 1.0) };
    let call_layout = call.map(|c| CallLayout::of(gfx, c, m));
    let pad = (col - 2.0 * m.dev_r) / 4.0;
    let devices_len = devices.map_or(0.0, |d| 2.0 * pad + m.dev_step * device_count(d) as f32);
    let lens = [music_len, call_layout.as_ref().map_or(0.0, |l| l.h), devices_len];
    // A song's note sits before the pills, which keep their place (see [`Scene::lead`]), or after
    // them at the start of the edge.
    let (note_h, note_font) = if m.words { (40.0, Font::Title) } else { (30.0, Font::Body) };
    let note_text_w = song.map_or(0.0, |s| gfx.measure(note_font, &s.text).min(320.0));
    let note_w = 14.0 + 20.0 + 8.0 + note_text_w + 16.0;
    let (note_len, note_depth) = if vertical { (note_h, note_w) } else { (note_w, note_h) };
    let note_room = if song.is_some() { note_len + PILL_GAP } else { 0.0 };
    let lead = if place.note_after { 0.0 } else { note_room };
    let starts: Vec<f32> = lens
        .iter()
        .scan(lead, |at, &len| {
            let start = *at;
            if len > 0.0 {
                *at += len + PILL_GAP;
            }
            Some(start)
        })
        .collect();
    let pills_len = lens.iter().filter(|len| **len > 0.0).map(|len| len + PILL_GAP).sum::<f32>() - PILL_GAP;
    let len = lead + pills_len + if place.note_after { note_room } else { 0.0 };
    // A speaker's name tag goes beside a column, under a row.
    let tag = call_layout.as_ref().filter(|l| l.tag_w > 0.0).map_or(0.0, |l| if vertical { l.tag_w } else { TAG_H } + 6.0);
    let depth = (col + tag).max(if song.is_some() { note_depth } else { 0.0 });
    let (w, h) = if vertical { (depth, len) } else { (len, depth) };
    let frame = Frame { edge: place.edge, w };
    let mut hits = Vec::new();
    let mut pulses = Vec::new();
    let mut spots = Vec::new();
    let (content, _) = gfx.painted(w, h, |p| {
        if let Some(song) = song {
            let at = if place.note_after { len - note_len } else { 0.0 };
            let [x, y, bw, bh] = frame.rect(at, 0.0, note_len, note_depth);
            tucked(p, place.edge, [x, y, bw, bh], note_h / 2.0, NORMAL_BG)?;
            // The cover where there is one, else the music glyph.
            let size = note_h - 12.0;
            // Ending where the glyph's slot ends, 8 before the text.
            let (ix, iy) = (x + 34.0 - size, y + (bh - size) / 2.0);
            let covered = match &song.cover {
                Some(cover) => p.image(cover, ix, iy, size, Some(6.0))?,
                None => false,
            };
            if !covered {
                p.text(&super::view::GLYPH_SONG.to_string(), Font::Icons, (x + 12.0, y, 22.0, bh), WHITE)?;
            }
            p.text(&song.text, note_font, (x + 42.0, y, note_text_w + 2.0, bh), WHITE)?;
            hits.push(([x, y, bw, bh], Hit::Song));
        }
        if music_len > 0.0 {
            let start = starts[0];
            tucked(p, place.edge, frame.rect(start, 0.0, music_len, col), col / 2.0, QUIET_BG)?;
            for (i, media) in media.iter().enumerate() {
                let along = start + mid + i as f32 * m.disc_step;
                let (cx, cy) = frame.pt(along, mid);
                let half = m.art / 2.0;
                // Clickable only when the pill is; quiet, clicks pass through.
                hits.push((frame.rect(along - m.disc_step / 2.0, 0.0, m.disc_step, col), Hit::Focus(i)));
                art(p, media.cover.as_deref().map(|c| c.as_slice()), media.video, cx - half, cy - half, m.art, None)?;
                if !media.playing {
                    // Paused: dimmed, still in its place so the card opens on the same row.
                    p.circle(cx, cy, half, Rgba::hex(0x000000, 0.5))?;
                }
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
            pulses.extend(paint_call(p, &frame, call, layout, m, starts[1]));
            hits.push((frame.rect(starts[1], 0.0, layout.h, col), Hit::CallMute));
            // Lit on the microphone, where paint_call draws it.
            let (mx, my) = frame.pt(starts[1] + layout.h - m.dev_r - 6.0, mid);
            let r = m.dev_r + 2.0;
            spots.push((hits.len() - 1, [mx - r, my - r, 2.0 * r, 2.0 * r]));
        }
        if let Some(devices) = devices {
            tucked(p, place.edge, frame.rect(starts[2], 0.0, devices_len, col), col / 2.0, QUIET_BG)?;
            device_column(p, &frame, devices, starts[2] + pad, &mut hits, m)?;
        }
        Ok(())
    })?;
    let rings = media
        .iter()
        .enumerate()
        .filter(|(_, media)| media.length_seconds().is_some())
        .map(|(i, media)| {
            let (cx, cy) = frame.pt(starts[0] + mid + i as f32 * m.disc_step, mid);
            Ring {
                cx,
                cy,
                r: m.ring_r,
                stroke: m.ring_stroke,
                fraction: media.fraction_now().unwrap_or(0.0) as f32,
                rate: media.length_seconds().filter(|_| media.playing).map_or(0.0, |len| (1.0 / len) as f32),
                color: if media.playing { ring_playing() } else { RING_PAUSED },
            }
        })
        .collect();
    Ok(Scene { w, h, lead, content, rings, bars: Vec::new(), pulses, hits, spots })
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

/// The call pill from `start`: faces along the edge (the speaking one lit green, its name beside
/// a column or under a row), then the microphone with its live dot. Returns the speaker's pulse.
fn paint_call(p: &mut Painter, frame: &Frame, call: &CallPill, layout: &CallLayout, m: Metrics, start: f32) -> Option<Pulse> {
    let col = m.col;
    let mid = col / 2.0;
    let vertical = frame.edge.vertical();
    let mut paint = || -> Result<()> {
        tucked(p, frame.edge, frame.rect(start, 0.0, layout.h, col), col / 2.0, NORMAL_BG)?;
        for (i, member) in call.members.iter().take(layout.shown).enumerate() {
            let (cx, cy) = frame.pt(start + 8.0 + i as f32 * m.face_step + m.face / 2.0, mid);
            let lit = Some(i) == layout.speaker;
            face(p, member, cx - m.face / 2.0, cy - m.face / 2.0, m.face, lit, m.words)?;
            if lit && layout.tag_w > 0.0 {
                let [tx, ty, tw, th] = if vertical {
                    frame.rect(cy - TAG_H / 2.0, col + 6.0, TAG_H, layout.tag_w)
                } else {
                    [(cx - layout.tag_w / 2.0).clamp(0.0, (frame.w - layout.tag_w).max(0.0)), col + 6.0, layout.tag_w, TAG_H]
                };
                p.round_rect(tx, ty, tw, th, TAG_H / 2.0, NORMAL_BG)?;
                p.text(&member.name, Font::Small, (tx + 10.0, ty, tw - 20.0, th), WHITE)?;
            }
        }
        let extra = call.members.len().saturating_sub(layout.shown);
        if extra > 0 && m.words {
            let along = start + 8.0 + m.face + m.face_step * (layout.shown as f32 - 1.0);
            let [x, y, w, h] = frame.rect(along - 2.0, 0.0, if vertical { 12.0 } else { 20.0 }, col);
            p.text(&format!("+{extra}"), Font::Small, (x, y, w, h), DIM_WHITE)?;
        }
        if let (0, Some(icon)) = (layout.shown, &call.icon) {
            let (cx, cy) = frame.pt(start + 8.0 + m.face / 2.0, mid);
            p.image(icon, cx - m.face / 2.0, cy - m.face / 2.0, m.face, Some(0.0))?;
        } else if layout.shown == 0 && m.words {
            if let Some(app) = call.app.map(|a| a.label()).or(call.holder.as_deref()) {
                let [x, y, w, h] = frame.rect(start + 8.0, 0.0, 16.0, col);
                p.text(&app.chars().take(2).collect::<String>(), Font::Small, (x, y, w, h), DIM_WHITE)?;
            }
        }
        let r = m.dev_r;
        let (cx, cy) = frame.pt(start + layout.h - r - 6.0, mid);
        p.circle(cx, cy, r, if call.muted { Rgba::hex(0x5A5A5A, 1.0) } else { Rgba::hex(0x3A3A3A, 1.0) })?;
        p.text(&(if call.muted { GLYPH_MIC_OFF } else { GLYPH_MIC }).to_string(), m.dev_font, (cx - r, cy - r, 2.0 * r, 2.0 * r), WHITE)?;
        if !call.muted {
            p.circle(cx + r * 0.7, cy - r * 0.7, r * 0.28, LIVE_DOT)?;
        }
        Ok(())
    };
    if let Err(e) = paint() {
        eprintln!("pill: could not paint the call ({e:#})");
    }
    layout.speaker.map(|i| {
        let (cx, cy) = frame.pt(start + 8.0 + i as f32 * m.face_step + m.face / 2.0, mid);
        Pulse { cx, cy, r: m.face / 2.0 + 1.0 }
    })
}

/// Faces on one line of the card's call row; more wrap onto lines below.
const CARD_FACES_PER_LINE: usize = 4;
/// Each further line of faces adds this much to the call row.
const CALL_LINE: f32 = 32.0;

/// How many lines the card's call row takes: everyone in the call is shown.
fn face_lines(call: &CallPill) -> usize {
    call.members.len().div_ceil(CARD_FACES_PER_LINE).max(1)
}

/// The card's call row at `top`: overlapping faces (everyone, four to a line), the channel (or
/// app) and who is talking, and the mute.
fn call_row(p: &mut Painter, call: &CallPill, w: f32, top: f32) -> Result<()> {
    const FACE: f32 = 28.0;
    for (line, members) in call.members.chunks(CARD_FACES_PER_LINE).enumerate() {
        let y = top + 12.0 + line as f32 * CALL_LINE;
        for (i, member) in members.iter().enumerate() {
            face(p, member, 14.0 + i as f32 * FACE * 0.7, y, FACE, member.speaking, false)?;
        }
    }
    // Where the faces' first line ends (or the icon in their place).
    let x = 14.0 + FACE * 0.7 * call.members.len().min(CARD_FACES_PER_LINE) as f32;
    if call.members.is_empty() {
        match &call.icon {
            Some(icon) => {
                p.image(icon, 14.0, top + 12.0, FACE, Some(0.0))?;
            }
            None => p.text(&GLYPH_MIC.to_string(), Font::Icons, (14.0, top + 12.0, FACE, FACE), WHITE)?,
        }
    }
    let x = if call.members.is_empty() { x + FACE } else { x };
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
    p.text(&title, Font::Title, (text_x, top + 8.0, text_w, 20.0), WHITE)?;
    p.text(&status, Font::Small, (text_x, top + 28.0, text_w, 16.0), DIM_WHITE)?;
    let (bx, by) = (w - 34.0, top + 28.0);
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
