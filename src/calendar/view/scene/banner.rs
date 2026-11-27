//! A banner's card (one notification arriving, over the bell) and a glance's pill, laid out, with
//! the pictures Audio Tray draws in a photo's place.

use super::*;

/// A banner's app line: its logo, its name and when it came.
pub(super) const BANNER_HEAD_H: f32 = 30.0;

/// What a banner draws in its photo's place: Audio Tray's own notices illustrated, a
/// conversation's faces.
#[derive(Clone, Debug, PartialEq)]
pub enum Art {
    None,
    /// A battery filled to `level` (0..1): amber, red once low.
    Battery { level: f32, low: bool },
    /// A speaker playing: its four bars' heights (0..1).
    Speaker { bars: [f32; 4] },
    /// A glyph on a disc of its colour (a meeting's calendar, focus done, the digest's moon).
    Glyph { glyph: char, rgb: [u8; 3] },
    /// A conversation's senders, overlapping (photos, or their initials on a colour).
    Faces(Vec<Face>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Face {
    Photo(String, Arc<Vec<u8>>),
    Initials(String, [u8; 3]),
}

/// A banner: one notification arriving, on a card of its own over the bell, drawn as the
/// notification centre draws it under its app's line, in its app's colour.
pub struct Banner {
    pub app: String,
    pub logo: Option<(String, Arc<Vec<u8>>)>,
    pub when: String,
    /// Its notification (`index` 0: a banner holds one).
    pub toast: Toast,
    pub hover: Option<Hit>,
    pub pressed: Option<Hit>,
    /// The app's colour, from its logo: washed in from the left, on its countdown and its art.
    pub tint: Option<[u8; 3]>,
    /// Its time left (1 to 0), a line along its foot; `None`: it stays (a call, a reminder).
    pub left: Option<f32>,
    /// The pointer holds it: the line stops, brighter.
    pub paused: bool,
    /// A conversation's banner: how many more came since its first, and how big that count is
    /// drawn (1, more for a moment as it grows).
    pub more: usize,
    pub more_pop: f32,
    /// A call: where its rings are in their pulse (0..1).
    pub pulse: Option<f32>,
    /// Confetti over the card (focus done): each piece's place, colour and opacity.
    pub confetti: Vec<(f32, f32, [u8; 3], u8)>,
    /// Its open reply box's caret is drawn (it blinks).
    pub caret: bool,
}

/// Audio Tray's own colours: its notices' (a battery amber or red, a meeting blue, focus green), a
/// call's buttons (green to answer, red to decline), and the digest's purple.
pub const AMBER: [u8; 3] = [0xF2, 0xA3, 0x3A];
pub const RED: [u8; 3] = [0xE5, 0x48, 0x4D];
pub const BLUE: [u8; 3] = [0x2F, 0x86, 0xE0];
pub const GREEN: [u8; 3] = [0x3F, 0xBF, 0x7F];
pub const PURPLE: [u8; 3] = [0xB5, 0x5C, 0xC4];

/// A banner's time line: in from each side, up from the card's foot (to its top), and its height,
/// in DIPs.
pub const LINE_INSET: f32 = 20.0;
pub const LINE_UP: f32 = 5.0;
pub const LINE_H: f32 = 2.0;

/// A banner's card.
pub fn banner(b: &Banner) -> Scene {
    let h = 6.0 + BANNER_HEAD_H + b.toast.height() + 8.0;
    let main = R::new(0.0, 0.0, W, h);
    let mut ink = Layout::new(b.hover, b.pressed, None, b.caret);
    ink.draw(main, Draw::Card);
    if let Some(rgb) = b.tint {
        ink.draw(main, Draw::Wash { rgb, alpha: 84, reach: 0.6, radius: 0.0 });
    }
    let line = R::new(8.0, 6.0, W - 16.0, BANNER_HEAD_H);
    let mut tx = line.x + 8.0;
    if let Some((key, bytes)) = &b.logo {
        ink.draw(R::new(tx, line.y + (line.h - 16.0) / 2.0, 16.0, 16.0), Draw::Logo { key: key.clone(), bytes: bytes.clone() });
        tx += 24.0;
    }
    ink.text(R::new(tx, line.y, line.right() - tx - 140.0, line.h), b.app.clone(), Font::CaptionStrong, Ink::Text2, Align::Left);
    let when_w = text_w(&b.when, Font::Caption).max(30.0);
    ink.text(R::new(line.right() - 8.0 - when_w, line.y, when_w, line.h), b.when.clone(), Font::Caption, Ink::Text3, Align::Right);
    if b.more > 0 {
        // "+2 more", before its time, growing for a moment when it counts up.
        let label = (words().more)(b.more);
        let lw = text_w(&label, Font::CaptionStrong) * b.more_pop + 4.0;
        let at = R::new(line.right() - 16.0 - when_w - lw, line.y, lw, line.h);
        ink.text(at, label, Font::CaptionStrong, Ink::Accent, Align::Right);
    }
    let r = R::new(8.0, line.bottom(), W - 16.0, b.toast.height());
    notification(&mut ink, r, &b.toast);
    let spot = R::new(r.x + 12.0, r.y + 12.0, 36.0, 36.0);
    let tint = b.tint.unwrap_or(PURPLE);
    if let Some(phase) = b.pulse {
        // A call: two rings going out from the caller, one after the other.
        for k in 0..2 {
            let p = (phase + k as f32 * 0.5) % 1.0;
            let grow = 2.0 + p * 10.0;
            let ring = R::new(spot.x - grow, spot.y - grow, spot.w + 2.0 * grow, spot.h + 2.0 * grow);
            ink.draw(ring, Draw::Ring { rgb: tint, alpha: ((1.0 - p) * 200.0) as u8, width: 2.0 });
        }
    }
    if let Some(left) = b.left {
        // Its time left, along its foot: brighter while the pointer holds it.
        let track = R::new(LINE_INSET, h - LINE_UP, W - 2.0 * LINE_INSET, LINE_H);
        ink.draw(track, Draw::Rect { fill: Fill::Rgba(tint, 40), radius: 1.0 });
        let run = R::new(track.x, track.y, track.w * left.clamp(0.0, 1.0), track.h);
        ink.draw(run, Draw::Rect { fill: Fill::Rgba(tint, if b.paused { 255 } else { 190 }), radius: 1.0 });
    }
    for &(x, y, rgb, alpha) in &b.confetti {
        ink.draw(R::new(x, y, 5.0, 8.0), Draw::Rect { fill: Fill::Rgba(rgb, alpha), radius: 1.5 });
    }
    ink.into_scene(main)
}

/// A glance's height: a pill, as the edge pill's notices.
pub(super) const GLANCE_H: f32 = 52.0;

/// A picture of its own (a key, its bytes): a song's cover on its glance.
pub type Cover = (String, Arc<Vec<u8>>);

/// A glance: news for a moment, never listed (a keyboard layout switched, a song, an app
/// listening).
pub struct Glance<'a> {
    /// Its glyph in the disc, and a picture over it (a song's cover; the glyph shows where it
    /// cannot be drawn).
    pub glyph: char,
    pub cover: Option<Cover>,
    pub title: &'a str,
    pub body: &'a str,
    /// Its time left, 1 to 0.
    pub left: f32,
    pub hover: Option<Hit>,
    /// A colour of its own (an alert's red), its words then in white; `None`: the accent.
    pub tint: Option<[u8; 3]>,
    /// It has a close button.
    pub closable: bool,
}

/// A glance's pill: not a notification's card but Audio Tray's own solid accent pill: its glyph
/// in a disc, its words, and a hairline running down the time it has left.
pub fn glance(g: &Glance) -> Scene {
    let Glance { glyph, ref cover, title, body, left, hover, tint, closable } = *g;
    let words_w = text_w(title, Font::Strong).max(text_w(body, Font::Caption));
    // Room for its close button at the right end, when it has one.
    let close_w = if closable { 36.0 } else { 0.0 };
    let w = (8.0 + 36.0 + 12.0 + words_w + 24.0 + close_w).clamp(180.0, W);
    let main = R::new(0.0, 0.0, w, GLANCE_H);
    let mut ink = Layout::new(hover, None, None, false);
    let round = GLANCE_H / 2.0;
    // The accent, or a colour of its own (an alert's red), its words then in white.
    let lit = hover == Some(Hit::Toast(0));
    let (fill, ink_main, ink_soft) = match tint {
        Some(rgb) => (Fill::Rgba(rgb, if lit { 225 } else { 255 }), Ink::Rgba([0xFF; 3], 255), Ink::Rgba([0xFF; 3], 205)),
        None => (if lit { Fill::AccentHover } else { Fill::Accent }, Ink::OnAccent, Ink::OnAccent2),
    };
    ink.draw(main, Draw::Rect { fill, radius: round });
    // A click on it does its thing (a glance just goes); its close button only closes it.
    ink.hit(main, Hit::Toast(0));
    let disc = R::new(8.0, 8.0, 36.0, 36.0);
    ink.draw(disc, Draw::Rect { fill: Fill::OnAccentWash, radius: 18.0 });
    ink.glyph(disc, glyph, Font::Icon, ink_main);
    if let Some((key, bytes)) = cover {
        // A song's cover, cut to the disc, over its glyph.
        ink.draw(disc, Draw::Avatar { key: key.clone(), bytes: bytes.clone() });
    }
    let tx = disc.right() + 12.0;
    let tw = w - tx - 20.0 - close_w;
    if body.is_empty() {
        ink.text(R::new(tx, 0.0, tw, GLANCE_H), title, Font::Strong, ink_main, Align::Left);
    } else {
        ink.text(R::new(tx, 7.0, tw, 20.0), title, Font::Strong, ink_main, Align::Left);
        ink.text(R::new(tx, 26.0, tw, 18.0), body, Font::Caption, ink_soft, Align::Left);
    }
    if closable {
        let x = R::new(w - 14.0 - 28.0, (GLANCE_H - 28.0) / 2.0, 28.0, 28.0);
        if hover == Some(Hit::Dismiss(0)) {
            ink.draw(x, Draw::Rect { fill: Fill::OnAccentWash, radius: 14.0 });
        }
        ink.glyph(x, '\u{E711}', Font::IconSmall, ink_main);
        ink.hit(x, Hit::Dismiss(0));
    }
    // The time left, along the foot inside the curve.
    let run = w - 2.0 * round;
    if left > 0.0 && run > 0.0 {
        ink.draw(R::new(round, GLANCE_H - 5.0, run * left.clamp(0.0, 1.0), 2.0), Draw::Rect { fill: Fill::OnAccentWash, radius: 1.0 });
    }
    ink.into_scene(main)
}

/// A banner's picture in its photo's place.
pub(super) fn art(ink: &mut Layout, spot: R, art: &Art) {
    match art {
        Art::None => {}
        Art::Battery { level, low } => {
            let body = R::new(spot.x + 4.0, spot.y + 11.0, 25.0, 14.0);
            ink.draw(body, Draw::Outline { ink: Ink::Text, radius: 3.0 });
            ink.draw(R::new(body.right(), body.y + 4.0, 3.0, 6.0), Draw::Rect { fill: Fill::Rgba([0xFF; 3], 200), radius: 1.0 });
            let fill = if *low { RED } else { AMBER };
            let inner = R::new(body.x + 2.5, body.y + 2.5, (body.w - 5.0) * level.clamp(0.04, 1.0), body.h - 5.0);
            ink.draw(inner, Draw::Rect { fill: Fill::Rgba(fill, 255), radius: 1.5 });
        }
        Art::Speaker { bars } => {
            ink.draw(spot, Draw::Rect { fill: Fill::Accent, radius: 18.0 });
            for (n, height) in bars.iter().enumerate() {
                let bh = 4.0 + height.clamp(0.0, 1.0) * 16.0;
                let bar = R::new(spot.x + 9.0 + n as f32 * 5.0, spot.y + (spot.h - bh) / 2.0, 3.0, bh);
                ink.draw(bar, Draw::Rect { fill: Fill::Rgba([0xFF; 3], 240), radius: 1.5 });
            }
        }
        Art::Glyph { glyph, rgb } => {
            ink.draw(spot, Draw::Rect { fill: Fill::Rgba(*rgb, 255), radius: 18.0 });
            ink.glyph(spot, *glyph, Font::Icon, Ink::Rgba([0xFF; 3], 255));
        }
        Art::Faces(faces) => {
            // Up to three, each a step to the right and down, the newest in front.
            let shown: Vec<&Face> = faces.iter().take(3).collect();
            let many = shown.len() > 1;
            let size = if many { 26.0 } else { 36.0 };
            for (n, face) in shown.iter().enumerate().rev() {
                let (dx, dy) = if many { (n as f32 * 9.0 - 4.0, n as f32 * 3.0 - 3.0) } else { (0.0, 0.0) };
                let f = R::new(spot.x + dx, spot.y + (spot.h - size) / 2.0 + dy, size, size);
                match face {
                    Face::Photo(key, bytes) => ink.draw(f, Draw::Avatar { key: key.clone(), bytes: bytes.clone() }),
                    Face::Initials(text, rgb) => {
                        ink.draw(f, Draw::Rect { fill: Fill::Rgba(*rgb, 255), radius: size / 2.0 });
                        ink.text(f, text.clone(), Font::CaptionStrong, Ink::Rgba([0xFF; 3], 255), Align::Center);
                    }
                }
            }
        }
    }
}
